//! cg_specHud 2-4: CS-style team broadcast panels, plus the mode-4 telemetry sweep.
//!
//! Mode 4 walks the server's own follow cycle with `+attack` button edges in
//! ordinary usercmds (jaPRO `SpectatorThink`: attack edge while following ->
//! `Cmd_FollowCycle_f(ent, 1)`). Button edges are not reliable client
//! commands, so `sv_floodProtect` never drops them and they never use up the
//! chat/`score` flood budget. Each confirmed follow target contributes one real
//! followed-playerState sample; the sweep ends when the server's cycle wraps
//! back to the original player. Cycle order does not matter: every player in
//! the server's cycle is visited once per sweep.
//!
//! While the server temporarily follows someone else, the presented snapshot
//! is synthesized: the original player's playerState and every entity seen
//! during the sweep are extrapolated from their latest authoritative state, and
//! fresh entities from the probe snapshot near the presented view replace them.
//! Only the presentation copy handed to the game session is replaced; the
//! protocol decoder keeps its own delta baselines untouched. Event sequences
//! are carried over unchanged, so nothing re-fires.
use super::{App, DemoViewMode, Duration, Instant, ProtocolSnapshot};
use crate::{
    cgame::{
        ET_BODY, ET_EVENTS, ET_HOLOCRON, ET_ITEM, ET_MISSILE, ET_NPC, ET_PLAYER, GT_TEAM,
        TEAM_BLUE, TEAM_RED,
    },
    ui,
};
use jka_assets::bsp::Visibility;
use jka_protocol::{
    gamestate::{EntityState, ENTITY_FIELDS},
    netchan::UserCmd,
    server::PlayerState,
};
use std::collections::BTreeMap;

const PMF_FOLLOW: i32 = 4096;
const ENTITYNUM_NONE: i32 = 1023;
const EF_DEAD: i32 = 1 << 1;
const DEFAULT_VIEWHEIGHT: i32 = 36;
/// jaPRO SpectatorThink `ps.speed = 400`; bg_pmove pm_spectatorfriction,
/// pm_flyaccelerate, and PM_FlyMove's +altattack "turbo boost" scale.
const SPECTATOR_SPEED: f32 = 400.0;
const SPECTATOR_FRICTION: f32 = 5.0;
const FLY_ACCELERATE: f32 = 8.0;
const SPECTATOR_TURBO: f32 = 10.0;
/// Wait this long after the server stops following before the virtual free
/// camera takes over (the user may be about to pick a target).
const FREE_START_DELAY: Duration = Duration::from_millis(500);
/// Unseen missiles leave the free-camera world model after this long.
const FREE_MISSILE_TTL_MS: i32 = 2_000;
const BUTTON_ATTACK: i32 = jka_movement::BUTTON_ATTACK;
const BUTTON_ALT_ATTACK: i32 = jka_movement::BUTTON_ALT_ATTACK;
const STAT_HEALTH: usize = 0;
const STAT_ARMOR: usize = 5;
const STAT_MAX_HEALTH: usize = 8;
const PERS_SCORE: usize = 0;
const PERS_KILLED: usize = 8;
use super::spectator_broadcast_bars::{BAR_FORCE, BAR_HEALTH};
const PW_REDFLAG: usize = 4;
const PW_BLUEFLAG: usize = 5;
/// Radar: last-known positions linger this long after leaving the view.
const RADAR_LAST_KNOWN: Duration = Duration::from_secs(4);

const TR_STATIONARY: i32 = 0;
const TR_INTERPOLATE: i32 = 1;
const TR_LINEAR: i32 = 2;
const TR_LINEAR_STOP: i32 = 3;
/// g_gravity default; entityState carries no gravity of its own.
const DEFAULT_GRAVITY: f32 = 800.0;

/// Gap between the end of one sweep schedule and the start of the next.
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
/// Real POV time between two excursions, so the followed view is mostly
/// authoritative and only briefly extrapolated.
const HOME_DWELL: Duration = Duration::from_millis(250);
/// A pressed edge must produce a new follow target within this time.
const STEP_TIMEOUT: Duration = Duration::from_millis(1000);
/// Hard cap on one excursion (high ping / stuck mod), then restore explicitly.
const EXCURSION_TIMEOUT: Duration = Duration::from_secs(3);
/// OpenJK EVENT_VALID_MSEC: the server frees temp entities after this.
const EVENT_VALID_MS: i32 = 300;
/// Give up presenting the synthesized view if an explicit restore stalls.
const RESTORE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Extrapolate at most this far past the latest authoritative state.
const MAX_EXTRAPOLATION_MS: i32 = 1500;
/// Entities first seen in a probe snapshot join the presented scene only when
/// they are this close to the presented view (the probe PVS is someone else's).
const NEW_ENTITY_RADIUS: f32 = 2048.0;
/// Reliable commands closer than sv_floodProtect's 1000 ms are dropped by the
/// server (and still reset its timer). Keep automatic commands clear of it.
const COMMAND_QUIET: Duration = Duration::from_millis(1250);
const SCORE_HEARTBEAT: Duration = Duration::from_secs(15);
const UI_INTERVAL: Duration = Duration::from_millis(250);
/// While the radar is drawn or a card bar animates: republish at up to 30 Hz.
const RADAR_UI_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct PlayerTelemetry {
    health: i32,
    max_health: i32,
    armor: i32,
    force: Option<i32>,
    weapon: Option<i32>,
    /// Ammo of `weapon` (None for the saber/melee or unknown).
    ammo: Option<i32>,
    score: i32,
    deaths: i32,
    sampled_at: Instant,
}

impl PlayerTelemetry {
    fn from_player_state(ps: &PlayerState, now: Instant) -> Self {
        Self {
            health: ps.stats[STAT_HEALTH],
            max_health: ps.stats[STAT_MAX_HEALTH],
            armor: ps.stats[STAT_ARMOR],
            force: ps.field_i32("fd.forcePower"),
            weapon: ps.field_i32("weapon"),
            // weaponData ammoIndex; weapons that spend no ammo on either fire
            // mode (Bryar pistol, saber, melee) have no count worth showing.
            ammo: ps.field_i32("weapon").and_then(|weapon| {
                let (index, energy, alt_energy) = jka_movement::weapon_info(weapon)?;
                let index = usize::try_from(index).ok().filter(|index| *index > 0)?;
                (energy > 0 || alt_energy > 0).then(|| ps.ammo.get(index).copied()).flatten()
            }),
            score: ps.persistant[PERS_SCORE],
            deaths: ps.persistant[PERS_KILLED],
            sampled_at: now,
        }
    }
}

/// One `tinfo` row (jaPRO TeamplayInfoMessage). Spectators receive it once a
/// second for the whole team of the player they follow, so following anyone
/// gives live health/armor/weapon for that entire team without probing.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct TeamInfoSample {
    health: i32,
    armor: i32,
    /// jaPRO only: decoded from the packed armor field.
    force: Option<i32>,
    weapon: i32,
    at: Instant,
}

/// jaPRO TeamplayInfoMessage sends `armor * 100 + forcePower` in the armor
/// field. Decoded like jaPRO's CG_TeamOverlayStats: a zero remainder is read
/// as full Force (0 armor/100 Force and 1 armor/0 Force encode identically).
fn decode_japro_armor(packed: i32) -> (i32, i32) {
    let mut armor = packed.max(0);
    let mut force = armor % 100;
    if force == 0 {
        force = 100;
        armor -= 100;
    }
    ((armor / 100).max(0), force)
}

/// One-shot temp entities (eType >= ET_EVENTS) seen in any received snapshot.
/// A changing POV can drop an event entity from one snapshot and carry it in
/// the next; CG_ResetEntity would then fire it a second time. Keep each one
/// present until the server has certainly freed it, so it fires exactly once.
#[derive(Default)]
struct EventEntities(BTreeMap<u16, (EntityState, i32)>);

impl EventEntities {
    fn observe(&mut self, snapshot: &ProtocolSnapshot) {
        for entity in &snapshot.entities {
            if entity.field_i32("eType").unwrap_or(0) >= ET_EVENTS {
                self.0.insert(entity.number, (entity.clone(), snapshot.server_time));
            }
        }
        let time = snapshot.server_time;
        self.0.retain(|_, (_, seen)| time - *seen <= EVENT_VALID_MS);
    }

