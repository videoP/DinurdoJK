//! Render Studio timeline -> playable protocol-26 demo for stock JKA/OpenJK.
//!
//! The camera is never re-derived here. A capture pass on the main thread
//! steps the timeline exactly like video export (same sources, Director
//! shots, Ghoul2 anchors, BSP collision, camera moves and comparison cameras)
//! and records the camera the Studio renders for each output frame. A worker
//! then re-encodes the source recordings' snapshots around those cameras with
//! the stock protocol writer. Studio-only presentation (lights, lens/FOV,
//! post, GPU transitions, imported audio, HUD layout) has no protocol-26
//! representation and is reported, not approximated.
use super::{App, FrontendPage, race_ghost};
use super::render_studio_timeline::{StudioCameraKey, StudioCameraSource, StudioDemoAnchor, StudioDemoClip,
    StudioDemoPov, StudioFollowPerspective, StudioDemoTransition};
use jka_protocol::{
    commands::{atoi, tokenize, BigConfigOutcome, BigConfigString},
    demo::{self, DemoReader},
    demo_bake::{canonicalize_snapshot, synthesize_snapshot_payload},
    gamestate::{EntityState, ENTITY_FIELDS, MAX_CONFIGSTRINGS},
    server::{Decoder, Event, PlayerState, Snapshot},
};
use std::{collections::{BTreeMap, BTreeSet, HashMap}, io::{Cursor, Write}, path::{Path, PathBuf},
    sync::{mpsc, OnceLock}};

const CS_PLAYERS: u16 = crate::cgame::CS_PLAYERS;
/// Server-clock configstrings (bg_public.h), shifted with the output clock.
const CLOCK_CONFIGSTRINGS: [u16; 3] = [5 /*CS_WARMUP*/, 8 /*CS_VOTE_TIME*/, 21 /*CS_LEVEL_START_TIME*/];
const SPECTATOR_INFO: &[u8] = br"\n\Studio Camera\t\3";
const MAX_DEMO_BYTES: usize = 512 * 1024 * 1024;
const MAX_SOURCE_FRAMES: usize = 200_000;
const MAX_EXPORT_FRAMES: u64 = 300_000;
/// MAX_RELIABLE_COMMANDS is 128: more new commands in one snapshot make
/// CL_GetServerCommand drop with "a reliable command was cycled out".
const MAX_COMMANDS_PER_SNAPSHOT: usize = 100;
/// cgame MAX_ENTITIES_IN_SNAPSHOT.
const MAX_SNAPSHOT_ENTITIES: usize = 256;
/// SV_SendConfigstring chunk size: MAX_STRING_CHARS - 24.
const MAX_CONFIGSTRING_CHUNK: usize = 1000;
/// Kept around every sampled source instant for interpolation brackets.
const SOURCE_PADDING_MS: f64 = 2000.0;
const OUTPUT_EPOCH_MS: i32 = 10_000;
const ENTITYNUM_NONE: u32 = 1023;
const PM_SPECTATOR: u32 = 4;
const PERS_TEAM: usize = 3;
const TEAM_SPECTATOR: i32 = 3;
const STAT_HEALTH: usize = 0;
const ET_PLAYER: u32 = 1;
const ET_NPC: u32 = 13;
const EF_TELEPORT_BIT: u32 = 1 << 3;
const TR_INTERPOLATE: i32 = 1;
const TR_LINEAR: i32 = 2;
const TR_LINEAR_STOP: i32 = 3;
const TR_GRAVITY: i32 = 6;
const DEFAULT_GRAVITY: f32 = 800.0;
/// Camera moves beyond this per output frame are cuts, not motion.
const CAMERA_CUT_UNITS: f32 = 160.0;
const CAMERA_CUT_DEGREES: f32 = 60.0;
/// playerState clocks compared against cg.time by cgame.
const PLAYER_CLOCKS: [&str; 13] = ["commandTime", "rocketLockTime", "rocketTargetTime", "weaponChargeTime",
    "weaponChargeSubtractTime", "electrifyTime", "saberLockTime", "duelTime", "hackingTime", "fallingToDeath",
    "zoomTime", "fd.forceRageRecoveryTime", "fd.forcePowerDebounce[FP_LEVITATION]"];

/// How one output frame is viewed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Dm26View {
    /// The recording's own playerState: real first person, weapon and HUD.
    Recorded,
    /// Studio camera in protocol space (origin, pitch/yaw/roll degrees).
    /// `hide` is the player whose body the Studio hides (first-person follow).
    Camera { origin: [f32; 3], angles: [f32; 3], hide: Option<u16> },
}

pub(in crate::app) struct Dm26Frame {
    /// Top visible demo clip; None is a timeline gap.
    pub clip: Option<usize>,
    pub source_ms: f64,
    pub view: Dm26View,
    /// Players the Studio leaves visible (hide other players, comparison
    /// clips); None = every player.
    pub players: Option<Vec<u16>>,
}
impl Dm26Frame {
    pub fn gap() -> Self { Self { clip: None, source_ms: 0.0, view: Dm26View::Recorded, players: None } }
}

/// Main-thread camera pass; replaces the video clock while it runs.
pub(in crate::app) struct StudioDm26Capture {
    pub index: u64,
    pub total: u64,
    pub fps: u32,
    begin: f64,
    frames: Vec<Dm26Frame>,
    warnings: BTreeSet<String>,
}

/// Classify the Studio's final camera for one sampled source frame.
pub(in crate::app) fn studio_dm26_view(key: StudioCameraKey, recorded: i32, camera_move: bool,
    race_camera: bool, camera: &crate::camera::Camera) -> Dm26View {
    let key = super::render_studio_demo::resolve_recorded_subject(key, recorded);
    let detached = |hide: Option<i32>| Dm26View::Camera {
        origin: super::scene::jka_position(camera.position.to_array()),
        angles: [-camera.pitch.to_degrees(), camera.yaw.to_degrees(), camera.roll.to_degrees()],
        hide: hide.and_then(|client| u16::try_from(client).ok()),
    };
    if race_camera || camera_move { return detached(None); }
    match key.source {
        StudioCameraSource::Animation | StudioCameraSource::Demo { pov: StudioDemoPov::Recorded, .. } => Dm26View::Recorded,
        StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client }
            if key.demo_follow.perspective == StudioFollowPerspective::FirstPerson => {
            let rig = key.demo_follow;
            let plain = rig.position_anchor == StudioDemoAnchor::Eyes && rig.position_offset == [0.0; 3]
                && rig.yaw_offset == 0.0 && rig.pitch_offset == 0.0 && !rig.aim_enabled && !rig.blade_view;
            // The recorded player's own eyes are its real first-person view.
            if plain && client == recorded { Dm26View::Recorded } else { detached(Some(client)) }
        }
        _ => detached(None),
    }
}

