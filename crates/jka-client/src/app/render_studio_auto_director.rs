//! Auto Director: turns a recording's player path into a cut of Director
//! shots. The recorded playerState (origin, velocity, ground, view height)
//! is reduced to beats (opening, big air, fast straight, turn, slow, finish,
//! closing); each beat picks a fitting shot type from a weighted table with
//! variety rules, and world-anchored shots are placed against the map's BSP
//! collision with line-of-sight checks to the subject.
//!
//! Everything here is pure for a given (path, clip, seed): the same inputs
//! always produce the same edit, and Regenerate only advances the seed.
use super::render_studio_director::{StudioDirectorPreset as P, StudioDirectorShot};
use super::render_studio_timeline::{StudioDemoAnchor, StudioDemoClip};
use glam::Vec3;
use jka_movement::{CollisionWorld, TraceQuery, TraceWorld};
use jka_protocol::server::PlayerState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Camera traces: the same mask as the sampler's chase/orbit clamp.
const MASK: i32 = 0x1 | 0x10 | 0x1000;
const ENTITYNUM_NONE: i32 = 1023;
const EF_TELEPORT_BIT: i32 = 1 << 3;
/// Subject samples tested for line of sight across a shot.
const LOS_SAMPLES: usize = 7;
/// A short recording/PVS gap may hold the composition, but never invent motion.
const VISIBILITY_LINGER_MS: i32 = 750;
/// Editorial limits use the rendered clock, including speed ramps.
const COMBAT_MIN_SHOT: f64 = 2.0;
const COMBAT_MIN_STATIC: f64 = 2.5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum StudioDirectorPacing { Calm, #[default] Balanced, Energetic }
impl StudioDirectorPacing {
    /// Typical coverage shot length, seconds.
    fn shot_seconds(self) -> f64 {
        match self { Self::Calm => 5.5, Self::Balanced => 3.6, Self::Energetic => 2.4 }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct PathSample {
    /// Studio source clock (0 = first active snapshot).
    pub ms: i32,
    /// Native JKA coordinates (Z up).
    pub origin: Vec3,
    pub velocity: Vec3,
    pub ground: bool,
    pub eye_height: f32,
    /// Discontinuity (teleport / respawn) between the previous sample and this one.
    pub cut: bool,
    /// Recorded client (follows spectator switches).
    pub client: i32,
    /// View yaw (degrees): poke detection.
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum CombatKind { Kill, Hit, Clash, Pain, Shot, Throw,
    /// Grip / lightning / drain on a nearby player (sampled while active).
    Force }
impl CombatKind {
    fn weight(self) -> f32 {
        match self { Self::Kill => 8.0, Self::Hit => 3.0, Self::Clash => 2.0, Self::Pain => 1.5, Self::Shot => 0.4, Self::Throw => 1.0, Self::Force => 2.0 }
    }
}

/// A combat moment read from the recording's entities.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct CombatEvent {
    pub ms: i32,
    pub kind: CombatKind,
    /// Native JKA position of the event.
    pub position: Vec3,
    /// Client numbers, -1 when unknown / world.
    pub attacker: i32,
    pub victim: i32,
    /// Kills: means of death (3 = MOD_SABER); -1 otherwise.
    pub means: i32,
}

/// The recorded player's path through one recording.
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct DirectorPath {
    pub map: String,
    pub samples: Vec<PathSample>,
    /// Last `duelTime` race segment (start, end) on the source clock.
    pub race: Option<(i32, i32)>,
    /// Other players' origins (client, [(ms, origin)]) at up to 10 Hz, while visible.
    pub players: Vec<(i32, Vec<(i32, Vec3)>)>,
    /// Kills, saber hits / clashes, pain and fire, in time order.
    pub events: Vec<CombatEvent>,
    /// Snapshot portal visibility, on the same source clock as the samples.
    pub area_masks: Vec<(i32, [u8; 32])>,
    pub alive: HashMap<i32, Vec<(i32, bool)>>,
    pub views: HashMap<i32, Vec<(i32, [f32; 2])>>,
}

#[derive(Clone, Debug)]
struct Engagement {
    start: i32,
    end: i32,
    participants: Vec<i32>,
    kills: Vec<CombatEvent>,
}

/// One decision shared by camera construction, scoring and impact validation.
#[derive(Clone, Debug)]
struct ShotIntent {
    subject: i32,
    kills: Vec<CombatEvent>,
}
impl ShotIntent {
    fn primary(&self) -> Option<CombatEvent> { self.kills.first().copied() }
    fn sequence(&self) -> bool { self.kills.len() > 1 }
}

fn state_vec3(state: &PlayerState, base: &str) -> Option<Vec3> {
    super::playerstate_vec3(state, base).map(Vec3::from_array)
}

impl DirectorPath {
    fn push_state(&mut self, ms: i32, state: &PlayerState, teleport: &mut Option<i32>) {
        let Some(origin) = state_vec3(state, "origin") else { return };
        if !origin.is_finite() { return; }
        let velocity = state_vec3(state, "velocity").filter(|v| v.is_finite()).unwrap_or(Vec3::ZERO);
        let ground = state.field_i32("groundEntityNum").is_none_or(|g| g != ENTITYNUM_NONE);
        let eye_height = state.field_i32("viewheight").filter(|v| (4..=96).contains(v)).unwrap_or(36) as f32;
        let bit = state.field_i32("eFlags").map(|f| f & EF_TELEPORT_BIT);
        let mut cut = bit.is_some() && teleport.is_some() && bit != *teleport;
        *teleport = bit;
        if let Some(previous) = self.samples.last() {
            if ms <= previous.ms { return; }
            let dt = (ms - previous.ms) as f32 / 1000.0;
            let reach = previous.velocity.length().max(velocity.length()) * dt * 2.5 + 160.0;
            if previous.origin.distance(origin) > reach { cut = true; }
        }
        let client = state.field_i32("clientNum").unwrap_or(-1);
        let yaw = state.field_f32("viewangles[1]").filter(|v| v.is_finite()).unwrap_or(0.0);
        let pitch = state.field_f32("viewangles[0]").filter(|v| v.is_finite()).unwrap_or(0.0);
        self.samples.push(PathSample { ms, origin, velocity, ground, eye_height, cut, client, yaw, pitch });
        // JKA PM_DEAD=5; 2 is PM_FLOAT (a living player being gripped).
        let alive = state.stats[0] > 0 && state.field_i32("pm_type") != Some(5);
        let list = self.alive.entry(client).or_default();
        if list.last().is_none_or(|last| last.1 != alive) { list.push((ms, alive)); }
    }

    /// Race segment of a route-comparison run (`race_ghost` track).
    pub fn from_track(track: &super::race_ghost::RaceGhostTrack) -> Self {
        let mut path = Self { map: track.map_name.clone(), ..Self::default() };
        let mut teleport = None;
        for frame in &track.frames {
            path.push_state(track.race_source_start_ms + frame.elapsed_ms, &frame.player_state, &mut teleport);
        }
        path.race = Some((track.race_source_start_ms, track.race_source_start_ms + track.duration_ms()));
        path
    }

    fn index_at(&self, ms: i32) -> usize {
        self.samples.partition_point(|s| s.ms <= ms).saturating_sub(1).min(self.samples.len().saturating_sub(1))
    }
    fn sample(&self, ms: i32) -> PathSample {
        let i = self.index_at(ms);
        let a = self.samples[i];
        let Some(b) = self.samples.get(i + 1) else { return a };
        if b.cut || ms <= a.ms { return a; }
        let u = ((ms - a.ms) as f32 / (b.ms - a.ms).max(1) as f32).clamp(0.0, 1.0);
        PathSample { origin: a.origin.lerp(b.origin, u), velocity: a.velocity.lerp(b.velocity, u),
            yaw: a.yaw + wrap(b.yaw - a.yaw) * u, pitch: a.pitch + (b.pitch - a.pitch) * u, ..a }
    }
    fn eye(&self, ms: i32) -> Vec3 {
        let s = self.sample(ms);
        s.origin + Vec3::Z * s.eye_height
    }
    fn window(&self, a: i32, b: i32) -> &[PathSample] {
        let from = self.samples.partition_point(|s| s.ms < a);
        let to = self.samples.partition_point(|s| s.ms <= b);
        &self.samples[from..to.max(from)]
    }
    fn has_cut(&self, a: i32, b: i32) -> bool { self.window(a, b).iter().any(|s| s.ms > a && s.cut) }
    fn first_cut(&self, a: i32, b: i32) -> Option<i32> {
        self.window(a, b).iter().find(|s| s.ms > a && s.cut).map(|s| s.ms)
    }
    /// Evenly spaced subject eyes across [a, b].
    fn eyes(&self, a: i32, b: i32, n: usize) -> Vec<Vec3> {
        (0..n).map(|k| self.eye(a + ((b - a) as f64 * k as f64 / (n - 1).max(1) as f64) as i32)).collect()
    }
}

/// Every active snapshot of a recording: the whole demo, not only the race.
pub(in crate::app) fn parse_recorder_path(bytes: Vec<u8>) -> Result<DirectorPath, String> {
    use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
    const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut path = DirectorPath::default();
    let mut first_active = None::<i32>;
    let mut teleport = None;
    // (duelTime, last server time) of the open and of the last closed segment.
    let mut segment = None::<(i32, i32)>;
    let mut last_segment = None::<(i32, i32)>;
    // Combat: temp event entities live several snapshots (dedupe), player
    // entities carry events in a sequenced `event` field.
    let mut previous_temp = std::collections::HashSet::<(u16, i32)>::new();
    let mut last_raw = HashMap::<u16, i32>::new();
    // Saber throws: saberInFlight rising edges per client (recorder too).
    let mut in_flight = HashMap::<i32, bool>::new();
    // Offensive force powers: last sample time per caster.
    let mut last_force = HashMap::<i32, i32>::new();
    let mut players = std::collections::BTreeMap::<i32, Vec<(i32, Vec3)>>::new();
    while let Some(record) = reader.next_record().map_err(|e| format!("demo framing error: {e}"))? {
        let packet = decoder.parse_packet(record.sequence, &record.payload)
            .map_err(|e| format!("demo protocol error: {e}"))?;
        if !packet.events.iter().any(|e| matches!(e, Event::Snapshot { .. })) { continue; }
        let Some(snapshot) = decoder.latest_snapshot() else { continue };
        if snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE != 0 { continue; }
        let first = *first_active.get_or_insert(snapshot.server_time);
        if path.map.is_empty() {
            if let Some(map) = decoder.map_name() { path.map = map; }
        }
        let time = snapshot.server_time;
        let duel = snapshot.player_state.field_i32("duelTime").unwrap_or(0);
        if duel > 0 && time >= duel {
            match &mut segment {
                Some((d, end)) if *d == duel && time >= *end => *end = time,
                _ => {
                    if let Some(closed) = segment.take() { last_segment = Some(closed); }
                    segment = Some((duel, time));
                }
            }
        } else if let Some(closed) = segment.take() {
            last_segment = Some(closed);
        }
        path.push_state(time - first, &snapshot.player_state, &mut teleport);
        let ms = time - first;
        if path.area_masks.last().is_none_or(|(_, mask)| *mask != snapshot.area_mask) {
            path.area_masks.push((ms, snapshot.area_mask));
        }
        {
            let ps = &snapshot.player_state;
            let me = ps.field_i32("clientNum").unwrap_or(-1);
            let flying = ps.field_i32("saberInFlight").unwrap_or(0) != 0;
            if me >= 0 && in_flight.insert(me, flying) == Some(false) && flying {
                let at = state_vec3(ps, "origin").unwrap_or(Vec3::ZERO);
                path.events.push(CombatEvent { ms, kind: CombatKind::Throw, position: at, attacker: me, victim: -1, means: -1 });
            }
        }
        let mut temp = std::collections::HashSet::new();
        // (client, origin, offensive force active) for this snapshot.
        let mut casters: Vec<(i32, Vec3, bool)> = Vec::new();
        for entity in &snapshot.entities {
            use jka_protocol::entity_event::{EntityEvent as E, EV_EVENT_BITS};
            let entity_type = entity.field_i32("eType").unwrap_or(0);
            let f = |name: &str| entity.field_f32(name).unwrap_or(0.0);
            let position = Vec3::new(f("pos.trBase[0]"), f("pos.trBase[1]"), f("pos.trBase[2]"));
            if !position.is_finite() { continue; }
            let client = |n: i32| if (0..32).contains(&n) { n } else { -1 };
            if entity_type == crate::cgame::ET_PLAYER && entity.number < 32 {
                let id = i32::from(entity.number);
                if snapshot.player_state.field_i32("clientNum") != Some(id) {
                    let alive = entity.field_i32("eFlags").unwrap_or(0) & (1 << 1) == 0;
                    let list = path.alive.entry(id).or_default();
                    if list.last().is_none_or(|last| last.1 != alive) { list.push((ms, alive)); }
                    let list = path.views.entry(id).or_default();
                    if list.last().is_none_or(|last| ms - last.0 >= 100) {
                        list.push((ms, [f("apos.trBase[0]"), f("apos.trBase[1]")]));
                    }
                }
                let flying = entity.field_i32("saberInFlight").unwrap_or(0) != 0;
                if in_flight.insert(i32::from(entity.number), flying) == Some(false) && flying {
                    path.events.push(CombatEvent { ms, kind: CombatKind::Throw, position, attacker: i32::from(entity.number), victim: -1, means: -1 });
                }
                // FP_GRIP 6, FP_LIGHTNING 7, FP_DRAIN 13.
                let force = entity.field_i32("forcePowersActive").unwrap_or(0) & ((1 << 6) | (1 << 7) | (1 << 13)) != 0;
                casters.push((i32::from(entity.number), position, force));
                let list = players.entry(i32::from(entity.number)).or_default();
                // A new recorded PVS must not inherit the previous area's
                // remote actors during the normal 100 ms sampling interval.
                if path.samples.last().is_some_and(|s| s.cut)
                    || list.last().is_none_or(|last| ms - last.0 >= 100) { list.push((ms, position)); }
                let raw = entity.field_i32("event").unwrap_or(0);
                if last_raw.insert(entity.number, raw).is_some_and(|old| old != raw) {
                    let number = i32::from(entity.number);
                    let kind = match E::from_i32(raw & !EV_EVENT_BITS) {
                        Some(E::EV_PAIN) => Some((CombatKind::Pain, -1, number)),
                        Some(E::EV_FIRE_WEAPON | E::EV_ALT_FIRE) => Some((CombatKind::Shot, number, -1)),
                        _ => None,
                    };
                    if let Some((kind, attacker, victim)) = kind {
                        path.events.push(CombatEvent { ms, kind, position, attacker, victim, means: -1 });
                    }
                }
            } else if entity_type >= crate::cgame::ET_EVENTS {
                let event = entity_type - crate::cgame::ET_EVENTS;
                temp.insert((entity.number, event));
                if previous_temp.contains(&(entity.number, event)) { continue; }
                let other = client(entity.field_i32("otherEntityNum").unwrap_or(-1));
                let other2 = client(entity.field_i32("otherEntityNum2").unwrap_or(-1));
                let kind = match E::from_i32(event) {
                    Some(E::EV_OBITUARY) => Some(CombatKind::Kill),
                    Some(E::EV_SABER_HIT) => Some(CombatKind::Hit),
                    Some(E::EV_SABER_BLOCK | E::EV_SABER_CLASHFLARE) => Some(CombatKind::Clash),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let means = if kind == CombatKind::Kill { entity.field_i32("eventParm").unwrap_or(-1) } else { -1 };
                    path.events.push(CombatEvent { ms, kind, position, attacker: other2, victim: other, means });
                }
            }
        }
        previous_temp = temp;
        // A caster's target: the nearest other player (or the recorder) in reach.
        let own = path.samples.last().map(|s| (s.client, s.origin));
        for &(caster, origin, active) in &casters {
            if !active || last_force.get(&caster).is_some_and(|&t| ms - t < 400) { continue; }
            let target = casters.iter().filter(|c| c.0 != caster).map(|c| (c.0, c.1)).chain(own.filter(|o| o.0 != caster))
                .map(|(c, p)| (c, p.distance(origin))).filter(|(_, d)| *d < 900.0)
                .min_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| c);
            if let Some(victim) = target {
                last_force.insert(caster, ms);
                path.events.push(CombatEvent { ms, kind: CombatKind::Force, position: origin, attacker: caster, victim, means: -1 });
            }
        }
    }
    path.players = players.into_iter().filter(|(_, list)| list.len() >= 2).collect();
    if let Some(closed) = segment { last_segment = Some(closed); }
    let first = first_active.unwrap_or(0);
    path.race = last_segment.filter(|(d, end)| end - d >= 1000).map(|(d, end)| (d - first, end - first));
    if path.samples.len() < 4 { return Err("no usable recorded player positions".into()); }
    Ok(path)
}

/// Shared between jobs: decoded paths per (demo, game) and collision per map.
pub(in crate::app) type StudioPathCache = Arc<Mutex<HashMap<(String, String), Arc<DirectorPath>>>>;
pub(in crate::app) type StudioCollisionCache = Arc<Mutex<HashMap<String, CollisionWorld>>>;

fn open_assets(base: &std::path::Path, game: &str) -> Result<jka_assets::pk3::AssetSearchPath, String> {
    let source_game = (!game.is_empty()).then(|| base.parent().unwrap_or(base).join(game));
    let roots = jka_assets::pk3::configured_filesystem_roots(base);
    jka_assets::pk3::AssetSearchPath::open_search_dirs(
        jka_assets::pk3::filesystem_game_layer_dirs(base, source_game.as_deref(), &roots),
    ).map_err(|e| e.to_string())
}

pub(in crate::app) fn load_path(base: &std::path::Path, cache: &StudioPathCache, name: &str, game: &str)
    -> Result<Arc<DirectorPath>, String> {
    let key = (name.to_owned(), game.to_owned());
    if let Some(path) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) { return Ok(path); }
    let mut assets = open_assets(base, game)?;
    let qpath = format!("demos/{name}.dm_26");
    let asset = assets.read(&qpath, 512 * 1024 * 1024).map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Demo not found: {qpath}"))?;
    let path = Arc::new(parse_recorder_path(asset.bytes).map_err(|e| format!("{name}: {e}"))?);
    if let Ok(mut c) = cache.lock() { c.insert(key, path.clone()); }
    Ok(path)
}

/// Authoritative Render Studio Director source: always use the project's copy.
/// Normal game assets still come from the main JKA virtual filesystem.
fn load_project_path(project: &std::path::Path, cache: &StudioPathCache, clip: &StudioDemoClip)
    -> Result<Arc<DirectorPath>,String> {
    let key=(clip.name.clone(),clip.game.clone());
    if let Some(path)=cache.lock().ok().and_then(|c|c.get(&key).cloned()){return Ok(path);}
    let reference=clip.source_path.as_deref().ok_or("Demo is not imported into the project")?;
    let path=super::render_studio_media::media_path(project,reference)?;
    let metadata=std::fs::metadata(&path).map_err(|e|e.to_string())?;
    if metadata.len()==0 || metadata.len()>512*1024*1024 {return Err("Invalid director source size".into());}
    let bytes=std::fs::read(path).map_err(|e|e.to_string())?;
    let parsed=Arc::new(parse_recorder_path(bytes).map_err(|e|format!("{}: {e}",clip.name))?);
    if let Ok(mut c)=cache.lock(){c.insert(key,parsed.clone());}
    Ok(parsed)
}

/// Map collision for placement: the loaded source's world when the UI had
/// one, else the BSP read on the worker. None leaves only rig shots.
pub(in crate::app) fn load_collision(base: &std::path::Path, cache: &StudioCollisionCache, game: &str, map: &str,
    loaded: Option<CollisionWorld>) -> Option<CollisionWorld> {
    let key = map.to_ascii_lowercase();
    if key.is_empty() { return loaded; }
    if let Some(world) = loaded {
        if let Ok(mut c) = cache.lock() { c.entry(key).or_insert_with(|| world.clone()); }
        return Some(world);
    }
    if let Some(world) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) { return Some(world); }
    let source_game=(!game.is_empty()).then(||base.parent().unwrap_or(base).join(game));
    // Maps inherit base/retail content even when the selected demo belongs
    // to a mod directory. The demo browser intentionally searches only that
    // game's own layer; reusing its lookup silently lost base-map collision.
    let mut assets = jka_assets::pk3::AssetSearchPath::open_game(base,source_game.as_deref()).ok()?;
    let bsp = assets.read(&format!("maps/{map}.bsp"), 512 * 1024 * 1024).ok()??;
    let world = CollisionWorld::from_bsp(&bsp.bytes).ok()?;
    if let Ok(mut c) = cache.lock() { c.insert(key, world.clone()); }
    Some(world)
}

/// What a stretch of footage is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Beat { Opening, BigAir, Fast, Turn, Slow, Cruise, Finish, Closing, Compare,
    /// Combat: closing in on the opponent, trading blows, a kill, the moment after.
    Approach, Exchange, Kill, Aftermath }

impl Beat {
    fn candidates(self) -> &'static [(P, f32)] {
        match self {
            Self::Opening => &[(P::Establishing, 5.0), (P::Crane, 3.0), (P::Glide, 3.0), (P::Static, 1.5), (P::Orbit, 1.0)],
            Self::BigAir => &[(P::Pan, 4.0), (P::FlyBy, 3.0), (P::LowAngle, 2.0), (P::Tracking, 2.0), (P::Crane, 2.0),
                (P::Orbit, 1.0), (P::Static, 1.0)],
            Self::Fast => &[(P::Chase, 6.0), (P::Tracking, 1.5), (P::FlyBy, 2.0), (P::Overhead, 1.0), (P::Pan, 1.0)],
            Self::Turn => &[(P::Chase, 5.0), (P::Overhead, 3.0), (P::Orbit, 1.0), (P::Pan, 1.0)],
            Self::Slow => &[(P::CloseUp, 4.0), (P::Orbit, 3.0), (P::PushIn, 2.0), (P::LowAngle, 2.0), (P::DollyZoom, 1.0)],
            Self::Cruise => &[(P::Chase, 6.0), (P::FollowCam, 3.0), (P::RecordedPov, 1.5), (P::OverShoulder, 1.0),
                (P::Glide, 1.0), (P::Tracking, 0.5), (P::Pan, 0.5)],
            Self::Finish => &[(P::LowAngle, 3.0), (P::Orbit, 3.0), (P::Pan, 2.0), (P::Static, 2.0), (P::PushIn, 1.0)],
            Self::Closing => &[(P::Crane, 4.0), (P::Glide, 3.0), (P::Establishing, 2.0), (P::Orbit, 2.0), (P::Static, 1.0)],
            Self::Compare => &[(P::Chase, 6.0), (P::Tracking, 3.0), (P::Pan, 2.0), (P::Orbit, 1.5), (P::Overhead, 1.0), (P::Static, 0.5)],
            // Live combat: standard third person behind the fighter, often
            // with a shoulder offset; wider angles only now and then.
            Self::Approach => &[(P::FollowCam, 4.0), (P::Chase, 4.0), (P::OverShoulder, 2.5), (P::Static, 1.5), (P::Glide, 1.0),
                (P::TwoShot, 1.0)],
            Self::Exchange => &[(P::FollowCam, 5.0), (P::Chase, 5.0), (P::OverShoulder, 3.0), (P::Static, 1.0), (P::Glide, 0.5),
                (P::TwoShot, 0.5), (P::Pan, 0.3), (P::ThrowCam, 0.0)],
            // SaberCam's weight is decided per kill (saber kill / poke).
            Self::Kill => &[(P::Orbit, 3.0), (P::Static, 3.0), (P::Pan, 2.0), (P::Glide, 1.5), (P::LowAngle, 1.0), (P::DollyZoom, 1.0),
                (P::FollowCam, 0.0), (P::Chase, 0.0),
                (P::SaberCam, 0.0), (P::VictimPov, 0.0), (P::VictimBlade, 0.0), (P::ThrowCam, 0.0)],
            Self::Aftermath => &[(P::FollowCam, 3.0), (P::Orbit, 2.0), (P::Crane, 1.0), (P::LowAngle, 0.5)],
        }
    }
}

/// How a clip sits in the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum ClipRole {
    /// A whole recording: opening, coverage, finish.
    Whole,
    /// Route comparison backbone: first piece (opening), middle, last (finish).
    Lead, Middle, Tail,
    /// Route comparison: an alternate run's different line.
    Alternate,
}