    fn near(&self, origin: [f32; 3], time: i32) -> impl Iterator<Item = EntityState> + '_ {
        self.0.values()
            .filter(move |(entity, _)| distance(entity_position(entity, time), origin) <= NEW_ENTITY_RADIUS)
            .map(|(entity, _)| entity.clone())
    }

    /// Add retained events a real snapshot lacks (another POV carried them).
    fn complete(&self, mut snapshot: ProtocolSnapshot) -> ProtocolSnapshot {
        let missing: Vec<EntityState> = self.0.values()
            .filter(|(entity, _)| !snapshot.entities.iter().any(|e| e.number == entity.number))
            .map(|(entity, _)| entity.clone())
            .collect();
        if !missing.is_empty() {
            snapshot.entities.extend(missing);
            snapshot.entities.sort_by_key(|entity| entity.number);
        }
        snapshot
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leg {
    /// Pressing away from the original (`+attack` forward, `+altattack` back).
    Out,
    /// Pressing the opposite button until the original is followed again.
    Back,
    /// On the original's real POV until `until`, then the next excursion.
    Home { until: Instant },
}

/// Follow-cycle excursions: out `d` steps with one button and back with the
/// other, alternating sides with growing depth (+1, -1, +2, -2, ...). Each
/// excursion samples every player it passes; the original's real POV returns
/// between excursions instead of only after a full loop.
struct Sweep {
    original: i32,
    /// Latest authoritative view of the original player and its server time.
    view: PlayerState,
    view_time: i32,
    area_mask: [u8; 32],
    vehicle_player_state: Option<PlayerState>,
    /// Latest authoritative state (and server time) of every non-event entity
    /// seen during the sweep. Monotonic, so an entity never flips back to an
    /// older state (which could re-fire its event).
    entities: BTreeMap<u16, (EntityState, i32)>,
    /// Follow target the server last confirmed in a snapshot.
    current: i32,
    /// Signed excursion depths for this sweep.
    schedule: Vec<i32>,
    index: usize,
    leg: Leg,
    /// Steps away from the original along the current excursion.
    position: i32,
    leg_started: Instant,
    /// Button for the next press (`+attack` or `+altattack`).
    press_button: i32,
    /// Press `press_button` on the next authored usercmd.
    press_due: bool,
    events: EventEntities,
    /// Usercmds sent with attack released since the last press (edge guard).
    released_cmds: u32,
    last_press: Option<Instant>,
    /// Explicit `follow <original>` fallback: pending until the command
    /// channel is quiet, then the time it was sent.
    restore: Option<Option<Instant>>,
}

fn out_button(depth: i32) -> i32 {
    if depth > 0 { BUTTON_ATTACK } else { BUTTON_ALT_ATTACK }
}

fn back_button(depth: i32) -> i32 {
    if depth > 0 { BUTTON_ALT_ATTACK } else { BUTTON_ATTACK }
}

/// +1, -1, +2, -2, ... covering `others` players split over both directions.
fn excursion_schedule(others: usize) -> Vec<i32> {
    let forward = (others + 1) / 2;
    let backward = others - forward;
    let mut schedule = Vec::new();
    for depth in 1..=forward.max(backward) {
        if depth <= forward { schedule.push(depth as i32); }
        if depth <= backward { schedule.push(-(depth as i32)); }
    }
    schedule
}

impl Sweep {
    fn start(original: i32, anchor: &ProtocolSnapshot, now: Instant, others: usize) -> Self {
        let schedule = excursion_schedule(others.max(1));
        let first = schedule[0];
        let mut events = EventEntities::default();
        events.observe(anchor);
        Self {
            original,
            view: anchor.player_state.clone(),
            view_time: anchor.server_time,
            area_mask: anchor.area_mask,
            vehicle_player_state: anchor.vehicle_player_state.clone(),
            entities: anchor
                .entities
                .iter()
                .filter(|entity| entity.field_i32("eType").unwrap_or(0) < ET_EVENTS)
                .map(|entity| (entity.number, (entity.clone(), anchor.server_time)))
                .collect(),
            current: original,
            schedule,
            index: 0,
            leg: Leg::Out,
            position: 0,
            leg_started: now,
            press_button: out_button(first),
            press_due: true,
            events,
            released_cmds: 0,
            last_press: None,
            restore: None,
        }
    }

    /// True while the presented snapshot is synthesized (away from home).
    fn away(&self) -> bool {
        !matches!(self.leg, Leg::Home { .. }) || self.restore.is_some()
    }

    fn depth(&self) -> i32 {
        self.schedule.get(self.index).copied().unwrap_or(1)
    }

    /// The original player is still networked (sweep not yet left, or the
    /// first press not yet processed): refresh the anchor.
    fn refresh_anchor(&mut self, snapshot: &ProtocolSnapshot) {
        self.view = snapshot.player_state.clone();
        self.view_time = snapshot.server_time;
        self.area_mask = snapshot.area_mask;
        self.vehicle_player_state = snapshot.vehicle_player_state.clone();
        self.entities.clear();
        for entity in &snapshot.entities {
            if entity.field_i32("eType").unwrap_or(0) < ET_EVENTS {
                self.entities.insert(entity.number, (entity.clone(), snapshot.server_time));
            }
        }
    }

    /// Build the presentation snapshot for a probe snapshot (server following
    /// someone other than `original`).
    fn synthesize(&mut self, probe: &ProtocolSnapshot) -> ProtocolSnapshot {
        let now_time = probe.server_time;
        // A probe snapshot that still sees the original player gives us its
        // real body state; adopt it as the newest authoritative view.
        if let Some(body) = probe.entities.iter().find(|entity| {
            i32::from(entity.number) == self.original
                && entity.field_i32("eType") == Some(ET_PLAYER)
        }) {
            apply_entity_to_view(&mut self.view, body, now_time);
            self.view_time = now_time;
        }
        let view = extrapolate_player_state(&self.view, self.view_time, now_time);
        let view_origin = vec3_ps(&view, "origin");

        // Every entity of the probe snapshot refreshes the scene (not just
        // the probed player): newer authoritative states replace remembered
        // ones, so only what no probe POV saw is extrapolated.
        for entity in &probe.entities {
            if i32::from(entity.number) == self.original
                || entity.field_i32("eType").unwrap_or(0) >= ET_EVENTS
            {
                continue;
            }
            let known = self.entities.contains_key(&entity.number);
            let near = distance(entity_position(entity, now_time), view_origin) <= NEW_ENTITY_RADIUS;
            if known || near {
                self.entities.insert(entity.number, (entity.clone(), now_time));
            }
        }

        let mut entities: Vec<EntityState> = self
            .entities
            .values()
            .map(|(entity, seen)| extrapolate_entity(entity, *seen, now_time))
            .chain(self.events.near(view_origin, now_time))
            .collect();
        entities.sort_by_key(|entity| entity.number);
        entities.dedup_by_key(|entity| entity.number);

        ProtocolSnapshot {
            server_time: probe.server_time,
            message_num: probe.message_num,
            delta_num: probe.delta_num,
            snap_flags: probe.snap_flags,
            ping: probe.ping,
            server_command_num: probe.server_command_num,
            area_mask: self.area_mask,
            player_state: view,
            vehicle_player_state: self.vehicle_player_state.clone(),
            entities,
        }
    }
}

/// Mode 4 for a free-flying spectator: the camera is client-side (noclip,
/// like the demo free camera) while the server is kept cycling through the
/// follow targets with `+attack` edges. Every followed playerState is a real
/// telemetry sample. The presented scene is a world model merged from what
/// each followed player saw; the presented playerState is our own still
/// free spectator, so HUD, events and prediction never see the probe POVs.
struct FreeCam {
    own: PlayerState,
    own_client: i32,
    /// Render-space eye of the App-owned free camera (never sent).
    position: glam::Vec3,
    /// Render-space fly velocity (PM_FlyMove state, client-side only).
    velocity: glam::Vec3,
    /// Clients currently on RED/BLUE (CS_PLAYERS). Players are never dropped
    /// for being unseen; only for leaving the teams or disconnecting.
    team_players: u64,
    /// Latest authoritative state (and server time) of each entity.
    entities: BTreeMap<u16, (EntityState, i32)>,
    /// Follow target the server last confirmed (-1 before the first).
    current: i32,
    press_due: bool,
    released_cmds: u32,
    last_press: Option<Instant>,
    events: EventEntities,
}

impl FreeCam {
    fn start(anchor: &ProtocolSnapshot, position: glam::Vec3) -> Self {
        let own_client = anchor.player_state.field_i32("clientNum").unwrap_or(-1);
        Self {
            own: anchor.player_state.clone(),
            own_client,
            position,
            velocity: glam::Vec3::ZERO,
            team_players: u64::MAX,
            entities: anchor
                .entities
                .iter()
                .filter(|entity| entity.field_i32("eType").unwrap_or(0) < ET_EVENTS)
                .map(|entity| (entity.number, (entity.clone(), anchor.server_time)))
                .collect(),
            current: -1,
            press_due: true,
            released_cmds: 0,
            last_press: None,
            events: {
                let mut events = EventEntities::default();
                events.observe(anchor);
                events
            },
        }
    }

    fn eye_native(&self) -> [f32; 3] {
        crate::scene::jka_position(self.position.to_array())
    }

    /// Merge one probe snapshot into the world model.
    fn ingest(&mut self, probe: &ProtocolSnapshot, visibility: Option<&Visibility>) {
        let time = probe.server_time;
        let ps = &probe.player_state;
        self.events.observe(probe);
        let mut seen = [0u64; 16];
        for entity in &probe.entities {
            if i32::from(entity.number) == self.own_client {
                continue;
            }
            seen[usize::from(entity.number) / 64] |= 1 << (entity.number % 64);
            if entity.field_i32("eType").unwrap_or(0) < ET_EVENTS {
                self.entities.insert(entity.number, (entity.clone(), time));
            }
        }
        // The followed player is not in its own snapshot's entity list:
        // rebuild it from the playerState (BG_PlayerStateToEntityState).
        let following = ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0;
        let followed = ps.field_i32("clientNum")
            .filter(|client| following && *client >= 0 && *client != self.own_client);
        if followed.is_some() {
            if let Some(mut state) = crate::cgame::player_state_to_entity_state(ps) {
                // Movement/animation come from the playerState, but the event
                // fields must stay the server entity's: a rebuilt state lacks
                // its toggle bits, and every flip would re-fire EV_JUMP etc.
                let (event, parm) = self.entities.get(&state.number)
                    .map(|(previous, _)| (
                        previous.field_bits("event").unwrap_or(0),
                        previous.field_bits("eventParm").unwrap_or(0),
                    ))
                    .unwrap_or((0, 0));
                entity_bits_set(&mut state, "event", event);
                entity_bits_set(&mut state, "eventParm", parm);
                // Same trajectory the server sends for this client: with
                // g_smoothClients that is TR_LINEAR_STOP at its commandTime
                // (BG_PlayerStateToEntityStateExtraPolate). A TR_INTERPOLATE
                // copy would sit up to trDuration behind and jump on switch.
                if let Some((previous, _)) = self.entities.get(&state.number)
                    .filter(|(previous, _)| previous.field_i32("pos.trType") == Some(TR_LINEAR_STOP))
                {
                    entity_bits_set(&mut state, "pos.trType", TR_LINEAR_STOP as u32);
                    entity_bits_set(&mut state, "pos.trTime",
                        ps.field_bits("commandTime").unwrap_or(time as u32));
                    entity_bits_set(&mut state, "pos.trDuration",
                        previous.field_bits("pos.trDuration").unwrap_or(50));
                }
                seen[usize::from(state.number) / 64] |= 1 << (state.number % 64);
                self.entities.insert(state.number, (state, time));
            }
        }
        // A missile/item the probe POV could see (client PVS + area mask) but
        // the server did not send is gone. Players and NPCs are never culled
        // this way: servers may withhold them for reasons the PVS cannot see
        // (jaPRO anti-wallhack line-of-sight), so they only expire by TTL.
        // Brush movers and other world-anchored entities are never culled:
        // their trBase is not a world position.
        let eye = {
            let origin = vec3_ps(ps, "origin");
            let height = ps.field_i32("viewheight").unwrap_or(DEFAULT_VIEWHEIGHT) as f32;
            [origin[0], origin[1], origin[2] + height]
        };
        let from_cluster = visibility.and_then(|vis| vis.cluster_at(eye));
        let team_players = self.team_players;
        self.entities.retain(|number, (entity, seen_at)| {
            if seen[usize::from(*number) / 64] & (1 << (*number % 64)) != 0 {
                return true;
            }
            let kind = entity.field_i32("eType").unwrap_or(0);
            if kind == ET_PLAYER {
                // Keep every player's last real state; the more of the match
                // stays on screen the better. Only leaving the teams removes.
                return *number >= 64 || team_players & (1u64 << *number) != 0;
            }
            if kind == ET_MISSILE && time - *seen_at > FREE_MISSILE_TTL_MS {
                return false;
            }
            let cullable = matches!(kind, ET_ITEM | ET_MISSILE | ET_HOLOCRON | ET_BODY);
            let (Some(vis), Some(from), true) = (visibility, from_cluster, cullable) else { return true };
            let position = entity_position(entity, time);
            let Some(cluster) = vis.cluster_at(position) else { return true };
            if !vis.visible(Some(from), &[cluster]) {
                return true;
            }
            // A closed portal area hides it from the probe POV: keep it.
            vis.area_at(position).is_some_and(|area| {
                area < 256 && probe.area_mask[area / 8] & (1 << (area % 8)) != 0
            })
        });
    }

    /// The presented snapshot: our own still spectator at the virtual camera,
    /// the extrapolated world model, and this snapshot's events near the view.
    fn present(&self, probe: &ProtocolSnapshot) -> ProtocolSnapshot {
        let time = probe.server_time;
        let eye = self.eye_native();
        let mut ps = self.own.clone();
        let height = ps.field_i32("viewheight").unwrap_or(DEFAULT_VIEWHEIGHT) as f32;
        set_vec3_ps(&mut ps, "origin", [eye[0], eye[1], eye[2] - height]);
        set_vec3_ps(&mut ps, "velocity", [0.0; 3]);
        // Our own steady clock, never the probed player's commandTime.
        ps.set_field_bits("commandTime", probe.server_time as u32);
        let mut entities: Vec<EntityState> = self
            .entities
            .values()
            .filter(|(entity, _)| i32::from(entity.number) != self.own_client)
            .map(|(entity, seen)| extrapolate_entity(entity, *seen, time))
            .collect();
        entities.extend(self.events.near(eye, time));
        entities.sort_by_key(|entity| entity.number);
        entities.dedup_by_key(|entity| entity.number);
        ProtocolSnapshot {
            server_time: probe.server_time,
            message_num: probe.message_num,
            delta_num: probe.delta_num,
            snap_flags: probe.snap_flags,
            ping: probe.ping,
            server_command_num: probe.server_command_num,
            // Areas closed for the probe POV say nothing about our camera.
            area_mask: [0; 32],
            player_state: ps,
            vehicle_player_state: None,
            entities,
        }
    }
}

#[derive(Default)]
pub(in crate::app) struct SpectatorBroadcast {
    last_team_scores: Option<[i32; 2]>,
    last_score_check: Option<Instant>,
    pending_score_refresh: bool,
    map_name: Option<String>,
    normal_client: Option<i32>,
    last_ui_refresh: Option<Instant>,
    next_sweep_at: Option<Instant>,
    sweep: Option<Sweep>,
    free: Option<FreeCam>,
    /// Earliest start of the virtual free camera after leaving follow.
    free_ready_at: Option<Instant>,
    /// Edge state of the user's attack buttons while the free camera runs.
    free_attack_down: bool,
    /// Swallow a held attack/alt-attack until released (it already acted).
    swallow_attack: bool,
    /// Latest real snapshot of the normally followed player (mode 4 only).
    last_real: Option<ProtocolSnapshot>,
    samples: [Option<PlayerTelemetry>; 32],
    team_info: [Option<TeamInfoSample>; 32],
    last_team_info_sequence: Option<u32>,
    last_reliable_sequence: Option<i32>,
    last_reliable_at: Option<Instant>,
}

impl SpectatorBroadcast {
    pub(in crate::app) fn free_active(&self) -> bool {
        self.free.is_some()
    }

    /// True while the presented playerState is synthesized, not the server's.
    fn synthesized(&self) -> bool {
        self.sweep.as_ref().is_some_and(Sweep::away) || self.free.is_some()
    }

    /// One authored live usercmd. While the free camera runs, the server only
    /// ever sees a motionless spectator pressing `+attack` edges.
    fn shape_cmd(&mut self, cmd: &mut UserCmd, now: Instant) {
        if self.swallow_attack {
            if cmd.buttons & (BUTTON_ATTACK | BUTTON_ALT_ATTACK) != 0 {
                cmd.buttons &= !(BUTTON_ATTACK | BUTTON_ALT_ATTACK);
            } else {
                self.swallow_attack = false;
            }
        }
        if let Some(free) = self.free.as_mut() {
            cmd.forward_move = 0;
            cmd.right_move = 0;
            cmd.up_move = 0;
            cmd.generic_command = 0;
            cmd.buttons = 0;
            if free.press_due && free.released_cmds >= 1 {
                cmd.buttons = BUTTON_ATTACK;
                free.press_due = false;
                free.released_cmds = 0;
                free.last_press = Some(now);
            } else {
                free.released_cmds = free.released_cmds.saturating_add(1);
            }
            return;
        }
        self.shape_buttons(&mut cmd.buttons, now);
    }

    /// Explicit /follow, /follownext, /team and similar user commands win:
    /// drop the sweep without restoring, the user's command picks the view.
    pub(in crate::app) fn cancel_for_user(&mut self) {
        self.sweep = None;
        self.free = None;
        self.free_ready_at = Some(Instant::now() + Duration::from_secs(5));
        self.last_real = None;
        self.next_sweep_at = Some(Instant::now() + Duration::from_secs(5));
    }

    fn sample(&mut self, client: i32, ps: &PlayerState, now: Instant) {
        let Ok(slot) = usize::try_from(client) else { return };
        if let Some(entry) = self.samples.get_mut(slot) {
            *entry = Some(PlayerTelemetry::from_player_state(ps, now));
        }
    }

    fn command_channel_quiet(&self, now: Instant) -> bool {
        self.last_reliable_at
            .is_none_or(|at| now.saturating_duration_since(at) >= COMMAND_QUIET)
    }

    /// Mask/inject attack buttons on one authored usercmd. Returns true when a
    /// user press aborted the sweep (their click then reaches the server).
    fn shape_buttons(&mut self, buttons: &mut i32, now: Instant) -> bool {
        let Some(sweep) = self.sweep.as_mut() else { return false };
        if *buttons & (BUTTON_ATTACK | BUTTON_ALT_ATTACK) != 0 {
            // The user clicked: their follow-cycle input wins.
            self.sweep = None;
            self.last_real = None;
            self.next_sweep_at = Some(now + Duration::from_secs(5));
            return true;
        }
        if sweep.press_due && sweep.restore.is_none() && sweep.released_cmds >= 1 {
            *buttons |= sweep.press_button;
            sweep.press_due = false;
            sweep.released_cmds = 0;
            sweep.last_press = Some(now);
        } else {
            sweep.released_cmds = sweep.released_cmds.saturating_add(1);
        }
        false
    }

    /// Route one received live snapshot. Returns the snapshot to present.
    fn filter_snapshot(
        &mut self,
        snapshot: ProtocolSnapshot,
        now: Instant,
        visibility: Option<&Visibility>,
    ) -> ProtocolSnapshot {
        let ps = &snapshot.player_state;
        let following = ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0;
        let client = ps.field_i32("clientNum").unwrap_or(-1);
        if let Some(free) = self.free.as_mut() {
            if following && client >= 0 && client != free.own_client {
                if let Some(entry) = usize::try_from(client).ok().and_then(|slot| self.samples.get_mut(slot)) {
                    *entry = Some(PlayerTelemetry::from_player_state(ps, now));
                }
                if client != free.current {
                    // Confirmed: press again. Cycle order does not matter.
                    free.current = client;
                    free.press_due = true;
                }
            }
            free.ingest(&snapshot, visibility);
            return free.present(&snapshot);
        }
        let Some(sweep) = self.sweep.as_mut() else {
            if client >= 0 {
                self.last_real = Some(snapshot.clone());
            }
            return snapshot;
        };
        sweep.events.observe(&snapshot);
        if !following || client < 0 {
            // The server stopped following (free-fly/team change): nothing
            // left to restore, present the real state.
            self.sweep = None;
            self.last_real = None;
            return snapshot;
        }
        if client == sweep.original {
            if sweep.current != sweep.original {
                sweep.current = sweep.original;
                sweep.position = 0;
                if sweep.restore.is_some() {
                    // The explicit restore landed: end this schedule.
                    self.sweep = None;
                    self.next_sweep_at = Some(now + SWEEP_INTERVAL);
                    self.last_real = Some(snapshot.clone());
                    return snapshot;
                }
                // Back home (or the cycle wrapped early): dwell on the real POV.
                sweep.leg = Leg::Home { until: now + HOME_DWELL };
                sweep.press_due = false;
            }
            sweep.refresh_anchor(&snapshot);
            self.last_real = Some(snapshot.clone());
            return sweep.events.complete(snapshot);
        }
        let telemetry = PlayerTelemetry::from_player_state(&snapshot.player_state, now);
        if client != sweep.current {
            sweep.current = client;
            match sweep.leg {
                Leg::Out => {
                    sweep.position += 1;
                    if sweep.position >= sweep.depth().abs() {
                        sweep.leg = Leg::Back;
                        sweep.leg_started = now;
                        sweep.press_button = back_button(sweep.depth());
                    }
                    sweep.press_due = sweep.restore.is_none();
                }
                Leg::Back => {
                    sweep.position -= 1;
                    // Two steps past home without meeting the original: the
                    // cycle changed (join/leave). Restore explicitly.
                    if sweep.position <= -2 && sweep.restore.is_none() {
                        sweep.restore = Some(None);
                    }
                    sweep.press_due = sweep.restore.is_none();
                }
                Leg::Home { .. } => {
                    // Someone else moved our follow target: put it back.
                    if sweep.restore.is_none() {
                        sweep.restore = Some(None);
                    }
                    sweep.press_due = false;
                }
            }
        }
        let presented = sweep.synthesize(&snapshot);
        if let Some(entry) = usize::try_from(client).ok().and_then(|slot| self.samples.get_mut(slot)) {
            *entry = Some(telemetry);
        }
        presented
    }
}

fn entity_bits_set(entity: &mut EntityState, name: &str, bits: u32) {
    if let Some(index) = ENTITY_FIELDS.iter().position(|(candidate, _)| *candidate == name) {
        entity.fields[index] = bits;
    }
}

fn vec3_entity(entity: &EntityState, base: &str) -> [f32; 3] {
    [0, 1, 2].map(|axis| entity.field_f32(&format!("{base}[{axis}]")).unwrap_or(0.0))
}

fn vec3_ps(ps: &PlayerState, base: &str) -> [f32; 3] {
    ps.field_vec3(base).unwrap_or([0.0; 3])
}

fn set_vec3_entity(entity: &mut EntityState, base: &str, value: [f32; 3]) {
    for (axis, component) in value.into_iter().enumerate() {
        entity_bits_set(entity, &format!("{base}[{axis}]"), component.to_bits());
    }
}

fn set_vec3_ps(ps: &mut PlayerState, base: &str, value: [f32; 3]) {
    for (axis, component) in value.into_iter().enumerate() {
        ps.set_field_bits(&format!("{base}[{axis}]"), component.to_bits());
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// BG_EvaluateTrajectory for the trajectory types players use.
fn entity_position(entity: &EntityState, time: i32) -> [f32; 3] {
    let base = vec3_entity(entity, "pos.trBase");
    let delta = vec3_entity(entity, "pos.trDelta");
    let tr_time = entity.field_i32("pos.trTime").unwrap_or(time);
    let elapsed_ms = match entity.field_i32("pos.trType").unwrap_or(TR_STATIONARY) {
        TR_LINEAR => (time - tr_time).max(0),
        TR_LINEAR_STOP => {
            let duration = entity.field_i32("pos.trDuration").unwrap_or(0);
            (time - tr_time).clamp(0, duration.max(0))
        }
        _ => 0,
    };
    let t = elapsed_ms as f32 / 1000.0;
    [0, 1, 2].map(|axis| base[axis] + delta[axis] * t)
}

/// Ballistic step: linear, plus gravity when airborne.
fn integrate(origin: [f32; 3], velocity: [f32; 3], airborne: bool, gravity: f32, dt: f32) -> ([f32; 3], [f32; 3]) {
    let mut origin = [0, 1, 2].map(|axis| origin[axis] + velocity[axis] * dt);
    let mut velocity = velocity;
    if airborne {
        origin[2] -= 0.5 * gravity * dt * dt;
        velocity[2] -= gravity * dt;
    }
    (origin, velocity)
}

fn extrapolation_seconds(from: i32, to: i32) -> f32 {
    (to - from).clamp(0, MAX_EXTRAPOLATION_MS) as f32 / 1000.0
}

/// The followed player's presented playerState at `to`. Only movement fields
/// change; stats, events and eventSequence stay as last observed.
fn extrapolate_player_state(view: &PlayerState, from: i32, to: i32) -> PlayerState {
    let mut ps = view.clone();
    let dt = extrapolation_seconds(from, to);
    let airborne = ps.field_i32("groundEntityNum").unwrap_or(ENTITYNUM_NONE) == ENTITYNUM_NONE;
    let gravity = ps.field_i32("gravity").filter(|g| *g > 0).map_or(DEFAULT_GRAVITY, |g| g as f32);
    let (origin, velocity) = integrate(vec3_ps(&ps, "origin"), vec3_ps(&ps, "velocity"), airborne, gravity, dt);
    set_vec3_ps(&mut ps, "origin", origin);
    set_vec3_ps(&mut ps, "velocity", velocity);
    let command_time = ps.field_i32("commandTime").unwrap_or(from);
    ps.set_field_bits("commandTime", (command_time + (to - from).max(0)) as u32);
    ps
}

/// The probe snapshot still sees the original player as an ET_PLAYER: copy
/// its body state into the presented playerState (BG_PlayerStateToEntityState
/// in reverse, for the fields the camera and model need).
fn apply_entity_to_view(view: &mut PlayerState, body: &EntityState, time: i32) {
    set_vec3_ps(view, "origin", entity_position(body, time));
    set_vec3_ps(view, "velocity", vec3_entity(body, "pos.trDelta"));
    set_vec3_ps(view, "viewangles", vec3_entity(body, "apos.trBase"));
    for field in ["legsAnim", "torsoAnim", "groundEntityNum"] {
        if let Some(bits) = body.field_bits(field) {
            view.set_field_bits(field, bits);
        }
    }
    let command_time = view.field_i32("commandTime").unwrap_or(time);
    view.set_field_bits("commandTime", command_time.max(time) as u32);
}

/// Players/NPCs carry their motion in trDelta: move trBase forward to `to`.
/// Other entity types are already time-parametric (movers, missiles, items)
/// and are presented unchanged so CG evaluates them on its own clock.
fn extrapolate_entity(entity: &EntityState, seen: i32, to: i32) -> EntityState {
    let kind = entity.field_i32("eType").unwrap_or(0);
    if (kind != ET_PLAYER && kind != ET_NPC) || to <= seen {
        // Current in this snapshot: exactly as the server sent it.
        return entity.clone();
    }
    let tr_type = entity.field_i32("pos.trType").unwrap_or(TR_STATIONARY);
    if !matches!(tr_type, TR_INTERPOLATE | TR_LINEAR | TR_LINEAR_STOP) {
        return entity.clone();
    }
    // The time trBase refers to: the snapshot for TR_INTERPOLATE, trTime for
    // the linear types (g_smoothClients stamps the owner's commandTime).
    // Extrapolating from that time keeps every source on one time base.
    let base_time = if tr_type == TR_INTERPOLATE {
        seen
    } else {
        entity.field_i32("pos.trTime").unwrap_or(seen)
    };
    if to <= base_time {
        return entity.clone();
    }
    let mut out = entity.clone();
    let airborne = entity.field_i32("groundEntityNum").unwrap_or(ENTITYNUM_NONE) == ENTITYNUM_NONE;
    let (origin, velocity) = integrate(
        vec3_entity(entity, "pos.trBase"),
        vec3_entity(entity, "pos.trDelta"),
        airborne,
        DEFAULT_GRAVITY,
        extrapolation_seconds(base_time, to),
    );
    set_vec3_entity(&mut out, "pos.trBase", origin);
    set_vec3_entity(&mut out, "pos.trDelta", velocity);
    if tr_type != TR_INTERPOLATE {
        // Re-anchor the trajectory at the presented time.
        entity_bits_set(&mut out, "pos.trTime", to as u32);
    }
    out
}

impl App {
    fn spectator_broadcast_sweep_allowed(&self) -> bool {
        self.japro_cg.spec_hud == 4
            && !self.japro_cg.follow_fastest
            && self.game_session.as_ref().is_some_and(|s| s.live && s.client_game.gametype() >= GT_TEAM)
            && self.live_spec_hud_eligible()
            && self.live_connected()
    }

    /// Called for every live snapshot before it is queued for the session.
    pub(in crate::app) fn filter_spectator_broadcast_snapshot(
        &mut self,
        snapshot: ProtocolSnapshot,
    ) -> ProtocolSnapshot {
        if self.japro_cg.spec_hud != 4 && !self.spectator_broadcast.synthesized() {
            self.spectator_broadcast.last_real = None;
            return snapshot;
        }
        // The free camera is App-owned (demo free camera); the presented
        // spectator sits wherever it is.
        let team_players = self.game_session.as_ref().map_or(u64::MAX, |s| {
            (0..32_usize).filter(|&slot| s.client_game.client_info(slot, &s.siege_classes)
                .is_some_and(|p| p.team == TEAM_RED || p.team == TEAM_BLUE))
                .fold(0u64, |mask, slot| mask | 1 << slot)
        });
        if let Some(free) = self.spectator_broadcast.free.as_mut() {
            free.position = self.camera.position;
            free.team_players = team_players;
        }
        let visibility = self.map_visibility.as_ref();
        self.spectator_broadcast.filter_snapshot(snapshot, Instant::now(), visibility)
    }

    /// Called on every authored live usercmd.
    pub(in crate::app) fn shape_spectator_broadcast_cmd(&mut self, cmd: &mut UserCmd) {
        self.spectator_broadcast.shape_cmd(cmd, Instant::now());
    }

    /// PM_FlyMove for the virtual free camera: spectator friction, fly
    /// acceleration toward the wished direction, +altattack turbo (x10), then
    /// the spectator collision slide. Runs on the client only.
    pub(in crate::app) fn update_spectator_free_flight(&mut self, dt: Duration) {
        use winit::keyboard::KeyCode;
        let Some(mut velocity) = self.spectator_broadcast.free.as_ref().map(|free| free.velocity) else { return };
        let frametime = dt.as_secs_f32().min(0.1);
        // PM_Friction (PM_SPECTATOR branch).
        let speed = velocity.length();
        if speed < 1.0 {
            velocity = glam::Vec3::ZERO;
        } else {
            let drop = speed * SPECTATOR_FRICTION * frametime;
            velocity *= (speed - drop).max(0.0) / speed;
        }
        // Wish direction: forward follows pitch, right stays horizontal,
        // +moveup/+movedown are world up/down (PM_FlyMove axes).
        let forward = self.camera.forward();
        let (sin_yaw, cos_yaw) = self.camera.yaw.sin_cos();
        let right = glam::Vec3::new(sin_yaw, 0.0, cos_yaw);
        let keys = &self.movement_keys;
        let mut wish = glam::Vec3::ZERO;
        if keys.contains(&KeyCode::KeyW) { wish += forward; }
        if keys.contains(&KeyCode::KeyS) { wish -= forward; }
        if keys.contains(&KeyCode::KeyD) { wish += right; }
        if keys.contains(&KeyCode::KeyA) { wish -= right; }
        if keys.contains(&KeyCode::Space) { wish += glam::Vec3::Y; }
        if keys.contains(&KeyCode::ControlLeft) { wish -= glam::Vec3::Y; }
        if wish.length_squared() > 0.0 {
            let wish_dir = wish.normalize();
            let turbo = if self.live_buttons.contains("+altattack") { SPECTATOR_TURBO } else { 1.0 };
            let wish_speed = SPECTATOR_SPEED * turbo;
            // PM_Accelerate.
            let add = wish_speed - velocity.dot(wish_dir);
            if add > 0.0 {
                velocity += wish_dir * (FLY_ACCELERATE * frametime * wish_speed).min(add);
            }
        }
        let before = self.camera.position;
        let mut after = before;
        if velocity.length_squared() > 0.0 && frametime > 0.0 {
            after = self.clip_spectator_free_camera(before, before + velocity * frametime);
            // Whatever the slide removed (into walls) is gone from velocity too.
            velocity = (after - before) / frametime;
        }
        self.camera.position = after;
        if let Some(free) = self.spectator_broadcast.free.as_mut() {
            free.velocity = velocity;
            free.position = after;
        }
    }

    /// Spectator collision for the virtual free camera: OpenJK PM_SPECTATOR
    /// box (+/-8) and trace mask (MASK_PLAYERSOLID without CONTENTS_BODY, so
    /// it passes through players), sliding along whatever it hits like
    /// PM_SlideMove. World and brush movers only; purely client-side.
    pub(in crate::app) fn clip_spectator_free_camera(
        &mut self,
        from: glam::Vec3,
        to: glam::Vec3,
    ) -> glam::Vec3 {
        use jka_movement::TraceWorld;
        const MASK: i32 = 0x1 | 0x10 | 0x1000; // SOLID | PLAYERCLIP | TERRAIN
        const HALF: f32 = 8.0;
        let solids = self.game_session.as_ref()
            .and_then(|session| session.current_snapshot.as_ref().map(|snapshot|
                crate::net::solid_entities(&session.presented_entities, snapshot.server_time)))
            .unwrap_or_default();
        let own = self.net.as_ref().map_or(-1, |net| net.session().client_num());
        let Some(world) = self.map_collision.as_mut() else { return to };
        let mut position = crate::scene::jka_position(from.to_array());
        let target = crate::scene::jka_position(to.to_array());
        let mut remaining = [0, 1, 2].map(|axis| target[axis] - position[axis]);
        for _ in 0..4 {
            if remaining.iter().map(|v| v * v).sum::<f32>() < 1e-4 {
                break;
            }
            let end = [0, 1, 2].map(|axis| position[axis] + remaining[axis]);
            let result = crate::net::PredictionWorld { world: &mut *world, solids: &solids, client_num: own }
                .trace(jka_movement::TraceQuery {
                    start: position,
                    mins: [-HALF; 3],
                    maxs: [HALF; 3],
                    end,
                    pass_entity: own,
                    mask: MASK,
                });
            if result.all_solid != 0 {
                // Already inside a brush (e.g. took over in a wall): let the
                // camera move freely until it is out.
                return to;
            }
            position = result.end;
            if result.fraction >= 1.0 {
                break;
            }
            // Slide: drop the part of the leftover move that goes into the plane.
            let left = remaining.map(|v| v * (1.0 - result.fraction));
            let into = (0..3).map(|axis| left[axis] * result.normal[axis]).sum::<f32>();
            remaining = [0, 1, 2].map(|axis| left[axis] - result.normal[axis] * into * 1.001);
        }
        glam::Vec3::from_array(crate::scene::render_position(position))
    }

    /// The user clicked in the virtual free camera: follow the player nearest
    /// the crosshair (or stay on the server's current target) for real.
    fn leave_spectator_free_camera(&mut self, now: Instant) {
        let Some(free) = self.spectator_broadcast.free.take() else { return };
        let forward = self.camera.forward();
        let eye = self.camera.position;
        let min_dot = 12.0_f32.to_radians().cos();
        let target = free.entities.values()
            .filter(|(entity, _)| entity.field_i32("eType") == Some(ET_PLAYER)
                && entity.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0
                && i32::from(entity.number) != free.own_client)
            .filter_map(|(entity, seen)| {
                let mut body = entity_position(entity, *seen);
                body[2] += 24.0;
                let to = glam::Vec3::from_array(crate::scene::render_position(body)) - eye;
                let dot = to.normalize_or_zero().dot(forward);
                (dot >= min_dot).then_some((i32::from(entity.number), dot))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(client, _)| client);
        let state = &mut self.spectator_broadcast;
        state.swallow_attack = true;
        state.last_real = None;
        state.next_sweep_at = Some(now + SWEEP_INTERVAL);
        state.free_ready_at = None;
        if let Some(client) = target.filter(|client| *client != free.current) {
            let _ = self.queue_server_command(&format!("follow {client}"));
        }
    }

    /// Called *after* network/cgame advances. Returns true when the broadcast
    /// UI should be republished.
    pub(in crate::app) fn tick_spectator_broadcast(&mut self, now: Instant) -> bool {
        self.sync_broadcast_minimap(now);
        let mode = self.japro_cg.spec_hud;
        let active = self.game_session.as_ref().is_some_and(|s| s.live && s.client_game.gametype() >= GT_TEAM)
            && self.live_spec_hud_eligible() && self.live_connected();
        let mut state = std::mem::take(&mut self.spectator_broadcast);
        let map_name = self.game_session.as_ref().and_then(|session| session.map_name.clone());
        if state.map_name != map_name {
            state = SpectatorBroadcast::default();
            state.map_name = map_name;
        }
        // Fresh `tinfo` (live or recorded): real values for one whole team.
        if let Some(game) = self.game_session.as_ref().map(|session| &session.client_game) {
            let sequence = game.team_info_sequence();
            if state.last_team_info_sequence != Some(sequence) {
                state.last_team_info_sequence = Some(sequence);
                let japro = game.is_japro();
                for entry in game.team_info() {
                    if let Some(slot) = state.team_info.get_mut(usize::from(entry.client)) {
                        let (armor, force) = if japro {
                            let (armor, force) = decode_japro_armor(entry.armor);
                            (armor, Some(force))
                        } else {
                            (entry.armor, None)
                        };
                        *slot = Some(TeamInfoSample {
                            health: entry.health,
                            armor,
                            force,
                            weapon: entry.weapon,
                            at: now,
                        });
                    }
                }
            }
        }
        // Track every reliable command (chat, score, userinfo, ...) so our own
        // automatic commands stay outside the server's flood window.
        if let Some(sequence) = self.net.as_ref().map(|net| net.session().reliable_sequence()) {
            if state.last_reliable_sequence != Some(sequence) {
                state.last_reliable_sequence = Some(sequence);
                state.last_reliable_at = Some(now);
            }
        }
        if !active || mode < 2 {
            if let Some(sweep) = state.sweep.take() {
                // Switched off mid-sweep while still spectating: put the
                // server back on the original player.
                if active && sweep.current != sweep.original {
                    let _ = self.queue_server_command(&format!("follow {}", sweep.original));
                }
            }
            state.last_real = None;
            state.next_sweep_at = None;
            let keep = SpectatorBroadcast {
                map_name: state.map_name.take(),
                last_reliable_sequence: state.last_reliable_sequence,
                last_reliable_at: state.last_reliable_at,
                last_ui_refresh: state.last_ui_refresh,
                // Observed values stay valid for the map (demo seeks, toggles).
                samples: state.samples,
                team_info: state.team_info,
                last_team_info_sequence: state.last_team_info_sequence,
                ..Default::default()
            };
            self.spectator_broadcast = keep;
            // Demo playback has no network work but still needs the panels
            // refreshed as the presented playerState and score timeline move.
            if mode >= 2 && self.demo_playback_active() {
                let interval = self.broadcast_ui_interval();
                let state = &mut self.spectator_broadcast;
                let refresh = state.last_ui_refresh
                    .is_none_or(|at| now.saturating_duration_since(at) >= interval);
                if refresh { state.last_ui_refresh = Some(now); }
                return refresh;
            }
            return false;
        }
        let Some((client, flags, ps)) = self.game_session.as_ref()
            .and_then(|s| s.current_snapshot.as_ref())
            .map(|s| (s.player_state.field_i32("clientNum").unwrap_or(-1),
                      s.player_state.field_i32("pm_flags").unwrap_or(0), s.player_state.clone()))
            else {
                self.spectator_broadcast = state;
                return false;
            };
        // The presented playerState is authoritative for its clientNum
        // (followed player, or our own) whenever no sweep synthesizes it.
        if !state.synthesized() && client >= 0 {
            state.sample(client, &ps, now);
        }
        // Mode 3 refreshes the team scoreboard with `score`. Mode 4 does not
        // while following: each sweep reads PERS_SCORE / PERS_KILLED from real
        // followed states. A free-flying spectator has no sweep (a follow cycle
        // cannot hand the free camera position back), so it falls back to `score`.
        if mode == 3 || (mode == 4 && flags & PMF_FOLLOW == 0 && state.free.is_none()) {
            let team_scores = self.game_session.as_ref().map(|s| {
                let limits = s.client_game.match_limits(); [limits.scores1, limits.scores2]
            });
            let changed = state.last_team_scores.is_some() && state.last_team_scores != team_scores;
            let heartbeat = state.last_score_check.is_none_or(|at| now.saturating_duration_since(at) >= SCORE_HEARTBEAT);
            state.pending_score_refresh |= changed || heartbeat;
            let typing = self.overlay == ui::OverlayMode::Chat;
            if state.pending_score_refresh && !typing && state.command_channel_quiet(now) {
                // `request_scores_if_due` owns the shared two-second guard.
                // Never set `scores_showing` or clear the legacy board.
                self.request_scores_if_due(false);
                state.last_score_check = Some(now);
                state.pending_score_refresh = false;
            }
            state.last_team_scores = team_scores;
        }
        self.spectator_broadcast = state;
        self.tick_spectator_sweep(now, client, flags);
        let interval = self.broadcast_ui_interval();
        let state = &mut self.spectator_broadcast;
        let refresh = state.last_ui_refresh.is_none_or(|at| now.saturating_duration_since(at) >= interval);
        if refresh { state.last_ui_refresh = Some(now); }
        refresh
    }

    /// Card values change slowly; radar blips and bar easing need a smooth refresh.
    fn broadcast_ui_interval(&self) -> Duration {
        let radar = self.japro_cg.spec_hud_map != 0 && self.broadcast_minimap.map.is_some();
        if radar || self.broadcast_bars.borrow().animating(Instant::now()) {
            RADAR_UI_INTERVAL
        } else {
            UI_INTERVAL
        }
    }

    fn tick_spectator_sweep(&mut self, now: Instant, client: i32, flags: i32) {
        let allowed = self.spectator_broadcast_sweep_allowed();
        if self.spectator_broadcast.free.is_some() {
            if !allowed {
                // Mode changed: the server keeps following its current target.
                self.spectator_broadcast.free = None;
                return;
            }
            // +attack follows the crosshair player. +altattack is the
            // spectator speed boost here (demo free camera multiplier); it
            // only cycles backward while actually following.
            let attack = self.live_buttons.contains("+attack");
            let pressed = attack && !self.spectator_broadcast.free_attack_down;
            self.spectator_broadcast.free_attack_down = attack;
            if pressed {
                self.leave_spectator_free_camera(now);
                return;
            }
            if let Some(free) = self.spectator_broadcast.free.as_mut() {
                // A press that never produced a new target (dropped, or the
                // server ignored it): simply press again.
                if !free.press_due
                    && free.last_press.is_some_and(|at| now.saturating_duration_since(at) >= STEP_TIMEOUT)
                {
                    free.press_due = true;
                }
            }
            return;
        }
        if allowed && flags & PMF_FOLLOW == 0 && client >= 0 && self.spectator_broadcast.sweep.is_none() {
            // Free-flying: hand the camera to the client, keep the server cycling.
            let ready = *self.spectator_broadcast.free_ready_at.get_or_insert(now + FREE_START_DELAY);
            let holding = self.live_buttons.contains("+attack");
            let others = self.game_session.as_ref().is_some_and(|s| {
                (0..32_usize).any(|slot| slot as i32 != client
                    && s.client_game.client_info(slot, &s.siege_classes)
                        .is_some_and(|p| p.team == TEAM_RED || p.team == TEAM_BLUE))
            });
            if now >= ready && !holding && others {
                let position = self.camera.position;
                let state = &mut self.spectator_broadcast;
                if let Some(anchor) = state.last_real.as_ref()
                    .filter(|snap| snap.player_state.field_i32("clientNum") == Some(client))
                {
                    state.free = Some(FreeCam::start(anchor, position));
                    state.free_attack_down = false;
                }
            }
            return;
        }
        if flags & PMF_FOLLOW != 0 {
            self.spectator_broadcast.free_ready_at = None;
        }
        if !allowed || flags & PMF_FOLLOW == 0 {
            if let Some(sweep) = self.spectator_broadcast.sweep.take() {
                // Mode changed mid-sweep: put the server back on the original.
                if sweep.current != sweep.original && flags & PMF_FOLLOW != 0 {
                    let _ = self.queue_server_command(&format!("follow {}", sweep.original));
                }
            }
            self.spectator_broadcast.last_real = None;
            self.spectator_broadcast.next_sweep_at = Some(now + SWEEP_INTERVAL);
            return;
        }
        let quiet = self.spectator_broadcast.command_channel_quiet(now);
        if let Some(sweep) = self.spectator_broadcast.sweep.as_mut() {
            match sweep.restore {
                Some(None) => {
                    if quiet {
                        let original = sweep.original;
                        if self.queue_server_command(&format!("follow {original}")).is_ok() {
                            if let Some(sweep) = self.spectator_broadcast.sweep.as_mut() {
                                sweep.restore = Some(Some(now));
                            }
                        } else {
                            self.spectator_broadcast.sweep = None;
                        }
                    }
                }
                Some(Some(sent)) => {
                    if now.saturating_duration_since(sent) >= RESTORE_TIMEOUT {
                        // Stuck server/mod: stop presenting a synthesized view.
                        self.spectator_broadcast.sweep = None;
                        self.spectator_broadcast.next_sweep_at = Some(now + Duration::from_secs(10));
                    }
                }
                None => match sweep.leg {
                    Leg::Home { until } => {
                        if now >= until {
                            sweep.index += 1;
                            if sweep.index >= sweep.schedule.len() {
                                self.spectator_broadcast.sweep = None;
                                self.spectator_broadcast.next_sweep_at = Some(now + SWEEP_INTERVAL);
                            } else {
                                sweep.leg = Leg::Out;
                                sweep.position = 0;
                                sweep.leg_started = now;
                                sweep.press_button = out_button(sweep.depth());
                                sweep.press_due = true;
                            }
                        }
                    }
                    Leg::Out | Leg::Back => {
                        let step_stalled = !sweep.press_due
                            && sweep.last_press.is_some_and(|at| now.saturating_duration_since(at) >= STEP_TIMEOUT);
                        let too_long = now.saturating_duration_since(sweep.leg_started) >= EXCURSION_TIMEOUT;
                        if step_stalled || too_long {
                            if sweep.current == sweep.original {
                                // Never left the original: nothing to restore.
                                self.spectator_broadcast.sweep = None;
                                self.spectator_broadcast.next_sweep_at = Some(now + Duration::from_secs(10));
                            } else {
                                sweep.press_due = false;
                                sweep.restore = Some(None);
                            }
                        }
                    }
                },
            }
            return;
        }
        // Idle: a user-chosen target change postpones the next sweep.
        let state = &mut self.spectator_broadcast;
        if state.normal_client.is_some_and(|previous| previous != client) {
            state.next_sweep_at = Some(now + SWEEP_INTERVAL);
        }
        state.normal_client = Some(client);
        if client < 0 || state.next_sweep_at.is_some_and(|due| now < due) {
            return;
        }
        if self.live_buttons.contains("+attack") || self.live_buttons.contains("+altattack") {
            return;
        }
        // Someone else must be in the cycle, or a press just lands back on
        // the original and could never be confirmed.
        let others = self.game_session.as_ref().map_or(0, |s| {
            (0..32_usize).filter(|&slot| slot as i32 != client
                && s.client_game.client_info(slot, &s.siege_classes)
                    .is_some_and(|p| p.team == TEAM_RED || p.team == TEAM_BLUE))
                .count()
        });
        let state = &mut self.spectator_broadcast;
        let Some(anchor) = state.last_real.as_ref()
            .filter(|snap| snap.player_state.field_i32("clientNum") == Some(client))
            else { return };
        if others == 0 {
            state.next_sweep_at = Some(now + SWEEP_INTERVAL);
            return;
        }
        state.sweep = Some(Sweep::start(client, anchor, now, others));
    }

    pub(in crate::app) fn spectator_broadcast_ui(&self) -> Option<ui::SpectatorBroadcastUi> {
        let mode = self.japro_cg.spec_hud;
        if mode < 2 || !(self.demo_playback_active() || self.live_spec_hud_eligible()) {
            return None;
        }
        let session = self.game_session.as_ref()?;
        if session.client_game.gametype() < GT_TEAM { return None; }
        let limits = session.client_game.match_limits();
        let now = Instant::now();
        let sweeping = self.spectator_broadcast.synthesized();
        // Demos carry their own indexed `scores` timeline.
        let demo_board = (!session.live && self.scoreboard.is_none())
            .then(|| session.demo_scoreboard_ui(now, self.score_deaths))
            .flatten();
        let scoreboard = self.scoreboard.as_ref().or(demo_board.as_ref());
        let mut players = Vec::new();
        for client in 0..32_usize {
            let Some(info) = session.client_game.client_info(client, &session.siege_classes) else { continue };
            if info.team != TEAM_RED && info.team != TEAM_BLUE { continue; }
            let board = scoreboard
                .and_then(|b| b.entries.iter().find(|entry| entry.client == client as i32));
            let entity = session.presented_entities.iter()
                .find(|ent| usize::from(ent.number) == client && ent.entity_type == ET_PLAYER);
            // The presented playerState is authoritative for its clientNum
            // (followed player, demo recorder, or ourselves) outside a sweep.
            let followed = session.current_snapshot.as_ref()
                .filter(|s| !sweeping
                    && s.player_state.field_i32("clientNum") == Some(client as i32))
                .map(|s| PlayerTelemetry::from_player_state(&s.player_state, now));
            let observed = followed.or(self.spectator_broadcast.samples[client]);
            let age_ms = |at: Instant| now.saturating_duration_since(at).as_millis().min(u32::MAX as u128) as u32;
            // `tinfo` wins when it is newer than the last playerState sample;
            // Force only exists in playerStates, so keep it while still fresh.
            let team_info = self.spectator_broadcast.team_info[client]
                .filter(|info| observed.is_none_or(|sample| info.at > sample.sampled_at));
            // Only display numbers actually observed (playerState or tinfo).
            let vitals = match (team_info, observed) {
                (Some(info), sample) => Some(ui::BroadcastVitals {
                    health: Some(info.health),
                    max_health: sample.map(|s| s.max_health).filter(|max| *max > 0),
                    armor: Some(info.armor),
                    force: info.force.or_else(|| sample.filter(|s| age_ms(s.sampled_at) <= 5_000)
                        .and_then(|s| s.force)).filter(|force| *force >= 0),
                    age_ms: age_ms(info.at),
                }),
                (None, Some(sample)) => Some(ui::BroadcastVitals {
                    health: Some(sample.health),
                    max_health: (sample.max_health > 0).then_some(sample.max_health),
                    armor: Some(sample.armor),
                    force: sample.force.filter(|force| *force >= 0),
                    age_ms: age_ms(sample.sampled_at),
                }),
                (None, None) => None,
            };
            // Mode 4 reads score/deaths from sampled playerStates; modes 2/3
            // prefer the `score` board and fall back to samples.
            let sampled_score = observed.map(|sample| (sample.score, sample.deaths));
            let (score, deaths) = match (mode, sampled_score, board) {
                (4, Some((score, deaths)), _) => (Some(score), Some(deaths)),
                (_, _, Some(entry)) => (Some(entry.score), entry.deaths),
                (_, Some((score, deaths)), None) => (Some(score), Some(deaths)),
                _ => (None, None),
            };
            let weapon = entity.and_then(|e| e.state.field_i32("weapon"))
                .or_else(|| team_info.map(|info| info.weapon).filter(|weapon| *weapon > 0))
                .or_else(|| observed.and_then(|sample| sample.weapon));
            // Only an ammo count observed for the weapon now shown.
            let ammo = observed.filter(|sample| sample.weapon == weapon)
                .and_then(|sample| sample.ammo);
            // Radar: the presented playerState, then the presented entity,
            // then a recent last-known position.
            let presented_ps = session.current_snapshot.as_ref()
                .map(|snapshot| &snapshot.player_state)
                .filter(|ps| ps.field_i32("clientNum") == Some(client as i32));
            let flag_bits = |red: bool, blue: bool| {
                if red { Some(TEAM_RED) } else if blue { Some(TEAM_BLUE) } else { None }
            };
            let mut last_known_alive = None;
            let (position, yaw, position_stale, carrying_flag) = if let Some(ps) = presented_ps {
                (Some(vec3_ps(ps, "origin")), vec3_ps(ps, "viewangles")[1], false,
                    flag_bits(ps.powerups[PW_REDFLAG] != 0, ps.powerups[PW_BLUEFLAG] != 0))
            } else if let Some(entity) = entity {
                let powerups = entity.state.field_i32("powerups").unwrap_or(0);
                (Some(entity.origin), entity.angles[1], false,
                    flag_bits(powerups & (1 << PW_REDFLAG) != 0, powerups & (1 << PW_BLUEFLAG) != 0))
            } else {
                match self.broadcast_minimap.seen[client]
                    .filter(|seen| now.saturating_duration_since(seen.at) <= RADAR_LAST_KNOWN)
                {
                    Some(seen) => {
                        last_known_alive = Some(seen.alive);
                        (Some(seen.position), seen.yaw, true, None)
                    }
                    None => (None, 0.0, false, None),
                }
            };
            let fallback = info.missing_model_fallback();
            let alive_now = entity.map(|e| e.state.field_i32("eFlags").unwrap_or(0) & 2 == 0)
                .or_else(|| vitals.as_ref().and_then(|v| v.health.map(|hp| hp > 0)))
                .or(last_known_alive);
            let (health_bar, force_bar) = {
                let living = vitals.as_ref().filter(|_| alive_now != Some(false));
                let health = living.and_then(|v| {
                    let max = v.max_health.filter(|max| *max > 0).unwrap_or(100);
                    v.health.map(|hp| hp.max(0) as f32 / max as f32)
                });
                let force = living.and_then(|v| v.force).map(|force| force.max(0) as f32 / 100.0);
                let mut bars = self.broadcast_bars.borrow_mut();
                (bars.sample(client, BAR_HEALTH, health, now), bars.sample(client, BAR_FORCE, force, now))
            };
            players.push(ui::BroadcastPlayerUi {
                client: client as i32, team: info.team, name: info.name,
                score, deaths,
                weapon,
                ammo,
                alive: alive_now,
                vitals,
                model: format!("{}/{}", info.model_name, info.skin_name),
                model_fallback: format!("{}/{}", fallback.model_name, fallback.skin_name),
                position: position.filter(|p| p.iter().all(|v| v.is_finite()))
                    .map(|p| p.map(|v| v.round() as i32)),
                yaw: yaw.rem_euclid(360.0).round() as i16,
                position_stale,
                carrying_flag,
                health_bar,
                force_bar,
            });
        }
        players.sort_by(|a,b| a.team.cmp(&b.team).then_with(|| b.score.cmp(&a.score)).then(a.client.cmp(&b.client)));

        // The player the camera presents (same rule as the specHud outlines).
        let observed_client = if session.live {
            match session.spectate_info() {
                Some(info) => info.followed.map(|(client, _)| client),
                None => session.client_game.presentation_client_num(),
            }
        } else if session.local {
            session.client_game.presentation_client_num()
        } else {
            match self.demo_view_mode {
                DemoViewMode::Authoritative => session.demo_followed_client(),
                DemoViewMode::Follow(client) => Some(client),
                DemoViewMode::Free => None,
            }
        };
        let observed = observed_client
            .filter(|client| players.iter().any(|player| player.client == *client))
            .map(|client| {
                // Ammo/style need the real presented playerState of that client.
                let ps = session.current_snapshot.as_ref()
                    .map(|snapshot| &snapshot.player_state)
                    .filter(|ps| !sweeping && ps.field_i32("clientNum") == Some(client));
                let weapon = ps.and_then(|ps| ps.field_i32("weapon"));
                let ammo = ps.zip(weapon).and_then(|(ps, weapon)| {
                    let index = jka_movement::weapon_info(weapon)
                        .map_or(0, |(index, _, _)| index.max(0) as usize);
                    (index > 0).then(|| ps.ammo.get(index).copied()).flatten()
                });
                let saber_style = ps.filter(|_| weapon == Some(3))
                    .and_then(|ps| ps.field_i32("fd.saberDrawAnimLevel"))
                    .filter(|style| *style > 0);
                ui::BroadcastObservedUi {
                    client,
                    ammo,
                    saber_style,
                    // Only when the native meters would show this same player.
                    replaces_native: ps.is_some(),
                }
            });

        // Match clock: counts down under a timelimit, up otherwise.
        let clock = session.current_snapshot.as_ref().map(|snapshot| {
            let elapsed = snapshot.server_time.saturating_sub(limits.level_start_time).max(0);
            let seconds = if limits.timelimit > 0 {
                (limits.timelimit.saturating_mul(60_000) - elapsed).max(0).saturating_add(999) / 1000
            } else {
                elapsed / 1000
            };
            format!("{}:{:02}", seconds / 60, seconds % 60)
        });
        const GT_CTF: i32 = 8;
        const GT_CTY: i32 = 9;
        let game_type = session.client_game.gametype();
        let score_limit = match game_type {
            GT_CTF | GT_CTY => session.client_game.serverinfo_value(b"capturelimit")
                .and_then(|value| value.trim().parse::<i32>().ok()),
            GT_TEAM => Some(limits.fraglimit),
            _ => None,
        }
        .filter(|limit| *limit > 0);

        // Radar: a free camera (no observed player) shows as a view cone.
        let camera = (session.live && observed.is_none())
            .then(|| session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state))
            .flatten()
            .map(|ps| (vec3_ps(ps, "origin").map(|v| v.round() as i32),
                vec3_ps(ps, "viewangles")[1].rem_euclid(360.0).round() as i16));
        let map_mode = self.japro_cg.spec_hud_map;
        let minimap = (map_mode != 0)
            .then(|| self.broadcast_minimap.map.clone())
            .flatten()
            .map(ui::MinimapHandle);
        Some(ui::SpectatorBroadcastUi {
            team_scores: [limits.scores1, limits.scores2],
            game_type,
            clock,
            score_limit,
            players,
            observed,
            scanning: sweeping,
            map_mode,
            minimap,
            camera,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps_following(client: i32) -> PlayerState {
        let mut ps = PlayerState::default();
        ps.set_field_bits("clientNum", client as u32);
        ps.set_field_bits("pm_flags", PMF_FOLLOW as u32);
        ps.set_field_bits("groundEntityNum", 0);
        ps
    }

    fn snapshot(time: i32, client: i32, entities: Vec<EntityState>) -> ProtocolSnapshot {
        ProtocolSnapshot {
            server_time: time,
            message_num: time,
            delta_num: 0,
            snap_flags: 0,
            ping: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state: ps_following(client),
            vehicle_player_state: None,
            entities,
        }
    }

    fn player_entity(number: u16, origin: [f32; 3], velocity: [f32; 3]) -> EntityState {
        let mut entity = EntityState { number, fields: [0; ENTITY_FIELDS.len()] };
        entity_bits_set(&mut entity, "eType", ET_PLAYER as u32);
        entity_bits_set(&mut entity, "pos.trType", TR_INTERPOLATE as u32);
        set_vec3_entity(&mut entity, "pos.trBase", origin);
        set_vec3_entity(&mut entity, "pos.trDelta", velocity);
        entity
    }

    #[test]
    fn press_is_an_edge_and_user_click_aborts() {
        let now = Instant::now();
        let mut state = SpectatorBroadcast::default();
        state.sweep = Some(Sweep::start(1, &snapshot(100, 1, Vec::new()), now, 1));
        // The first command only releases; the second carries the press.
        let mut buttons = 0;
        state.shape_buttons(&mut buttons, now);
        assert_eq!(buttons & BUTTON_ATTACK, 0);
        let mut buttons = 0;
        state.shape_buttons(&mut buttons, now);
        assert_ne!(buttons & BUTTON_ATTACK, 0);
        // No second press until the server confirms a new target.
        let mut buttons = 0;
        state.shape_buttons(&mut buttons, now);
        assert_eq!(buttons & BUTTON_ATTACK, 0);
        let mut buttons = BUTTON_ALT_ATTACK;
        assert!(state.shape_buttons(&mut buttons, now));
        assert_eq!(buttons, BUTTON_ALT_ATTACK);
        assert!(state.sweep.is_none());
    }

    #[test]
    fn sweep_presents_original_view_and_completes_on_wrap() {
        let now = Instant::now();
        let mut state = SpectatorBroadcast::default();
        let mut anchor = snapshot(1000, 1, vec![player_entity(5, [0.0; 3], [100.0, 0.0, 0.0])]);
        set_vec3_ps(&mut anchor.player_state, "velocity", [200.0, 0.0, 0.0]);
        anchor.player_state.persistant[PERS_SCORE] = 7;
        state.last_real = Some(anchor.clone());
        state.sweep = Some(Sweep::start(1, &anchor, now, 1));

        // Server now follows client 3; it cannot see entity 5.
        let mut probe = snapshot(1100, 3, Vec::new());
        probe.player_state.stats[STAT_HEALTH] = 42;
        probe.player_state.persistant[PERS_SCORE] = 9;
        let shown = state.filter_snapshot(probe, now, None);
        assert_eq!(shown.player_state.field_i32("clientNum"), Some(1));
        let origin = vec3_ps(&shown.player_state, "origin");
        assert!((origin[0] - 20.0).abs() < 1e-3, "{origin:?}");
        let body = shown.entities.iter().find(|e| e.number == 5).expect("held entity");
        assert!((vec3_entity(body, "pos.trBase")[0] - 10.0).abs() < 1e-3);
        let sample = state.samples[3].expect("probe sample");
        assert_eq!((sample.health, sample.score), (42, 9));
        // Depth reached: press +altattack back toward the original.
        assert!(state.sweep.as_ref().is_some_and(|s| s.press_due && s.current == 3
            && s.leg == Leg::Back && s.press_button == BUTTON_ALT_ATTACK));

        // Back on the original: real snapshot, dwell at home.
        let back = state.filter_snapshot(snapshot(1200, 1, Vec::new()), now, None);
        assert_eq!(back.server_time, 1200);
        assert!(state.sweep.as_ref().is_some_and(|s| matches!(s.leg, Leg::Home { .. })));
        assert!(!state.synthesized());
    }

    #[test]
    fn schedule_alternates_sides_with_growing_depth() {
        assert_eq!(excursion_schedule(1), vec![1]);
        assert_eq!(excursion_schedule(2), vec![1, -1]);
        assert_eq!(excursion_schedule(5), vec![1, -1, 2, -2, 3]);
    }

    #[test]
    fn rebuilt_followed_player_keeps_server_event_bits() {
        let now = Instant::now();
        let mut server_body = player_entity(3, [0.0; 3], [0.0; 3]);
        entity_bits_set(&mut server_body, "event", 0x1_05);
        let anchor = snapshot(0, 1, vec![server_body]);
        let mut free = FreeCam::start(&anchor, glam::Vec3::ZERO);
        // Probe snapshot following client 3: its body is rebuilt from the PS.
        free.ingest(&snapshot(50, 3, Vec::new()), None);
        let (body, _) = free.entities.get(&3).expect("rebuilt body");
        assert_eq!(body.field_bits("event"), Some(0x1_05));
        let _ = now;
    }

    #[test]
    fn event_entities_persist_across_pov_changes() {
        let mut event = player_entity(700, [0.0; 3], [0.0; 3]);
        entity_bits_set(&mut event, "eType", (ET_EVENTS + 10) as u32);
        let mut events = EventEntities::default();
        events.observe(&snapshot(100, 1, vec![event]));
        events.observe(&snapshot(150, 3, Vec::new()));
        assert_eq!(events.near([0.0; 3], 150).count(), 1);
        events.observe(&snapshot(500, 3, Vec::new()));
        assert_eq!(events.near([0.0; 3], 500).count(), 0);
    }

    #[test]
    fn overshooting_home_requests_restore() {
        let now = Instant::now();
        let mut state = SpectatorBroadcast::default();
        state.sweep = Some(Sweep::start(1, &snapshot(0, 1, Vec::new()), now, 1));
        state.filter_snapshot(snapshot(50, 3, Vec::new()), now, None); // out -> back
        state.filter_snapshot(snapshot(100, 4, Vec::new()), now, None); // position 0
        state.filter_snapshot(snapshot(150, 5, Vec::new()), now, None); // -1
        state.filter_snapshot(snapshot(200, 6, Vec::new()), now, None); // -2
        let sweep = state.sweep.as_ref().expect("restoring");
        assert!(matches!(sweep.restore, Some(None)));
        assert!(!sweep.press_due);
    }

    #[test]
    fn japro_tinfo_armor_unpacks_force() {
        assert_eq!(decode_japro_armor(2600), (25, 100));
        assert_eq!(decode_japro_armor(2537), (25, 37));
        assert_eq!(decode_japro_armor(100), (0, 100));
        assert_eq!(decode_japro_armor(0), (0, 100));
    }

    #[test]
    fn cancel_drops_sweep() {
        let mut state = SpectatorBroadcast::default();
        state.sweep = Some(Sweep::start(1, &snapshot(0, 1, Vec::new()), Instant::now(), 1));
        assert!(state.sweep.is_some());
        state.cancel_for_user();
        assert!(state.sweep.is_none());
    }
}