struct EntityTrajectory { time: usize, kind: usize, duration: usize, base: [usize; 3], delta: [usize; 3] }
struct EntityFields {
    pos: EntityTrajectory, apos: EntityTrajectory,
    entity_type: usize, flags: usize, event: usize, event_parm: usize,
    /// BG_PlayerStateToEntityState: weaponChargeTime / electrifyTime.
    charge_time: usize, electrify_time: usize,
}
fn field(name: &str) -> usize {
    ENTITY_FIELDS.iter().position(|(candidate, _)| *candidate == name).unwrap_or_else(|| panic!("entity field {name}"))
}
fn fields() -> &'static EntityFields {
    static FIELDS: OnceLock<EntityFields> = OnceLock::new();
    FIELDS.get_or_init(|| {
        let trajectory = |p: &str| EntityTrajectory {
            time: field(&format!("{p}.trTime")), kind: field(&format!("{p}.trType")),
            duration: field(&format!("{p}.trDuration")),
            base: std::array::from_fn(|i| field(&format!("{p}.trBase[{i}]"))),
            delta: std::array::from_fn(|i| field(&format!("{p}.trDelta[{i}]"))),
        };
        EntityFields { pos: trajectory("pos"), apos: trajectory("apos"), entity_type: field("eType"),
            flags: field("eFlags"), event: field("event"), event_parm: field("eventParm"),
            charge_time: field("constantLight"), electrify_time: field("emplacedOwner") }
    })
}
fn is_player(entity: &EntityState) -> bool { entity.fields[fields().entity_type] == ET_PLAYER }

fn lerp_angle(a: f32, b: f32, f: f32) -> f32 {
    let mut delta = b - a;
    if delta > 180.0 { delta -= 360.0 } else if delta < -180.0 { delta += 360.0 }
    a + delta * f
}

/// CG_InterpolateEntityPosition inputs at the presented source instant:
/// TR_INTERPOLATE origins/angles between the bracketing snapshots.
fn interpolate_entities(a: &[EntityState], b: Option<&[EntityState]>, f: f32) -> Vec<EntityState> {
    let fields = fields();
    a.iter().map(|from| {
        let mut entity = from.clone();
        let to = b.and_then(|b| b.binary_search_by_key(&from.number, |e| e.number).ok().map(|i| &b[i]));
        let Some(to) = to.filter(|to| (from.fields[fields.flags] ^ to.fields[fields.flags]) & EF_TELEPORT_BIT == 0
            && from.fields[fields.entity_type] == to.fields[fields.entity_type]) else { return entity };
        for (trajectory, angles) in [(&fields.pos, false), (&fields.apos, true)] {
            if from.fields[trajectory.kind] as i32 != TR_INTERPOLATE || to.fields[trajectory.kind] as i32 != TR_INTERPOLATE { continue; }
            for i in trajectory.base {
                let (x, y) = (f32::from_bits(from.fields[i]), f32::from_bits(to.fields[i]));
                entity.fields[i] = if angles { lerp_angle(x, y, f) } else { x + (y - x) * f }.to_bits();
            }
        }
        entity
    }).collect()
}

/// CG_InterpolatePlayerState (demo playback): origin, velocity and angles.
fn interpolate_player(a: &PlayerState, b: Option<&PlayerState>, f: f32) -> PlayerState {
    let mut ps = a.clone();
    let Some(b) = b else { return ps };
    let same = |name: &str| a.field_bits(name) == b.field_bits(name);
    if !same("clientNum") || !same("pm_type")
        || (a.field_bits("eFlags").unwrap_or(0) ^ b.field_bits("eFlags").unwrap_or(0)) & EF_TELEPORT_BIT != 0 { return ps; }
    for i in 0..3 {
        for (name, angle) in [("origin", false), ("velocity", false), ("viewangles", true)] {
            let name = format!("{name}[{i}]");
            let (x, y) = (a.field_f32(&name).unwrap_or(0.0), b.field_f32(&name).unwrap_or(0.0));
            ps.set_field_bits(&name, if angle { lerp_angle(x, y, f) } else { x + (y - x) * f }.to_bits());
        }
    }
    ps
}

fn shift_clock(value: u32, shift: i32) -> u32 {
    let value = value as i32;
    if value > 0 { value.saturating_add(shift) as u32 } else { value as u32 }
}

/// Re-express a parametric trajectory on the output clock. At timeline speed 1
/// a constant shift is exact; otherwise linear/gravity paths are rebased at
/// the presented source instant with their rate scaled by the speed.
fn retime_trajectory(entity: &mut EntityState, trajectory: &EntityTrajectory, source: f64, out: i32, speed: f64) {
    let kind = entity.fields[trajectory.kind] as i32;
    if kind < TR_LINEAR { return; }
    let start = entity.fields[trajectory.time] as i32;
    if (speed - 1.0).abs() <= 0.001 || !matches!(kind, TR_LINEAR | TR_LINEAR_STOP | TR_GRAVITY) {
        entity.fields[trajectory.time] = start.wrapping_add((f64::from(out) - source).round() as i32) as u32;
        return;
    }
    // BG_EvaluateTrajectory at the source instant.
    let duration = entity.fields[trajectory.duration] as i32;
    let end = f64::from(start) + f64::from(duration);
    let at = if kind == TR_LINEAR_STOP { source.min(end) } else { source };
    let mut dt = ((at - f64::from(start)) * 0.001) as f32;
    if kind == TR_LINEAR_STOP { dt = dt.max(0.0); }
    let mut position: [f32; 3] = std::array::from_fn(|i|
        f32::from_bits(entity.fields[trajectory.base[i]]) + f32::from_bits(entity.fields[trajectory.delta[i]]) * dt);
    let mut velocity: [f32; 3] = std::array::from_fn(|i| f32::from_bits(entity.fields[trajectory.delta[i]]));
    if kind == TR_GRAVITY {
        position[2] -= 0.5 * DEFAULT_GRAVITY * dt * dt;
        velocity[2] -= DEFAULT_GRAVITY * dt;
    }
    if kind == TR_LINEAR_STOP {
        let remaining = (end - source).max(0.0);
        entity.fields[trajectory.duration] = if speed > 0.001 { (remaining / speed).round() as i32 } else { 0 } as u32;
    }
    for i in 0..3 {
        entity.fields[trajectory.base[i]] = position[i].to_bits();
        entity.fields[trajectory.delta[i]] = (velocity[i] * speed as f32).to_bits();
    }
    entity.fields[trajectory.time] = out as u32;
}