/// Motion statistics of a stretch of path.
#[derive(Clone, Copy, Debug, Default)]
struct Features {
    speed: f32,
    peak_speed: f32,
    /// Heading change, degrees per second.
    turn_rate: f32,
    /// Net travel heading (degrees, JKA yaw).
    heading: f32,
    airborne: f32,
}

fn heading_of(v: Vec3) -> f32 { v.y.atan2(v.x).to_degrees() }
fn wrap(a: f32) -> f32 { (a + 180.0).rem_euclid(360.0) - 180.0 }
/// Native forward for a JKA yaw / pitch (positive pitch looks down).
fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    Vec3::new(cy * cp, sy * cp, -sp)
}
fn flat(v: Vec3) -> Vec3 { Vec3::new(v.x, v.y, 0.0).normalize_or_zero() }
fn render(p: Vec3) -> [f32; 3] { super::scene::render_position(p.to_array()) }

fn features(path: &DirectorPath, a: i32, b: i32) -> Features {
    let window = path.window(a, b);
    if window.len() < 2 {
        let s = path.sample(a);
        let speed = Vec3::new(s.velocity.x, s.velocity.y, 0.0).length();
        return Features { speed, peak_speed: speed, heading: heading_of(s.velocity), ..Features::default() };
    }
    let mut f = Features::default();
    let mut turn = 0.0f32;
    let mut last_heading = None::<f32>;
    for s in window {
        let speed = Vec3::new(s.velocity.x, s.velocity.y, 0.0).length();
        f.speed += speed;
        f.peak_speed = f.peak_speed.max(speed);
        if !s.ground { f.airborne += 1.0; }
        if speed > 150.0 {
            let h = heading_of(s.velocity);
            if let Some(last) = last_heading { if !s.cut { turn += wrap(h - last).abs(); } }
            last_heading = Some(h);
        }
    }
    let n = window.len() as f32;
    f.speed /= n;
    f.airborne /= n;
    f.turn_rate = turn / ((b - a).max(1) as f32 / 1000.0);
    let travel = window[window.len() - 1].origin - window[0].origin;
    f.heading = if flat(travel) != Vec3::ZERO { heading_of(travel) } else { heading_of(window[0].velocity) };
    f
}

fn classify(f: Features) -> Beat {
    if f.speed < 260.0 { Beat::Slow }
    else if f.turn_rate > 75.0 && f.speed > 320.0 { Beat::Turn }
    else if f.speed > 850.0 { Beat::Fast }
    else { Beat::Cruise }
}

/// Long or high airborne stretches: (takeoff, landing, score).
fn big_airs(path: &DirectorPath, a: i32, b: i32) -> Vec<(i32, i32, f32)> {
    let mut out = Vec::new();
    let mut run: Option<(i32, f32, f32, f32)> = None; // (takeoff, z0, zmax, zmin)
    for s in path.window(a, b) {
        if s.cut { run = None; continue; }
        match (&mut run, s.ground) {
            (None, false) => run = Some((s.ms, s.origin.z, s.origin.z, s.origin.z)),
            (Some(r), false) => { r.2 = r.2.max(s.origin.z); r.3 = r.3.min(s.origin.z); }
            (Some((takeoff, z0, zmax, zmin)), true) => {
                let (air, rise, drop) = (s.ms - *takeoff, *zmax - *z0, *z0 - *zmin);
                if air >= 900 || rise >= 96.0 || drop >= 192.0 {
                    out.push((*takeoff, s.ms, air as f32 / 1000.0 + rise / 128.0 + drop / 256.0));
                }
                run = None;
            }
            (None, true) => {}
        }
    }
    out
}

/// Deterministic SplitMix64.
struct Rng(u64);
impl Rng {
    fn new(parts: &[u64]) -> Self {
        let mut h = 0x9E37_79B9_7F4A_7C15u64;
        for p in parts { h = (h ^ p).wrapping_mul(0xBF58_476D_1CE4_E5B9).rotate_left(27); }
        Self(h)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 { (self.next() >> 40) as f32 / (1u64 << 24) as f32 }
    fn range(&mut self, a: f32, b: f32) -> f32 { a + (b - a) * self.unit() }
    fn sign(&mut self) -> f32 { if self.next() & 1 == 0 { 1.0 } else { -1.0 } }
    fn below(&mut self, n: usize) -> usize { (self.next() % n.max(1) as u64) as usize }
}

fn name_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// One clip to direct: its timeline placement and the recording's path.
pub(in crate::app) struct ClipJob {
    pub clip: StudioDemoClip,
    pub path: Arc<DirectorPath>,
    pub role: ClipRole,
}

/// Who the Auto Director builds the cut around.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum StudioDirectorFocus {
    /// The demo's recorded player (follows its spectator switches).
    #[default] Recorded,
    /// Whoever is in the most intense fight, switching between players.
    BestCombat,
    /// One client, whenever the recording can see them.
    Player(i32),
}

pub(in crate::app) struct AutoDirector {
    world: Option<CollisionWorld>,
    visibility: Option<Arc<jka_assets::bsp::Visibility>>,
    fov: f32,
    pacing: StudioDirectorPacing,
    seed: u32,
    history: Vec<P>,
    uses: HashMap<P, u32>,
    focus: StudioDirectorFocus,
    /// BestCombat: (from ms, client) protagonist switches of the current clip.
    track: Vec<(i32, i32)>,
    /// 180-degree rule: per fight (start, end, side of the action line the
    /// camera keeps to: +1 left, -1 right, seen from the protagonist).
    fight_sides: Vec<(i32, i32, f32)>,
    /// Respawn moments per client (death, then a jump to a spawn point).
    respawns: HashMap<i32, Vec<i32>>,
    /// Victim height above the floor at each kill (kill ms -> units).
    kill_air: HashMap<i32, f32>,
    engagements: Vec<Engagement>,
    combat: bool,
}

impl AutoDirector {
    pub fn new(world: Option<CollisionWorld>, fov: f32, pacing: StudioDirectorPacing, seed: u32) -> Self {
        Self { world, visibility: None, fov, pacing, seed, history: Vec::new(), uses: HashMap::new(),
            focus: StudioDirectorFocus::Recorded, track: Vec::new(), fight_sides: Vec::new(),
            respawns: HashMap::new(), kill_air: HashMap::new(), engagements: Vec::new(), combat: false }
    }
    pub fn set_focus(&mut self, focus: StudioDirectorFocus) { self.focus = focus; }