/// BG_PlayerStateToEntityState, including the playerState event queue the
/// server folds into the entity (one event per snapshot, held until replaced).
fn player_entity(ps: &PlayerState, events: &mut HashMap<u16, (i32, u32, u32)>) -> Option<EntityState> {
    let mut entity = crate::cgame::player_state_to_entity_state(ps)?;
    if ps.field_i32("externalEvent").unwrap_or(0) == 0 {
        let fields = fields();
        let sequence = ps.field_i32("eventSequence").unwrap_or(0);
        let held = events.entry(entity.number).or_insert((sequence, 0, 0));
        if held.0 != sequence {
            let latest = sequence.wrapping_sub(1);
            let slot = (latest & 1) as usize;
            let event = ps.field_i32(["events[0]", "events[1]"][slot]).unwrap_or(0);
            let parm = ps.field_i32(["eventParms[0]", "eventParms[1]"][slot]).unwrap_or(0);
            *held = (sequence, (event | ((latest & 3) << 8)) as u32, parm as u32);
        }
        entity.fields[fields.event] = held.1;
        entity.fields[fields.event_parm] = held.2;
    }
    Some(entity)
}

/// A free spectator at the Studio camera: PM_SPECTATOR forces cgame first
/// person, no weapon, no view bob/damage kicks, and no viewheight offset.
fn camera_player_state(origin: [f32; 3], angles: [f32; 3], slot: u16, time: i32) -> PlayerState {
    let mut ps = PlayerState::default();
    ps.set_field_bits("commandTime", time as u32);
    for i in 0..3 {
        ps.set_field_bits(&format!("origin[{i}]"), origin[i].to_bits());
        ps.set_field_bits(&format!("viewangles[{i}]"), angles[i].to_bits());
    }
    ps.set_field_bits("pm_type", PM_SPECTATOR);
    ps.set_field_bits("clientNum", u32::from(slot));
    ps.set_field_bits("groundEntityNum", ENTITYNUM_NONE);
    ps.stats[STAT_HEALTH] = 100;
    ps.persistant[PERS_TEAM] = TEAM_SPECTATOR;
    ps
}

struct GameState { map: String, baselines: BTreeMap<u16, EntityState>, checksum: u32 }
struct SourceFrame {
    /// Milliseconds since the first active snapshot (the Studio source clock).
    time: i32,
    gamestate: usize,
    config: usize,
    snap: Snapshot,
    /// Reliable text commands that arrived with this snapshot (not `cs`).
    commands: Vec<Vec<u8>>,
}
struct Recording {
    gamestates: Vec<GameState>,
    configs: Vec<BTreeMap<u16, Vec<u8>>>,
    frames: Vec<SourceFrame>,
    first_server_time: i32,
    clients: BTreeSet<u16>,
    race_anchor: Option<i32>,
}
impl Recording {
    /// Latest snapshot at or before `ms`, the next one, and the fraction.
    fn bracket(&self, ms: f64) -> (usize, Option<usize>, f32) {
        let next = self.frames.partition_point(|frame| f64::from(frame.time) <= ms);
        let a = next.saturating_sub(1);
        match self.frames.get(next) {
            Some(b) if next > 0 && b.gamestate == self.frames[a].gamestate => {
                let start = f64::from(self.frames[a].time);
                let span = f64::from(b.time) - start;
                (a, Some(next), if span > 0.0 { ((ms - start) / span).clamp(0.0, 1.0) as f32 } else { 0.0 })
            }
            _ => (a, None, 0.0),
        }
    }
}

/// The project's managed copy, exactly as the Studio sources load it.
fn recording_bytes(project: &Path, reference: Option<&str>, name: &str) -> Result<Vec<u8>, String> {
    let reference = reference.ok_or_else(|| format!("{name} is not imported into the project"))?;
    let path = super::render_studio_media::media_path(project, reference)?;
    let size = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size == 0 || size > MAX_DEMO_BYTES as u64 { return Err(format!("{name}: invalid recording size")); }
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Managed media reference for a (name, game) recording: a clip's own
/// import, else any composite assignment of it.
fn media_reference<'a>(clips: &'a [StudioDemoClip], name: &str, game: &str) -> Option<&'a str> {
    clips.iter().filter(|c| c.name == name && c.game == game).find_map(|c| c.source_path.as_deref())
        .or_else(|| clips.iter().flat_map(|c| &c.race_overlays)
            .filter(|o| o.name == name && o.game == game).find_map(|o| o.source_path.as_deref()))
}

/// Commands that would end or restart playback, or that are configstrings.
fn replayable(name: &[u8]) -> bool {
    !["disconnect", "map_restart", "cs", "bcs0", "bcs1", "bcs2"].iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked.as_bytes()))
}

/// Decode a recording, keeping snapshots only inside `wanted` source ranges
/// (milliseconds, sorted and merged) so long demos stay bounded in memory.
fn decode_recording(bytes: &[u8], wanted: &[(f64, f64)]) -> Result<Recording, String> {
    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut recording = Recording { gamestates: Vec::new(), configs: Vec::new(), frames: Vec::new(),
        first_server_time: 0, clients: BTreeSet::new(), race_anchor: None };
    let mut config = BTreeMap::<u16, Vec<u8>>::new();
    let (mut config_dirty, mut clients_dirty) = (true, true);
    let mut big = BigConfigString::default();
    let mut pending = Vec::<Vec<u8>>::new();
    let (mut first, mut last, mut count) = (None::<i32>, None::<i32>, 0usize);
    while let Some(record) = reader.next_record().map_err(|e| format!("Invalid DM26 record: {e}"))? {
        let packet = decoder.parse_packet(record.sequence, &record.payload)
            .map_err(|e| format!("Source packet {}: {e}", record.sequence))?;
        for event in packet.events {
            match event {
                Event::Gamestate { .. } => {
                    let map = decoder.map_name().ok_or("Demo gamestate has no mapname")?;
                    recording.gamestates.push(GameState { map, baselines: decoder.baselines.clone(),
                        checksum: decoder.checksum_feed });
                    if let Ok(client) = u16::try_from(decoder.client_number) { recording.clients.insert(client.min(1023)); }
                    config = decoder.configstrings.clone();
                    (config_dirty, clients_dirty) = (true, true);
                    pending.clear();
                    big = BigConfigString::default();
                }
                Event::ServerCommand(command) => pending.push(command.text),
                Event::Snapshot { .. } => {
                    let Some(snap) = decoder.latest_snapshot() else { continue };
                    if snap.snap_flags & 2 != 0 { continue; } // SNAPFLAG_NOT_ACTIVE
                    if recording.gamestates.is_empty() { return Err("Snapshot before gamestate".into()); }
                    let mut commands = Vec::new();
                    for text in pending.drain(..) {
                        let text = match big.feed(&text) {
                            BigConfigOutcome::PassThrough => text,
                            BigConfigOutcome::Complete(text) => text,
                            BigConfigOutcome::Pending | BigConfigOutcome::Overflow => continue,
                        };
                        let args = tokenize(&text);
                        let name = args.first().map(Vec::as_slice).unwrap_or_default();
                        if name.eq_ignore_ascii_case(b"cs") {
                            // CL_ConfigstringModified: index, then Cmd_ArgsFrom(2).
                            let index = args.get(1).map_or(-1, |index| atoi(index));
                            if let Some(index) = u16::try_from(index).ok().filter(|i| *i < MAX_CONFIGSTRINGS) {
                                config.insert(index, args.get(2..).unwrap_or_default().join(&b' '));
                                (config_dirty, clients_dirty) = (true, true);
                            }
                        } else if replayable(name) {
                            commands.push(text);
                        }
                    }
                    let first = *first.get_or_insert(snap.server_time);
                    let elapsed = snap.server_time.saturating_sub(first);
                    if last.is_some_and(|last| elapsed < last) {
                        return Err("Source demo server clock goes backwards; split multi-map recordings into separate clips".into());
                    }
                    last = Some(elapsed);
                    count += 1;
                    if count > MAX_SOURCE_FRAMES { return Err("Source recording exceeds 200,000 snapshots".into()); }
                    // Never hand an occupied slot to the camera or a composite actor.
                    if let Ok(client) = u16::try_from(snap.player_state.field_i32("clientNum").unwrap_or(-1)) {
                        recording.clients.insert(client);
                    }
                    recording.clients.extend(snap.entities.iter().map(|e| e.number).filter(|n| *n < 32));
                    if clients_dirty {
                        recording.clients.extend((0u16..32).filter(|c| config.get(&(CS_PLAYERS + c)).is_some_and(|v| !v.is_empty())));
                        clients_dirty = false;
                    }
                    let at = f64::from(elapsed);
                    let keep = wanted.partition_point(|range| range.1 < at) < wanted.len()
                        && wanted[wanted.partition_point(|range| range.1 < at)].0 <= at;
                    if keep {
                        if config_dirty || recording.configs.is_empty() {
                            recording.configs.push(config.clone());
                            config_dirty = false;
                        }
                        recording.frames.push(SourceFrame { time: elapsed, gamestate: recording.gamestates.len() - 1,
                            config: recording.configs.len() - 1, snap: snap.clone(), commands });
                    }
                }
                _ => {}
            }
        }
    }
    if recording.frames.is_empty() { return Err("No active snapshots in the selected source range".into()); }
    recording.first_server_time = first.unwrap_or(0);
    Ok(recording)
}

/// SV_SendConfigstring: `cs`, or `bcs0/1/2` chunks for long strings. None
/// when a value cannot survive the quoted command (needs a gamestate).
fn configstring_commands(index: u16, value: &[u8]) -> Option<Vec<Vec<u8>>> {
    if value.iter().any(|&b| b == b'"' || b == 0) { return None; }
    let command = |name: &str, chunk: &[u8]| {
        let mut text = format!("{name} {index} \"").into_bytes();
        text.extend_from_slice(chunk);
        text.extend_from_slice(b"\"\n");
        text
    };
    if value.len() < MAX_CONFIGSTRING_CHUNK { return Some(vec![command("cs", value)]); }
    let (mut sent, mut commands) = (0, Vec::new());
    while sent < value.len() {
        let remaining = value.len() - sent;
        let name = if sent == 0 { "bcs0" } else if remaining < MAX_CONFIGSTRING_CHUNK { "bcs2" } else { "bcs1" };
        let end = (sent + MAX_CONFIGSTRING_CHUNK - 1).min(value.len());
        commands.push(command(name, &value[sent..end]));
        sent = end;
    }
    Some(commands)
}

fn configstring_diff(old: &BTreeMap<u16, Vec<u8>>, new: &BTreeMap<u16, Vec<u8>>) -> Option<Vec<Vec<u8>>> {
    let mut commands = Vec::new();
    for (&index, value) in new {
        if old.get(&index) != Some(value) { commands.extend(configstring_commands(index, value)?); }
    }
    for &index in old.keys().filter(|index| !new.contains_key(index)) {
        commands.extend(configstring_commands(index, b"")?);
    }
    Some(commands)
}

fn gamestate_payload(config: &BTreeMap<u16, Vec<u8>>, gamestate: &GameState, client: u16) -> Result<Vec<u8>, String> {
    let mut decoder = Decoder::new();
    decoder.configstrings = config.clone();
    decoder.baselines = gamestate.baselines.clone();
    decoder.client_number = i32::from(client);
    decoder.checksum_feed = gamestate.checksum;
    demo::synthesize_gamestate_payload(&decoder, 0).map_err(|e| format!("Gamestate encode failed: {e}"))
}

struct Overlay { slot: u16, info: Vec<u8>, track: race_ghost::RaceGhostTrack }
impl Overlay {
    /// Interpolated ghost playerState at race clock `ms` (as RaceGhost::sample).
    fn state_at(&self, ms: f64) -> Option<PlayerState> {
        let frames = &self.track.frames;
        let next = frames.partition_point(|frame| f64::from(frame.elapsed_ms) <= ms);
        let a = frames.get(next.saturating_sub(1))?;
        let b = frames.get(next).filter(|_| next > 0);
        let f = b.map_or(0.0, |b| {
            let span = f64::from(b.elapsed_ms - a.elapsed_ms);
            if span > 0.0 { ((ms - f64::from(a.elapsed_ms)) / span).clamp(0.0, 1.0) as f32 } else { 0.0 }
        });
        let mut ps = interpolate_player(&a.player_state, b.map(|b| &b.player_state), f);
        ps.set_field_bits("clientNum", u32::from(self.slot));
        // Clocks of another recording mean nothing on this one.
        for name in ["weaponChargeTime", "electrifyTime"] { ps.set_field_bits(name, 0); }
        Some(ps)
    }
}