    /// The protagonist at `ms`: focus choice when visible, else the recorder.
    fn subject_at(&self, path: &DirectorPath, ms: i32) -> i32 {
        let recorder = path.sample(ms).client;
        let chosen = match self.focus {
            StudioDirectorFocus::Recorded => recorder,
            StudioDirectorFocus::Player(client) => client,
            StudioDirectorFocus::BestCombat => {
                let i = self.track.partition_point(|t| t.0 <= ms);
                if i == 0 { recorder } else { self.track[i - 1].1 }
            }
        };
        let chosen = if chosen == recorder || player_at(path, chosen, ms).is_some()
            || lingering_player(path, chosen, ms).is_some() { chosen } else { recorder };
        // Respawns are hard cuts. Keep the selected identity at the new
        // spawn instead of detouring to the killer for another 2.5 seconds.
        chosen
    }
    /// How far above the floor each victim was when killed (a player hull
    /// traced down from the kill position; 0 without map collision).
    pub fn kill_heights(&mut self, path: &DirectorPath) -> HashMap<i32, f32> {
        let mut out = HashMap::new();
        for kill in path.events.iter().filter(|ev| ev.kind == CombatKind::Kill) {
            let height = match self.world.as_mut() {
                Some(world) => {
                    let start = kill.position + Vec3::Z * 4.0;
                    let t = world.trace(TraceQuery { start: start.to_array(), end: (start - Vec3::Z * 4096.0).to_array(),
                        mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 40.0], pass_entity: -1, mask: MASK });
                    if t.start_solid != 0 { 0.0 } else { (start.z - t.end[2]).max(0.0) }
                }
                None => 0.0,
            };
            out.insert(kill.ms, height);
        }
        out
    }
    /// Killed and not yet respawned at `ms` (within 30 s of the death).
    fn dead_at(&self, path: &DirectorPath, client: i32, ms: i32) -> bool {
        let Some(death) = path.events.iter().rev()
            .find(|ev| ev.kind == CombatKind::Kill && ev.victim == client && ev.ms <= ms && ev.ms >= ms - 30000)
            .map(|ev| ev.ms) else { return false };
        !self.respawns.get(&client).is_some_and(|list| list.iter().any(|&r| r > death && r <= ms))
    }
    /// Respawn moments of whoever was the recorded player at the time.
    pub fn recorder_respawns(&self, path: &DirectorPath) -> Vec<i32> {
        let mut out: Vec<i32> = self.respawns.iter()
            .flat_map(|(&client, list)| list.iter().copied().filter(move |&r| path.sample(r).client == client))
            .collect();
        out.sort_unstable();
        out
    }
    fn prepare_focus(&mut self, path: &DirectorPath, a: i32, b: i32) {
        self.respawns = find_respawns(path);
        self.kill_air = self.kill_heights(path);
        self.track = if self.focus == StudioDirectorFocus::BestCombat { best_combat_track(path, a, b, &self.kill_air) } else { Vec::new() };
        let fights = engagements(path, a, b, &|t| self.subject_at(path, t));
        self.engagements = fights.iter().map(|&(s, e)| {
            let mut fight=combat_engagement(path, s, e, &|t| self.subject_at(path, t));
            if let Some(first)=fight.kills.first() {fight.start=fight.start.min((first.ms-2200).max(a));}
            fight
        }).collect();
        self.fight_sides = fights.into_iter().map(|(s, e)| {
            let mut rng = Rng::new(&[u64::from(self.seed), s as u64, 0x180]);
            (s, e, rng.sign())
        }).collect();
    }
    /// Camera side for a moment inside a fight (0 outside fights).
    fn side_at(&self, ms: i32) -> f32 {
        self.fight_sides.iter().find(|f| ms >= f.0 && ms <= f.1).map_or(0.0, |f| f.2)
    }

    fn intent(&self, path: &DirectorPath, s: i32, e: i32, beat: Beat) -> ShotIntent {
        // Establish the protagonist at the cut, not halfway through a future
        // damage/death sequence. Keep that identity for the whole shot.
        let subject = self.subject_at(path, if self.combat { s } else { (s + e) / 2 });
        let from = if beat == Beat::Aftermath { s - 2500 } else { s };
        let mut kills: Vec<_> = self.engagements.iter()
            .filter(|fight| fight.start <= e && fight.end >= from && fight.participants.contains(&subject))
            .flat_map(|fight| fight.kills.iter().copied())
            .filter(|kill| kill.ms >= from && kill.ms < e)
            .collect();
        if self.engagements.is_empty() {
            kills = relevant_kills(path, from, e, &|t| self.subject_at(path, t));
        }
        kills.sort_by_key(|kill| (kill.ms, kill.attacker, kill.victim));
        kills.dedup_by_key(|kill| (kill.ms, kill.attacker, kill.victim));
        if beat == Beat::Aftermath {
            kills = kills.into_iter().rev().take(1).collect();
        }
        ShotIntent { subject, kills }
    }

    /// These boundaries survive editorial merging, including very short shots.
    fn mandatory_cuts(&self, path: &DirectorPath, a: i32, b: i32) -> Vec<i32> {
        let mut cuts: Vec<_> = self.track.iter().map(|t| t.0).collect();
        for pair in path.samples.windows(2) {
            if pair[1].cut || pair[1].client != pair[0].client { cuts.push(pair[1].ms); }
        }
        for (&client, list) in &self.respawns {
            for &r in list {
                if self.subject_at(path, r - 100) == client || path.sample(r - 100).client == client {
                    cuts.push(r);
                }
            }
        }
        for (client, samples) in &path.players {
            for pair in samples.windows(2) {
                if pair[1].0 - pair[0].0 <= 400 && pair[0].1.distance(pair[1].1) > 400.0
                    && self.subject_at(path, pair[1].0 - 1) == *client { cuts.push(pair[1].0); }
            }
        }
        let mut previous = self.subject_at(path, a);
        for ms in (a..b).step_by(50) {
            let subject = self.subject_at(path, ms);
            if subject != previous { cuts.push(ms); previous = subject; }
        }
        cuts.retain(|&t| t > a && t < b);
        cuts.sort_unstable();
        cuts.dedup();
        cuts
    }

    /// Physical discontinuities need a reset, but PVS flicker and changes in
    /// fight ranking are editorial suggestions, not reasons to insert a flash.
    fn combat_resets(&self, path: &DirectorPath, a: i32, b: i32) -> Vec<i32> {
        let mut cuts = Vec::new();
        for pair in path.samples.windows(2) {
            if (pair[1].cut || pair[1].client != pair[0].client)
                && [pair[0].client, pair[1].client].contains(&self.subject_at(path, pair[1].ms - 1)) {
                cuts.push(pair[1].ms);
            }
        }
        for (&client, times) in &self.respawns {
            cuts.extend(times.iter().copied().filter(|&ms| self.subject_at(path, ms - 1) == client));
        }
        for (client, samples) in &path.players {
            for pair in samples.windows(2) {
                if pair[1].0 - pair[0].0 <= 400 && pair[0].1.distance(pair[1].1) > 400.0
                    && self.subject_at(path, pair[1].0 - 1) == *client { cuts.push(pair[1].0); }
            }
        }
        cuts.retain(|&ms| ms > a && ms < b);
        cuts.sort_unstable();
        cuts.dedup();
        cuts
    }

    /// Keep the same composition through local damage and the entire kill.
    /// Windows merge so a flurry or multi-kill cannot introduce an internal cut.
    fn combat_holds(&self, path: &DirectorPath, a: i32, b: i32) -> Vec<(i32, i32)> {
        let mut holds: Vec<(i32, i32)> = Vec::new();
        for ev in path.events.iter().filter(|ev| ev.ms >= a && ev.ms < b) {
            let (lead, tail) = match ev.kind {
                CombatKind::Kill => (2200, 1500),
                CombatKind::Hit | CombatKind::Pain | CombatKind::Force => (450, 900),
                CombatKind::Clash => (250, 500),
                _ => continue,
            };
            let subject = self.subject_at(path, ev.ms);
            let relevant = ev.attacker == subject || ev.victim == subject
                || player_at(path, subject, ev.ms).is_some_and(|p| p.distance(ev.position) < 650.0);
            if relevant { holds.push(((ev.ms - lead).max(a), (ev.ms + tail).min(b))); }
        }
        holds.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::new();
        for (s, e) in holds {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        merged
    }

    /// Apply all boundaries before enforcing playback duration. Closely spaced
    /// real resets share one live chase rig: it follows the source's jumps
    /// directly, without a spline through the map or extra camera-style cuts.
    fn combat_edit(&self, job: &ClipJob, beats: &[(i32, i32, Beat)]) -> Vec<(i32, i32, Beat, bool)> {
        let (a, b) = (job.clip.in_ms, job.clip.out_ms);
        let resets = self.combat_resets(&job.path, a, b);
        let holds = self.combat_holds(&job.path, a, b);
        let mut boundaries = vec![a, b];
        for at in beats.iter().map(|beat| beat.0).chain(self.track.iter().map(|t| t.0)) {
            // Pull an optional cut back before the action instead of entering
            // on the damage, or drop it if that would leave a short neighbor.
            let at = holds.iter().find(|&&(s, e)| at > s && at < e).map_or(at, |h| h.0);
            if at > a && at < b { boundaries.push(at); }
        }
        boundaries.extend(resets.iter().copied());
        boundaries.sort_unstable();
        boundaries.dedup();
        // Slow motion is conditional on the final camera actually covering
        // the kill. It must never justify a short insert if that ramp is removed.
        let mut editorial_clip = job.clip.clone();
        for key in &mut editorial_clip.speed_keys { key.rate = key.rate.max(1.0); }
        let duration = |s, e| editorial_clip.timeline_at_source_ms(e) - editorial_clip.timeline_at_source_ms(s);
        let mut i = 1;
        while i + 1 < boundaries.len() {
            if resets.binary_search(&boundaries[i]).is_err()
                && (duration(boundaries[i - 1], boundaries[i]) < COMBAT_MIN_SHOT
                    || duration(boundaries[i], boundaries[i + 1]) < COMBAT_MIN_SHOT) {
                boundaries.remove(i);
                i = i.saturating_sub(1).max(1);
            } else { i += 1; }
        }
        let mut ranges: Vec<(i32, i32)> = boundaries.windows(2).map(|w| (w[0], w[1])).collect();
        let mut i = 0;
        while ranges.len() > 1 && i < ranges.len() {
            if duration(ranges[i].0, ranges[i].1) < COMBAT_MIN_SHOT {
                if i > 0 && i + 1 < ranges.len() {
                    // Cover both sides of a reset cluster with the same rig,
                    // including the recovery after the last discontinuity.
                    ranges[i - 1].1 = ranges[i + 1].1;
                    ranges.drain(i..=i + 1);
                    i -= 1;
                }
                else if i > 0 { let end = ranges.remove(i).1; ranges[i - 1].1 = end; i -= 1; }
                else { let start = ranges.remove(0).0; ranges[0].0 = start; }
            } else { i += 1; }
        }
        ranges.into_iter().map(|(s, e)| {
            let beat = if beats.iter().any(|&(x, y, beat)| beat == Beat::Kill && x < e && y > s) { Beat::Kill }
                else { beats.iter().find(|&&(x, y, _)| x <= s && y > s).map_or(Beat::Cruise, |b| b.2) };
            let live = resets.iter().any(|&at| at > s && at < e);
            (s, e, beat, live)
        }).collect()
    }

    /// Last-resort coverage still uses an outside camera. Runtime wall/PVS
    /// guards can shorten this boom, without requesting a first-person insert.
    fn live_combat_shot(&self, job: &ClipJob, s: i32, e: i32, bridge: bool) -> StudioDirectorShot {
        let subject = self.subject_at(&job.path, s);
        let follows_recorder = bridge && self.focus != StudioDirectorFocus::Player(subject);
        let mut shot = StudioDirectorShot::new(0, P::Chase, if follows_recorder { -1 } else { subject },
            job.clip.timeline_at_source_ms(s), job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s));
        P::Chase.seed_shot(&mut shot, self.fov);
        shot.rig.range = 220.0;
        shot.rig.pitch_offset = 10.0;
        shot.auto = true;
        shot
    }

    fn recorded_visible(&self, path: &DirectorPath, ms: i32, point: Vec3) -> bool {
        let Some(vis) = self.visibility.as_ref() else { return true };
        let Some(cluster) = vis.cluster_at(point.to_array()) else { return false };
        if !vis.visible(vis.cluster_at(path.eye(ms).to_array()), &[cluster]) { return false; }
        let i = path.area_masks.partition_point(|(t, _)| *t <= ms);
        let mask = i.checked_sub(1).map(|i| &path.area_masks[i].1);
        vis.area_at(point.to_array()).is_none_or(|area| area >= 256
            || mask.is_none_or(|mask| mask[area / 8] & (1 << (area % 8)) == 0))
    }
    pub fn has_collision(&self) -> bool { self.world.is_some() }

    /// One camera for all runners in a comparison section. Every candidate is
    /// checked against every run, so cloning it keeps the comparison fair.
    pub fn route_replay_camera(&mut self, ranges: &[(&DirectorPath, i32, i32)], moving: bool)
        -> Option<StudioDirectorShot> {
        let &(path, a, b) = ranges.first()?;
        if !self.has_collision() || ranges.iter().any(|(p, _, _)| p.samples.is_empty()) { return None; }
        let extra: Vec<_> = ranges.iter().skip(1).flat_map(|(p, s, e)| p.eyes(*s, *e, LOS_SAMPLES)).collect();
        let mut eyes = path.eyes(a, b, LOS_SAMPLES);
        eyes.extend_from_slice(&extra);
        let mut shot = StudioDirectorShot::new(0, P::Static, -1, 0.0, 1.0);
        let mut rng = Rng::new(&[u64::from(self.seed), a as u64, b as u64, 0x726f757465]);
        if !self.place_static(&mut shot, path, a, b, &extra, &mut rng) { return None; }
        let p = Vec3::from_array(super::scene::jka_position(shot.position_override?));
        if self.visible_count(p, &eyes) != eyes.len() { return None; }
        if moving {
            let center = Vec3::from_array(super::scene::jka_position(shot.focus_target?));
            // A small pull-back keeps the same composition and is repeated for
            // each runner. Check the whole move, not just its endpoint.
            let end = p + (p - center).normalize_or_zero() * p.distance(center).min(800.0) * 0.12;
            if self.free(end) && self.clear(p, end)
                && [0.25, 0.5, 0.75, 1.0].into_iter()
                    .all(|u| self.visible_count(p.lerp(end, u), &eyes) == eyes.len()) {
                shot.preset = P::Glide;
                shot.position_end = Some(render(end));
            }
        }
        shot.auto = true;
        shot.valid().then_some(shot)
    }

    /// Source-time beats covering the clip, contiguous.
    fn beats(&self, job: &ClipJob) -> Vec<(i32, i32, Beat)> {
        let (a, b) = (job.clip.in_ms, job.clip.out_ms);
        let path = &job.path;
        let len_ms = (self.pacing.shot_seconds() * 1000.0) as i32;
        if job.role == ClipRole::Alternate {
            return if b - a > len_ms * 5 / 2 {
                let split = a + ((b - a) / 2).max(len_ms * 7 / 5).min(b - a - 1200);
                vec![(a, split, Beat::Compare), (split, b, classify(features(path, split, b)))]
            } else { vec![(a, b, Beat::Compare)] };
        }
        let mut fixed: Vec<(i32, i32, Beat)> = Vec::new();
        if matches!(job.role, ClipRole::Whole | ClipRole::Lead) && b - a >= 4000 {
            let end = match path.race {
                Some((start, _)) if start > a + 800 && start < a + 7000 => start + 700,
                _ => a + len_ms,
            }.clamp(a + 2200, a + 6000).min(b);
            fixed.push((a, end, Beat::Opening));
        }
        if matches!(job.role, ClipRole::Whole | ClipRole::Tail) {
            match path.race {
                Some((_, finish)) if finish > a + 3000 && finish <= b + 200 => {
                    let start = (finish - 1500).max(a);
                    let end = (finish + 1500).min(b);
                    if b - end < 2500 { fixed.push((start, b, Beat::Finish)); }
                    else { fixed.push((start, end, Beat::Finish)); fixed.push((end, b, Beat::Closing)); }
                }
                _ if b - a >= 9000 => fixed.push((b - 3000, b, Beat::Closing)),
                _ => {}
            }
        }
        // Opening and ending never collide on short clips: the ending wins.
        if fixed.len() > 1 && fixed[0].2 == Beat::Opening && fixed[1].0 < fixed[0].1 + 600 { fixed.remove(0); }
        let mut airs = big_airs(path, a, b);
        airs.sort_by(|x, y| y.2.total_cmp(&x.2));
        let cap = ((b - a) / (len_ms * 2)).max(1) as usize;
        let mut taken = 0;
        for (takeoff, landing, _) in airs {
            if taken >= cap { break; }
            let start = (takeoff - 450).max(a);
            let end = (landing + 500).min(b).min(start + 7000);
            if end - start < 1200 { continue; }
            if fixed.iter().any(|f| start < f.1 + 300 && end > f.0 - 300) { continue; }
            fixed.push((start, end, Beat::BigAir));
            taken += 1;
        }
        fixed.sort_by_key(|f| f.0);
        let mut out: Vec<(i32, i32, Beat)> = Vec::new();
        let mut cursor = a;
        let fill = |out: &mut Vec<(i32, i32, Beat)>, x: i32, y: i32| {
            if y - x < 1200 {
                match out.last_mut() { Some(last) => last.1 = y, None => out.push((x, y, classify(features(path, x, y)))) }
                return;
            }
            let n = ((f64::from(y - x) / f64::from(len_ms)).round() as i32).max(1);
            for k in 0..n {
                let (s, e) = (x + (y - x) * k / n, x + (y - x) * (k + 1) / n);
                out.push((s, e, classify(features(path, s, e))));
            }
        };
        for (s, e, beat) in fixed {
            let s = s.max(cursor);
            if e <= s { continue; }
            if s > cursor { fill(&mut out, cursor, s); }
            // A tiny leading gap folds into the first fixed beat.
            match out.last() {
                Some(&(ls, le, lb)) if le - ls < 400 && out.len() == 1 && lb != Beat::Opening => { out[0] = (ls, e, beat); }
                _ => out.push((s, e, beat)),
            }
            cursor = e;
        }
        if cursor < b { fill(&mut out, cursor, b); }
        // World shots cannot span a teleport: split beats at path cuts.
        let mut split = Vec::with_capacity(out.len());
        for (s, e, beat) in out {
            match path.first_cut(s, e) {
                Some(c) if c - s >= 900 && e - c >= 900 => {
                    split.push((s, c, beat));
                    split.push((c, e, classify(features(path, c, e))));
                }
                _ => split.push((s, e, beat)),
            }
        }
        split
    }

    /// Context of an existing shot (for Regenerate).
    fn beat_for(&self, job: &ClipJob, s: i32, e: i32) -> Beat {
        if job.role == ClipRole::Alternate { return Beat::Compare; }
        let path = &job.path;
        let first = matches!(job.role, ClipRole::Whole | ClipRole::Lead);
        if first && s <= job.clip.in_ms + 300 && job.clip.out_ms - job.clip.in_ms >= 4000 { return Beat::Opening; }
        if let Some((_, finish)) = path.race {
            if s <= finish && e >= finish { return Beat::Finish; }
            if s >= finish { return Beat::Closing; }
        }
        if self.intent(path, s, e, Beat::Kill).primary().is_some() { return Beat::Kill; }
        if engagements(path, s, e, &|t| self.subject_at(path, t)).iter()
            .any(|&(a, b)| (b.min(e) - a.max(s)) * 2 >= e - s) { return Beat::Exchange; }
        let air: i32 = big_airs(path, s, e).iter().map(|a| a.1 - a.0).sum();
        if air * 100 >= (e - s) * 45 { return Beat::BigAir; }
        classify(features(path, s, e))
    }

    /// Combat view: beats from engagements (approach, exchanges, kill cams,
    /// hero moment); route coverage between fights. Also (fights, kills).
    pub fn combat_beats(&self, job: &ClipJob) -> (Vec<(i32, i32, Beat)>, usize, usize) {
        let (a, b) = (job.clip.in_ms, job.clip.out_ms);
        let path = &job.path;
        let fights = &self.engagements;
        if fights.is_empty() { return (self.beats(job), 0, 0); }
        let fast = ((self.pacing.shot_seconds() * 1000.0) as i32).max(1500);
        let mut fixed: Vec<(i32, i32, Beat)> = Vec::new();
        if job.role == ClipRole::Whole && fights[0].start - a >= 3000 {
            let len = ((self.pacing.shot_seconds() * 1000.0) as i32).clamp(2200, 5000);
            fixed.push((a, (a + len).min(fights[0].start), Beat::Opening));
        }
        let mut kills_total = 0;
        for fight in fights {
            let (s, e) = (fight.start, fight.end);
            kills_total += fight.kills.len();
            let mut cursor = s;
            let mut first = true;
            let exchange = |fixed: &mut Vec<(i32, i32, Beat)>, from: i32, to: i32, first: &mut bool| {
                let n = ((to - from) / fast).max(1);
                for k in 0..n {
                    let (x, y) = (from + (to - from) * k / n, from + (to - from) * (k + 1) / n);
                    fixed.push((x, y, if *first { Beat::Approach } else { Beat::Exchange }));
                    *first = false;
                }
            };
            for chain in kill_sequences(&fight.kills) {
                let kill = chain[0];
                let last = chain.last().unwrap();
                let mut ks = (kill.ms - 2200).max(cursor);
                let ke = (last.ms + 1500).max(ks + 800).min(b);
                if ks - cursor >= 700 { exchange(&mut fixed, cursor, ks, &mut first); } else { ks = cursor; }
                fixed.push((ks, ke, Beat::Kill));
                first = false;
                cursor = ke;
                let hero = self.subject_at(path, last.ms);
                if last.attacker >= 0 && last.attacker == hero && e.max(cursor) - cursor >= 1800 {
                    fixed.push((cursor, cursor + 1500, Beat::Aftermath));
                    cursor += 1500;
                }
            }
            if e - cursor >= 700 { exchange(&mut fixed, cursor, e, &mut first); }
            else if let Some(last) = fixed.last_mut().filter(|l| l.1 == cursor && e > cursor) { last.1 = e; }
        }
        // Route-style coverage between fights; short gaps join the shot before.
        let len_ms = (self.pacing.shot_seconds() * 1000.0) as i32;
        let mut out: Vec<(i32, i32, Beat)> = Vec::new();
        let mut cursor = a;
        let fill = |out: &mut Vec<(i32, i32, Beat)>, x: i32, y: i32| {
            if y - x < 1200 {
                match out.last_mut() { Some(last) => last.1 = y, None => out.push((x, y, Beat::Cruise)) }
                return;
            }
            let n = ((f64::from(y - x) / f64::from(len_ms)).round() as i32).max(1);
            for k in 0..n {
                let (s, e) = (x + (y - x) * k / n, x + (y - x) * (k + 1) / n);
                // Between fights speed means nothing (often a respawn run back):
                // plain coverage, no fly-bys or tracking for going fast.
                out.push((s, e, Beat::Cruise));
            }
        };
        for (s, e, beat) in fixed {
            let s = s.max(cursor);
            if e <= s { continue; }
            if s > cursor { fill(&mut out, cursor, s); }
            out.push((s, e, beat));
            cursor = e;
        }
        if cursor < b { fill(&mut out, cursor, b); }
        // Final editing applies damage holds and playback-time duration limits.
        (out, fights.len(), kills_total)
    }

    /// The clip's fights (padded source ranges) for the current focus.
    pub fn fights(&mut self, job: &ClipJob) -> Vec<(i32, i32)> {
        self.prepare_focus(&job.path, job.clip.in_ms, job.clip.out_ms);
        self.engagements.iter().map(|fight| (fight.start, fight.end)).collect()
    }

    /// Combat view of one clip: shots plus (fights, kills).
    pub fn direct_combat_clip(&mut self, job: &ClipJob) -> (Vec<StudioDirectorShot>, usize, usize) {
        self.combat = true;
        self.prepare_focus(&job.path, job.clip.in_ms, job.clip.out_ms);
        let (beats, fights, kills) = self.combat_beats(job);
        (self.direct_beats(job, beats), fights, kills)
    }

    /// Cut a whole clip into shots (ids 0; the caller assigns them).
    pub fn direct_clip(&mut self, job: &ClipJob) -> Vec<StudioDirectorShot> {
        self.combat = false;
        self.prepare_focus(&job.path, job.clip.in_ms, job.clip.out_ms);
        let beats = self.beats(job);
        self.direct_beats(job, beats)
    }

    fn direct_beats(&mut self, job: &ClipJob, beats: Vec<(i32, i32, Beat)>) -> Vec<StudioDirectorShot> {
        // A shot of a few frames is jarring: fold pieces under 1.2 s into the
        // one before (a kill keeps its kill cam).
        let mut merged: Vec<(i32, i32, Beat)> = Vec::with_capacity(beats.len());
        for (s, e, beat) in beats {
            match merged.last_mut() {
                Some(last) if e - s < 1200 => { last.1 = e; if beat == Beat::Kill { last.2 = Beat::Kill; } }
                Some(last) if last.1 - last.0 < 1200 => { last.1 = e; if beat == Beat::Kill { last.2 = Beat::Kill; } }
                _ => merged.push((s, e, beat)),
            }
        }
        let cuts = self.mandatory_cuts(&job.path, job.clip.in_ms, job.clip.out_ms);
        let beats = if self.combat { self.combat_edit(job, &merged) }
            else { split_beats(merged, &cuts).into_iter().map(|(s, e, beat)| (s, e, beat, false)).collect() };
        let clip_key = name_hash(&job.clip.name);
        let mut shots = Vec::new();
        // Source start of the current follow-cam run (for merging).
        let mut follow_from: Option<i32> = None;
        for (s, e, beat, live) in beats {
            let mut rng = Rng::new(&[u64::from(self.seed), clip_key, s as u64]);
            let neighbors = [self.history.last().copied(), self.history.iter().rev().nth(1).copied()];
            // Even a sub-frame discontinuity needs coverage; short mandatory
            // fragments use the original footage rather than a camera move.
            let chosen = if self.combat && live { Some(self.live_combat_shot(job, s, e, true)) }
            else if !self.combat && job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s) < 0.4 {
                let mut shot = StudioDirectorShot::new(0, P::RecordedPov, -1,
                    job.clip.timeline_at_source_ms(s), job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s));
                shot.auto = true;
                Some(shot)
            } else { self.choose(beat, s, e, job, neighbors, None, &mut rng) };
            if let Some(mut shot) = chosen {
                shot.seed = self.seed;
                let mandatory = live || cuts.binary_search(&s).is_ok();
                // Same standard view on the same subject: keep rolling, no cut.
                let continues = !mandatory && shots.last().is_some_and(|last: &StudioDirectorShot| {
                    matches!(shot.preset, P::Chase | P::RecordedPov) && last.preset == shot.preset
                        && last.path_keys.is_empty() && shot.path_keys.is_empty()
                        && last.client == shot.client && last.rig.aim_client == shot.rig.aim_client
                        && (last.end() - shot.start).abs() < 0.05
                });
                if continues {
                    let last = shots.last_mut().unwrap();
                    last.duration = shot.end() - last.start;
                    continue;
                }
                // Follow cam after follow cam on the same player: re-plan both
                // stretches as one continuous damped move.
                if shot.preset == P::FollowCam && !mandatory {
                    if let (Some(last), Some(&from)) = (shots.last(), follow_from.as_ref()) {
                        let last: &StudioDirectorShot = last;
                        if last.preset == P::FollowCam && last.client == shot.client && (last.end() - shot.start).abs() < 0.05 {
                            let mut merged = last.clone();
                            merged.duration = shot.end() - last.start;
                            let subject = self.subject_at(&job.path, (from + e) / 2);
                            let foe = opponent(&job.path, from, e, subject);
                            let params = [last.rig.range, last.rig.vertical_offset, last.rig.horizontal_offset, last.rig.blend_seconds];
                            let intent = self.intent(&job.path, from, e, beat);
                            if self.plan_follow(&mut merged, &job.path, from, e, subject, foe, params, &intent.kills)
                                && self.frames_ok(&job.path, &merged, beat, from, e, &intent) {
                                *shots.last_mut().unwrap() = merged;
                                continue;
                            }
                        }
                    }
                }
                follow_from = (shot.preset == P::FollowCam).then_some(s);
                self.history.push(shot.preset);
                *self.uses.entry(shot.preset).or_default() += 1;
                shots.push(shot);
            }
        }
        // Planners and their acceptance checks run on source time. Playback
        // evaluates keys on the rendered clock; a slow-motion ramp must not
        // advance the camera past the damage/kill before the actors get there.
        for shot in &mut shots { retime_camera_path(&job.clip, shot); }
        shots
    }

    /// A different decision for `shot` (new type), or a fresh placement of
    /// `keep` when given. The shot keeps its id, timing and subject.
    pub fn regenerate(&mut self, shot: &StudioDirectorShot, job: &ClipJob, neighbors: [Option<P>; 2], keep: Option<P>)
        -> Option<StudioDirectorShot> {
        let s = job.clip.source_time_ms(shot.start);
        let e = job.clip.source_time_ms(shot.end());
        if e - s < 100 { return None; }
        let seed = shot.seed.wrapping_add(1);
        let mut rng = Rng::new(&[u64::from(seed), name_hash(&job.clip.name), s as u64, shot.id]);
        let saved = self.focus;
        if shot.client >= 0 { self.focus = StudioDirectorFocus::Player(shot.client); }
        self.prepare_focus(&job.path, job.clip.in_ms, job.clip.out_ms);
        let beat = self.beat_for(job, s, e);
        let intent = self.intent(&job.path, s, e, beat);
        let built = match keep {
            Some(preset) => self.build(preset, beat, s, e, job, &mut rng, &intent),
            None => self.choose(beat, s, e, job, neighbors, Some(shot.preset), &mut rng),
        };
        self.focus = saved;
        built.map(|mut new| {
            new.id = shot.id;
            // Auto shots may change subject (kill cams); hand-made ones keep it.
            if !shot.auto { new.client = shot.client; }
            new.enabled = shot.enabled;
            new.seed = seed;
            // Keep the authored timing exactly (no source-clock rounding).
            new.start = shot.start;
            new.duration = shot.duration;
            retime_camera_path(&job.clip, &mut new);
            new
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn choose(&mut self, beat: Beat, s: i32, e: i32, job: &ClipJob, neighbors: [Option<P>; 2], exclude: Option<P>,
        rng: &mut Rng) -> Option<StudioDirectorShot> {
        let intent = self.intent(&job.path, s, e, beat);
        let beat = if beat == Beat::Kill && intent.kills.is_empty() { Beat::Exchange } else { beat };
        let combat = self.combat || matches!(beat, Beat::Approach | Beat::Exchange | Beat::Kill | Beat::Aftermath);
        let needs_hold = shot_sample_times(s, e, 17, &[]).iter().any(|&ms|
            intent.subject != job.path.sample(ms).client && player_at(&job.path, intent.subject, ms).is_none());
        let world_ok = self.world.is_some() && !job.path.has_cut(s, e);
        // A dead protagonist is shown from outside (their corpse), never
        // through their eyes; the victim views belong to the kill cam only.
        let lying_dead = self.dead_at(&job.path, self.subject_at(&job.path, (s + e) / 2), (s + e) / 2);
        let mut pool: Vec<(P, f32)> = beat.candidates().iter().map(|&(preset, weight)| {
            let mut w = weight;
            // A chain stays in a readable coverage view; a victim-specific
            // angle would abandon the attacker before the next impact.
            if beat == Beat::Kill && intent.sequence() {
                w = match preset { P::FollowCam => 8.0, P::Chase => 4.0,
                    P::Static | P::Pan => 2.0, P::Orbit => 1.0, _ => 0.0 };
            }
            if lying_dead && matches!(preset, P::RecordedPov | P::PlayerEyes) { w = 0.0; }
            if combat && preset == P::Tracking { w = 0.0; }
            // The standard views may continue (they merge into one shot);
            // anything else never repeats back to back.
            let standard = matches!(preset, P::Chase | P::RecordedPov | P::FollowCam);
            if Some(preset) == exclude || (!standard && Some(preset) == neighbors[0]) { w = 0.0; }
            if !standard && Some(preset) == neighbors[1] { w *= 0.4; }
            if preset.world_anchored() && !world_ok { w = 0.0; }
            let used = self.uses.get(&preset).copied().unwrap_or(0) as f32;
            if matches!(preset, P::DollyZoom | P::PushIn | P::CloseUp | P::LowAngle | P::PlayerEyes) {
                w /= 1.0 + used * 1.5;
            }
            if matches!(preset, P::SaberCam | P::VictimPov | P::VictimBlade) && beat == Beat::Kill
                && Some(preset) != exclude && Some(preset) != neighbors[0] {
                let saber = if intent.sequence() { 0.0 } else {
                    intent.primary().map_or(0.0, |kill| saber_cam_weight(&job.path, &kill)) };
                // Victim angles at half the saber cam's weight (poke: 3, saber: 0.75).
                w = if preset == P::SaberCam { saber } else { saber * 0.5 };
            }
            // Throw cam: thrown-saber kills, and now and then a throw mid-fight.
            if preset == P::ThrowCam && Some(preset) != exclude && Some(preset) != neighbors[0] {
                w = match beat {
                    Beat::Kill => intent.primary().filter(|_| !intent.sequence()).filter(|k| throw_before(&job.path, k.attacker, k.ms))
                        .map_or(0.0, |_| 4.0),
                    Beat::Exchange => if throw_in(&job.path, self.subject_at(&job.path, (s + e) / 2), s, e) { 1.5 } else { 0.0 },
                    _ => 0.0,
                };
            }
            // Combat coverage stays outside the fighters. Blade/eyes shots
            // remain available for explicit manual placement only.
            if combat && matches!(preset, P::RecordedPov | P::PlayerEyes | P::SaberCam | P::VictimPov | P::VictimBlade) { w = 0.0; }
            if combat && preset == P::Static
                && job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s) < COMBAT_MIN_STATIC { w = 0.0; }
            if needs_hold { w = if preset == P::FollowCam { 8.0 } else { 0.0 }; }
            (preset, w)
        }).collect();
        if needs_hold && !pool.iter().any(|p| p.0 == P::FollowCam) { pool.push((P::FollowCam, 8.0)); }
        // Collect up to three valid candidates (weighted picks), then favour
        // the one with the most action on screen; sometimes keep the first
        // pick for variety.
        let mut valid: Vec<(StudioDirectorShot, f32)> = Vec::new();
        if combat && !needs_hold && self.world.is_some()
            && job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s) >= COMBAT_MIN_STATIC && rng.unit() < 0.4 {
            if let Some(shot) = self.telephoto(job, s, e, &intent, rng) {
                if self.frames_ok(&job.path, &shot, beat, s, e, &intent) {
                    let score = self.coverage(&job.path, &shot, s, e, &intent) * 1.15;
                    valid.push((shot, score));
                }
            }
        }
        while valid.len() < 3 && pool.iter().any(|p| p.1 > 0.0) {
            let total: f32 = pool.iter().map(|p| p.1).sum();
            let mut pick = rng.unit() * total;
            let index = pool.iter().position(|p| { pick -= p.1; p.1 > 0.0 && pick <= 0.0 })
                .unwrap_or_else(|| pool.iter().rposition(|p| p.1 > 0.0).unwrap_or(0));
            let preset = pool[index].0;
            if let Some(shot) = self.build(preset, beat, s, e, job, rng, &intent) {
                if self.frames_ok(&job.path, &shot, beat, s, e, &intent) {
                    let score = self.coverage(&job.path, &shot, s, e, &intent);
                    valid.push((shot, score));
                }
            }
            pool[index].1 = 0.0;
        }
        if !valid.is_empty() {
            let best = (0..valid.len()).max_by(|&a, &b| valid[a].1.total_cmp(&valid[b].1)).unwrap_or(0);
            let index = if rng.unit() < 0.7 { best } else { 0 };
            return Some(valid.swap_remove(index).0);
        }
        // Always cuttable: a framed chase if there is one, else any chase /
        // orbit, and finally the recorded view so the cut never has a hole.
        // A kill cam must show the victim (slow motion plays here): an orbit
        // on the victim is built to, even when the framing check is strict.
        if beat == Beat::Kill && !intent.sequence() {
            if let Some(shot) = self.build_kill(P::Orbit, s, e, job, rng, &intent) {
                if self.frames_ok(&job.path, &shot, beat, s, e, &intent) { return Some(shot); }
            }
        }
        let fallback = if exclude == Some(P::Chase) { P::Orbit } else { P::Chase };
        for _ in 0..4 {
            if let Some(shot) = self.build(fallback, beat, s, e, job, rng, &intent) {
                if self.frames_ok(&job.path, &shot, beat, s, e, &intent) { return Some(shot); }
            }
        }
        if combat {
            let mut shot = self.live_combat_shot(job, s, e, false);
            // An unvalidated cinematic fallback was the source of empty/wall
            // shots. Use dense, subject-centered coverage when no angle passes.
            self.plan_coverage(&mut shot, &job.path, s, e, intent.subject);
            return Some(shot);
        }
        let last_resort = if lying_dead { P::Orbit } else { P::RecordedPov };
        self.build(last_resort, beat, s, e, job, rng, &intent)
    }

    fn free(&mut self, p: Vec3) -> bool {
        let Some(world) = self.world.as_mut() else { return false };
        if world.point_contents(p.to_array(), -1) & MASK != 0 { return false; }
        let t = world.trace(TraceQuery { start: p.to_array(), end: (p + Vec3::Z * 0.5).to_array(),
            mins: [-8.0; 3], maxs: [8.0; 3], pass_entity: -1, mask: MASK });
        t.start_solid == 0 && t.all_solid == 0
    }
    fn clear(&mut self, a: Vec3, b: Vec3) -> bool {
        let Some(world) = self.world.as_mut() else { return false };
        let t = world.trace(TraceQuery { start: a.to_array(), end: b.to_array(),
            mins: [0.0; 3], maxs: [0.0; 3], pass_entity: -1, mask: MASK });
        t.start_solid == 0 && t.fraction >= 0.995
    }
    fn visible_count(&mut self, p: Vec3, eyes: &[Vec3]) -> usize {
        eyes.iter().filter(|&&eye| self.clear(p, eye)).count()
    }
    /// Fraction of the way from `eye` to `camera` that is open.
    fn reach(&mut self, eye: Vec3, camera: Vec3) -> f32 {
        let Some(world) = self.world.as_mut() else { return 1.0 };
        let t = world.trace(TraceQuery { start: eye.to_array(), end: camera.to_array(),
            mins: [-4.0; 3], maxs: [4.0; 3], pass_entity: -1, mask: MASK });
        if t.start_solid != 0 { 0.0 } else { t.fraction }
    }
    /// Mean open fraction of a follow shot's camera boom across [s, e]
    /// (recorder subject; approximate for the chase rig).
    /// Planned, whole-shot alternatives for a blocked follow shot: the other
    /// side, a higher angle, a bit closer. The first that stays clear for the
    /// entire shot is kept (never adjusted per frame).
    fn fit_rig_shot(&mut self, path: &DirectorPath, shot: &mut StudioDirectorShot, s: i32, e: i32, subject: i32) -> bool {
        if self.rig_shot_clear(path, shot, s, e, subject) { return true; }
        let original = shot.rig;
        let flipped_yaw = |yaw: f32| if yaw.abs() < 1.0 || (yaw.abs() - 180.0).abs() < 1.0 { yaw } else { wrap(-yaw) };
        let variants: [fn(&mut super::render_studio_timeline::StudioDemoFollowRig, &dyn Fn(f32) -> f32); 4] = [
            |rig, flip| { rig.horizontal_offset = -rig.horizontal_offset; rig.yaw_offset = flip(rig.yaw_offset); },
            |rig, _| { rig.pitch_offset = (rig.pitch_offset + 15.0).min(60.0); },
            |rig, _| { rig.range = (rig.range * 0.75).max(70.0); },
            |rig, _| { rig.pitch_offset = (rig.pitch_offset + 25.0).min(65.0); rig.range = (rig.range * 0.75).max(70.0); },
        ];
        for variant in variants {
            shot.rig = original;
            variant(&mut shot.rig, &flipped_yaw);
            if self.rig_shot_clear(path, shot, s, e, subject) { return true; }
        }
        shot.rig = original;
        false
    }

    /// Would this follow shot's camera stay clear the whole time? Samples the
    /// boom across the shot for `subject` (recorder: its view yaw; anyone else:
    /// their movement heading) and rejects the shot if any sample is pulled in
    /// by a wall (the camera would be squashed onto the player or into it).
    /// Shots are disqualified here rather than moved around at playback.
    fn rig_shot_clear(&mut self, path: &DirectorPath, shot: &StudioDirectorShot, s: i32, e: i32, subject: i32) -> bool {
        if self.world.is_none() { return true; }
        let rig = shot.rig;
        if shot.preset.perspective() == super::render_studio_timeline::StudioFollowPerspective::FirstPerson
            || rig.range < 48.0 { return true; }
        let offset = Vec3::from_array(super::scene::jka_position(rig.position_offset));
        let client = if shot.client >= 0 { shot.client } else { subject };
        let n = ((e-s)/100+1).clamp(7,121);
        for k in 0..n {
            let ms = s + (e - s) * k / (n - 1);
            let own = path.sample(ms);
            let (origin, facing) = if client < 0 || client == own.client { (own.origin, own.yaw) } else {
                let Some(now) = player_at(path, client, ms) else { continue };
                let before = player_at(path, client, ms - 200).unwrap_or(now);
                let moved = now - before;
                (now, if moved.truncate().length() > 8.0 { moved.y.atan2(moved.x).to_degrees() } else { own.yaw })
            };
            let eye = origin + Vec3::Z * 36.0 + offset;
            let u = k as f32 / (n - 1) as f32;
            let yaw = if rig.orbit_world_angles { rig.yaw_offset } else { facing + rig.yaw_offset } + shot.sweep * u;
            let range = shot.range_end.map_or(rig.range, |end| rig.range + (end - rig.range) * u);
            let right = Vec3::new(yaw.to_radians().sin(), -yaw.to_radians().cos(), 0.0);
            let camera = eye - forward(yaw, rig.pitch_offset) * range + Vec3::Z * rig.vertical_offset
                + right * rig.horizontal_offset;
            if self.reach(eye, camera) < 0.8 { return false; }
        }
        true
    }

    /// Mean open fraction of an orbit camera around the subject across [s, e].
    fn rig_clearance(&mut self, path: &DirectorPath, s: i32, e: i32, yaw: f32, pitch: f32, range: f32) -> f32 {
        if self.world.is_none() { return 1.0; }
        let eyes = path.eyes(s, e, 5);
        let back = forward(yaw, pitch) * range;
        eyes.iter().map(|&eye| self.reach(eye, eye - back)).sum::<f32>() / eyes.len() as f32
    }

    /// Parameterize `preset` for this stretch of path. None when a world
    /// shot cannot be placed with a clear view of the subject.
    #[allow(clippy::too_many_arguments)]
    fn build(&mut self, preset: P, beat: Beat, s: i32, e: i32, job: &ClipJob, rng: &mut Rng, intent: &ShotIntent) -> Option<StudioDirectorShot> {
        if beat == Beat::Kill && !intent.sequence() && matches!(preset, P::Orbit | P::LowAngle | P::DollyZoom | P::Static | P::Pan | P::SaberCam
            | P::VictimPov | P::VictimBlade | P::ThrowCam) {
            return self.build_kill(preset, s, e, job, rng, intent);
        }
        let clip = &job.clip;
        let path = job.path.clone();
        let start = clip.timeline_at_source_ms(s).max(clip.start);
        let end = clip.timeline_at_source_ms(e).min(clip.end());
        if end - start < 0.4 { return None; }
        let mid = (s + e) / 2;
        let recorder = path.sample(mid).client;
        let subject = intent.subject;
        let other_subject = subject != recorder;
        // World placement reads the recorder's path; another protagonist
        // gets rig shots (kill cams place around the kill instead).
        if other_subject && preset.world_anchored() && preset != P::Glide { return None; }
        let side = match self.side_at(mid) { s if s != 0.0 => s, _ => rng.sign() };
        let combat = self.combat || matches!(beat, Beat::Approach | Beat::Exchange | Beat::Kill | Beat::Aftermath);
        let standard_chase = !combat && preset == P::Chase && rng.unit() < 0.5;
        let mut shot = StudioDirectorShot::new(0, preset, -1, start, end - start);
        preset.seed_shot(&mut shot, self.fov);
        shot.auto = true;
        let f = features(&path, s, e);
        let heading = f.heading;
        let rig = &mut shot.rig;
        match preset {
            P::Chase if standard_chase => {
                // The game's standard third-person view: straight behind, not
                // re-aimed.
                rig.range = rng.range(100.0, 130.0);
                rig.pitch_offset = rng.range(8.0, 12.0);
                rig.horizontal_offset = 0.0;
            }
            P::Chase => {
                rig.motion_direction = f.speed > 500.0 && !combat;
                rig.range = if combat { rng.range(190.0, 260.0) } else { rng.range(150.0, 200.0) };
                rig.pitch_offset = rng.range(10.0, 16.0);
                // Mostly the standard view; sometimes a slight shoulder offset
                // on the fight's side of the line (180-degree rule).
                if rng.unit() < 0.3 { rig.horizontal_offset = -side * rng.range(6.0, 12.0); }
            }
            P::PlayerEyes | P::RecordedPov => {}
            P::Tracking => {
                let range = rng.range(190.0, 240.0);
                let first = rng.sign();
                let (yaw_a, yaw_b) = (wrap(heading + 90.0 * first), wrap(heading - 90.0 * first));
                let (ca, cb) = (self.rig_clearance(&path, s, e, yaw_a, 6.0, range),
                    self.rig_clearance(&path, s, e, yaw_b, 6.0, range));
                let rig = &mut shot.rig;
                rig.orbit_world_angles = true;
                rig.yaw_offset = if cb > ca + 0.15 { yaw_b } else { yaw_a };
                rig.pitch_offset = rng.range(3.0, 9.0);
                rig.range = range;
                if f.turn_rate > 60.0 { rig.orbit_world_angles = false; rig.yaw_offset = 90.0 * first; }
            }
            P::Overhead => {
                let (pitch, range) = (rng.range(62.0, 74.0), rng.range(380.0, 520.0));
                if self.world.is_some() && self.rig_clearance(&path, s, e, heading, pitch, range) < 0.55 { return None; }
                let rig = &mut shot.rig;
                rig.yaw_offset = wrap(heading);
                rig.pitch_offset = pitch;
                rig.range = range;
            }
            P::CloseUp => {
                rig.yaw_offset = wrap(rng.range(150.0, 210.0));
                rig.range = rng.range(52.0, 70.0);
                shot.fov = rng.range(28.0, 36.0);
            }
            P::LowAngle => {
                // Gently low: still shows the ground around the player.
                rig.orbit_world_angles = true;
                rig.yaw_offset = wrap(heading + 180.0 + rng.sign() * rng.range(20.0, 40.0));
                rig.pitch_offset = rng.range(-5.0, 0.0);
                rig.range = rng.range(160.0, 210.0);
            }
            P::PushIn => {
                rig.orbit_world_angles = true;
                rig.yaw_offset = wrap(heading + 180.0 + rng.sign() * rng.range(25.0, 50.0));
                rig.range = rng.range(240.0, 290.0);
                shot.range_end = Some(rng.range(100.0, 130.0));
            }
            P::DollyZoom => {
                rig.orbit_world_angles = true;
                rig.yaw_offset = wrap(heading + 180.0 + rng.range(-10.0, 10.0));
                rig.range = rng.range(320.0, 360.0);
                shot.range_end = Some(rng.range(95.0, 115.0));
            }
            P::Orbit => {
                rig.orbit_world_angles = true;
                rig.yaw_offset = wrap(heading - side * rng.range(30.0, 75.0));
                rig.pitch_offset = rng.range(12.0, 20.0);
                rig.range = rng.range(170.0, 220.0);
                shot.sweep = rng.sign() * rng.range(60.0, 120.0) * (shot.duration / 4.0).clamp(0.6, 1.6) as f32;
            }
            P::Establishing => {
                if !self.place_establishing(&mut shot, &path, s, e, heading, beat, rng) { return None; }
            }
            P::Static => {
                let extra = if matches!(beat, Beat::Approach | Beat::Exchange | Beat::Kill | Beat::Aftermath) {
                    action_points(&path, s, e, subject)
                } else { Vec::new() };
                if !self.place_static(&mut shot, &path, s, e, &extra, rng) { return None; }
            }
            P::Pan => { if !self.place_pan(&mut shot, &path, s, e, rng) { return None; } }
            P::FlyBy => { if !self.place_flyby(&mut shot, &path, s, e, rng) { return None; } }
            P::Crane => { if !self.place_crane(&mut shot, &path, s, e, heading, beat, rng) { return None; } }
            P::OverShoulder => {
                let opponent = opponent(&path, s, e, subject)?;
                // Some exchanges reverse the angle: over the opponent's shoulder.
                let rig = &mut shot.rig;
                rig.aim_client = opponent;
                rig.horizontal_offset = -side * rng.range(18.0, 28.0);
                rig.range = rng.range(115.0, 155.0);
                rig.pitch_offset = rng.range(4.0, 9.0);
                shot.fov = rng.range(62.0, 70.0);
            }
            P::Glide => { if !self.place_glide(&mut shot, &path, s, e, subject, side, rng) { return None; } }
            P::FollowCam => {
                let foe = opponent(&path, s, e, subject);
                let range = rng.range(150.0, 200.0);
                let height = rng.range(45.0, 80.0);
                // Big lateral offset on the fight's side of the line.
                let lateral = -side * rng.range(35.0, 90.0);
                let damp = rng.range(0.35, 0.6);
                if !self.plan_follow(&mut shot, &path, s, e, subject, foe, [range, height, lateral, damp], &intent.kills) { return None; }
                shot.fov = rng.range(60.0, 70.0);
            }
            // Need a saber kill to ride (kill cams only).
            P::SaberCam | P::VictimPov | P::VictimBlade => return None,
            P::ThrowCam => {
                if !throw_in(&path, subject, s, e) { return None; }
                shot.rig.aim_client = opponent(&path, s, e, subject)?;
            }
            P::TwoShot => {
                let opponent = opponent(&path, s, e, subject)?;
                let me = player_at(&path, subject, mid)?;
                let other = player_at(&path, opponent, mid)?;
                let line = other - me;
                let distance = line.truncate().length();
                let rig = &mut shot.rig;
                rig.orbit_world_angles = true;
                // Behind the subject, 25-45 degrees off the line to the opponent.
                rig.yaw_offset = wrap(line.y.atan2(line.x).to_degrees() - side * rng.range(25.0, 45.0));
                rig.pitch_offset = rng.range(8.0, 14.0);
                rig.range = (distance * 0.8 + 220.0).clamp(240.0, 700.0);
                // Centre the frame between the fighters (renderer-space offset).
                rig.position_offset = render(line * 0.5);
                shot.fov = rng.range(66.0, 74.0);
            }
        }
        if preset == P::Chase && combat {
            let foe = opponent(&path, s, e, subject);
            if !self.plan_chase(&mut shot, &path, s, e, subject, foe) { return None; }
        }
        // Fights keep both combatants in frame: frame the midpoint between
        // them (or aim at the opponent) and back off to fit the gap.
        if matches!(beat, Beat::Approach | Beat::Exchange) && !standard_chase && shot.path_keys.is_empty() {
            self.frame_both(&mut shot, &path, s, e, subject);
        }
        // Reject rig shots whose camera would sit in or against walls for
        // most of the shot (the runtime guard then has less to rescue).
        if shot.position_override.is_none() && shot.path_keys.is_empty()
            && !self.fit_rig_shot(&path, &mut shot, s, e, subject) { return None; }
        if (other_subject || self.combat) && shot.client < 0 {
            shot.client = subject;
            // Angles derived from the recorder's travel don't apply: use the
            // type's facing-relative default.
            if other_subject && shot.rig.orbit_world_angles && shot.preset != P::TwoShot {
                let mut defaults = shot.rig;
                shot.preset.seed(&mut defaults);
                shot.rig.orbit_world_angles = false;
                shot.rig.yaw_offset = defaults.yaw_offset;
            }
        }
        self.player_lens(&mut shot, beat, rng);
        self.crowd_fov(&path, subject, s, e, &mut shot);
        let range = shot.rig.range.max(16.0);
        shot.lens.focus_distance = range;
        shot.valid().then_some(shot)
    }

    /// Pick one valid candidate among the best few, varied by the seed.
    fn pick(candidates: Vec<(f32, Vec3, Vec3)>, rng: &mut Rng) -> Option<(Vec3, Vec3)> {
        let mut candidates = candidates;
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        let n = candidates.len().min(3);
        (n > 0).then(|| { let c = candidates[rng.below(n)]; (c.1, c.2) })
    }

    #[allow(clippy::too_many_arguments)]
    fn place_establishing(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32,
        heading: f32, beat: Beat, rng: &mut Rng) -> bool {
        let anchor = path.eye(s + (e - s) / 4);
        let eyes = path.eyes(s, e, LOS_SAMPLES);
        // Opening: look back at the subject from ahead. Closing: from behind.
        let base = if beat == Beat::Closing { heading + 180.0 } else { heading };
        let mut found = Vec::new();
        for (order, yaw) in [0.0f32, 35.0, -35.0, 70.0, -70.0, 110.0, -110.0, 180.0].into_iter().enumerate() {
            for (dist, height) in [(760.0f32, 300.0f32), (560.0, 220.0), (400.0, 140.0)] {
                let p = anchor + flat(forward(base + yaw, 0.0)) * dist + Vec3::Z * height;
                if !self.free(p) { continue; }
                let seen = self.visible_count(p, &eyes);
                if seen + 1 < LOS_SAMPLES { continue; }
                let drift = p + (anchor - p).normalize_or_zero() * dist * 0.12;
                if !self.clear(p, drift) { continue; }
                found.push((seen as f32 * 10.0 - order as f32 + dist / 400.0, p, drift));
            }
            if found.len() >= 6 { break; }
        }
        let Some((p, drift)) = Self::pick(found, rng) else { return false };
        shot.position_override = Some(render(p));
        shot.position_end = Some(render(drift));
        shot.fov = rng.range(80.0, 92.0);
        shot.rig.range = p.distance(anchor);
        true
    }

    fn place_static(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, extra: &[Vec3],
        rng: &mut Rng) -> bool {
        let mut eyes = path.eyes(s, e, LOS_SAMPLES);
        // Other active players to fit in the same frame (fights).
        eyes.extend_from_slice(extra);
        let center = eyes.iter().copied().sum::<Vec3>() / eyes.len() as f32;
        let extent = eyes.iter().map(|p| p.distance(center)).fold(0.0, f32::max);
        let axis = flat(eyes[eyes.len() - 1] - eyes[0]);
        let axis = if axis == Vec3::ZERO { Vec3::X } else { axis };
        let right = Vec3::new(axis.y, -axis.x, 0.0);
        let first = rng.sign();
        let mut found = Vec::new();
        for side in [first, -first] {
            for scale in [0.9f32, 0.7, 1.25] {
                let dist = (extent * scale + 200.0).clamp(200.0, 1600.0);
                for height in [56.0f32, 140.0, 24.0] {
                    let p = center + right * side * dist + Vec3::Z * height;
                    if !self.free(p) || self.visible_count(p, &eyes) + extra.len() / 4 < eyes.len() { continue; }
                    let look = (center - p).normalize_or_zero();
                    let spread = eyes.iter().map(|&eye| look.angle_between((eye - p).normalize_or_zero()).to_degrees())
                        .fold(0.0, f32::max);
                    let fov = spread * 2.0 * 1.15 + 6.0;
                    if fov > 105.0 { continue; }
                    found.push((-fov - dist / 200.0, p, Vec3::new(fov.max(40.0), 0.0, 0.0)));
                }
            }
        }
        let Some((p, fov)) = Self::pick(found, rng) else { return false };
        shot.position_override = Some(render(p));
        shot.focus_target = Some(render(center));
        shot.fov = fov.x.clamp(40.0, 105.0);
        shot.rig.aim_enabled = false;
        // Framing is fixed; focus follows the subject wherever it moves.
        shot.rig.autofocus = true;
        shot.lens.focus_distance = p.distance(center).clamp(1.0, 65536.0);
        shot.rig.range = p.distance(center);
        true
    }

    fn place_pan(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, rng: &mut Rng) -> bool {
        let mid = s + (e - s) / 2;
        let eyes = path.eyes(s, e, LOS_SAMPLES);
        let m = path.eye(mid);
        let dir = flat(path.sample(mid).velocity);
        let dir = if dir == Vec3::ZERO { flat(eyes[eyes.len() - 1] - eyes[0]) } else { dir };
        let dir = if dir == Vec3::ZERO { Vec3::X } else { dir };
        let right = Vec3::new(dir.y, -dir.x, 0.0);
        let first = rng.sign();
        let mut found = Vec::new();
        for side in [first, -first] {
            for dist in [260.0f32, 340.0, 200.0, 420.0] {
                for ahead in [0.0f32, 150.0, -100.0] {
                    for height in [40.0f32, 96.0, 16.0] {
                        let p = m + right * side * dist + dir * ahead + Vec3::Z * height;
                        if !self.free(p) { continue; }
                        let seen = self.visible_count(p, &eyes);
                        if seen + 1 < LOS_SAMPLES { continue; }
                        found.push((seen as f32 * 10.0 - dist / 200.0, p, p));
                    }
                }
            }
            if found.len() >= 4 { break; }
        }
        let Some((p, _)) = Self::pick(found, rng) else { return false };
        shot.position_override = Some(render(p));
        shot.fov = rng.range(50.0, 62.0);
        shot.rig.range = p.distance(m);
        true
    }

    fn place_flyby(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, rng: &mut Rng) -> bool {
        let (from, to) = (s + (e - s) / 5, e - (e - s) / 5);
        let fastest = path.window(from, to).iter()
            .max_by(|a, b| a.velocity.length().total_cmp(&b.velocity.length())).map_or(s + (e - s) / 2, |p| p.ms);
        let m = path.eye(fastest);
        let eyes = path.eyes(s, e, LOS_SAMPLES);
        let dir = flat(path.sample(fastest).velocity);
        let dir = if dir == Vec3::ZERO { Vec3::X } else { dir };
        let right = Vec3::new(dir.y, -dir.x, 0.0);
        let first = rng.sign();
        let mut found = Vec::new();
        for side in [first, -first] {
            for dist in [110.0f32, 150.0, 90.0, 200.0] {
                for height in [12.0f32, 36.0, -8.0] {
                    let p = m + right * side * dist + Vec3::Z * height;
                    if !self.free(p) || !self.clear(p, m) { continue; }
                    let seen = self.visible_count(p, &eyes);
                    if seen + 2 < LOS_SAMPLES { continue; }
                    found.push((seen as f32 * 10.0 - dist / 100.0, p, p));
                }
            }
            if found.len() >= 3 { break; }
        }
        let Some((p, _)) = Self::pick(found, rng) else { return false };
        shot.position_override = Some(render(p));
        shot.fov = rng.range(80.0, 92.0);
        shot.rig.range = p.distance(m);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn place_crane(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32,
        heading: f32, beat: Beat, rng: &mut Rng) -> bool {
        let (eye_s, eye_m, eye_e) = (path.eye(s), path.eye(s + (e - s) / 2), path.eye(e));
        // Openings rise off the subject; endings rise away from where it stopped.
        let subject = if matches!(beat, Beat::Closing | Beat::Finish) { eye_e } else { eye_s };
        let mut found = Vec::new();
        for yaw in [60.0f32, -60.0, 120.0, -120.0, 180.0] {
            let p0 = subject + flat(forward(heading + yaw, 0.0)) * 220.0 + Vec3::Z * 8.0;
            let rise = rng.range(300.0, 420.0);
            let p1 = p0 + Vec3::Z * rise + flat(p0 - subject) * 160.0;
            if !self.free(p0) || !self.free(p1) || !self.clear(p0, p1) { continue; }
            if !self.clear(p0, eye_s) || !self.clear(p1, eye_e) { continue; }
            let mid = p0.lerp(p1, 0.5);
            if !self.clear(mid, eye_m) { continue; }
            found.push((rise / 100.0 - yaw.abs() / 90.0, p0, p1));
        }
        let Some((p0, p1)) = Self::pick(found, rng) else { return false };
        shot.position_override = Some(render(p0));
        shot.position_end = Some(render(p1));
        shot.rig.range = p0.distance(subject);
        shot.rig.aim_anchor = StudioDemoAnchor::Chest;
        true
    }
}

// ---------------------------------------------------------------------------
// Character stage: Director shots over animation / emote clips. There is no
// recorded path or map collision here; framing comes from the actor's own
// motion keys (position, facing, smoothed transitions), so world shots are
// placed relative to where the actor stands during the shot.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StageBeat { Opening, Showcase, Moving, Ending }

impl StageBeat {
    fn candidates(self) -> &'static [(P, f32)] {
        match self {
            Self::Opening => &[(P::Establishing, 4.0), (P::Crane, 3.0), (P::Orbit, 2.0), (P::Static, 1.0)],
            Self::Showcase => &[(P::CloseUp, 4.0), (P::LowAngle, 3.0), (P::Orbit, 3.0), (P::PushIn, 2.0),
                (P::Static, 2.0), (P::Pan, 1.0), (P::Overhead, 1.0), (P::DollyZoom, 1.0)],
            Self::Moving => &[(P::Tracking, 4.0), (P::Chase, 3.0), (P::Pan, 3.0), (P::FlyBy, 2.0),
                (P::Orbit, 1.0), (P::LowAngle, 1.0)],
            Self::Ending => &[(P::Crane, 3.0), (P::Orbit, 3.0), (P::LowAngle, 2.0), (P::Static, 1.0),
                (P::Establishing, 1.0)],
        }
    }
}