struct BakeJob {
    /// Render Studio project folder (managed media root).
    project: PathBuf,
    clips: Vec<StudioDemoClip>,
    frames: Vec<Dm26Frame>,
    begin: f64,
    fps: u32,
    warnings: BTreeSet<String>,
}

/// Result includes non-representable edits; the file itself is strictly stock.
pub(in crate::app) struct BakeResult { pub path: PathBuf, pub frames: usize, pub warnings: Vec<String> }

fn bake(job: BakeJob, output: &Path) -> Result<BakeResult, String> {
    let mut warnings = job.warnings;
    let fps = f64::from(job.fps);
    // Source ranges each recording must keep around the sampled instants.
    let mut wanted = HashMap::<(String, String), Vec<(f64, f64)>>::new();
    for frame in &job.frames {
        let Some(clip) = frame.clip.map(|index| &job.clips[index]) else { continue };
        wanted.entry((clip.name.clone(), clip.game.clone())).or_default()
            .push((frame.source_ms - SOURCE_PADDING_MS, frame.source_ms + SOURCE_PADDING_MS));
    }
    if wanted.is_empty() { return Err("No demo clip is visible in the export range".into()); }
    let mut sources = HashMap::<(String, String), Recording>::new();
    for (key, mut ranges) in wanted {
        ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged = Vec::<(f64, f64)>::new();
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.0 <= last.1 => last.1 = last.1.max(range.1),
                _ => merged.push(range),
            }
        }
        let bytes = recording_bytes(&job.project, media_reference(&job.clips, &key.0, &key.1), &key.0)?;
        let mut recording = decode_recording(&bytes, &merged).map_err(|e| format!("{}: {e}", key.0))?;
        // Same race anchor the Studio source uses for non-comparison overlays.
        if job.clips.iter().any(|c| c.name == key.0 && c.game == key.1 && c.comparison.is_none()
            && c.race_overlays.iter().any(|o| o.enabled)) {
            recording.race_anchor = race_ghost::parse_track(key.0.clone(), bytes).ok().map(|t| t.race_start_server_ms);
        }
        sources.insert(key, recording);
    }
    // The camera and composite actors take the lowest slots no recording uses.
    let mut used = sources.values().flat_map(|s| s.clients.iter().copied()).collect::<BTreeSet<u16>>();
    let mut allocate = || { let slot = (0u16..32).find(|slot| !used.contains(slot))?; used.insert(slot); Some(slot) };
    let spectator = allocate().ok_or("No free client slot for the DM26 camera (all 32 are used by the recordings)")?;
    let mut overlays = HashMap::<(String, String), Overlay>::new();
    let used_clips = job.frames.iter().filter_map(|f| f.clip).collect::<BTreeSet<_>>();
    for setting in used_clips.iter().flat_map(|&i| job.clips[i].race_overlays.iter()).filter(|o| o.enabled) {
        let key = (setting.name.clone(), setting.game.clone());
        if overlays.contains_key(&key) { continue; }
        let reference = setting.source_path.as_deref().or_else(|| media_reference(&job.clips, &key.0, &key.1));
        let track = match recording_bytes(&job.project, reference, &key.0).and_then(|bytes| race_ghost::parse_track(key.0.clone(), bytes)) {
            Ok(track) => track,
            Err(error) => { warnings.insert(format!("Composite actor {} omitted: {error}", key.0)); continue; }
        };
        let Some(slot) = allocate() else { warnings.insert("No free client slots left for composite actors".into()); break; };
        let info = track.configstrings.get(&(CS_PLAYERS + track.client_num as u16)).cloned()
            .unwrap_or_else(|| br"\n\Composite\t\0".to_vec());
        overlays.insert(key, Overlay { slot, info, track });
    }

    let mut writer = std::io::BufWriter::new(std::fs::File::create(output)
        .map_err(|e| format!("Unable to create output: {e}"))?);
    let mut validation = Decoder::new();
    let mut sequence = 1_i32;
    let mut previous = None::<Snapshot>;
    let mut active = None::<(String, String)>;
    let mut active_config = BTreeMap::<u16, Vec<u8>>::new();
    let mut active_baselines = BTreeMap::new();
    let mut command_seq = 0_i32;
    // (clip, source ms, bracket snapshot) of the previous written frame.
    let mut last = None::<(usize, f64, usize)>;
    let mut last_view = None::<Dm26View>;
    let mut ps_parity = 0_u32;
    let mut entity_parity = HashMap::<u16, u32>::new();
    let mut events = HashMap::<u16, (i32, u32, u32)>::new();
    let mut config_shift = None::<i32>;
    let (mut written, mut mismatches) = (0usize, 0usize);
    for (index, frame) in job.frames.iter().enumerate() {
        let Some(clip_index) = frame.clip else {
            warnings.insert("Timeline gaps have no source snapshots; the last frame holds until the next clip".into());
            last = None;
            continue;
        };
        let clip = &job.clips[clip_index];
        let source = &sources[&(clip.name.clone(), clip.game.clone())];
        let time = job.begin + index as f64 / fps;
        let out = OUTPUT_EPOCH_MS + (index as f64 * 1000.0 / fps).round() as i32;
        let (a, b, f) = source.bracket(frame.source_ms);
        let (from, to) = (&source.frames[a], b.map(|b| &source.frames[b]));
        let gamestate = &source.gamestates[from.gamestate];
        let source_time = f64::from(source.first_server_time) + frame.source_ms;
        let shift = (f64::from(out) - source_time).round() as i32;
        // Timeline speed (source ms per output ms) from the neighbouring samples.
        let speed = [(index + 1, index), (index, index.wrapping_sub(1))].iter().find_map(|&(next, prev)| {
            let (next, prev) = (job.frames.get(next)?, job.frames.get(prev)?);
            (next.clip == Some(clip_index) && prev.clip == Some(clip_index))
                .then(|| (next.source_ms - prev.source_ms) * fps / 1000.0)
        }).unwrap_or(1.0).max(0.0);
        let mut cut = !last.is_some_and(|(c, s, _)| c == clip_index && frame.source_ms >= s && frame.source_ms - s <= 1000.0);

        let mut config = source.configs[from.config].clone();
        config.insert(CS_PLAYERS + spectator, SPECTATOR_INFO.to_vec());
        for setting in clip.race_overlays.iter().filter(|o| o.enabled) {
            if let Some(overlay) = overlays.get(&(setting.name.clone(), setting.game.clone())) {
                config.insert(CS_PLAYERS + overlay.slot, overlay.info.clone());
            }
        }
        // Game clock/warmup/vote times follow the output clock; resync on cuts
        // and only on large slow-motion drift so it is not a per-frame command.
        if cut || config_shift.is_none_or(|s| (s - shift).abs() > 1000) { config_shift = Some(shift); }
        for cs in CLOCK_CONFIGSTRINGS {
            if let Some(value) = config.get_mut(&cs) {
                let clock = atoi(value);
                if clock > 0 { *value = clock.saturating_add(config_shift.unwrap_or(0)).to_string().into_bytes(); }
            }
        }
        let identity = (gamestate.map.clone(), clip.game.clone());
        let mut commands = Vec::new();
        let diff = (active.as_ref() == Some(&identity)).then(|| configstring_diff(&active_config, &config)).flatten()
            .filter(|diff| diff.len() <= MAX_COMMANDS_PER_SNAPSHOT);
        match diff {
            Some(diff) => commands = diff,
            None => {
                if active.as_ref() == Some(&identity) {
                    warnings.insert("A cut between recordings changed too many configstrings; a full gamestate (brief map reload) was written".into());
                } else if active.is_some() {
                    warnings.insert("Map/mod changes write a new gamestate; clients reload the map at that cut".into());
                }
                let payload = gamestate_payload(&config, gamestate, spectator)?;
                demo::write_record(&mut writer, sequence, &payload).map_err(|e| e.to_string())?;
                validation.parse_packet(sequence, &payload).map_err(|e| format!("Gamestate validation: {e}"))?;
                sequence = sequence.checked_add(1).ok_or("Output packet counter overflow")?;
                (previous, command_seq, cut) = (None, 0, true);
                active = Some(identity);
                active_baselines = gamestate.baselines.clone();
            }
        }
        active_config = config;
        // Reliable text (prints, chat, center prints, scores) of the source
        // snapshots passed since the previous output frame.
        if let Some((_, _, previous_a)) = last.filter(|_| !cut) {
            if a > previous_a && a - previous_a <= 64 {
                commands.extend(source.frames[previous_a + 1..=a].iter().flat_map(|f| f.commands.iter().cloned()));
            }
        }
        if commands.len() > MAX_COMMANDS_PER_SNAPSHOT {
            commands.truncate(MAX_COMMANDS_PER_SNAPSHOT);
            warnings.insert("Some source server text commands were dropped to stay within the reliable command window".into());
        }
        if cut { events.clear(); }

        let mut recorded = interpolate_player(&from.snap.player_state, to.map(|to| &to.snap.player_state), f);
        for name in PLAYER_CLOCKS {
            if let Some(value) = recorded.field_bits(name) { recorded.set_field_bits(name, shift_clock(value, shift)); }
        }
        let recorded_client = recorded.field_i32("clientNum").and_then(|c| u16::try_from(c).ok());
        let mut snap = from.snap.clone();
        snap.entities = interpolate_entities(&from.snap.entities, to.map(|to| to.snap.entities.as_slice()), f);
        let entity_fields = fields();
        for entity in &mut snap.entities {
            retime_trajectory(entity, &entity_fields.pos, source_time, out, speed);
            retime_trajectory(entity, &entity_fields.apos, source_time, out, speed);
            let entity_type = entity.fields[entity_fields.entity_type];
            if entity_type == ET_PLAYER || entity_type == ET_NPC {
                for i in [entity_fields.charge_time, entity_fields.electrify_time] {
                    entity.fields[i] = shift_clock(entity.fields[i], shift);
                }
            }
        }
        if let Some(players) = &frame.players {
            snap.entities.retain(|entity| !is_player(entity) || players.contains(&entity.number));
        }
        match frame.view {
            Dm26View::Recorded => {
                snap.player_state = recorded;
            }
            Dm26View::Camera { origin, angles, hide } => {
                // The recorded player is an ordinary entity seen by the camera.
                let visible = frame.players.as_ref().is_none_or(|p| recorded_client.is_some_and(|c| p.contains(&c)));
                if visible {
                    if let Some(actor) = player_entity(&recorded, &mut events) {
                        if actor.number < 32 && actor.number != spectator && !snap.entities.iter().any(|e| e.number == actor.number) {
                            snap.entities.push(actor);
                        }
                    }
                }
                if let Some(hide) = hide { snap.entities.retain(|e| e.number != hide); }
                // Never put the lens inside a body (e.g. a follow fallback to eyes).
                snap.entities.retain(|entity| {
                    if !is_player(entity) { return true; }
                    let p: [f32; 3] = std::array::from_fn(|i| f32::from_bits(entity.fields[entity_fields.pos.base[i]]));
                    (origin[0] - p[0]).hypot(origin[1] - p[1]) >= 12.0 || origin[2] < p[2] - 24.0 || origin[2] > p[2] + 48.0
                });
                snap.player_state = camera_player_state(origin, angles, spectator, out);
                snap.vehicle_player_state = None;
                snap.area_mask = [0; 32];
                warnings.insert("Detached cameras only see what the recording's player could see: entities outside the original PVS were never recorded".into());
            }
        }
        for setting in clip.race_overlays.iter().filter(|o| o.enabled) {
            let Some(overlay) = overlays.get(&(setting.name.clone(), setting.game.clone())) else { continue };
            if !overlay.track.map_name.eq_ignore_ascii_case(&gamestate.map) {
                warnings.insert(format!("Composite {} is on {}, not {}; omitted", setting.name, overlay.track.map_name, gamestate.map));
                continue;
            }
            let duration = overlay.track.duration_ms();
            let clock = match &clip.comparison {
                Some(race) => {
                    let sample = race.sample(time - clip.start);
                    race.runners.iter().position(|r| r.name == setting.name && r.game == setting.game)
                        .map(|i| f64::from(sample.clocks[i].clamp(0, duration)))
                }
                None => source.race_anchor.map(|anchor| source_time - f64::from(anchor))
                    .filter(|clock| *clock >= 0.0).map(|clock| clock.min(f64::from(duration))),
            };
            if let Some(entity) = clock.and_then(|clock| overlay.state_at(clock)).and_then(|ps| player_entity(&ps, &mut events)) {
                snap.entities.retain(|e| e.number != entity.number);
                snap.entities.push(entity);
            }
            if setting.alpha < 1.0 {
                warnings.insert("Composite transparency/silhouettes are unavailable in stock JKA; actors are opaque".into());
            }
        }
        snap.entities.retain(|entity| entity.number != spectator);
        snap.entities.sort_by_key(|entity| entity.number);
        snap.entities.dedup_by_key(|entity| entity.number);
        if snap.entities.len() > MAX_SNAPSHOT_ENTITIES {
            snap.entities.truncate(MAX_SNAPSHOT_ENTITIES);
            warnings.insert("Snapshots over 256 entities were truncated (cgame limit)".into());
        }

        // EF_TELEPORT_BIT: cgame must not interpolate across cuts.
        let camera_cut = match (last_view, frame.view) {
            (Some(Dm26View::Camera { origin: o0, angles: a0, .. }), Dm26View::Camera { origin, angles, .. }) => {
                let distance = (0..3).map(|i| (origin[i] - o0[i]).powi(2)).sum::<f32>().sqrt();
                let turn = (0..3).map(|i| lerp_angle(a0[i], angles[i], 1.0) - a0[i]).map(f32::abs).fold(0.0, f32::max);
                distance > CAMERA_CUT_UNITS || turn > CAMERA_CUT_DEGREES
            }
            _ => false,
        };
        let flags = |ps: &PlayerState| ps.field_bits("eFlags").unwrap_or(0);
        if cut || camera_cut {
            let source_bit = flags(&snap.player_state) & EF_TELEPORT_BIT;
            ps_parity = previous.as_ref()
                .filter(|p| p.player_state.field_bits("clientNum") == snap.player_state.field_bits("clientNum"))
                .map_or(0, |p| source_bit ^ (flags(&p.player_state) & EF_TELEPORT_BIT ^ EF_TELEPORT_BIT));
        }
        let ps_flags = flags(&snap.player_state) ^ ps_parity;
        snap.player_state.set_field_bits("eFlags", ps_flags);
        if cut {
            entity_parity.clear();
            if let Some(previous) = &previous {
                for entity in &snap.entities {
                    if let Ok(i) = previous.entities.binary_search_by_key(&entity.number, |e| e.number) {
                        let old = previous.entities[i].fields[entity_fields.flags] & EF_TELEPORT_BIT;
                        let source_bit = entity.fields[entity_fields.flags] & EF_TELEPORT_BIT;
                        entity_parity.insert(entity.number, source_bit ^ old ^ EF_TELEPORT_BIT);
                    }
                }
            }
        }
        entity_parity.retain(|number, _| snap.entities.binary_search_by_key(number, |e| e.number).is_ok());
        for entity in &mut snap.entities {
            entity.fields[entity_fields.flags] ^= entity_parity.get(&entity.number).copied().unwrap_or(0);
        }

        snap.server_time = out;
        snap.snap_flags = 0;
        snap.message_num = sequence;
        snap.delta_num = if previous.is_some() { sequence - 1 } else { -1 };
        canonicalize_snapshot(&mut snap);
        let numbered = commands.into_iter().map(|text| { command_seq += 1; (command_seq, text) }).collect::<Vec<_>>();
        let payload = synthesize_snapshot_payload(sequence, 0, previous.as_ref(), &snap, &active_baselines, &numbered)
            .map_err(|e| format!("Snapshot {index}: {e}"))?;
        demo::write_record(&mut writer, sequence, &payload).map_err(|e| e.to_string())?;
        validation.parse_packet(sequence, &payload)
            .map_err(|e| format!("Baked snapshot validation failed at frame {index}: {e}"))?;
        let decoded = validation.latest_snapshot().cloned()
            .filter(|decoded| decoded.message_num == sequence)
            .ok_or_else(|| format!("Encoder produced no snapshot at frame {index}"))?;
        if decoded.server_time != snap.server_time || decoded.entities != snap.entities || decoded.player_state != snap.player_state {
            mismatches += 1;
        }
        // Delta from exactly what a client holds, never from the encoder's input.
        previous = Some(decoded);
        last = Some((clip_index, frame.source_ms, a));
        last_view = Some(frame.view);
        written += 1;
        sequence = sequence.checked_add(1).ok_or("Output packet counter overflow")?;
    }
    if written == 0 { return Err("No active demo frames in the export range".into()); }
    if mismatches > 0 {
        warnings.insert(format!("{mismatches} snapshots decoded differently than authored (wire quantization)"));
    }
    demo::write_end(&mut writer).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(BakeResult { path: output.to_path_buf(), frames: written, warnings: warnings.into_iter().collect() })
}