/// Horizontal renderer-space direction of a JKA yaw.
fn stage_flat(yaw: f32) -> Vec3 {
    let (s, c) = yaw.to_radians().sin_cos();
    Vec3::new(c, 0.0, -s)
}
fn stage_chest(origin: Vec3) -> Vec3 {
    origin + Vec3::Y * super::render_studio_director::stage_anchor_height(StudioDemoAnchor::Chest)
}

pub(in crate::app) struct StageDirector<'a> {
    timeline: &'a super::render_studio_timeline::StudioTimeline,
    zoom: f32,
    base_yaw: f32,
    fov: f32,
    pacing: StudioDirectorPacing,
    seed: u32,
    history: Vec<P>,
    uses: HashMap<P, u32>,
}

impl<'a> StageDirector<'a> {
    pub fn new(timeline: &'a super::render_studio_timeline::StudioTimeline, zoom: f32, base_yaw: f32, fov: f32,
        pacing: StudioDirectorPacing, seed: u32) -> Self {
        Self { timeline, zoom, base_yaw, fov, pacing, seed, history: Vec::new(), uses: HashMap::new() }
    }

    fn sample(&self, lane: usize, time: f64, clip_end: f64) -> Option<(Vec3, f32)> {
        self.timeline.stage_actor_at(lane, time.min(clip_end - 1e-3), self.zoom, self.base_yaw)
    }
    /// Distance the actor travels across [a, b].
    fn travel(&self, lane: usize, a: f64, b: f64, end: f64) -> f32 {
        let points: Vec<Vec3> = (0..=6).filter_map(|k| self.sample(lane, a + (b - a) * f64::from(k) / 6.0, end).map(|s| s.0)).collect();
        points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// Cut one animation clip into a shot sequence (ids 0).
    pub fn direct_clip(&mut self, index: usize) -> Vec<StudioDirectorShot> {
        let Some(clip) = self.timeline.clips.get(index) else { return Vec::new() };
        let (start, end, lane) = (clip.start, clip.end(), clip.lane);
        let duration = end - start;
        let n = if duration < 2.4 { 1 } else { ((duration / self.pacing.shot_seconds()).round() as usize).max(2) };
        let mut shots = Vec::new();
        for k in 0..n {
            let s = start + duration * k as f64 / n as f64;
            let e = start + duration * (k + 1) as f64 / n as f64;
            let moving = self.travel(lane, s, e, end) > 48.0;
            let beat = if n == 1 { if moving { StageBeat::Moving } else { StageBeat::Showcase } }
                else if k == 0 { StageBeat::Opening }
                else if k == n - 1 { StageBeat::Ending }
                else if moving { StageBeat::Moving } else { StageBeat::Showcase };
            let mut rng = Rng::new(&[u64::from(self.seed), name_hash(&clip.name), (s * 1000.0) as u64, lane as u64]);
            let neighbors = [self.history.last().copied(), self.history.iter().rev().nth(1).copied()];
            if let Some(mut shot) = self.choose(beat, s, e, lane, end, neighbors, None, &mut rng) {
                shot.seed = self.seed;
                self.history.push(shot.preset);
                *self.uses.entry(shot.preset).or_default() += 1;
                shots.push(shot);
            }
        }
        shots
    }

    /// Different decision (or re-placement as `keep`) for a stage shot.
    pub fn regenerate(&mut self, shot: &StudioDirectorShot, neighbors: [Option<P>; 2], keep: Option<P>)
        -> Option<StudioDirectorShot> {
        let lane = shot.actor_lane;
        let clip = self.timeline.clip_at_lane(shot.start + shot.duration * 0.5, lane)?;
        let (s, e, end) = (shot.start.max(clip.start), shot.end().min(clip.end()), clip.end());
        if e - s < 0.1 { return None; }
        let long = clip.duration() >= 3.0;
        let beat = if long && s <= clip.start + 0.05 { StageBeat::Opening }
            else if long && e >= clip.end() - 0.05 { StageBeat::Ending }
            else if self.travel(lane, s, e, end) > 48.0 { StageBeat::Moving } else { StageBeat::Showcase };
        let seed = shot.seed.wrapping_add(1);
        let mut rng = Rng::new(&[u64::from(seed), name_hash(&clip.name), (s * 1000.0) as u64, shot.id]);
        let built = match keep {
            Some(preset) => self.build(preset, s, e, lane, end, &mut rng),
            None => self.choose(beat, s, e, lane, end, neighbors, Some(shot.preset), &mut rng),
        };
        built.map(|mut new| {
            (new.id, new.client, new.enabled, new.seed, new.start, new.duration) =
                (shot.id, shot.client, shot.enabled, seed, shot.start, shot.duration);
            new
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn choose(&mut self, beat: StageBeat, s: f64, e: f64, lane: usize, end: f64, neighbors: [Option<P>; 2],
        exclude: Option<P>, rng: &mut Rng) -> Option<StudioDirectorShot> {
        let mut pool: Vec<(P, f32)> = beat.candidates().iter().map(|&(preset, weight)| {
            let mut w = weight;
            if Some(preset) == neighbors[0] || Some(preset) == exclude { w = 0.0; }
            if Some(preset) == neighbors[1] { w *= 0.4; }
            let used = self.uses.get(&preset).copied().unwrap_or(0) as f32;
            if matches!(preset, P::DollyZoom | P::PushIn | P::CloseUp | P::LowAngle) { w /= 1.0 + used * 1.5; }
            (preset, w)
        }).collect();
        while pool.iter().any(|p| p.1 > 0.0) {
            let total: f32 = pool.iter().map(|p| p.1).sum();
            let mut pick = rng.unit() * total;
            let index = pool.iter().position(|p| { pick -= p.1; p.1 > 0.0 && pick <= 0.0 })
                .unwrap_or_else(|| pool.iter().rposition(|p| p.1 > 0.0).unwrap_or(0));
            if let Some(shot) = self.build(pool[index].0, s, e, lane, end, rng) { return Some(shot); }
            pool[index].1 = 0.0;
        }
        let fallback = if exclude == Some(P::Orbit) || neighbors[0] == Some(P::Orbit) { P::Chase } else { P::Orbit };
        self.build(fallback, s, e, lane, end, rng)
    }

    fn build(&self, preset: P, s: f64, e: f64, lane: usize, end: f64, rng: &mut Rng) -> Option<StudioDirectorShot> {
        if e - s < 0.1 { return None; }
        let (o0, yaw0) = self.sample(lane, s, end)?;
        let (om, yawm) = self.sample(lane, (s + e) * 0.5, end)?;
        let (oe, _) = self.sample(lane, e, end)?;
        let moving = self.travel(lane, s, e, end) > 48.0;
        let travel = oe - o0;
        let heading = if travel.x.hypot(travel.z) > 16.0 { (-travel.z).atan2(travel.x).to_degrees() } else { yawm };
        let mut shot = StudioDirectorShot::new(0, preset, -1, s, e - s);
        preset.seed_shot(&mut shot, self.fov);
        shot.auto = true;
        shot.actor_lane = lane;
        let side = rng.sign();
        let rig = &mut shot.rig;
        match preset {
            P::Chase => {
                rig.motion_direction = moving;
                rig.range = rng.range(110.0, 150.0);
                rig.pitch_offset = rng.range(8.0, 12.0);
            }
            P::PlayerEyes | P::RecordedPov => {}
            P::Tracking => {
                rig.range = rng.range(170.0, 210.0);
                rig.pitch_offset = rng.range(3.0, 9.0);
                if moving { rig.orbit_world_angles = true; rig.yaw_offset = wrap(heading + 90.0 * side); }
                else { rig.yaw_offset = 90.0 * side; }
            }
            P::CloseUp => {
                rig.yaw_offset = wrap(rng.range(150.0, 210.0));
                rig.range = rng.range(50.0, 65.0);
                shot.fov = rng.range(28.0, 36.0);
            }
            P::LowAngle => {
                rig.yaw_offset = wrap(180.0 + side * rng.range(20.0, 40.0));
                rig.pitch_offset = rng.range(-14.0, -8.0);
                rig.range = rng.range(100.0, 130.0);
            }
            P::Overhead => {
                rig.orbit_world_angles = false;
                rig.yaw_offset = 180.0;
                rig.pitch_offset = rng.range(62.0, 72.0);
                rig.range = rng.range(260.0, 340.0);
            }
            P::PushIn => {
                rig.yaw_offset = wrap(180.0 + side * rng.range(25.0, 50.0));
                rig.range = rng.range(240.0, 280.0);
                shot.range_end = Some(rng.range(100.0, 120.0));
            }
            P::DollyZoom => {
                rig.yaw_offset = wrap(180.0 + rng.range(-10.0, 10.0));
                rig.range = rng.range(300.0, 340.0);
                shot.range_end = Some(rng.range(95.0, 115.0));
            }
            P::Orbit => {
                rig.yaw_offset = wrap(180.0 + rng.range(-60.0, 60.0));
                rig.pitch_offset = rng.range(10.0, 18.0);
                rig.range = rng.range(150.0, 190.0);
                shot.sweep = side * rng.range(60.0, 120.0) * (shot.duration / 4.0).clamp(0.6, 1.6) as f32;
            }
            P::Establishing => {
                let subject = stage_chest(o0);
                let p = subject + stage_flat(yaw0 + rng.range(-40.0, 40.0)) * rng.range(320.0, 420.0)
                    + Vec3::Y * rng.range(120.0, 180.0);
                shot.position_override = Some(p.to_array());
                shot.position_end = Some((p + (subject - p) * 0.12).to_array());
                shot.fov = rng.range(70.0, 80.0);
                shot.rig.range = p.distance(subject);
            }
            P::Static => {
                let chests: Vec<Vec3> = (0..5).filter_map(|k| self.sample(lane, s + (e - s) * f64::from(k) / 4.0, end))
                    .map(|(o, _)| stage_chest(o)).collect();
                let center = chests.iter().copied().sum::<Vec3>() / chests.len().max(1) as f32;
                let extent = chests.iter().map(|p| p.distance(center)).fold(0.0, f32::max);
                let distance = (extent * 1.2 + 150.0).max(rng.range(180.0, 260.0));
                let p = center + stage_flat(yawm + side * rng.range(30.0, 70.0)) * distance + Vec3::Y * rng.range(10.0, 40.0);
                let look = (center - p).normalize_or_zero();
                let spread = chests.iter().map(|&c| look.angle_between((c - p).normalize_or_zero()).to_degrees()).fold(0.0, f32::max);
                shot.position_override = Some(p.to_array());
                shot.focus_target = Some(center.to_array());
                shot.fov = (spread * 2.3 + 30.0).clamp(35.0, 90.0);
                shot.rig.aim_enabled = false;
                shot.rig.autofocus = false;
                shot.lens.focus_distance = p.distance(center).clamp(1.0, 65536.0);
                shot.rig.range = p.distance(center);
            }
            P::Pan => {
                let subject = stage_chest(om);
                let p = subject + stage_flat(yawm + side * rng.range(50.0, 100.0)) * rng.range(160.0, 240.0)
                    + Vec3::Y * rng.range(10.0, 40.0);
                shot.position_override = Some(p.to_array());
                shot.fov = rng.range(50.0, 62.0);
                shot.rig.range = p.distance(subject);
            }
            P::OverShoulder | P::TwoShot | P::SaberCam | P::VictimPov | P::VictimBlade | P::ThrowCam | P::Glide
            | P::FollowCam => return None,
            P::FlyBy => {
                if !moving { return None; }
                let subject = stage_chest(om);
                let p = subject + stage_flat(heading - 90.0) * side * rng.range(70.0, 110.0) + Vec3::Y * rng.range(0.0, 20.0);
                shot.position_override = Some(p.to_array());
                shot.fov = rng.range(80.0, 90.0);
                shot.rig.range = p.distance(subject);
            }
            P::Crane => {
                let (origin, yaw) = if e >= end - 0.05 { (oe, yawm) } else { (o0, yaw0) };
                let subject = stage_chest(origin);
                let p0 = subject + stage_flat(yaw + side * rng.range(30.0, 60.0)) * 150.0 - Vec3::Y * 10.0;
                let away = Vec3::new(p0.x - subject.x, 0.0, p0.z - subject.z).normalize_or_zero();
                let p1 = p0 + Vec3::Y * rng.range(200.0, 260.0) + away * 120.0;
                shot.position_override = Some(p0.to_array());
                shot.position_end = Some(p1.to_array());
                shot.rig.range = p0.distance(subject);
            }
        }
        shot.lens.focus_distance = shot.rig.range.max(16.0);
        if let Some(target) = shot.focus_target {
            if let Some(p) = shot.position_override {
                shot.lens.focus_distance = Vec3::from_array(p).distance(Vec3::from_array(target)).clamp(1.0, 65536.0);
            }
        }
        shot.valid().then_some(shot)
    }
}

/// Summary for a stage cut.
pub(in crate::app) fn stage_summary(shots: &[StudioDirectorShot], clips: usize, seed: u32) -> String {
    format!("Directed {clips} clip{}: {} shots · {} · variation {seed}", if clips == 1 { "" } else { "s" },
        shots.len(), type_summary(shots))
}


// ---------------------------------------------------------------------------
// Combat analysis.
// ---------------------------------------------------------------------------

fn split_beats(beats: Vec<(i32, i32, Beat)>, cuts: &[i32]) -> Vec<(i32, i32, Beat)> {
    let mut out = Vec::new();
    for (s, e, beat) in beats {
        let mut from = s;
        for &at in cuts.iter().filter(|&&at| at > s && at < e) {
            out.push((from, at, beat));
            from = at;
        }
        if e > from { out.push((from, e, beat)); }
    }
    out
}

fn retime_camera_path(clip: &StudioDemoClip, shot: &mut StudioDirectorShot) {
    if shot.path_keys.is_empty() || clip.speed_keys.is_empty() { return; }
    let (s, e) = (clip.source_time_ms(shot.start), clip.source_time_ms(shot.end()));
    for key in &mut shot.path_keys {
        let ms = s + ((e - s) as f64 * f64::from(key.t)).round() as i32;
        key.t = ((clip.timeline_at_source_ms(ms) - shot.start) / shot.duration).clamp(0.0, 1.0) as f32;
    }
    if let Some(first) = shot.path_keys.first_mut() { first.t = 0.0; }
    if let Some(last) = shot.path_keys.last_mut() { last.t = 1.0; }
}

fn retime_edit(old: &StudioDemoClip, new: &StudioDemoClip, shots: &mut [StudioDirectorShot]) {
    for shot in shots {
        let (s, e) = (old.source_time_ms(shot.start), old.source_time_ms(shot.end()));
        let key_ms: Vec<_> = shot.path_keys.iter().map(|k| old.source_time_ms(shot.start + shot.duration * f64::from(k.t))).collect();
        shot.start = new.timeline_at_source_ms(s);
        shot.duration = new.timeline_at_source_ms(e) - shot.start;
        for (key, ms) in shot.path_keys.iter_mut().zip(key_ms) {
            key.t = ((new.timeline_at_source_ms(ms) - shot.start) / shot.duration).clamp(0.0, 1.0) as f32;
        }
        if let Some(key) = shot.path_keys.first_mut() { key.t = 0.0; }
        if let Some(key) = shot.path_keys.last_mut() { key.t = 1.0; }
    }
}

fn viewed_subject(path: &DirectorPath, clip: &StudioDemoClip, shots: &[StudioDirectorShot], ms: i32) -> i32 {
    let time = clip.timeline_at_source_ms(ms);
    let recorder = path.sample(ms).client;
    shots.iter().find(|shot| shot.contains(time)).filter(|shot| shot.client >= 0 && player_at(path, shot.client, ms).is_some())
        .filter(|shot| shot.client == recorder || path.window(ms - 400, ms).iter().rev().find(|s| s.cut).is_none_or(|cut|
            path.players.iter().find(|(c, _)| *c == shot.client).and_then(|(_, list)|
                list.partition_point(|(t, _)| *t <= ms).checked_sub(1).map(|i| list[i].0)).is_some_and(|t| t >= cut.ms)))
        .map_or(recorder, |shot| shot.client)
}

fn relevant_kills(path: &DirectorPath, s: i32, e: i32, who: &dyn Fn(i32) -> i32) -> Vec<CombatEvent> {
    path.events.iter().filter(|ev| ev.kind == CombatKind::Kill && ev.ms >= s && ev.ms < e)
        .filter(|ev| { let subject = who(ev.ms); subject >= 0 && (ev.attacker == subject || ev.victim == subject) })
        .copied().collect()
}

fn combat_engagement(path: &DirectorPath, s: i32, e: i32, who: &dyn Fn(i32) -> i32) -> Engagement {
    let kills = relevant_kills(path, s, e, who);
    let mut participants = Vec::new();
    for event in path.events.iter().filter(|ev| ev.ms >= s && ev.ms <= e) {
        let subject = who(event.ms);
        if event.attacker == subject || event.victim == subject {
            participants.extend([subject, event.attacker, event.victim]);
        }
    }
    let mut ms = s;
    while ms <= e { participants.push(who(ms)); ms += 250; }
    participants.retain(|&client| client >= 0);
    participants.sort_unstable();
    participants.dedup();
    Engagement { start: s, end: e, participants, kills }
}

/// Preserve every event; consecutive impacts share one uninterrupted shot.
fn kill_sequences(kills: &[CombatEvent]) -> Vec<&[CombatEvent]> {
    let mut out = Vec::new();
    let mut from = 0;
    for i in 1..kills.len() {
        if kills[i].ms - kills[i - 1].ms >= 1600 {
            out.push(&kills[from..i]);
            from = i;
        }
    }
    if from < kills.len() { out.push(&kills[from..]); }
    out
}

fn shot_sample_times(s: i32, e: i32, n: i32, kills: &[CombatEvent]) -> Vec<i32> {
    let mut times: Vec<_> = (0..n).map(|i| s + (e - s - 1).max(0) * i / (n - 1).max(1)).collect();
    times.extend(kills.iter().filter(|kill| kill.ms >= s && kill.ms < e).map(|kill| kill.ms));
    times.sort_unstable();
    times.dedup();
    times
}

/// Hold the last known point only after a real observation. Never interpolate
/// through a teleport, extend a previous spectator's path, or borrow a future spawn.
fn lingering_player(path: &DirectorPath, client: i32, ms: i32) -> Option<Vec3> {
    let samples = &path.players.iter().find(|(c, _)| *c == client)?.1;
    let i = samples.partition_point(|(t, _)| *t <= ms).checked_sub(1)?;
    let (time, position) = samples[i];
    (ms - time <= VISIBILITY_LINGER_MS).then_some(position)
}

#[derive(Clone, Copy)]
struct FramePoint { x: f32, y: f32, height: f32 }
impl FramePoint {
    fn inside(self) -> bool { self.x.abs() < 0.88 && self.y.abs() < 0.88 }
}

/// cg_fov is horizontal on a 4:3 baseline. Use one projection for both
/// acceptance and ranking, including a stable basis when looking straight down.
fn frame_point(camera: Vec3, look: Vec3, fov: f32, point: Vec3) -> Option<FramePoint> {
    let view = (look - camera).normalize_or_zero();
    if view == Vec3::ZERO { return None; }
    let right = view.cross(if view.z.abs() > 0.999 { Vec3::Y } else { Vec3::Z }).normalize_or_zero();
    let up = right.cross(view);
    let delta = point - camera;
    let depth = delta.dot(view);
    if depth <= 1.0 { return None; }
    let tan_x = (fov.clamp(10.0, 140.0).to_radians() * 0.5).tan();
    let tan_y = tan_x * 0.75;
    Some(FramePoint { x: delta.dot(right) / (depth * tan_x), y: delta.dot(up) / (depth * tan_y),
        height: 88.0 / (2.0 * depth * tan_y) })
}

/// A deterministic angular controller: rest while the action fits comfortably,
/// then reframe with bounded speed and acceleration. State is baked in source
/// time, never integrated in render-frame order.
#[derive(Default)]
struct CameraAim { direction: Option<Vec3>, speed: f32, reframing: bool }
impl CameraAim {
    fn step(&mut self, camera: Vec3, target: Vec3, dt: f32) -> Vec3 {
        let desired = (target - camera).normalize_or_zero();
        if desired == Vec3::ZERO { return target; }
        let Some(current) = self.direction else { self.direction = Some(desired); return target };
        let angle = current.angle_between(desired);
        if !self.reframing && angle > 12.0f32.to_radians() { self.reframing = true; }
        if self.reframing && angle < 2.0f32.to_radians() { self.reframing = false; }
        let acceleration = 180.0f32.to_radians();
        let goal = if self.reframing { 90.0f32.to_radians().min((2.0 * acceleration * angle).sqrt()) } else { 0.0 };
        self.speed += (goal - self.speed).clamp(-acceleration * dt, acceleration * dt);
        if self.speed > 0.0 && angle > 0.00001 {
            let turn = (self.speed * dt).min(angle);
            let axis = current.cross(desired).try_normalize()
                .unwrap_or_else(|| current.any_orthonormal_vector());
            self.direction = Some(glam::Quat::from_axis_angle(axis, turn) * current);
        }
        camera + self.direction.unwrap() * camera.distance(target).max(1.0)
    }
}

/// Origin of `client` at `ms`: the recorded player from its path, anyone
/// else from the snapshot samples (within 300 ms of a visible sample).
fn player_at(path: &DirectorPath, client: i32, ms: i32) -> Option<Vec3> {
    if client < 0 { return None; }
    let own = path.sample(ms);
    if own.client == client { return Some(own.origin); }
    let (_, list) = path.players.iter().find(|(c, _)| *c == client)?;
    let i = list.partition_point(|p| p.0 <= ms);
    let before = i.checked_sub(1).map(|j| list[j]);
    let after = list.get(i).copied();
    match (before, after) {
        (Some(a), Some(b)) if b.0 - a.0 <= 400 && a.1.distance(b.1) <= 400.0 => {
            let u = (ms - a.0) as f32 / (b.0 - a.0).max(1) as f32;
            Some(a.1.lerp(b.1, u.clamp(0.0, 1.0)))
        }
        (Some(a), _) if ms - a.0 <= 300 => Some(a.1),
        _ => None,
    }
}

/// The player the recorder is fighting across [s, e]: hits / kills between
/// them first, then time spent close together.
fn opponent(path: &DirectorPath, s: i32, e: i32, recorder: i32) -> Option<i32> {
    let mut score: HashMap<i32, f32> = HashMap::new();
    let from = path.events.partition_point(|ev| ev.ms < s - 1500);
    for event in path.events[from..].iter().take_while(|ev| ev.ms <= e + 1500) {
        if event.attacker == recorder && event.victim >= 0 && event.victim != recorder {
            *score.entry(event.victim).or_default() += 3.0;
        }
        if event.victim == recorder && event.attacker >= 0 && event.attacker != recorder {
            *score.entry(event.attacker).or_default() += 3.0;
        }
    }
    for (client, list) in &path.players {
        if *client == recorder { continue; }
        let close = list.iter().filter(|(ms, p)| *ms >= s && *ms <= e
            && player_at(path, recorder, *ms).is_some_and(|me| me.distance(*p) < 450.0)).count();
        if close > 0 { *score.entry(*client).or_default() += close as f32 * 0.2; }
    }
    score.into_iter().filter(|&(client, v)| v > 0.0 && [s, (s + e) / 2, e - 1].into_iter()
        .any(|ms| player_at(path, client, ms).is_some()))
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(c, _)| c)
}

impl AutoDirector {
    fn frame_both(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, subject: i32) {
        let Some(opponent) = opponent(path, s, e, subject) else { return };
        if shot.client >= 0 { return; }
        let mid = (s + e) / 2;
        let Some(me) = player_at(path, subject, mid) else { return };
        let Some(other) = player_at(path, opponent, mid) else { return };
        let half = (other - me) * 0.5;
        let gap = (other - me).truncate().length();
        if gap > 2000.0 { return; }
        let rig = &mut shot.rig;
        match shot.preset {
            // Behind you, looking at them: both in frame.
            P::Chase => {
                rig.aim_enabled = true;
                rig.aim_client = opponent;
                rig.aim_anchor = StudioDemoAnchor::Chest;
                rig.range = rig.range.max(160.0);
            }
            // Orbit-type shots look at their anchor: move it to the midpoint.
            P::Tracking | P::Orbit | P::LowAngle => {
                rig.aim_enabled = false;
                rig.position_offset = render(half);
                rig.range = rig.range.max(gap * 1.0 + 170.0).min(1500.0);
                shot.fov = shot.fov.max(60.0);
            }
            // World pans aim at the midpoint and widen to fit both.
            P::Pan if shot.position_override.is_some() => {
                rig.aim_offset = render(half);
                let camera = super::scene::jka_position(shot.position_override.unwrap());
                let camera = Vec3::from_array(camera);
                let distance = camera.distance(me + half).max(1.0);
                let needed = (2.0 * (gap * 0.5 / distance).atan().to_degrees() * 1.6 + 20.0).clamp(50.0, 100.0);
                shot.fov = shot.fov.max(needed);
            }
            _ => {}
        }
    }
}

/// BestCombat protagonist: every visible player's own fight heat (hits and
/// kills they're part of, plus standing close to someone), the hottest one
/// followed, holding each choice >= 3 s and switching only on a clear lead.
fn best_combat_track(path: &DirectorPath, a: i32, b: i32, kill_air: &HashMap<i32, f32>) -> Vec<(i32, i32)> {
    const STEP: i32 = 250;
    let mut clients: Vec<i32> = path.players.iter().map(|(c, _)| *c).collect();
    for sample in path.window(a, b) { if !clients.contains(&sample.client) { clients.push(sample.client); } }
    clients.sort_unstable();
    clients.dedup();
    let mut track: Vec<(i32, i32)> = Vec::new();
    let mut current = path.sample(a).client;
    let mut hold_until = a;
    let mut t = a;
    while t <= b {
        let recorder = path.sample(t).client;
        let positions: Vec<(i32, Vec3)> = clients.iter().filter_map(|&c| player_at(path, c, t).map(|p| (c, p))).collect();
        let from = path.events.partition_point(|ev| ev.ms < t - 2400);
        let window = &path.events[from..path.events.partition_point(|ev| ev.ms <= t + 2400)];
        // Real fighting only (hits, clashes, pain, kills: no weapon fire or
        // throws), near the action the recording can show: score fades with
        // distance from the recorded player. Movement speed plays no part.
        let recorder_origin = path.sample(t).origin;
        let heat = |client: i32, origin: Vec3| -> f32 {
            let mut h = 0.0;
            for event in window.iter().filter(|ev| (ev.attacker == client || ev.victim == client)
                && !matches!(ev.kind, CombatKind::Shot | CombatKind::Throw)) {
                let dt = (event.ms - t) as f32 / 900.0;
                // Midair kills count far more, the higher the more.
                let weight = if event.kind == CombatKind::Kill {
                    event.kind.weight() + (kill_air.get(&event.ms).copied().unwrap_or(0.0) / 24.0).min(16.0)
                } else { event.kind.weight() };
                h += weight * 1.5 * (-dt * dt).exp();
            }
            if positions.iter().any(|&(other, p)| other != client && p.distance(origin) < 320.0) { h += 0.6; }
            h / (1.0 + origin.distance(recorder_origin) / 1500.0)
        };
        let scores: Vec<(i32, f32)> = positions.iter().map(|&(c, p)| (c, heat(c, p))).collect();
        let score_of = |c: i32| scores.iter().find(|s| s.0 == c).map(|s| s.1);
        let current_score = score_of(current);
        // A kill's heat must not attract us to its victim just before death.
        // A new protagonist must offer lead-in and remain visible; a one-frame
        // PVS reappearance cannot win the next shot.
        let best = scores.iter().copied().filter(|&(c, _)| c == current || (
            alive_at(path, c, t)
            && !path.events.iter().any(|ev| ev.victim == c && (
                ev.kind == CombatKind::Kill && ev.ms >= t - 3000 && ev.ms <= t + 2200
                || matches!(ev.kind, CombatKind::Hit | CombatKind::Pain | CombatKind::Force)
                    && ev.ms >= t - 900 && ev.ms <= t + 450))
            && [t - 500, t + 500, t + 1500].into_iter()
                .filter(|&ms| ms >= a && ms < b).all(|ms| player_at(path, c, ms).is_some())
        )).max_by(|x, y| x.1.total_cmp(&y.1).then_with(|| y.0.cmp(&x.0)));
        let next = match (current_score, best) {
            // Preserve the story across a brief missing snapshot/PVS sample.
            (None, _) if lingering_player(path, current, t).is_some() => None,
            // Lost sight of the protagonist: back to the recorder.
            (None, _) => Some(recorder),
            // Stay with a protagonist who is still fighting; switch only when
            // they've gone quiet and someone else is clearly in a fight.
            // (A big midair kill elsewhere may cut in sooner.) Never right
            // after the protagonist died: stay on them for 3 s.
            // Finish the story: not while the protagonist is closing in on
            // someone (a jump or run at an opponent that hasn't landed yet).
            (Some(cs), Some((c, bs))) if c != current && t >= hold_until && bs > 2.0
                && (cs < 1.0 && bs > cs * 2.5 || bs > cs * 4.0)
                && !closing_in(path, current, t)
                && !path.events.iter().any(|ev| (ev.attacker == current || ev.victim == current)
                    && (ev.kind == CombatKind::Kill && ev.ms >= t - 1500 && ev.ms <= t + 2200
                        || matches!(ev.kind, CombatKind::Hit | CombatKind::Pain | CombatKind::Force)
                            && ev.ms >= t - 900 && ev.ms <= t + 450))
                && !path.events.iter().any(|ev| ev.kind == CombatKind::Kill && ev.victim == current
                    && ev.ms <= t && ev.ms >= t - 3000) => Some(c),
            _ => None,
        };
        if let Some(c) = next.filter(|c| *c != current) {
            let at = t;
            track.push((at, c));
            current = c;
            hold_until = t + 4000;
        }
        t += STEP;
    }
    track
}

const MOD_SABER: i32 = 3;

/// How much a kill wants the saber cam: 0 for non-saber kills, light for
/// saber kills, strong for pokes. A poke is the killer whipping the view back
/// and forth just before the hit (only measurable when the recorder killed).
fn saber_cam_weight(path: &DirectorPath, kill: &CombatEvent) -> f32 {
    if kill.means != MOD_SABER || kill.attacker < 0 { return 0.0; }
    if kill.attacker != path.sample(kill.ms).client { return 1.5; }
    let window = path.window(kill.ms - 700, kill.ms + 100);
    let (mut turned, mut reversals, mut last_sign) = (0.0f32, 0, 0.0f32);
    for pair in window.windows(2) {
        if pair[1].cut { continue; }
        let delta = wrap(pair[1].yaw - pair[0].yaw);
        turned += delta.abs();
        if delta.abs() > 2.0 {
            let sign = delta.signum();
            if last_sign != 0.0 && sign != last_sign { reversals += 1; }
            last_sign = sign;
        }
    }
    let seconds = window.last().zip(window.first()).map_or(0.8, |(l, f)| ((l.ms - f.ms) as f32 / 1000.0).max(0.1));
    let poke = turned / seconds > 350.0 && reversals >= 2;
    if poke { 6.0 } else { 1.5 }
}

/// A saber throw by `client` within 1.5 s before `ms` (still in flight).
fn throw_before(path: &DirectorPath, client: i32, ms: i32) -> bool {
    client >= 0 && path.events.iter().any(|ev| ev.kind == CombatKind::Throw && ev.attacker == client
        && ev.ms <= ms && ev.ms >= ms - 1500)
}
/// A saber throw by `client` starting within [s - 0.3 s, e].
fn throw_in(path: &DirectorPath, client: i32, s: i32, e: i32) -> bool {
    client >= 0 && path.events.iter().any(|ev| ev.kind == CombatKind::Throw && ev.attacker == client
        && ev.ms >= s - 300 && ev.ms <= e)
}

impl AutoDirector {
    /// Where a planned shot's camera is and what it looks at, at `ms`
    /// (native coordinates). Rig shots use the subject's facing: the
    /// recorder's view yaw, else toward `foe`, else its movement.
    fn estimate_view(&self, path: &DirectorPath, shot: &StudioDirectorShot, ms: i32, u: f32, follow: i32, foe: Option<i32>)
        -> Option<(Vec3, Vec3)> {
        use super::render_studio_director::stage_anchor_height as height;
        use super::render_studio_timeline::StudioFollowPerspective as Fp;
        let native = |p: [f32; 3]| Vec3::from_array(super::scene::jka_position(p));
        let e = if shot.ease { u * u * (3.0 - 2.0 * u) } else { u };
        let rig = shot.rig;
        let aim_client = if rig.aim_client >= 0 { rig.aim_client } else { follow };
        let aim = || player_at(path, aim_client, ms).map(|p| p + Vec3::Z * height(rig.aim_anchor) + native(rig.aim_offset));
        if let Some((position, look)) = shot.path_at(u) { return Some((native(position), native(look))); }
        if let Some(p0) = shot.position_override.map(native) {
            let p2 = shot.position_end.map(native).unwrap_or(p0);
            let camera = match shot.position_mid.map(native) {
                Some(m) => p0 * (1.0 - e) * (1.0 - e) + m * 2.0 * (1.0 - e) * e + p2 * e * e,
                None => p0.lerp(p2, e),
            };
            let look = match (shot.focus_target.map(native), shot.focus_end.map(native)) {
                (Some(a), Some(b)) => a.lerp(b, e),
                (Some(a), None) => a,
                _ => aim()?,
            };
            return Some((camera, look));
        }
        let origin = player_at(path, follow, ms)?;
        let own = path.sample(ms);
        let facing = if follow == own.client || follow < 0 { own.yaw } else {
            match foe.and_then(|f| player_at(path, f, ms)) {
                Some(other) if (other - origin).truncate().length() > 16.0 => (other.y - origin.y).atan2(other.x - origin.x).to_degrees(),
                _ => {
                    let before = player_at(path, follow, ms - 200).unwrap_or(origin);
                    let moved = origin - before;
                    if moved.truncate().length() > 8.0 { moved.y.atan2(moved.x).to_degrees() } else { own.yaw }
                }
            }
        };
        let anchor = origin + Vec3::Z * height(rig.position_anchor) + native(rig.position_offset);
        let aim_point = if rig.aim_enabled { aim() } else { None };
        let range = shot.range_at(e);
        let yaw_offset = rig.yaw_offset + shot.sweep * e;
        let right_of = |yaw: f32| Vec3::new(yaw.to_radians().sin(), -yaw.to_radians().cos(), 0.0);
        Some(match shot.preset.perspective() {
            Fp::FirstPerson => (anchor, aim_point.unwrap_or(anchor + forward(facing + yaw_offset, rig.pitch_offset) * 100.0)),
            Fp::ThirdPerson => {
                let yaw = facing + yaw_offset;
                let camera = anchor - forward(yaw, rig.pitch_offset) * range + Vec3::Z * rig.vertical_offset
                    + right_of(yaw) * rig.horizontal_offset;
                (camera, aim_point.unwrap_or(anchor + forward(yaw, 0.0) * 96.0))
            }
            Fp::Orbit => {
                let yaw = if rig.orbit_world_angles { yaw_offset } else { facing + yaw_offset };
                let mut camera = anchor - forward(yaw, rig.pitch_offset) * range + Vec3::Z * rig.vertical_offset
                    + right_of(yaw) * rig.horizontal_offset;
                if let (true, Some(aim)) = (rig.chase_aim, aim_point) {
                    let away = (anchor - aim).normalize_or_zero();
                    if away != Vec3::ZERO { camera = anchor + away * range + Vec3::Z * rig.vertical_offset; }
                }
                (camera, aim_point.unwrap_or(anchor))
            }
        })
    }

    /// Does this shot frame the action? Who must be on screen depends on the
    /// moment: the subject (when the camera follows them), the opponent in a
    /// fight, the victim in a kill cam (the attacker for victim views). Also
    /// rejects shots looking into a wall right behind what they frame.
    fn frames_ok(&mut self, path: &DirectorPath, shot: &StudioDirectorShot, beat: Beat, s: i32, e: i32, intent: &ShotIntent) -> bool {
        use super::render_studio_timeline::StudioFollowPerspective as Fp;
        if shot.preset == P::RecordedPov { return true; }
        let planned_path = !shot.path_keys.is_empty();
        let world_shot = shot.position_override.is_some() || planned_path;
        let first_person = !world_shot && shot.preset.perspective() == Fp::FirstPerson;
        let follow = if shot.client >= 0 { shot.client } else { intent.subject };
        let foe = opponent(path, s, e, follow);
        let mut targets = Vec::new();
        if !first_person && (!world_shot || planned_path) { targets.push(follow); }
        match beat {
            Beat::Approach | Beat::Exchange => targets.extend(foe),
            Beat::Kill | Beat::Aftermath if !intent.sequence() => if let Some(kill) = intent.primary() {
                let who = if matches!(shot.preset, P::VictimPov | P::VictimBlade) { kill.attacker } else { kill.victim };
                if who >= 0 { targets.push(who); }
            },
            _ => {}
        }
        if world_shot && targets.is_empty() { targets.push(follow); }
        targets.sort_unstable();
        targets.dedup();
        let n = ((e - s) / 50 + 2).clamp(7, 4096);
        let (mut seen, mut known) = (vec![0; targets.len()], vec![0; targets.len()]);
        let (mut walls, mut samples) = (0, 0);
        for ms in shot_sample_times(s, e, n, &intent.kills) {
            let u = (ms - s) as f32 / (e - s).max(1) as f32;
            let Some((camera, look)) = self.estimate_view(path, shot, ms, u, follow, foe) else { return false };
            if !self.recorded_visible(path, ms, camera) { return false; }
            // Validate the interpolated path, not just its control points.
            if world_shot && self.world.is_some() && !self.free(camera) { return false; }
            let view = (look - camera).normalize_or_zero();
            if view == Vec3::ZERO { return false; }
            samples += 1;
            let eased = if shot.ease { u * u * (3.0 - 2.0 * u) } else { u };
            let mut people_on_screen = 0;
            for (i, &who) in targets.iter().enumerate() {
                // A missing actor may briefly leave the frame while the
                // baked camera holds; impact checks below still require data.
                let Some(p) = player_at(path, who, ms) else { continue };
                known[i] += 1;
                let body = who == follow && !first_person && !matches!(shot.preset, P::CloseUp | P::SaberCam | P::VictimBlade);
                let heights: &[f32] = if body { &[-24.0, 30.0, 64.0] } else { &[30.0] };
                let inside = heights.iter().all(|&height| frame_point(camera, look, shot.fov_at(eased), p + Vec3::Z * height)
                    .is_some_and(FramePoint::inside));
                if inside && self.recorded_visible(path, ms, p + Vec3::Z * 30.0)
                    && (self.world.is_none() || self.clear(camera, p + Vec3::Z * 30.0)) { seen[i] += 1; people_on_screen += 1; }
            }
            // A good average cannot hide an empty ending or a wall occluding
            // everyone for part of the shot. Test every pose, including the tail.
            if !first_person && people_on_screen == 0 { return false; }
            // Every impact is mandatory, including simultaneous kills. A
            // good average must never conceal a missed final blow.
            for kill in intent.kills.iter().filter(|kill| kill.ms == ms) {
                for who in [kill.attacker, kill.victim] {
                    if who < 0 || (first_person && who == follow) { continue; }
                    let Some(p) = player_at(path, who, ms) else { return false };
                    let point = p + Vec3::Z * 30.0;
                    if !frame_point(camera, look, shot.fov_at(eased), point)
                        .is_some_and(|p| p.inside() && p.height >= 0.04)
                        || !self.recorded_visible(path, ms, point)
                        || (self.world.is_some() && !self.clear(camera, point)) { return false; }
                }
            }
            if !first_person && self.world.is_some() && self.reach(look, look + view * 96.0) < 0.5 { walls += 1; }
        }
        let framed = (0..targets.len()).all(|i| if targets[i] == follow { known[i] > 0 && seen[i] * 10 >= known[i] * 9 }
            else { known[i] == 0 || seen[i] * 10 >= known[i] * 7 });
        samples > 0 && framed && walls * 2 <= samples
    }

    /// Glide: a world camera on a curve beside the action, keeping to the
    /// fight's side of the line, its view drifting from where the action is at
    /// the start of the shot to where it is at the end.
    #[allow(clippy::too_many_arguments)]
    fn place_glide(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, subject: i32,
        side: f32, rng: &mut Rng) -> bool {
        if self.world.is_none() { return false; }
        let foe = opponent(path, s, e, subject);
        let center = |ms: i32| -> Option<Vec3> {
            let me = player_at(path, subject, ms)? + Vec3::Z * 36.0;
            Some(match foe.and_then(|f| player_at(path, f, ms)) {
                Some(other) if other.distance(me) < 1500.0 => (me + other + Vec3::Z * 36.0) * 0.5,
                _ => me,
            })
        };
        let (Some(c0), Some(c1)) = (center(s), center(e)) else { return false };
        let mid = (s + e) / 2;
        let line = foe.and_then(|f| Some(player_at(path, f, mid)? - player_at(path, subject, mid)?))
            .map(flat).filter(|d| *d != Vec3::ZERO)
            .or_else(|| Some(flat(c1 - c0)).filter(|d| *d != Vec3::ZERO)).unwrap_or(Vec3::X);
        let left = Vec3::new(-line.y, line.x, 0.0) * side;
        for _ in 0..12 {
            let distance = rng.range(260.0, 380.0);
            let p0 = c0 + left * distance - line * rng.range(0.0, 120.0) + Vec3::Z * rng.range(80.0, 150.0);
            let swing = glam::Quat::from_rotation_z((side * rng.range(25.0, 45.0)).to_radians());
            let p2 = c1 + (swing * left) * distance + Vec3::Z * rng.range(60.0, 140.0);
            let p1 = (p0 + p2) * 0.5 + left * rng.range(40.0, 100.0) + Vec3::Z * rng.range(10.0, 40.0);
            if !(self.free(p0) && self.free(p1) && self.free(p2)) { continue; }
            if !(self.clear(p0, p1) && self.clear(p1, p2)) { continue; }
            if !(self.clear(p0, c0) && self.clear(p1, (c0 + c1) * 0.5) && self.clear(p2, c1)) { continue; }
            shot.position_override = Some(render(p0));
            shot.position_mid = Some(render(p1));
            shot.position_end = Some(render(p2));
            shot.focus_target = Some(render(c0));
            shot.focus_end = Some(render(c1));
            shot.rig.aim_enabled = false;
            shot.rig.autofocus = true;
            shot.rig.range = p0.distance(c0);
            shot.lens.focus_distance = p0.distance(c0).clamp(1.0, 65536.0);
            shot.fov = rng.range(62.0, 72.0);
            return true;
        }
        false
    }
}

impl AutoDirector {
    /// Follow cam: desired camera every 0.2 s behind the player relative to
    /// the line toward the action (opponent, else ahead), with the planned
    /// lateral / height offsets; pulled in along the boom where a wall is in
    /// the way; then low-pass filtered in time (`damp` seconds) so the camera
    /// lags and drifts instead of being bolted on. The look point is damped
    /// at half that. Params: [range, height, lateral, damp] (also stored on
    /// the rig so a merge can re-plan with the same feel).
    #[allow(clippy::too_many_arguments)]
    fn plan_follow(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, subject: i32,
        foe: Option<i32>, params: [f32; 4], kills: &[CombatEvent]) -> bool {
        use super::render_studio_director::StudioPathKey;
        let [range, height, lateral, damp] = params;
        const STEP: i32 = 50;
        let span = (e - s).max(1) as f32;
        let mut keys = Vec::new();
        let (mut camera, mut look): (Option<Vec3>, Option<Vec3>) = (None, None);
        let mut last_origin: Option<Vec3> = None;
        let (mut blocked, mut total) = (0, 0);
        let mut t = s;
        loop {
            let ms = t.min(e - 1);
            if player_at(path, subject, ms).is_none() && lingering_player(path, subject, ms).is_some() {
                if let (Some(c), Some(l)) = (camera, look) {
                    if !self.recorded_visible(path, ms, c) { return false; }
                    keys.push(StudioPathKey { t: ((ms - s) as f32 / span).clamp(0.0, 1.0), position: render(c), look: render(l) });
                    if ms >= e - 1 { break; }
                    t += STEP;
                    continue;
                }
            }
            if let Some(origin) = player_at(path, subject, ms).or_else(|| lingering_player(path, subject, ms)) {
                // A respawn / teleport inside the shot: no planned path across the map.
                if last_origin.is_some_and(|o| o.distance(origin) > 400.0) { return false; }
                last_origin = Some(origin);
                let eye = origin + Vec3::Z * 40.0;
                let own = path.sample(ms);
                let facing = if subject == own.client { own.yaw } else {
                    let before = player_at(path, subject, ms - 200).unwrap_or(origin);
                    let moved = origin - before;
                    if moved.truncate().length() > 8.0 { moved.y.atan2(moved.x).to_degrees() } else { own.yaw }
                };
                // Anticipate the next impact in a chain rather than continuing
                // to stare at the first victim after they have fallen.
                let next_foe = kills.iter().find(|kill| kill.ms >= ms - 150)
                    .or_else(|| kills.last()).and_then(|kill| {
                        if kill.attacker == subject { Some(kill.victim) }
                        else if kill.victim == subject { Some(kill.attacker) } else { None }
                    }).filter(|&client| client >= 0).or(foe);
                let target = next_foe.and_then(|f| player_at(path, f, ms).or_else(|| lingering_player(path, f, ms)))
                    .filter(|o| o.distance(origin) < 1400.0)
                    .map(|o| eye.lerp(o + Vec3::Z * 36.0, 0.45))
                    .unwrap_or_else(|| eye + forward(facing, 0.0) * 90.0);
                let back = Some(flat(target - eye)).filter(|d| *d != Vec3::ZERO).unwrap_or_else(|| flat(forward(facing, 0.0)));
                let right = Vec3::new(back.y, -back.x, 0.0);
                // This is an offline plan: feed the upcoming motion into the
                // positional damping instead of lagging a fast/jumping fighter.
                let ahead_ms = (ms + (damp * 700.0) as i32).min(e - 1);
                let lead = player_at(path, subject, ahead_ms).filter(|p| p.distance(origin) < 300.0)
                    .map_or(Vec3::ZERO, |p| p - origin);
                let mut desired = eye + lead - back * range + Vec3::Z * height + right * lateral;
                total += 1;
                if self.world.is_some() {
                    let open = self.reach(eye, desired);
                    if open < 0.6 { blocked += 1; }
                    desired = eye.lerp(desired, (open - 0.05).max(0.0));
                }
                let dt = STEP as f32 / 1000.0;
                let follow = 1.0 - (-dt / damp.max(0.05)).exp();
                let mut c = camera.map_or(desired, |c| c.lerp(desired, follow));
                if self.world.is_some() {
                    let reach = self.reach(eye, c);
                    if reach < 1.0 { c = eye.lerp(c, (reach - 0.03).max(0.0)); }
                    // Damping around a corner must not cut through its wall.
                    if let Some(previous) = camera {
                        if !self.clear(previous, c) { return false; }
                    }
                }
                if !self.recorded_visible(path, ms, c) { return false; }
                // Keep present actors in frame. A second low-pass followed by
                // an angular dead zone delayed aim even after camera catch-up.
                let l = target;
                camera = Some(c);
                look = Some(l);
                keys.push(StudioPathKey { t: ((ms - s) as f32 / span).clamp(0.0, 1.0), position: render(c), look: render(l) });
            }
            if ms >= e - 1 { break; }
            t += STEP;
        }
        if keys.len() < 2 || keys.len() > 4096 || blocked * 10 > total * 3 { return false; }
        // Spline ends exactly at 0 and 1.
        keys[0].t = 0.0;
        if let Some(last) = keys.last_mut() { last.t = 1.0; }
        shot.path_keys = keys;
        shot.position_override = None;
        shot.rig.range = range;
        shot.rig.vertical_offset = height;
        shot.rig.horizontal_offset = lateral;
        shot.rig.blend_seconds = damp;
        shot.rig.aim_enabled = false;
        shot.rig.autofocus = true;
        let recorder = path.sample((s + e) / 2).client;
        shot.client = if !self.combat && subject == recorder { -1 } else { subject };
        shot.lens.focus_distance = range.clamp(1.0, 65536.0);
        true
    }
}

impl AutoDirector {
    /// Conservative outside coverage when an authored angle fails. Every
    /// sampled pose must show the actual subject through clear map geometry.
    fn plan_coverage(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32, subject: i32) -> bool {
        use super::render_studio_director::StudioPathKey;
        let mut keys = Vec::new();
        let mut previous: Option<Vec3> = None;
        let mut angle = 0.0;
        for ms in shot_sample_times(s, e, ((e - s) / 50 + 2).clamp(2, 4096), &[]) {
            let Some(origin) = player_at(path, subject, ms) else { return false };
            let eye = origin + Vec3::Z * 36.0;
            let look = origin + Vec3::Z * 24.0;
            let yaw = view_at(path, subject, ms, None)[1];
            let mut selected = None;
            for offset in [angle, 0.0, -35.0, 35.0, -70.0, 70.0, 180.0] {
                for range in [220.0, 160.0, 110.0] {
                    let desired = eye - forward(yaw + offset, 12.0) * range + Vec3::Z * 24.0;
                    let camera = if self.world.is_some() { eye.lerp(desired, (self.reach(eye, desired) - 0.025).clamp(0.0, 1.0)) } else { desired };
                    if camera.distance(look) < 55.0 || !self.recorded_visible(path, ms, camera) { continue; }
                    if self.world.is_some() && (!self.free(camera) || !self.clear(camera, look)
                        || !self.clear(camera, origin + Vec3::Z * 54.0)) { continue; }
                    if ![-12.0, 30.0, 60.0].into_iter().all(|height|
                        frame_point(camera, look, shot.fov, origin + Vec3::Z * height).is_some_and(FramePoint::inside)) { continue; }
                    // Preserve an angle until geometry demands another one.
                    // Runtime validation also checks spline poses between keys.
                    let score = previous.map_or(0.0, |p| p.distance(camera));
                    if selected.as_ref().is_none_or(|&(_, _, old)| score < old) { selected = Some((camera, offset, score)); }
                }
            }
            let Some((camera, offset, _)) = selected else { return false };
            previous = Some(camera); angle = offset;
            keys.push(StudioPathKey { t: (ms - s) as f32 / (e - s).max(1) as f32, position: render(camera), look: render(look) });
        }
        if keys.len() < 2 { return false; }
        keys.last_mut().unwrap().t = 1.0;
        shot.path_keys = keys;
        shot.client = subject;
        true
    }

    /// A slow-motion event belongs to the final camera, not merely to an
    /// engagement that was hot while the abstract protagonist track was built.
    fn covered_kill(&mut self, path: &DirectorPath, clip: &StudioDemoClip, shots: &[StudioDirectorShot], kill: &CombatEvent) -> bool {
        let time = clip.timeline_at_source_ms(kill.ms);
        let Some(shot) = shots.iter().find(|shot| shot.contains(time)) else { return false };
        let subject = if shot.client >= 0 { shot.client } else { path.sample(kill.ms).client };
        if subject != kill.attacker && subject != kill.victim { return false; }
        // A live fallback can change at playback; do not slow it speculatively.
        if shot.path_keys.is_empty() && shot.position_override.is_none() { return false; }
        for ms in [kill.ms - 250, kill.ms, kill.ms + 250, kill.ms + 650] {
            if ms < clip.in_ms || ms >= clip.out_ms { continue; }
            let at = clip.timeline_at_source_ms(ms);
            if !shot.contains(at) { return false; }
            let u = ((at - shot.start) / shot.duration) as f32;
            let Some((camera, look)) = self.estimate_view(path, shot, ms, u, subject, None) else { return false };
            for who in [kill.attacker, kill.victim] {
                let Some(origin) = player_at(path, who, ms) else { return false };
                let point = origin + Vec3::Z * 30.0;
                if !frame_point(camera, look, shot.fov_at(shot.progress(at)), point)
                    .is_some_and(|p| p.inside() && p.height >= 0.04)
                    || (self.world.is_some() && !self.clear(camera, point)) { return false; }
            }
        }
        true
    }

    fn telephoto(&mut self, job: &ClipJob, s: i32, e: i32, intent: &ShotIntent, rng: &mut Rng) -> Option<StudioDirectorShot> {
        let subject = intent.subject;
        let foe = opponent(&job.path, s, e, subject);
        let mut points = Vec::new();
        for ms in shot_sample_times(s, e, 7, &intent.kills) {
            points.extend([Some(subject), foe].into_iter().flatten()
                .filter_map(|who| player_at(&job.path, who, ms)).map(|p| p + Vec3::Z * 30.0));
        }
        if points.is_empty() { return None; }
        let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
        let yaw = view_at(&job.path, subject, (s + e) / 2, foe)[1];
        let fov = rng.range(30.0, 42.0);
        for distance in [550.0, 700.0, 950.0, 1250.0] {
            for offset in [0.0, -30.0, 30.0, -150.0, 150.0] {
                let camera = center - forward(yaw + offset, 0.0) * distance + Vec3::Z * 65.0;
                if !self.free(camera) || points.iter().any(|&p| !frame_point(camera, center, fov, p).is_some_and(FramePoint::inside)) { continue; }
                if points.iter().any(|&p| !self.clear(camera, p)) { continue; }
                let mut shot = StudioDirectorShot::new(0, P::Static, subject,
                    job.clip.timeline_at_source_ms(s), job.clip.timeline_at_source_ms(e) - job.clip.timeline_at_source_ms(s));
                shot.auto = true;
                shot.fov = fov;
                shot.position_override = Some(render(camera));
                shot.focus_target = Some(render(center));
                shot.rig.aim_enabled = false;
                shot.rig.range = distance;
                shot.lens.focus_distance = camera.distance(center);
                shot.lens.aperture = Some(2.8);
                return Some(shot);
            }
        }
        None
    }

    /// Preserve recorded yaw and the player's pitch intention, with a damped
    /// vertical correction when looking at the floor hides the nearby fight.
    fn plan_chase(&mut self, shot: &mut StudioDirectorShot, path: &DirectorPath, s: i32, e: i32,
        subject: i32, foe: Option<i32>) -> bool {
        use super::render_studio_director::StudioPathKey;
        let mut keys = Vec::new();
        let mut offset = None;
        let mut previous = None;
        for ms in shot_sample_times(s, e, ((e - s) / 50 + 2).clamp(2, 4096), &[]) {
            let Some(origin) = player_at(path, subject, ms).or_else(|| lingering_player(path, subject, ms)) else { return false };
            let eye = origin + Vec3::Z * 36.0;
            let [pitch, yaw] = view_at(path, subject, ms, foe);
            let desired = combat_pitch(path, subject, foe, ms, pitch, yaw);
            let correction = offset.map_or(desired - pitch, |last: f32| last + (desired - pitch - last) * (1.0 - (-0.05f32 / 0.35).exp()));
            offset = Some(correction);
            let pitch = pitch + correction;
            let mut camera = eye - forward(yaw + shot.rig.yaw_offset, pitch + shot.rig.pitch_offset) * shot.rig.range
                + Vec3::Z * shot.rig.vertical_offset;
            if self.world.is_some() {
                let reach = self.reach(eye, camera);
                camera = eye.lerp(camera, (reach - 0.025).clamp(0.0, 1.0));
                if !self.free(camera) { return false; }
                if previous.is_some_and(|p: Vec3| p.distance(origin) > 400.0) { return false; }
            }
            if !self.recorded_visible(path, ms, camera) { return false; }
            previous = Some(origin);
            let look = eye + forward(yaw, pitch) * 256.0;
            keys.push(StudioPathKey { t: (ms - s) as f32 / (e - s).max(1) as f32, position: render(camera), look: render(look) });
        }
        if keys.len() < 2 { return false; }
        keys.last_mut().unwrap().t = 1.0;
        shot.path_keys = keys;
        shot.client = if !self.combat && subject == path.sample((s + e) / 2).client { -1 } else { subject };
        shot.rig.aim_enabled = false;
        true
    }

    /// Lens relative to the player's own cg_fov (most play around 100): each
    /// type is a fraction of it. Lens-driven shots (close-up, push-in, dolly
    /// zoom), the fitted static frame and the standard chase keep theirs.
    fn player_lens(&self, shot: &mut StudioDirectorShot, beat: Beat, rng: &mut Rng) {
        if shot.position_override.is_some() && shot.rig.range >= 550.0 && shot.fov <= 45.0 { return; }
        let base = self.fov.clamp(70.0, 120.0);
        let (lo, hi) = match shot.preset {
            P::FollowCam => (0.85, 0.95),
            P::OverShoulder | P::Orbit | P::Overhead => (0.75, 0.85),
            P::TwoShot | P::Glide => (0.8, 0.92),
            P::LowAngle | P::Crane => (0.72, 0.82),
            P::Pan | P::Tracking => (0.65, 0.78),
            P::Establishing | P::FlyBy | P::SaberCam => (0.88, 0.98),
            P::ThrowCam => (0.82, 0.92),
            P::VictimPov | P::VictimBlade => (0.9, 1.0),
            _ => return,
        };
        shot.fov = (base * rng.range(lo, hi)).clamp(10.0, 140.0);
        // Frag-montage moments: now and then an ultra-wide lens on kills and
        // big air (115-135% of cg_fov, up to 135 degrees), sometimes punching
        // in to a normal lens across the shot.
        let montage = matches!(shot.preset, P::FollowCam | P::Orbit | P::LowAngle | P::FlyBy | P::SaberCam
            | P::ThrowCam | P::Glide | P::Pan | P::Crane);
        if montage && matches!(beat, Beat::Kill | Beat::BigAir) && rng.unit() < 0.35 {
            shot.fov = (base * rng.range(1.15, 1.35)).min(135.0);
            if shot.preset != P::FollowCam && rng.unit() < 0.35 {
                shot.fov_end = Some((base * rng.range(0.8, 0.9)).clamp(10.0, 140.0));
            }
        }
    }

    /// A busy scene gets a wider lens: other players within 700 units of the
    /// subject (most at the start, middle or end of the shot). Lens-driven
    /// shots (close-up, zooms, first person, saber/victim cams) keep theirs.
    fn crowd_fov(&self, path: &DirectorPath, subject: i32, s: i32, e: i32, shot: &mut StudioDirectorShot) {
        if shot.position_override.is_some() && shot.rig.range >= 550.0 && shot.fov <= 45.0 { return; }
        if matches!(shot.preset, P::CloseUp | P::PushIn | P::DollyZoom | P::SaberCam | P::VictimPov | P::VictimBlade
            | P::PlayerEyes | P::RecordedPov) || shot.zoom_lock || shot.fov_end.is_some() { return; }
        let crowd = [s, (s + e) / 2, e].into_iter().map(|ms| {
            let Some(me) = player_at(path, subject, ms) else { return 0 };
            let own = path.sample(ms);
            path.players.iter().map(|(c, _)| *c).chain(std::iter::once(own.client))
                .filter(|&c| c != subject).filter_map(|c| player_at(path, c, ms))
                .filter(|p| p.distance(me) < 700.0).count()
        }).max().unwrap_or(0);
        let base = self.fov.clamp(70.0, 120.0);
        let wide = match crowd { 0 | 1 => return, 2 => base * 0.9, 3 => base, _ => base * 1.08 };
        shot.fov = shot.fov.max(wide).min(130.0);
    }
}

/// Players in the action around [s, e]: anyone with a combat event within
/// 2.5 s, visible, within 3000 units of `subject`. (client, weight).
fn active_players(path: &DirectorPath, s: i32, e: i32, subject: i32) -> Vec<(i32, f32)> {
    let Some(me) = player_at(path, subject, (s + e) / 2) else { return Vec::new() };
    let from = path.events.partition_point(|ev| ev.ms < s - 2500);
    let mut out: Vec<(i32, f32)> = Vec::new();
    for event in path.events[from..].iter().take_while(|ev| ev.ms <= e + 2500) {
        if event.attacker != subject && event.victim != subject { continue; }
        for who in [event.attacker, event.victim] {
            if who < 0 || who == subject || out.iter().any(|o| o.0 == who) { continue; }
            if player_at(path, who, (s + e) / 2).is_some_and(|p| p.distance(me) < 3000.0) {
                out.push((who, if event.kind == CombatKind::Shot { 1.0 } else { 2.0 }));
            }
        }
    }
    out
}

/// Eye points of the active players across [s, e] (for fitting a frame).
fn action_points(path: &DirectorPath, s: i32, e: i32, subject: i32) -> Vec<Vec3> {
    let active = active_players(path, s, e, subject);
    let mut points = Vec::new();
    for k in 0..3 {
        let ms = s + (e - s) * k / 2;
        points.extend(active.iter().filter_map(|(c, _)| player_at(path, *c, ms)).map(|p| p + Vec3::Z * 36.0));
    }
    points
}

impl AutoDirector {
    /// Score readable composition through the same rectangular projection as
    /// validation. Important impacts count more than incidental spectators.
    fn coverage(&mut self, path: &DirectorPath, shot: &StudioDirectorShot, s: i32, e: i32, intent: &ShotIntent) -> f32 {
        let follow = if shot.client >= 0 { shot.client } else { intent.subject };
        let mut who = active_players(path, s, e, follow);
        who.push((follow, 3.0));
        for kill in &intent.kills {
            for client in [kill.attacker, kill.victim] {
                if client >= 0 && !who.iter().any(|p| p.0 == client) { who.push((client, 3.0)); }
            }
        }
        let foe = opponent(path, s, e, follow);
        let mut total = 0.0;
        let times = shot_sample_times(s, e, 7, &intent.kills);
        let mut denominator = 0.0;
        for ms in times {
            let u = (ms - s) as f32 / (e - s).max(1) as f32;
            let impact = intent.kills.iter().any(|kill| kill.ms == ms);
            let importance = if impact { 4.0 } else { 1.0 };
            denominator += importance;
            let Some((camera, look)) = self.estimate_view(path, shot, ms, u, follow, foe) else { continue };
            if !self.recorded_visible(path, ms, camera) { continue; }
            let eased = if shot.ease { u * u * (3.0 - 2.0 * u) } else { u };
            let mut projected = Vec::new();
            let mut score = 0.0;
            for &(client, weight) in &who {
                let Some(p) = player_at(path, client, ms) else { continue };
                let point = p + Vec3::Z * 30.0;
                let Some(frame) = frame_point(camera, look, shot.fov_at(eased), point).filter(|p| p.inside()) else { continue };
                if !self.recorded_visible(path, ms, point) || (self.world.is_some() && !self.clear(camera, point)) { continue; }
                let readable = (frame.height / 0.18).clamp(0.0, 1.0);
                let edge = 1.0 - 0.25 * frame.x.abs().max(frame.y.abs());
                score += weight * readable * edge;
                projected.push((client, frame));
            }
            for kill in intent.kills.iter().filter(|kill| kill.ms == ms) {
                if let (Some((_, a)), Some((_, b))) = (projected.iter().find(|p| p.0 == kill.attacker), projected.iter().find(|p| p.0 == kill.victim)) {
                    let separation = (a.x - b.x).hypot(a.y - b.y);
                    score += 2.0 * (separation / 0.2).clamp(0.0, 1.0);
                }
            }
            total += score * importance;
        }
        total / denominator.max(1.0)
    }
}

/// Respawns: a client's death (kill event as victim) followed within 30 s
/// by a jump of over 400 units in under 0.4 s (recorder: the path's cut flag).
/// Teleporters without a death before them are not respawns.
fn find_respawns(path: &DirectorPath) -> HashMap<i32, Vec<i32>> {
    let died = |client: i32, ms: i32| path.events.iter().any(|ev| ev.kind == CombatKind::Kill && ev.victim == client
        && ev.ms <= ms && ev.ms >= ms - 30000);
    let mut map: HashMap<i32, Vec<i32>> = HashMap::new();
    for (&client, states) in &path.alive {
        for pair in states.windows(2) {
            if !pair[0].1 && pair[1].1 { map.entry(client).or_default().push(pair[1].0); }
        }
    }
    for pair in path.samples.windows(2) {
        if pair[1].cut && pair[1].client == pair[0].client && died(pair[1].client, pair[1].ms) {
            map.entry(pair[1].client).or_default().push(pair[1].ms);
        }
    }
    for (client, list) in &path.players {
        for pair in list.windows(2) {
            if pair[1].0 - pair[0].0 <= 400 && pair[1].1.distance(pair[0].1) > 400.0 && died(*client, pair[1].0) {
                map.entry(*client).or_default().push(pair[1].0);
            }
        }
    }
    for times in map.values_mut() { times.sort_unstable(); times.dedup(); }
    map
}

/// Whoever last killed `client` (within 30 s before `ms`).
fn killer_of(path: &DirectorPath, client: i32, ms: i32) -> Option<i32> {
    path.events.iter().rev().find(|ev| ev.kind == CombatKind::Kill && ev.victim == client && ev.ms <= ms && ev.ms >= ms - 30000)
        .map(|ev| ev.attacker).filter(|&a| a >= 0 && a != client)
}

fn view_at(path: &DirectorPath, client: i32, ms: i32, foe: Option<i32>) -> [f32; 2] {
    let own = path.sample(ms);
    if client == own.client { return [own.pitch, own.yaw]; }
    if let Some(views) = path.views.get(&client) {
        let i = views.partition_point(|(t, _)| *t <= ms);
        if let Some((time, a)) = i.checked_sub(1).map(|i| views[i]) {
            if ms - time <= 300 {
                if let Some(&(end, b)) = views.get(i).filter(|p| p.0 - time <= 400) {
                    let u = (ms - time) as f32 / (end - time).max(1) as f32;
                    return [a[0] + (b[0] - a[0]) * u, a[1] + wrap(b[1] - a[1]) * u];
                }
                return a;
            }
        }
    }
    let origin = player_at(path, client, ms).unwrap_or(own.origin);
    let direction = foe.and_then(|foe| player_at(path, foe, ms)).map(|p| p - origin)
        .unwrap_or_else(|| origin - player_at(path, client, ms - 200).unwrap_or(origin));
    [0.0, if direction.truncate().length() > 8.0 { heading_of(direction) } else { own.yaw }]
}

fn combat_pitch(path: &DirectorPath, subject: i32, foe: Option<i32>, ms: i32, pitch: f32, yaw: f32) -> f32 {
    let opponent_pitch = foe.and_then(|who| Some(player_at(path, who, ms)? - player_at(path, subject, ms)?))
        .filter(|delta| delta.length() < 1400.0 && flat(*delta).dot(flat(forward(yaw, 0.0))) > 0.15)
        .map(|delta| (-delta.z).atan2(delta.truncate().length()).to_degrees());
    let desired = opponent_pitch.map_or(pitch.clamp(-35.0, 28.0), |target| pitch + (target - pitch) * 0.7);
    pitch + (desired - pitch).clamp(-55.0, 20.0)
}

fn alive_at(path: &DirectorPath, client: i32, ms: i32) -> bool {
    path.alive.get(&client).and_then(|states| {
        states.partition_point(|(t, _)| *t <= ms).checked_sub(1).map(|i| states[i].1)
    }).unwrap_or(true)
}

/// Positive evidence of an empty area is required before speeding up. Saber
/// circling, approaches and defensive exchanges remain action without hit events.
fn combat_busy(path: &DirectorPath, client: i32, t: i32) -> bool {
    let Some(origin) = player_at(path, client, t) else { return true };
    if !alive_at(path, client, t) { return true; }
    let nearby: Vec<_> = path.players.iter().map(|(c, _)| *c)
        .chain(std::iter::once(path.sample(t).client)).filter(|&c| c != client && alive_at(path, c, t))
        .filter_map(|c| player_at(path, c, t).map(|p| (c, p))).collect();
    if nearby.iter().any(|(_, p)| p.distance(origin) < 1100.0) { return true; }
    if nearby.iter().enumerate().any(|(i, (_, a))| a.distance(origin) < 2000.0
        && nearby[i + 1..].iter().any(|(_, b)| b.distance(origin) < 2000.0 && a.distance(*b) < 450.0)) { return true; }
    let spawn = path.alive.get(&client).and_then(|states| states.windows(2)
        .rev().find(|pair| !pair[0].1 && pair[1].1 && pair[1].0 <= t).map(|pair| pair[1].0));
    let from = path.events.partition_point(|ev| ev.ms < t - 2000);
    path.events[from..].iter().take_while(|ev| ev.ms <= t + 2500)
        .filter(|ev| spawn.is_none_or(|spawn| ev.ms >= spawn))
        .filter(|ev| !matches!(ev.kind, CombatKind::Shot | CombatKind::Throw))
        .any(|ev| ev.attacker == client || ev.victim == client || ev.position.distance(origin) < 1100.0
            || [ev.attacker, ev.victim].iter().any(|&c| nearby.iter().any(|&(id, p)| id == c && p.distance(origin) < 1700.0)))
}

/// `client` is closing on the nearest other player (within 900 units and
/// at least 40 units nearer than half a second ago): a move in progress.
fn closing_in(path: &DirectorPath, client: i32, t: i32) -> bool {
    let (Some(now), Some(before)) = (player_at(path, client, t), player_at(path, client, t - 500)) else { return false };
    let nearest = |me: Vec3, ms: i32| path.players.iter().map(|(c, _)| *c).chain(std::iter::once(path.sample(ms).client))
        .filter(|&c| c != client).filter_map(|c| player_at(path, c, ms).map(|p| p.distance(me)))
        .fold(f32::INFINITY, f32::min);
    let (d_now, d_before) = (nearest(now, t), nearest(before, t - 500));
    d_now < 900.0 && d_now < d_before - 40.0
}

/// Fights in [a, b] (padded source ranges): a heat curve of combat events
/// near or involving the recorder, plus time spent close to another player.
fn engagements(path: &DirectorPath, a: i32, b: i32, who: &dyn Fn(i32) -> i32) -> Vec<(i32, i32)> {
    if path.events.is_empty() && path.players.is_empty() { return Vec::new(); }
    const STEP: i32 = 100;
    let mut hot: Vec<(i32, i32)> = Vec::new();
    let mut open: Option<i32> = None;
    let mut t = a;
    while t <= b {
        let client = who(t);
        let Some(origin) = player_at(path, client, t) else { t += STEP; continue };
        let own = PathSample { origin, client, ..path.sample(t) };
        let from = path.events.partition_point(|ev| ev.ms < t - 2100);
        let mut heat = 0.0f32;
        for event in path.events[from..].iter().take_while(|ev| ev.ms <= t + 2100) {
            let relevance = if event.attacker == own.client || event.victim == own.client { 1.5 } else {
                match event.position.distance(own.origin) { d if d < 600.0 => 0.5, d if d < 900.0 => 0.2, _ => 0.0 }
            };
            let dt = (event.ms - t) as f32 / 700.0;
            heat += event.kind.weight() * relevance * (-dt * dt).exp();
        }
        if alive_at(path, own.client, t) && path.players.iter().any(|(c, _)| *c != own.client && alive_at(path, *c, t)
            && player_at(path, *c, t).is_some_and(|p| p.distance(own.origin) < 650.0)) { heat += 1.5; }
        match (heat > 1.2, open) {
            (true, None) => open = Some(t),
            (false, Some(s)) => { hot.push((s, t)); open = None; }
            _ => {}
        }
        t += STEP;
    }
    if let Some(s) = open { hot.push((s, b)); }
    // Merge close stretches, drop blips, pad for lead-in / reaction.
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (s, e) in hot {
        match merged.last_mut() { Some(last) if s - last.1 < 1500 => last.1 = e, _ => merged.push((s, e)) }
    }
    let mut padded: Vec<(i32, i32)> = Vec::new();
    for (s, e) in merged.into_iter().filter(|(s, e)| e - s >= 800) {
        let (s, e) = ((s - 600).max(a), (e + 800).min(b));
        match padded.last_mut() { Some(last) if s <= last.1 => last.1 = last.1.max(e), _ => padded.push((s, e)) }
    }
    padded
}

impl AutoDirector {
    /// Kill cam: the victim (or the killer for the hero angle) around the kill.
    fn build_kill(&mut self, preset: P, s: i32, e: i32, job: &ClipJob, rng: &mut Rng, intent: &ShotIntent) -> Option<StudioDirectorShot> {
        let path = job.path.clone();
        let kill = intent.primary()?;
        let clip = &job.clip;
        let start = clip.timeline_at_source_ms(s).max(clip.start);
        let end = clip.timeline_at_source_ms(e).min(clip.end());
        if end - start < 0.4 { return None; }
        let recorder = path.sample(kill.ms).client;
        let as_subject = |c: i32| if c < 0 || (!self.combat && c == recorder) { -1 } else { c };
        let victim = as_subject(kill.victim);
        let killer = as_subject(kill.attacker);
        let mut shot = StudioDirectorShot::new(0, preset, -1, start, end - start);
        preset.seed_shot(&mut shot, self.fov);
        shot.auto = true;
        let center = kill.position + Vec3::Z * 36.0;
        match preset {
            P::Orbit => {
                shot.client = victim;
                shot.rig.orbit_world_angles = true;
                shot.rig.yaw_offset = wrap(rng.range(-180.0, 180.0));
                shot.rig.pitch_offset = rng.range(14.0, 24.0);
                shot.rig.range = rng.range(170.0, 230.0);
                shot.sweep = rng.sign() * rng.range(50.0, 90.0);
            }
            P::LowAngle => {
                // Killer and victim together: frame their midpoint from a
                // gently low angle, far enough back to hold both.
                if kill.attacker < 0 || kill.attacker == kill.victim { return None; }
                let killer_at = player_at(&path, kill.attacker, kill.ms)?;
                let gap = kill.position - killer_at;
                shot.client = killer;
                shot.rig.aim_enabled = false;
                shot.rig.orbit_world_angles = true;
                let side = match self.side_at(kill.ms) { s if s != 0.0 => s, _ => rng.sign() };
                shot.rig.yaw_offset = wrap(gap.y.atan2(gap.x).to_degrees() - side * rng.range(60.0, 100.0));
                shot.rig.position_offset = render(gap * 0.5);
                shot.rig.pitch_offset = rng.range(-4.0, 2.0);
                shot.rig.range = (gap.truncate().length() * 1.1 + 170.0).clamp(180.0, 900.0);
                shot.fov = shot.fov.max(66.0);
            }
            P::DollyZoom => {
                shot.client = victim;
                shot.rig.range = rng.range(320.0, 360.0);
                shot.range_end = Some(rng.range(140.0, 170.0));
            }
            P::SaberCam => {
                if kill.means != MOD_SABER || kill.attacker < 0 || kill.victim < 0 { return None; }
                shot.client = killer;
                shot.rig.aim_client = kill.victim;
                shot.rig.range = rng.range(14.0, 24.0);
                shot.rig.pitch_offset = rng.range(3.0, 10.0);
                shot.rig.horizontal_offset = rng.sign() * rng.range(0.0, 8.0);
                shot.fov = rng.range(80.0, 90.0);
            }
            P::ThrowCam => {
                if kill.attacker < 0 || kill.victim < 0 || !throw_before(&path, kill.attacker, kill.ms) { return None; }
                shot.client = killer;
                shot.rig.aim_client = kill.victim;
                shot.rig.range = rng.range(52.0, 76.0);
            }
            P::VictimPov | P::VictimBlade => {
                if kill.means != MOD_SABER || kill.attacker < 0 || kill.victim < 0 || kill.attacker == kill.victim { return None; }
                shot.client = victim;
                shot.rig.aim_client = kill.attacker;
                shot.fov = if preset == P::VictimPov { rng.range(85.0, 95.0) } else { rng.range(80.0, 90.0) };
            }
            P::Static | P::Pan => {
                if self.world.is_none() { return None; }
                let recorder_eye = path.eye(kill.ms);
                let mut found = Vec::new();
                for k in 0..8 {
                    let yaw = k as f32 * 45.0 + rng.range(0.0, 20.0);
                    for (dist, height) in [(220.0f32, 48.0f32), (300.0, 96.0), (170.0, 24.0)] {
                        let p = center + flat(forward(yaw, 0.0)) * dist + Vec3::Z * height;
                        if !self.free(p) || !self.clear(p, center) || !self.clear(p, recorder_eye) { continue; }
                        found.push((height / 50.0 - dist / 300.0, p, p));
                    }
                }
                let (p, _) = Self::pick(found, rng)?;
                shot.position_override = Some(render(p));
                shot.rig.range = p.distance(center);
                if preset == P::Static {
                    shot.focus_target = Some(render(center));
                    shot.rig.aim_enabled = false;
                    // Focus on the victim, wherever they fall.
                    shot.client = victim;
                    shot.rig.autofocus = true;
                    shot.fov = rng.range(55.0, 70.0);
                    shot.lens.focus_distance = p.distance(center).clamp(1.0, 65536.0);
                } else {
                    shot.client = victim;
                    shot.rig.aim_client = if victim >= 0 { victim } else { -1 };
                }
            }
            _ => return None,
        }
        if shot.focus_target.is_none() { shot.lens.focus_distance = shot.rig.range.max(16.0); }
        let around = if kill.victim >= 0 { kill.victim } else { recorder };
        self.player_lens(&mut shot, Beat::Kill, rng);
        self.crowd_fov(&path, around, s, e, &mut shot);
        if matches!(preset, P::Orbit | P::LowAngle | P::DollyZoom | P::ThrowCam) && shot.position_override.is_none()
            && !self.rig_shot_clear(&path, &shot, s, e, recorder) { return None; }
        shot.valid().then_some(shot)
    }
}

/// One continuous ramp covers a rapid chain, including its last impact.
/// Merge overlapping slow-motion windows rather than dropping later kills.
fn slowmo_keys(kills: &[i32], kill_air: &HashMap<i32, f32>, clip: &StudioDemoClip) -> Vec<super::render_studio_timeline::StudioSpeedKey> {
    use super::render_studio_timeline::StudioSpeedKey;
    let mut kills: Vec<_> = kills.iter().copied().filter(|&ms| ms >= clip.in_ms && ms < clip.out_ms).collect();
    kills.sort_unstable();
    kills.dedup();
    let mut spans: Vec<(i32, i32, bool)> = Vec::new();
    for kill in kills {
        let air = kill_air.get(&kill).copied().unwrap_or(0.0) > 48.0;
        match spans.last_mut() {
            Some(last) if kill - last.1 < 2400 => { last.1 = kill; last.2 |= air; }
            _ => spans.push((kill, kill, air)),
        }
    }
    let mut keys: Vec<StudioSpeedKey> = Vec::new();
    for (first, last, air) in spans {
        let rate = if air { 0.2 } else { 0.3 };
        let ramp = if air { [(first - 600, 1.0), (first - 200, rate), (last + 1300, rate), (last + 1800, 1.0)] }
            else { [(first - 450, 1.0), (first - 150, rate), (last + 800, rate), (last + 1200, 1.0)] };
        for (ms, rate) in ramp {
            let time = f64::from(ms.clamp(clip.in_ms, clip.out_ms)) / 1000.0;
            if let Some(last) = keys.last_mut().filter(|last| (last.time - time).abs() < 0.001) {
                last.rate = rate;
            } else { keys.push(StudioSpeedKey { time, rate }); }
        }
    }
    keys
}

/// Combat view: per clip (index, clip, latest allowed end on its layer).
/// Downtime between fights (a respawn run back, wandering): 2.5x speed
/// in gaps of 6 s or more, ramping in and out over half a second, leaving
/// 1.5 s at normal speed at each end. The opening stays at normal speed.
fn downtime_keys(path: &DirectorPath, _fights: &[(i32, i32)], _kills: &[i32], respawns: &[i32], clip: &StudioDemoClip,
    who: &dyn Fn(i32) -> i32) -> Vec<super::render_studio_timeline::StudioSpeedKey> {
    use super::render_studio_timeline::StudioSpeedKey;
    let mut times: Vec<_> = (clip.in_ms..=clip.out_ms).step_by(100).collect();
    times.extend(respawns.iter().copied().filter(|&r| r >= clip.in_ms && r < clip.out_ms));
    times.push(clip.out_ms);
    times.sort_unstable();
    times.dedup();
    let mut quiet = Vec::new();
    let mut open = None;
    for ms in times {
        let safe = ms < clip.out_ms && !combat_busy(path, who(ms), ms) && !closing_in(path, who(ms), ms);
        match (safe, open) {
            (true, None) => open = Some(ms),
            (false, Some(start)) => { quiet.push((start, ms)); open = None; }
            _ => {}
        }
    }
    let mut keys = Vec::new();
    for (start, end) in quiet {
        let spawn = respawns.iter().copied().find(|&r| (r - start).abs() <= 100
            && who(r) == who(start) && !combat_busy(path, who(r), r));
        let (a, b, rate, ramp) = if let Some(spawn) = spawn {
            (spawn, end - 600, 3.0, 0)
        } else {
            if end - start < 6000 { continue; }
            (start + 1000, end - 1000, 2.5, 500)
        };
        if b - a < 1000 { continue; }
        // A 1 ms approach leaves the spawn snapshot already at full speed.
        let beginning = if ramp == 0 { [(a.saturating_sub(1), 1.0), (a, rate)] }
            else { [(a, 1.0), (a + ramp, rate)] };
        for (ms, rate) in beginning.into_iter().chain([(b - 400, rate), (b, 1.0)]) {
            if ms >= clip.in_ms && ms <= clip.out_ms {
                keys.push(StudioSpeedKey { time: f64::from(ms) / 1000.0, rate });
            }
        }
    }
    keys
}

/// Never carry a slow-motion tail across a camera change or a respawn. A
/// respawn is a new scene even when minimum-duration coverage kept one rig.
fn slowmo_for_edit(path: &DirectorPath, kills: &[CombatEvent], heights: &HashMap<i32, f32>, clip: &StudioDemoClip,
    shots: &[StudioDirectorShot], respawns: &HashMap<i32, Vec<i32>>)
    -> Vec<super::render_studio_timeline::StudioSpeedKey> {
    let mut keys: Vec<super::render_studio_timeline::StudioSpeedKey> = Vec::new();
    for shot in shots {
        let (a,b) = (clip.source_time_ms(shot.start), clip.source_time_ms(shot.end()));
        let mut boundaries = vec![a,b];
        for (&client, times) in respawns {
            boundaries.extend(times.iter().copied().filter(|&r| r > a && r < b
                && viewed_subject(path, clip, shots, r) == client));
        }
        boundaries.sort_unstable(); boundaries.dedup();
        for pair in boundaries.windows(2) {
            let mut local = clip.clone(); local.in_ms=pair[0]; local.out_ms=pair[1];
            let events: Vec<_> = kills.iter().filter(|kill| kill.ms >= pair[0] && kill.ms < pair[1]).map(|kill| kill.ms).collect();
            keys.extend(slowmo_keys(&events, heights, &local));
        }
    }
    keys.sort_by(|a,b|a.time.total_cmp(&b.time));
    keys.dedup_by(|a,b| {if (a.time-b.time).abs()<1e-6 {b.rate=b.rate.max(a.rate);true} else {false}});
    keys
}

fn combine_speed_keys(slow: Vec<super::render_studio_timeline::StudioSpeedKey>, fast: Vec<super::render_studio_timeline::StudioSpeedKey>)
    -> Vec<super::render_studio_timeline::StudioSpeedKey> {
    use super::render_studio_timeline::{studio_speed_at, StudioSpeedKey};
    let mut times: Vec<_> = slow.iter().chain(&fast).map(|key| key.time).collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 0.000001);
    times.into_iter().map(|time| {
        let (a, b) = (studio_speed_at(&slow, time), studio_speed_at(&fast, time));
        StudioSpeedKey { time, rate: if a < 1.0 || b < 1.0 { a.min(b) } else { a.max(b) } }
    }).collect()
}

pub(in crate::app) fn combat_view_plan(mut ctx: DirectorWorkerContext, clips: Vec<(usize, StudioDemoClip, f64)>, slowmo: bool,
    fast_forward: bool)
    -> Result<super::render_studio_director::StudioDirectorPlan, String> {
    let mut plan = super::render_studio_director::StudioDirectorPlan::default();
    let mut director: Option<(String, AutoDirector)> = None;
    let (mut fights, mut kills, mut slowed) = (0, 0, 0);
    for (index, mut clip, limit) in clips {
        let path = load_project_path(&ctx.project_dir, &ctx.paths, &clip)?;
        if director.as_ref().is_none_or(|(map, _)| !map.eq_ignore_ascii_case(&path.map)) {
            let mut next = ctx.director(&clip.game, &path.map);
            if let Some((_, previous)) = director.take() { next.history = previous.history; next.uses = previous.uses; }
            director = Some((path.map.clone(), next));
        }
        plan.clear_ranges.push((clip.start, clip.end()));
        if slowmo || fast_forward {
            let fights = director.as_mut().unwrap().1.fights(&ClipJob { clip: clip.clone(), path: path.clone(), role: ClipRole::Whole });
            let all_kills: Vec<i32> = path.events.iter().filter(|ev| ev.kind == CombatKind::Kill).map(|ev| ev.ms).collect();
            // Only kills belonging to the selected story may slow its footage.
            let mut fight_kills: Vec<i32> = director.as_ref().unwrap().1.engagements.iter()
                .flat_map(|fight| fight.kills.iter().map(|kill| kill.ms)).collect();
            fight_kills.sort_unstable();
            fight_kills.dedup();
            let air = director.as_ref().unwrap().1.kill_air.clone();
            let slow = if slowmo { slowmo_keys(&fight_kills, &air, &clip) } else { Vec::new() };
            let fast = if fast_forward {
                let auto = &director.as_ref().unwrap().1;
                let mut respawns = Vec::new();
                for (&client, times) in &auto.respawns {
                    respawns.extend(times.iter().copied().filter(|&ms| auto.subject_at(&path, ms) == client));
                }
                respawns.sort_unstable(); respawns.dedup();
                downtime_keys(&path, &fights, &all_kills, &respawns, &clip, &|ms| auto.subject_at(&path, ms))
            } else { Vec::new() };
            let keys = combine_speed_keys(slow, fast);
            {
                let mut slowed_clip = clip.clone();
                slowed_clip.speed_keys = keys.clone();
                if slowed_clip.valid() && slowed_clip.end() <= limit + 1e-6 {
                    clip = slowed_clip;
                    plan.speed_keys.push((index, keys));
                    slowed += 1;
                }
            }
        }
        plan.clear_ranges.push((clip.start, clip.end()));
        let (_, auto) = director.as_mut().unwrap();
        let (mut shots, f, k) = auto.direct_combat_clip(&ClipJob { clip: clip.clone(), path: path.clone(), role: ClipRole::Whole });
        if slowmo || fast_forward {
            let respawns: Vec<_> = auto.respawns.values().flatten().copied().collect();
            let mut fast = if fast_forward {
                downtime_keys(&path, &[], &[], &respawns, &clip,
                    &|ms| viewed_subject(&path, &clip, &shots, ms))
            } else { Vec::new() };
            let mut covered: Vec<_> = if slowmo {
                path.events.iter().filter(|ev| ev.kind == CombatKind::Kill && ev.ms >= clip.in_ms && ev.ms < clip.out_ms)
                    .filter(|ev| auto.covered_kill(&path, &clip, &shots, ev)).copied().collect()
            } else { Vec::new() };
            // Coverage can only remove slow-motion events. Re-evaluate after
            // remapping the actual edit; never retain a ramp on another fight.
            for pass in 0..5 {
                let slow = slowmo_for_edit(&path, &covered, &auto.kill_air, &clip, &shots, &auto.respawns);
                let mut final_clip = clip.clone();
                final_clip.speed_keys = combine_speed_keys(slow, fast.clone());
                if !final_clip.valid() || final_clip.end() > limit + 1e-6 { break; }
                retime_edit(&clip, &final_clip, &mut shots);
                clip = final_clip;
                // Final speed changes must still obey the editorial minimum.
                let mut i = 0;
                while shots.len() > 1 && i < shots.len() {
                    let minimum = if shots[i].preset == P::Static { COMBAT_MIN_STATIC } else { COMBAT_MIN_SHOT };
                    if shots[i].duration + 1e-6 < minimum {
                        let left = i.saturating_sub(1);
                        let right = if i == 0 { 1 } else { i };
                        let s = clip.source_time_ms(shots[left].start);
                        let e = clip.source_time_ms(shots[right].end());
                        let subject = viewed_subject(&path, &clip, &shots, s);
                        let job = ClipJob { clip: clip.clone(), path: path.clone(), role: ClipRole::Whole };
                        let mut coverage = auto.live_combat_shot(&job, s, e, false);
                        auto.plan_coverage(&mut coverage, &path, s, e, subject);
                        retime_camera_path(&clip, &mut coverage);
                        coverage.seed = auto.seed;
                        shots.splice(left..=right, [coverage]);
                        i = left;
                    } else { i += 1; }
                }
                let before = covered.len();
                covered.retain(|kill| auto.covered_kill(&path, &clip, &shots, kill));
                // Minimum-duration merges may change who a shot follows.
                // Recompute downtime for that final subject as well.
                let next_fast = if fast_forward {
                    downtime_keys(&path, &[], &[], &respawns, &clip,
                        &|ms| viewed_subject(&path, &clip, &shots, ms))
                } else { Vec::new() };
                let fast_stable = fast.len() == next_fast.len() && fast.iter().zip(&next_fast)
                    .all(|(a, b)| a.time == b.time && a.rate == b.rate);
                fast = next_fast;
                if covered.len() == before && fast_stable { break; }
                if pass == 3 { covered.clear(); fast.clear(); }
            }
            if let Some((_, keys)) = plan.speed_keys.iter_mut().find(|(i, _)| *i == index) { *keys = clip.speed_keys.clone(); }
            else { plan.speed_keys.push((index, clip.speed_keys.clone())); }
        }
        fights += f;
        kills += k;
        plan.added.extend(shots);
    }
    if plan.added.is_empty() { return Err("No shots fit this footage (clips too short?)".into()); }
    plan.summary = if fights == 0 {
        format!("Combat view: no fights found, so it was cut like a route view · {} shots · {}", plan.added.len(), type_summary(&plan.added))
    } else {
        format!("Combat view: {fights} fight{} · {kills} kill{} · {} shots · {}{}", if fights == 1 { "" } else { "s" },
            if kills == 1 { "" } else { "s" }, plan.added.len(), type_summary(&plan.added),
            if slowed > 0 { " · speed ramps (slow-mo kills / fast downtime)" } else { "" })
    };
    Ok(plan)
}

#[cfg(test)]
#[path = "render_studio_auto_director_combat_tests.rs"]
mod combat_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn line_path(speed: f32, seconds: i32, race: Option<(i32, i32)>) -> DirectorPath {
        let samples = (0..=seconds * 20).map(|i| {
            let ms = i * 50;
            PathSample { ms, origin: Vec3::new(speed * ms as f32 / 1000.0, 0.0, 0.0), velocity: Vec3::new(speed, 0.0, 0.0),
                ground: true, eye_height: 36.0, cut: false, client: 0, yaw: 0.0, pitch: 0.0 }
        }).collect();
        DirectorPath { map: "test".into(), samples, race, ..Default::default() }
    }

    fn clip(ms: i32) -> StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail: Default::default(), hud_overrides:Default::default(), hide_other_players:false,
            name: "run".into(), game: String::new(), start: 2.0, duration_ms: ms, in_ms: 0, out_ms: ms, speed_keys: vec![],
            transition_in: Default::default(), audio: Default::default(), layer: 0 }
    }

    #[test]
    fn whole_clip_is_covered_contiguously_with_opening_and_finish() {
        let path = Arc::new(line_path(900.0, 30, Some((3000, 26000))));
        let job = ClipJob { clip: clip(30000), path, role: ClipRole::Whole };
        let mut director = AutoDirector::new(None, 90.0, StudioDirectorPacing::Balanced, 7);
        let shots = director.direct_clip(&job);
        assert!(shots.len() >= 5);
        assert!((shots[0].start - 2.0).abs() < 1e-6);
        assert!((shots.last().unwrap().end() - job.clip.end()).abs() < 0.01);
        for w in shots.windows(2) {
            assert!((w[0].end() - w[1].start).abs() < 0.01, "contiguous");
            assert_ne!(w[0].preset, w[1].preset, "no back-to-back repeats");
        }
        // Without collision nothing is world-anchored.
        assert!(shots.iter().all(|s| !s.preset.world_anchored() && s.valid() && s.auto));
    }

    #[test]
    fn same_seed_same_cut_and_regenerate_changes_type() {
        let path = Arc::new(line_path(400.0, 20, None));
        let job = ClipJob { clip: clip(20000), path, role: ClipRole::Whole };
        let a = AutoDirector::new(None, 90.0, StudioDirectorPacing::Energetic, 3).direct_clip(&job);
        let b = AutoDirector::new(None, 90.0, StudioDirectorPacing::Energetic, 3).direct_clip(&job);
        assert_eq!(a, b);
        let mut director = AutoDirector::new(None, 90.0, StudioDirectorPacing::Energetic, 3);
        let mut shot = a[2].clone();
        shot.id = 9;
        let again = director.regenerate(&shot, &job, [None, None], None).unwrap();
        assert_ne!(again.preset, shot.preset);
        assert_eq!((again.id, again.start, again.duration), (9, shot.start, shot.duration));
        let placed = director.regenerate(&shot, &job, [None, None], Some(P::Tracking)).unwrap();
        assert_eq!(placed.preset, P::Tracking);
    }

    #[test]
    fn big_air_becomes_its_own_beat() {
        let mut path = line_path(700.0, 20, None);
        for s in &mut path.samples {
            if (8000..10000).contains(&s.ms) {
                s.ground = false;
                s.origin.z = 200.0 * (std::f32::consts::PI * (s.ms - 8000) as f32 / 2000.0).sin();
            }
        }
        let job = ClipJob { clip: clip(20000), path: Arc::new(path), role: ClipRole::Middle };
        let director = AutoDirector::new(None, 90.0, StudioDirectorPacing::Balanced, 1);
        let beats = director.beats(&job);
        assert!(beats.iter().any(|b| b.2 == Beat::BigAir && b.0 <= 8000 && b.1 >= 10000));
        for w in beats.windows(2) { assert_eq!(w[0].1, w[1].0); }
    }
}