impl App {
    pub(in crate::app) fn start_render_studio_dm26_export(&mut self) {
        let studio = &mut self.render_studio;
        if studio.export.is_some() || studio.still.is_some() || studio.dm26_capture.is_some() || studio.dm26_export.is_some() {
            studio.status = "An export is already running".into();
            return;
        }
        if studio.timeline.demos.is_empty() {
            studio.status = "DM26 export needs a recorded demo clip on the timeline".into();
            return;
        }
        if studio.timeline.demos.iter().any(|clip| !clip.valid()) {
            studio.status = "Invalid Studio demo clip path or trim".into();
            return;
        }
        let fps = studio.fps;
        let (begin, end) = studio.timeline.export_range();
        let total = ((end - begin) * f64::from(fps)).ceil().max(1.0) as u64;
        if total > MAX_EXPORT_FRAMES {
            studio.status = format!("Trim the range to {MAX_EXPORT_FRAMES} output frames or fewer");
            return;
        }
        let timeline = &studio.timeline;
        let mut warnings = BTreeSet::new();
        if !timeline.clips.is_empty() { warnings.insert("Animation (character/emote) tracks have no network entity and are omitted".into()); }
        if timeline.audio_tracks.iter().any(|t| !t.clips.is_empty()) { warnings.insert("Imported audio cannot be stored in a DM26".into()); }
        if !timeline.light_layers.is_empty() { warnings.insert("Studio lights cannot be represented in a DM26".into()); }
        if timeline.demos.iter().any(|c| c.transition_in != StudioDemoTransition::Cut) {
            warnings.insert("Video transitions play as hard cuts".into());
        }
        if timeline.camera.iter().any(|k| (k.fov - 80.0).abs() > 0.01) || timeline.director_shots.iter().any(|s| s.enabled) {
            warnings.insert("Camera FOV and lens effects are the viewer's cg_fov; position and angles are exported".into());
        }
        warnings.insert("Viewers: cg_draw2D 0 hides the spectator HUD on detached camera shots".into());
        studio.timeline.seek(begin);
        studio.preview_sample_time = None;
        studio.playing = false;
        studio.dm26_capture = Some(StudioDm26Capture { index: 0, total, fps, begin,
            frames: Vec::with_capacity(total as usize), warnings });
        studio.status = format!("Capturing the Studio camera for {total} DM26 frames…");
        self.egui_repaint_requested = true;
    }