fn type_summary(shots: &[StudioDirectorShot]) -> String {
    let mut counts: Vec<(P, usize)> = Vec::new();
    for shot in shots {
        match counts.iter_mut().find(|c| c.0 == shot.preset) {
            Some(c) => c.1 += 1,
            None => counts.push((shot.preset, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.iter().take(6).map(|(p, n)| if *n > 1 { format!("{} ×{n}", p.label()) } else { p.label().to_owned() })
        .collect::<Vec<_>>().join(", ")
}

/// Shared worker inputs.
pub(in crate::app) struct DirectorWorkerContext {
    pub base: std::path::PathBuf,
    pub project_dir: std::path::PathBuf,
    pub paths: StudioPathCache,
    pub collision: StudioCollisionCache,
    /// The UI's loaded collision for the first recording's map, if any.
    pub loaded: Option<CollisionWorld>,
    pub fov: f32,
    pub pacing: StudioDirectorPacing,
    pub seed: u32,
    pub focus: StudioDirectorFocus,
}

impl DirectorWorkerContext {
    /// One director for the whole job, with collision for `map`.
    fn director(&mut self, game: &str, map: &str) -> AutoDirector {
        let world = load_collision(&self.base, &self.collision, game, map, self.loaded.take());
        let mut director = AutoDirector::new(world, self.fov, self.pacing, self.seed);
        // Parse once per director/map, then share the PVS for every candidate.
        // Missing maps retain the existing conservative collision-only behavior.
        director.visibility = open_assets(&self.base, game).ok()
            .and_then(|mut assets| assets.read(&format!("maps/{map}.bsp"), 512 * 1024 * 1024).ok().flatten())
            .and_then(|asset| jka_assets::bsp::Bsp::parse(&asset.bytes).ok())
            .and_then(|map| map.visibility).map(Arc::new);
        director.set_focus(self.focus);
        director
    }
}

/// Route view: direct whole recordings, optionally trimmed to their race.
pub(in crate::app) fn route_view_plan(mut ctx: DirectorWorkerContext, clips: Vec<(usize, StudioDemoClip)>, trim: bool)
    -> Result<super::render_studio_director::StudioDirectorPlan, String> {
    let mut plan = super::render_studio_director::StudioDirectorPlan::default();
    let mut director: Option<(String, AutoDirector)> = None;
    let mut trimmed = 0;
    for (index, mut clip) in clips {
        let path = load_project_path(&ctx.project_dir, &ctx.paths, &clip)?;
        if director.as_ref().is_none_or(|(map, _)| !map.eq_ignore_ascii_case(&path.map)) {
            let mut next = ctx.director(&clip.game, &path.map);
            // Keep variety across recordings on different maps.
            if let Some((_, previous)) = director.take() { next.history = previous.history; next.uses = previous.uses; }
            director = Some((path.map.clone(), next));
        }
        let old = (clip.start, clip.end());
        if trim {
            if let Some((race_start, race_end)) = path.race {
                let (from, to) = ((race_start - 2200).max(0), (race_end + 2200).min(clip.duration_ms));
                if to - from >= 1000 && (from, to) != (clip.in_ms, clip.out_ms) {
                    let mut candidate = clip.clone();
                    candidate.in_ms = from;
                    candidate.out_ms = to;
                    if candidate.valid() {
                        clip = candidate;
                        plan.trims.push((index, from, to));
                        trimmed += 1;
                    }
                }
            }
        }
        plan.clear_ranges.push(old);
        plan.clear_ranges.push((clip.start, clip.end()));
        let (_, auto) = director.as_mut().unwrap();
        plan.added.extend(auto.direct_clip(&ClipJob { clip, path, role: ClipRole::Whole }));
    }
    let collision = director.as_ref().is_some_and(|(_, d)| d.has_collision());
    plan.summary = format!("Route view: {} shots · {}{}{} · variation {}", plan.added.len(), type_summary(&plan.added),
        if trimmed > 0 { " · trimmed to the run" } else { "" },
        if collision { "" } else { " · map collision unavailable, so no world-placed shots" }, ctx.seed);
    if plan.added.is_empty() { return Err("No shots fit this footage (clips too short?)".into()); }
    Ok(plan)
}

/// Regenerate (new decision) or re-place (`keep`) existing shots.
pub(in crate::app) fn regenerate_plan(mut ctx: DirectorWorkerContext,
    items: Vec<(StudioDirectorShot, StudioDemoClip, [Option<P>; 2])>, keep: Option<P>)
    -> Result<super::render_studio_director::StudioDirectorPlan, String> {
    let mut plan = super::render_studio_director::StudioDirectorPlan::default();
    let mut director: Option<(String, AutoDirector)> = None;
    let mut changes = Vec::new();
    let mut failed = 0;
    for (shot, clip, neighbors) in items {
        let path = load_project_path(&ctx.project_dir, &ctx.paths, &clip)?;
        if director.as_ref().is_none_or(|(map, _)| !map.eq_ignore_ascii_case(&path.map)) {
            director = Some((path.map.clone(), ctx.director(&clip.game, &path.map)));
        }
        let (_, auto) = director.as_mut().unwrap();
        // The path is the recorded player's: shots on another client get
        // rig defaults (facing-relative) instead of path-derived framing.
        let other_subject = shot.client >= 0 && !shot.auto;
        let saved = other_subject.then(|| auto.world.take());
        let job = ClipJob { clip, path, role: ClipRole::Whole };
        let result = auto.regenerate(&shot, &job, neighbors, keep);
        if let Some(world) = saved { auto.world = world; }
        match result {
            Some(mut new) => {
                if other_subject {
                    let (id, start, duration, client, enabled, seed) = (new.id, new.start, new.duration, new.client, new.enabled, new.seed);
                    new.preset.seed_shot(&mut new, ctx.fov);
                    (new.id, new.start, new.duration, new.client, new.enabled, new.seed) = (id, start, duration, client, enabled, seed);
                }
                changes.push(if keep.is_some() { new.preset.label().to_owned() }
                    else { format!("{} → {}", shot.preset.label(), new.preset.label()) });
                plan.replaced.push(new);
            }
            None => failed += 1,
        }
    }
    plan.summary = match (plan.replaced.len(), keep) {
        (0, Some(preset)) => format!("Couldn't place a {} shot with a clear view of the player here; it keeps an orbit camera",
            preset.label()),
        (0, None) => "Regenerate: no shot could be rebuilt here".into(),
        (_, Some(_)) => format!("Placed: {}{}", changes.join(", "),
            if failed > 0 { format!(" · {failed} kept their camera (no clear view)") } else { String::new() }),
        _ => format!("Regenerated: {}{}", changes.join(", "),
            if failed > 0 { format!(" · {failed} unchanged") } else { String::new() }),
    };
    Ok(plan)
}