    /// Called by the Studio source tick once per sampled capture instant.
    pub(in crate::app) fn record_render_studio_dm26_frame(&mut self, frame: Dm26Frame) {
        let Some(capture) = self.render_studio.dm26_capture.as_mut() else { return };
        if capture.index >= capture.total { return; }
        capture.frames.push(frame);
        capture.index += 1;
    }

    pub(in crate::app) fn cancel_render_studio_dm26_export(&mut self, status: impl Into<String>) {
        self.render_studio.dm26_capture = None;
        self.render_studio.status = status.into();
    }

    /// Advance the capture pass, hand a finished capture to the worker and
    /// report worker results. Runs every app tick.
    pub(in crate::app) fn tick_render_studio_dm26_export(&mut self) {
        self.poll_render_studio_dm26_export();
        let Some((index, total)) = self.render_studio.dm26_capture.as_ref().map(|c| (c.index, c.total)) else { return };
        if !self.front_end || self.frontend_page != FrontendPage::RenderStudio {
            self.cancel_render_studio_dm26_export("DM26 export cancelled");
            return;
        }
        if self.render_studio.demo_preview.as_ref().is_some_and(|p| p.failed) {
            self.cancel_render_studio_dm26_export("DM26 export failed: a demo clip could not load");
            self.push_console_line(format!("^1RENDER STUDIO: {}", self.render_studio.status));
            return;
        }
        if index < total { return; }
        let capture = self.render_studio.dm26_capture.take().expect("checked above");
        let job = BakeJob { project: self.studio_project_directory(), clips: self.render_studio.timeline.demos.clone(),
            frames: capture.frames, begin: capture.begin, fps: capture.fps, warnings: capture.warnings };
        let folder = self.game_write_path("demos");
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let output = folder.join(format!("studio_{stamp}.dm_26"));
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("studio-dm26-bake".into()).spawn(move || {
            let result = std::fs::create_dir_all(&folder).map_err(|e| e.to_string()).and_then(|()| {
                let partial = output.with_extension("dm_26.partial");
                match bake(job, &partial) {
                    Ok(mut baked) => {
                        std::fs::rename(&partial, &output).map_err(|e| e.to_string())?;
                        baked.path = output;
                        Ok(baked)
                    }
                    Err(error) => { let _ = std::fs::remove_file(&partial); Err(error) }
                }
            });
            let _ = sender.send(result);
        });
        match spawned {
            Ok(_) => {
                self.render_studio.dm26_export = Some(receiver);
                self.render_studio.status = "Writing DM26 snapshots…".into();
            }
            Err(e) => self.render_studio.status = format!("DM26 worker: {e}"),
        }
    }

    fn poll_render_studio_dm26_export(&mut self) {
        let Some(receiver) = self.render_studio.dm26_export.as_ref() else { return };
        match receiver.try_recv() {
            Ok(result) => {
                self.render_studio.dm26_export = None;
                match result {
                    Ok(output) => {
                        self.render_studio.status = format!("DM26 exported: {} ({} snapshots, {} notes in console)",
                            output.path.display(), output.frames, output.warnings.len());
                        self.push_console_line(format!("^2RENDER STUDIO: {}", self.render_studio.status));
                        for warning in output.warnings { self.push_console_line(format!("^3DM26: {warning}")); }
                    }
                    Err(error) => {
                        self.render_studio.status = format!("DM26 export failed: {error}");
                        self.push_console_line(format!("^1RENDER STUDIO: {}", self.render_studio.status));
                    }
                }
                self.egui_repaint_requested = true;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.render_studio.dm26_export = None;
                self.render_studio.status = "DM26 export worker exited unexpectedly".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_configstrings_split_like_sv_sendconfigstring() {
        let value = vec![b'x'; 2500];
        let commands = configstring_commands(1131, &value).unwrap();
        let names: Vec<_> = commands.iter().map(|c| tokenize(c)[0].clone()).collect();
        assert_eq!(names, [b"bcs0".to_vec(), b"bcs1".to_vec(), b"bcs2".to_vec()]);
        let mut big = BigConfigString::default();
        let mut joined = None;
        for command in &commands {
            if let BigConfigOutcome::Complete(text) = big.feed(command) { joined = Some(text); }
        }
        assert_eq!(tokenize(&joined.unwrap())[2], value);
        assert!(configstring_commands(5, b"a\"b").is_none());
        assert_eq!(configstring_commands(5, b"12").unwrap(), [b"cs 5 \"12\"\n".to_vec()]);
    }

    #[test]
    fn slow_motion_rebases_linear_trajectories() {
        let fields = fields();
        let mut entity = EntityState { number: 70, fields: [0; ENTITY_FIELDS.len()] };
        entity.fields[fields.pos.kind] = TR_LINEAR as u32;
        entity.fields[fields.pos.time] = 1000;
        entity.fields[fields.pos.delta[0]] = 100f32.to_bits();
        retime_trajectory(&mut entity, &fields.pos, 1500.0, 20_000, 0.5);
        assert_eq!(entity.fields[fields.pos.time], 20_000);
        assert_eq!(f32::from_bits(entity.fields[fields.pos.base[0]]), 50.0);
        assert_eq!(f32::from_bits(entity.fields[fields.pos.delta[0]]), 50.0);
        let mut same = entity.clone();
        retime_trajectory(&mut same, &fields.pos, 21_000.0, 21_500, 1.0);
        assert_eq!(same.fields[fields.pos.time], 20_500);
    }

    #[test]
    fn playerstate_events_become_entity_events_once() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("clientNum", 3);
        ps.set_field_bits("eventSequence", 4);
        let mut events = HashMap::new();
        let first = player_entity(&ps, &mut events).unwrap();
        assert_eq!(first.fields[fields().event], 0);
        ps.set_field_bits("eventSequence", 5);
        ps.set_field_bits("events[0]", 27);
        ps.set_field_bits("eventParms[0]", 9);
        let fired = player_entity(&ps, &mut events).unwrap();
        assert_eq!(fired.fields[fields().event], 27 | ((4 & 3) << 8));
        assert_eq!(fired.fields[fields().event_parm], 9);
        assert_eq!(player_entity(&ps, &mut events).unwrap().fields[fields().event], fired.fields[fields().event]);
    }
}
