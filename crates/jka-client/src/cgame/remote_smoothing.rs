//! Live-only remote-player and spectator-follow *visual* sampling. No prediction, collisions, event
//! discovery, server timing, demos or protocol fields are changed here.
//!
//! Mode 0 is the unmodified OpenJK-style CGame presentation path. Modes 1-4
//! retain a small snapshot ring (including snapshots already received but not
//! yet transitioned by CGame) to supply actual past states for visual sampling.
//! ET: Legacy's cl_interpolation motivated a snapshot history; jaMME's demo
//! command-history smoother needs extended demo records not present on live servers.
//! Mode 4 is an *experiment* using live playerState commandTime gaps to
//! shape interpolation tangents (with fallback), NOT an exact jaMME port.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Instant;

use jka_protocol::{gamestate::EntityState, server::{PlayerState, Snapshot}};

use super::{evaluate_trajectory_at, field_i32, lerp_angle, trajectory, PresentedEntity,
    Trajectory, EF_TELEPORT_BIT, ET_PLAYER, SNAPFLAG_SERVERCOUNT, TR_INTERPOLATE};

const MAX_PLAYERS: usize = 32;
const HISTORY: usize = 12;
const HARD_SNAP_UNITS: f32 = 256.0;
const CORRECTION_LIMIT: f32 = 32.0;

// Session-only A/B controls: no config writing, no runtime environment variables,
// no network userinfo, no shader/renderer work in mode 0.
static MODE: AtomicU8 = AtomicU8::new(0);
static BUFFER_MS: AtomicU32 = AtomicU32::new(50);
static BUFFER_AUTO: AtomicU8 = AtomicU8::new(0);
static EXTRAP_MAX_MS: AtomicU32 = AtomicU32::new(35);
static DEBUG: AtomicU8 = AtomicU8::new(0);

pub(crate) fn mode() -> u8 { MODE.load(Ordering::Relaxed) }
pub(crate) fn set_mode(value: u8) { MODE.store(value.min(4), Ordering::Relaxed); }
pub(crate) fn buffer_ms() -> u32 { BUFFER_MS.load(Ordering::Relaxed) }
pub(crate) fn set_buffer_ms(value: u32) { BUFFER_MS.store(value.min(100), Ordering::Relaxed); }
pub(crate) fn buffer_auto() -> bool { BUFFER_AUTO.load(Ordering::Relaxed) != 0 }
pub(crate) fn set_buffer_auto(value: bool) { BUFFER_AUTO.store(value.into(), Ordering::Relaxed); }
pub(crate) fn extrap_max_ms() -> u32 { EXTRAP_MAX_MS.load(Ordering::Relaxed) }
pub(crate) fn set_extrap_max_ms(value: u32) { EXTRAP_MAX_MS.store(value.min(100), Ordering::Relaxed); }
pub(crate) fn debug() -> u8 { DEBUG.load(Ordering::Relaxed) }
pub(crate) fn set_debug(value: u8) { DEBUG.store(value.min(2), Ordering::Relaxed); }

#[derive(Clone)]
struct Pose {
    state: EntityState,
    origin: [f32; 3],
    angles: [f32; 3],
    velocity: [f32; 3],
    flags: i32,
    ground: i32,
    vehicle: i32,
}

impl Pose {
    fn new(state: &EntityState, time: i32) -> Option<Self> {
        let pos = trajectory(state, "pos");
        let apos = trajectory(state, "apos");
        let origin = evaluate_trajectory_at(pos, f64::from(time)).ok()?;
        let angles = evaluate_trajectory_at(apos, f64::from(time)).ok()?;
        origin.iter().chain(angles.iter()).chain(pos.delta.iter()).all(|n| n.is_finite())
            .then(|| Self {
                state: state.clone(), origin, angles, velocity: pos.delta,
                flags: field_i32(state, "eFlags"),
                ground: field_i32(state, "groundEntityNum"),
                vehicle: field_i32(state, "m_iVehicleNum"),
            })
    }
}

struct Frame {
    time: i32,
    message: i32,
    flags: u8,
    poses: [Option<Pose>; MAX_PLAYERS],
    follow: Option<FollowPose>,
    // Only movers supporting observed remote players are retained. This is
    // needed only when estimating platform translation during an underrun.
    movers: Vec<(u16, Trajectory)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Source { #[default] None, Interpolated, Extrapolated, Held, Discontinuity }

// The followed player's *snapshot playerState* is not a packet entity. It is
// usually omitted from snapshot.entities; therefore the ordinary remote-player
// ring cannot drive a spectator camera. Retain the POV PS separately.
#[derive(Clone)]
struct FollowPose {
    origin: [f32; 3],
    angles: [f32; 3],
    velocity: [f32; 3],
    client: i32,
    spawn: i32,
    flags: i32,
    pm_flags: i32,
    pm_type: i32,
    ground: i32,
    vehicle: i32,
    command_time: i32,
}

impl FollowPose {
    fn new(ps: &PlayerState) -> Option<Self> {
        let vec3 = |names: [&str; 3]| -> Option<[f32; 3]> {
            let values = names.map(|name| ps.field_f32(name).unwrap_or(f32::NAN));
            values.iter().all(|value| value.is_finite()).then_some(values)
        };
        let pm_flags = ps.field_i32("pm_flags")?;
        if pm_flags & 4096 == 0 { return None; } // PMF_FOLLOW
        Some(Self {
            origin: vec3(["origin[0]", "origin[1]", "origin[2]"])?,
            angles: vec3(["viewangles[0]", "viewangles[1]", "viewangles[2]"])?,
            velocity: vec3(["velocity[0]", "velocity[1]", "velocity[2]"])?,
            client: ps.field_i32("clientNum")?,
            spawn: ps.persistant[4], flags: ps.field_i32("eFlags").unwrap_or(0),
            pm_flags, pm_type: ps.field_i32("pm_type").unwrap_or(0),
            ground: ps.field_i32("groundEntityNum").unwrap_or(0),
            vehicle: ps.field_i32("m_iVehicleNum").unwrap_or(0),
            command_time: ps.field_i32("commandTime").unwrap_or(0),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct FollowSample {
    origin: [f32; 3],
    angles: [f32; 3],
    velocity: [f32; 3],
    source: Source,
    continuous: bool,
    used_command_time: bool,
}

struct Sample {
    origin: [f32; 3],
    angles: [f32; 3],
    state: EntityState,
    velocity: [f32; 3],
    source: Source,
    continuous: bool,
}

#[derive(Clone, Copy, Default)]
struct VisualContinuity {
    source: Source,
    raw_origin: [f32; 3],
    velocity: [f32; 3],
    at_ms: f64,
    offset: [f32; 3],
    last_wall: Option<Instant>,
}

/// Remote clock is fractional and never used to stamp usercmds. Small corrections
/// to the legacy integer serverTime are spread over wall-clock time, so a -2ms
/// network delta adjustment needn't freeze every remote player for several frames.
#[derive(Default)]
struct VisualClock { at_ms: Option<f64>, last_wall: Option<Instant> }
impl VisualClock {
    fn sample(&mut self, server_ms: f64, now: Instant) -> f64 {
        let Some((previous, last)) = self.at_ms.zip(self.last_wall) else {
            self.at_ms = Some(server_ms);
            self.last_wall = Some(now);
            return server_ms;
        };
        let delta = now.saturating_duration_since(last).as_secs_f64() * 1000.0;
        // Disconnect / hitch / map_restart / demo seek: hard reset the visual clock.
        if delta > 200.0 || (server_ms - previous).abs() > 150.0 {
            self.at_ms = Some(server_ms);
            self.last_wall = Some(now);
            return server_ms;
        }
        let predicted = previous + delta;
        let correction = (server_ms - predicted) * (1.0 - (-delta / 80.0).exp());
        let sampled = (predicted + correction.clamp(-delta * 0.25, delta * 0.25)).max(previous);
        self.at_ms = Some(sampled);
        self.last_wall = Some(now);
        sampled
    }
}

#[derive(Default)]
struct Counters {
    interpolated: u64,
    extrapolated: u64,
    held: u64,
    discontinuities: u64,
    corrections: u64,
    max_correction: f32,
    underrun_ms: f64,
    underruns: u64,
}

pub(crate) struct RemoteSmoothing {
    frames: VecDeque<Frame>,
    clock: VisualClock,
    continuity: [VisualContinuity; MAX_PLAYERS],
    last_receive: Option<(Instant, i32)>,
    interval_ms: f64,
    arrival_jitter_ms: f64,
    last_mode: u8,
    counters: Counters,
    last_report: Option<Instant>,
    automatic_delay_ms: Option<f64>,
    last_delay_wall: Option<Instant>,
    last_visual_time: Option<f64>,
    follow_last_client: Option<(i32, i32)>, // clientNum, spawnCount
    follow_continuity: VisualContinuity,
    follow_samples: [u64; 4], // interpolated, extrapolated, held, command-time segments
    follow_skip_until: Option<i32>, // prevent rewinding across follow switch/teleport
}

impl Default for RemoteSmoothing {
    fn default() -> Self {
        Self { frames: VecDeque::new(), clock: VisualClock::default(),
            continuity: [VisualContinuity::default(); MAX_PLAYERS], last_receive: None,
            interval_ms: 50.0, arrival_jitter_ms: 0.0, last_mode: 0,
            counters: Counters::default(), last_report: None,
            automatic_delay_ms: None, last_delay_wall: None,
            last_visual_time: None, follow_last_client: None,
            follow_continuity: VisualContinuity::default(), follow_samples: [0; 4],
            follow_skip_until: None }
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn continuous(a: &Pose, b: &Pose, old: &Frame, new: &Frame) -> bool {
    (a.flags ^ b.flags) & EF_TELEPORT_BIT == 0
        && (old.flags ^ new.flags) & SNAPFLAG_SERVERCOUNT == 0
        && a.vehicle == b.vehicle
        && distance(a.origin, b.origin) <= HARD_SNAP_UNITS
}

fn follow_continuous(a: &FollowPose, b: &FollowPose, old: &Frame, new: &Frame) -> bool {
    a.client == b.client && a.spawn == b.spawn && a.vehicle == b.vehicle
        && a.pm_flags & 4096 != 0 && b.pm_flags & 4096 != 0
        && a.pm_type == b.pm_type
        && (a.flags ^ b.flags) & EF_TELEPORT_BIT == 0
        && (old.flags ^ new.flags) & SNAPFLAG_SERVERCOUNT == 0
        && distance(a.origin, b.origin) <= HARD_SNAP_UNITS
}

fn angle_delta(a: f32, b: f32) -> f32 { (b - a + 180.0).rem_euclid(360.0) - 180.0 }

fn hermite_follow(a: &FollowPose, b: &FollowPose, fraction: f32, dt: f32) -> [f32; 3] {
    let t = fraction.clamp(0.0, 1.0);
    let h00 = 2.0 * t.powi(3) - 3.0 * t * t + 1.0;
    let h10 = t.powi(3) - 2.0 * t * t + t;
    let h01 = -2.0 * t.powi(3) + 3.0 * t * t;
    let h11 = t.powi(3) - t * t;
    std::array::from_fn(|i| {
        let d = b.origin[i] - a.origin[i];
        let max_tangent = 2.0 * d.abs();
        let m0 = (a.velocity[i] * dt).clamp(-max_tangent, max_tangent);
        let m1 = (b.velocity[i] * dt).clamp(-max_tangent, max_tangent);
        (h00 * a.origin[i] + h10 * m0 + h01 * b.origin[i] + h11 * m1)
            .clamp(a.origin[i].min(b.origin[i]), a.origin[i].max(b.origin[i]))
    })
}

/// Tangent-limited cubic Hermite. Velocity is JKA units/second; dt is seconds.
/// The component-wise segment clamp prevents moving beyond both authoritative
/// endpoints (and into geometry) on abrupt WA/WD reversals or bad velocities.
fn hermite(a: &Pose, b: &Pose, fraction: f32, dt: f32) -> [f32; 3] {
    let t = fraction.clamp(0.0, 1.0);
    let h00 = 2.0 * t * t * t - 3.0 * t * t + 1.0;
    let h10 = t * t * t - 2.0 * t * t + t;
    let h01 = -2.0 * t * t * t + 3.0 * t * t;
    let h11 = t * t * t - t * t;
    std::array::from_fn(|i| {
        let d = b.origin[i] - a.origin[i];
        let bound = 2.0 * d.abs();
        let m0 = (a.velocity[i] * dt).clamp(-bound, bound);
        let m1 = (b.velocity[i] * dt).clamp(-bound, bound);
        (h00 * a.origin[i] + h10 * m0 + h01 * b.origin[i] + h11 * m1)
            .clamp(a.origin[i].min(b.origin[i]), a.origin[i].max(b.origin[i]))
    })
}

impl RemoteSmoothing {
    fn sync_mode(&mut self, selected: u8) {
        if self.last_mode == selected { return; }
        if selected == 0 { self.frames = VecDeque::new(); }
        else { self.frames.clear(); }
        self.clock = VisualClock::default();
        self.continuity.fill(VisualContinuity::default());
        self.last_receive = None;
        self.arrival_jitter_ms = 0.0;
        self.interval_ms = 50.0;
        self.counters = Counters::default();
        self.last_report = None;
        self.automatic_delay_ms = None;
        self.last_delay_wall = None;
        self.last_visual_time = None;
        self.follow_last_client = None;
        self.follow_continuity = VisualContinuity::default();
        self.follow_samples = [0; 4];
        self.follow_skip_until = None;
        self.last_mode = selected;
    }

    /// Invoked by the cvar setter as well as the frame loop: switching back to
    /// 0 drops historical allocations immediately and prevents stale frames
    /// surviving a 2 -> 0 -> 2 A/B sequence.
    pub(crate) fn sync_from_cvar(&mut self) { self.sync_mode(mode()); }

    /// Called when a decoded live snapshot arrives, NOT when the CGame's time
    /// eventually transitions to it. This preserves real arrival jitter.
    pub(crate) fn receive(&mut self, snapshot: &Snapshot, now: Instant) {
        self.sync_mode(mode());
        if self.last_mode == 0 || snapshot.snap_flags & (1 << 1) != 0 { return; }
        if self.frames.back().is_some_and(|last| last.message >= snapshot.message_num) { return; }
        if let Some((previous_wall, previous_server)) = self.last_receive {
            let wall_ms = now.saturating_duration_since(previous_wall).as_secs_f64() * 1000.0;
            let server_ms = f64::from(snapshot.server_time - previous_server);
            if server_ms > 0.0 && server_ms < 1000.0 {
                self.interval_ms = self.interval_ms * 0.9 + server_ms * 0.1;
                self.arrival_jitter_ms = self.arrival_jitter_ms * 0.9
                    + (wall_ms - server_ms).abs().min(200.0) * 0.1;
            }
        }
        self.last_receive = Some((now, snapshot.server_time));
        self.insert_snapshot(snapshot);
    }

    // Never mistake historical snapshots primed on a cvar toggle for a burst
    // of packets that actually arrived simultaneously over the network.
    fn insert_snapshot(&mut self, snapshot: &Snapshot) {
        if snapshot.snap_flags & (1 << 1) != 0 { return; }
        if self.frames.back().is_some_and(|last| last.message >= snapshot.message_num) { return; }
        if self.frames.back().is_some_and(|last| last.time >= snapshot.server_time
            || (last.flags ^ snapshot.snap_flags) & SNAPFLAG_SERVERCOUNT != 0) {
            // Map restart/servercount, or a new timeline. Don't interpolate across it.
            self.frames.clear();
            self.continuity.fill(VisualContinuity::default());
            self.follow_continuity = VisualContinuity::default();
            self.follow_last_client = None;
            self.follow_skip_until = None;
            self.last_visual_time = None;
            self.clock = VisualClock::default();
            self.last_receive = None;
        }
        let mut poses: [Option<Pose>; MAX_PLAYERS] = std::array::from_fn(|_| None);
        let own = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
        for entity in &snapshot.entities {
            let id = usize::from(entity.number);
            if id < MAX_PLAYERS && i32::from(entity.number) != own
                && field_i32(entity, "eType") == ET_PLAYER {
                poses[id] = Pose::new(entity, snapshot.server_time);
            }
        }
        let follow = FollowPose::new(&snapshot.player_state);
        let mut movers = Vec::new();
        for entity in &snapshot.entities {
            if field_i32(entity, "eType") == super::ET_MOVER
                && (poses.iter().any(|pose| pose.as_ref().is_some_and(|p| p.ground == i32::from(entity.number)))
                    || follow.as_ref().is_some_and(|p| p.ground == i32::from(entity.number))) {
                movers.push((entity.number, trajectory(entity, "pos")));
            }
        }
        if self.frames.len() == HISTORY { self.frames.pop_front(); }
        self.frames.push_back(Frame { time: snapshot.server_time, message: snapshot.message_num,
            flags: snapshot.snap_flags, poses, follow, movers });
    }

    /// Cvars can be enabled mid-session, after snapshots were received while
    /// mode 0 intentionally did no buffering. Prime from existing snapshots.
    pub(crate) fn prime(&mut self, current: Option<&Snapshot>, next: Option<&Snapshot>,
                        queue: &VecDeque<Snapshot>) {
        self.sync_mode(mode());
        if self.last_mode == 0 || !self.frames.is_empty() { return; }
        if let Some(snapshot) = current { self.insert_snapshot(snapshot); }
        if let Some(snapshot) = next { self.insert_snapshot(snapshot); }
        for snapshot in queue { self.insert_snapshot(snapshot); }
    }

    fn requested_delay_ms(&self) -> f64 {
        if buffer_auto() {
            (self.interval_ms + self.arrival_jitter_ms * 1.5).clamp(0.0, 100.0)
        } else { f64::from(buffer_ms()) }
    }

    fn buffer_delay_ms(&mut self, now: Instant) -> f64 {
        let desired = self.requested_delay_ms();
        if !buffer_auto() {
            self.automatic_delay_ms = None;
            self.last_delay_wall = None;
            return desired;
        }
        let elapsed = self.last_delay_wall.map_or(0.0, |last|
            now.saturating_duration_since(last).as_secs_f64() * 1000.0).min(200.0);
        self.last_delay_wall = Some(now);
        let previous = self.automatic_delay_ms.unwrap_or(desired);
        // Raise the safety margin faster under jitter; release it gradually.
        // Changing EWMA estimates must never itself make the player jump.
        let time_constant = if desired > previous { 80.0 } else { 1000.0 };
        let delay = previous + (desired - previous) * (1.0 - (-elapsed / time_constant).exp());
        self.automatic_delay_ms = Some(delay);
        delay
    }

    fn sample_player(&self, id: usize, time: f64, selected: u8) -> Option<Sample> {
        let first = self.frames.front()?;
        let last = self.frames.back()?;
        if time < f64::from(first.time) { return None; }
        let mut a_idx = self.frames.len().saturating_sub(1);
        for (index, frame) in self.frames.iter().enumerate() {
            if f64::from(frame.time) > time {
                a_idx = index.checked_sub(1)?;
                break;
            }
        }
        let a = &self.frames[a_idx];
        let pose = a.poses[id].as_ref()?;
        if let Some(b) = self.frames.get(a_idx + 1) {
            let next_pose = b.poses[id].as_ref()?;
            if !continuous(pose, next_pose, a, b) {
                return Some(Sample { origin: next_pose.origin, angles: next_pose.angles,
                    state: next_pose.state.clone(), velocity: next_pose.velocity,
                    source: Source::Discontinuity, continuous: false });
            }
            let dt_ms = f64::from(b.time - a.time);
            if dt_ms <= 0.0 { return None; }
            let f = ((time - f64::from(a.time)) / dt_ms).clamp(0.0, 1.0) as f32;
            let origin = if selected >= 3 { hermite(pose, next_pose, f, dt_ms as f32 * 0.001) }
                else { std::array::from_fn(|axis| pose.origin[axis]
                    + f * (next_pose.origin[axis] - pose.origin[axis])) };
            return Some(Sample { origin, angles: std::array::from_fn(|axis|
                    lerp_angle(pose.angles[axis], next_pose.angles[axis], f)),
                state: pose.state.clone(), velocity: pose.velocity,
                source: Source::Interpolated, continuous: true });
        }
        let remaining = (time - f64::from(last.time)).max(0.0);
        let allowed = selected >= 2 && remaining <= f64::from(extrap_max_ms());
        let mut origin = pose.origin;
        if allowed {
            let dt = (remaining * 0.001) as f32;
            // Do not extrapolate a non-kinematic or invalid position. Vanilla
            // TR_INTERPOLATE still carries the server velocity in trDelta.
            let movement = trajectory(&pose.state, "pos");
            if movement.kind == TR_INTERPOLATE {
                // TR_INTERPOLATE evaluates to a constant base but still carries
                // the authoritative player velocity in trDelta.
                for axis in 0..3 { origin[axis] += pose.velocity[axis] * dt; }
            } else if let Ok(predicted) = evaluate_trajectory_at(movement, time) {
                // TR_LINEAR_STOP is bounded by its authored stop time; unlike
                // naive velocity integration, this never runs past that stop.
                for axis in 0..3 { origin[axis] = predicted[axis]; }
            }
            if pose.ground > 0 {
                if let Some((_, mover)) = a.movers.iter().find(|(num, _)| i32::from(*num) == pose.ground) {
                    if let (Ok(start), Ok(end)) = (evaluate_trajectory_at(*mover, f64::from(a.time)),
                        evaluate_trajectory_at(*mover, time)) {
                        for axis in 0..3 { origin[axis] += end[axis] - start[axis]; }
                    }
                }
            }
        }
        Some(Sample { origin, angles: pose.angles, state: pose.state.clone(),
            velocity: pose.velocity, source: if allowed { Source::Extrapolated } else { Source::Held },
            continuous: true })
    }

    /// Called strictly after local prediction consumed the *unmodified*
    /// PresentedEntity slice. Everything visual (Ghoul2, sabers, shadows,
    /// spatial audio) then reads the same sampled origin/angles/state.
    pub(crate) fn apply(&mut self, game_ms: f64, now: Instant,
                                entities: &mut [PresentedEntity]) {
        let selected = mode();
        self.sync_mode(selected);
        if selected == 0 || self.frames.is_empty() { return; }
        let time = self.clock.sample(game_ms, now) - self.buffer_delay_ms(now);
        // Sampled once per presentation frame. The spectator POV must use the
        // exact same visual clock as every remote entity and attachment.
        self.last_visual_time = Some(time);
        for entity in entities.iter_mut().filter(|e| e.entity_type == ET_PLAYER
                && usize::from(e.number) < MAX_PLAYERS) {
            let id = usize::from(entity.number);
            // Mode 4 tests a different POV clock; ordinary remote packet
            // entities retain mode 2's linear + bounded extrapolation.
            let entity_mode = if selected == 4 { 2 } else { selected };
            let Some(mut sample) = self.sample_player(id, time, entity_mode) else { continue; };
            if !sample.origin.iter().chain(sample.angles.iter()).all(|n| n.is_finite()) { continue; }
            let previous = &mut self.continuity[id];
            if !sample.continuous {
                *previous = VisualContinuity::default();
            } else {
                let dt = previous.last_wall.map_or(0.0, |wall|
                    now.saturating_duration_since(wall).as_secs_f32() * 1000.0).min(200.0);
                for value in &mut previous.offset { *value *= (-dt / 65.0).exp(); }
                if matches!(previous.source, Source::Extrapolated | Source::Held)
                    && sample.source == Source::Interpolated {
                    // Continue last extrapolation to this same sample time,
                    // then correct its bounded error without a one-frame pop.
                    let elapsed = ((time - previous.at_ms) * 0.001).clamp(0.0, 0.1) as f32;
                    let guess: [f32; 3] = std::array::from_fn(|axis|
                        previous.raw_origin[axis] + if previous.source == Source::Extrapolated {
                            previous.velocity[axis] * elapsed
                        } else { 0.0 });
                    let err = distance(guess, sample.origin);
                    if err > 0.01 && err <= CORRECTION_LIMIT {
                        for axis in 0..3 { previous.offset[axis] += guess[axis] - sample.origin[axis]; }
                        self.counters.corrections += 1;
                        self.counters.max_correction = self.counters.max_correction.max(err);
                    } else if err > CORRECTION_LIMIT { previous.offset = [0.0; 3]; }
                }
                // Never preserve a stale correction through a large visual jump.
                if previous.offset.iter().map(|v| v * v).sum::<f32>().sqrt() > CORRECTION_LIMIT {
                    previous.offset = [0.0; 3];
                }
                for axis in 0..3 { sample.origin[axis] += previous.offset[axis]; }
            }
            match sample.source {
                Source::Interpolated => self.counters.interpolated += 1,
                Source::Extrapolated => { self.counters.extrapolated += 1;
                    self.counters.underruns += 1;
                    self.counters.underrun_ms += (time - f64::from(self.frames.back().unwrap().time)).max(0.0);
                }
                Source::Held => { self.counters.held += 1; self.counters.underruns += 1;
                    self.counters.underrun_ms += (time - f64::from(self.frames.back().unwrap().time)).max(0.0);
                }
                Source::Discontinuity => self.counters.discontinuities += 1,
                Source::None => {}
            }
            // Keep correction bookkeeping in *unsmoothed* world space.
            previous.raw_origin = std::array::from_fn(|axis| sample.origin[axis] - previous.offset[axis]);
            previous.at_ms = time;
            previous.velocity = sample.velocity;
            previous.source = sample.source;
            previous.last_wall = Some(now);
            entity.origin = sample.origin;
            entity.angles = sample.angles;
            entity.state = sample.state;
        }
    }

    /// Prefer server time for modes 1-3. Mode 4 adapts jaMME's history
    /// idea to ordinary live snapshots, using valid commandTime intervals to
    /// scale Hermite velocity tangents. It cannot reconstruct extra usercmd
    /// states never sent by the server.
    fn sample_follow(&self, visual_ms: f64, target: i32, selected: u8) -> Option<FollowSample> {
        let first = self.frames.front()?;
        if visual_ms < f64::from(first.time) { return None; }
        let mut index = self.frames.len() - 1;
        for (i, frame) in self.frames.iter().enumerate() {
            if f64::from(frame.time) > visual_ms {
                index = i.checked_sub(1)?;
                break;
            }
        }
        let old = &self.frames[index];
        let a = old.follow.as_ref()?;
        if a.client != target { return None; }
        if let Some(new) = self.frames.get(index + 1) {
            let b = new.follow.as_ref()?;
            if b.client != target { return None; }
            if !follow_continuous(a, b, old, new) {
                // Never jump early to a future teleport or POV switch. Stay
                // at the old position until the new snapshot is due.
                return Some(FollowSample { origin: a.origin, angles: a.angles,
                    velocity: a.velocity, source: Source::Discontinuity,
                    continuous: false, used_command_time: false });
            }
            let elapsed = f64::from(new.time - old.time);
            if elapsed <= 0.0 { return None; }
            let server_f = ((visual_ms - f64::from(old.time)) / elapsed).clamp(0.0, 1.0);
            let command_interval = f64::from(b.command_time - a.command_time);
            // jaMME skips duplicate/empty command-history slots. Ordinary live
            // JKA has only one playerState per snapshot, so we cannot recover
            // those intermediate slots. We CAN use commandTime to estimate
            // each endpoint's velocity-tangent duration. Always evaluate the
            // curve at server-time fraction: both endpoints must remain exact
            // and continuous as the render clock crosses snapshot boundaries.
            let usable_command = selected == 4
                && command_interval >= 4.0 && command_interval <= 200.0
                && (command_interval - elapsed).abs() <= 35.0
                && a.command_time > 0;
            let f = server_f as f32;
            let origin = if selected == 3 || (selected == 4 && usable_command) {
                hermite_follow(a, b, f, if usable_command {
                    command_interval as f32 * 0.001
                } else {
                    elapsed as f32 * 0.001
                })
            } else {
                std::array::from_fn(|axis| a.origin[axis] + f * (b.origin[axis] - a.origin[axis]))
            };
            return Some(FollowSample {
                origin,
                angles: std::array::from_fn(|axis| lerp_angle(a.angles[axis], b.angles[axis], f)),
                velocity: std::array::from_fn(|axis| a.velocity[axis] + f * (b.velocity[axis] - a.velocity[axis])),
                source: Source::Interpolated, continuous: true,
                used_command_time: usable_command,
            });
        }
        // An underrun. Mode 1 holds. Mode 2/3/4 predicts only a short,
        // strictly limited visual interval; then it HOLDS THE CAPPED POSITION
        // (rather than snapping backwards to the last server pose).
        let elapsed = (visual_ms - f64::from(old.time)).max(0.0);
        let extrap = if selected >= 2 { elapsed.min(f64::from(extrap_max_ms())) } else { 0.0 };
        let mut origin: [f32; 3] = std::array::from_fn(|axis| a.origin[axis] + a.velocity[axis] * (extrap as f32 * 0.001));
        if extrap > 0.0 && a.ground > 0 {
            if let Some((_, mover)) = old.movers.iter().find(|(id, _)| i32::from(*id) == a.ground) {
                if let (Ok(start), Ok(end)) = (
                    evaluate_trajectory_at(*mover, f64::from(old.time)),
                    evaluate_trajectory_at(*mover, f64::from(old.time) + extrap),
                ) {
                    for axis in 0..3 { origin[axis] += end[axis] - start[axis]; }
                }
            }
        }
        let mut angles = a.angles;
        if extrap > 0.0 && index > 0 {
            if let Some(before) = self.frames[index - 1].follow.as_ref() {
                let previous = &self.frames[index - 1];
                let delta = f64::from(old.time - previous.time);
                if delta > 0.0 && delta <= 150.0 && follow_continuous(before, a, previous, old) {
                    for axis in 0..3 {
                        let turn = angle_delta(before.angles[axis], a.angles[axis]);
                        let rate = (turn / delta as f32).clamp(-0.72, 0.72); // degrees/ms
                        angles[axis] += rate * extrap as f32;
                    }
                }
            }
        }
        Some(FollowSample { origin, angles, velocity: a.velocity,
            source: if extrap > 0.0 && elapsed <= extrap { Source::Extrapolated } else { Source::Held },
            continuous: true, used_command_time: false })
    }

    /// Called after the original playerState has been chosen and input
    /// ownership resolved, but before its camera, Ghoul2 body, sabers, audio,
    /// and shadows are derived. Never change server gameplay/discrete fields.
    pub(crate) fn apply_follow(&mut self, ps: &mut PlayerState, now: Instant, teleported: bool) {
        let selected = mode();
        if selected == 0 || teleported {
            if teleported {
                // The old buffered timeline contains the position from before
                // the teleport. Never rewind the camera into it next frame.
                self.follow_skip_until = self.frames.back().map(|f| f.time);
            } else {
                self.follow_skip_until = None;
            }
            self.follow_last_client = None;
            self.follow_continuity = VisualContinuity::default();
            return;
        }
        let Some(current) = FollowPose::new(ps) else {
            self.follow_last_client = None;
            self.follow_skip_until = None;
            self.follow_continuity = VisualContinuity::default();
            return;
        };
        let owner = (current.client, current.spawn);
        if self.follow_last_client != Some(owner) {
            if self.follow_last_client.is_some() {
                self.follow_skip_until = self.frames.back().map(|f| f.time);
            }
            self.follow_last_client = Some(owner);
            self.follow_continuity = VisualContinuity::default();
        }
        let Some(time) = self.last_visual_time else { return; };
        if let Some(block) = self.follow_skip_until {
            if time < f64::from(block) { return; }
            self.follow_skip_until = None;
        }
        let Some(mut sampled) = self.sample_follow(time, current.client, selected) else { return; };
        if !sampled.continuous {
            self.follow_continuity = VisualContinuity::default();
        } else {
            let previous = &mut self.follow_continuity;
            let dt = previous.last_wall.map_or(0.0, |last|
                now.saturating_duration_since(last).as_secs_f32() * 1000.0).min(200.0);
            for component in &mut previous.offset { *component *= (-dt / 65.0).exp(); }
            if matches!(previous.source, Source::Extrapolated | Source::Held)
                && sampled.source == Source::Interpolated {
                let elapsed = ((time - previous.at_ms) * 0.001).clamp(0.0, 0.1) as f32;
                let guess: [f32; 3] = std::array::from_fn(|axis|
                    previous.raw_origin[axis] + if previous.source == Source::Extrapolated {
                        previous.velocity[axis] * elapsed
                    } else { 0.0 });
                let error = distance(guess, sampled.origin);
                if error > 0.01 && error <= CORRECTION_LIMIT {
                    for axis in 0..3 { previous.offset[axis] += guess[axis] - sampled.origin[axis]; }
                } else if error > CORRECTION_LIMIT {
                    previous.offset = [0.0; 3];
                }
            }
            if previous.offset.iter().map(|x| x * x).sum::<f32>() > CORRECTION_LIMIT.powi(2) {
                previous.offset = [0.0; 3];
            }
            for axis in 0..3 { sampled.origin[axis] += previous.offset[axis]; }
        }
        let previous = &mut self.follow_continuity;
        previous.raw_origin = std::array::from_fn(|axis| sampled.origin[axis] - previous.offset[axis]);
        previous.velocity = sampled.velocity;
        previous.at_ms = time;
        previous.source = sampled.source;
        previous.last_wall = Some(now);
        match sampled.source {
            Source::Interpolated => self.follow_samples[0] += 1,
            Source::Extrapolated => self.follow_samples[1] += 1,
            Source::Held => self.follow_samples[2] += 1,
            _ => {},
        }
        if sampled.used_command_time { self.follow_samples[3] += 1; }
        for (axis, name) in ["origin[0]", "origin[1]", "origin[2]"].iter().enumerate() {
            ps.set_field_bits(name, sampled.origin[axis].to_bits());
        }
        for (axis, name) in ["viewangles[0]", "viewangles[1]", "viewangles[2]"].iter().enumerate() {
            ps.set_field_bits(name, sampled.angles[axis].to_bits());
        }
        for (axis, name) in ["velocity[0]", "velocity[1]", "velocity[2]"].iter().enumerate() {
            ps.set_field_bits(name, sampled.velocity[axis].to_bits());
        }
    }

    pub(crate) fn summary(&self) -> String {
        let c = &self.counters;
        let total = c.interpolated + c.extrapolated + c.held;
        let rate = |count| if total == 0 { 0.0 } else { count as f64 * 100.0 / total as f64 };
        let follow = self.follow_samples;
        format!("[REMOTE SMOOTH] mode={} buffer={:.1}ms (auto={}) extrapMax={}ms history={} interval={:.1}ms arrivalJitter={:.1}ms samples={} interpolate={:.1}% extrapolate={:.1}% hold={:.1}% discontinuities={} correctionCount={} worstCorrection={:.2}u meanUnderrun={:.1}ms follow[interp={} extrap={} hold={} commandTime={}]",
            mode(), self.automatic_delay_ms.unwrap_or_else(|| self.requested_delay_ms()),
            buffer_auto() as u8, extrap_max_ms(),
            self.frames.len(), self.interval_ms, self.arrival_jitter_ms, total,
            rate(c.interpolated), rate(c.extrapolated), rate(c.held), c.discontinuities,
            c.corrections, c.max_correction,
            if c.underruns == 0 { 0.0 } else { c.underrun_ms / c.underruns as f64 },
            follow[0], follow[1], follow[2], follow[3])
    }

    pub(crate) fn report_if_enabled(&mut self, now: Instant) {
        if debug() == 0 || mode() == 0 { return; }
        if self.last_report.is_some_and(|last| now.saturating_duration_since(last).as_secs_f32() < 1.0) { return; }
        self.last_report = Some(now);
        crate::logging::write_console_line(crate::logging::ConsoleLineKind::Normal,
            format_args!("{}", self.summary()));
        if debug() >= 2 {
            for (id, pose) in self.continuity.iter().enumerate().filter(|(_, p)| p.source != Source::None) {
                crate::logging::write_console_line(crate::logging::ConsoleLineKind::Normal,
                    format_args!("[REMOTE SMOOTH] player={} state={:?} correction={:.2}u", id, pose.source,
                        pose.offset.iter().map(|v| v * v).sum::<f32>().sqrt()));
            }
        }
        self.counters = Counters::default();
        self.follow_samples = [0; 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_protocol::{gamestate::ENTITY_FIELDS, server::PlayerState};

    fn entity(x: f32, vel: f32, flags: i32) -> EntityState {
        let mut e = EntityState { number: 2, fields: [0; ENTITY_FIELDS.len()] };
        for (name, bits) in [("eType", ET_PLAYER as u32), ("eFlags", flags as u32),
            ("pos.trType", TR_INTERPOLATE as u32), ("pos.trBase[0]", x.to_bits()),
            ("pos.trDelta[0]", vel.to_bits())] {
            let index = ENTITY_FIELDS.iter().position(|(candidate, _)| *candidate == name).unwrap();
            e.fields[index] = bits;
        }
        e
    }
    fn snapshot(time: i32, msg: i32, x: f32, velocity: f32, flags: i32) -> Snapshot {
        Snapshot { server_time: time, message_num: msg, delta_num: -1, snap_flags: 0, ping: 0,
            server_command_num: 0, area_mask: [0; 32], player_state: PlayerState::default(),
            vehicle_player_state: None, entities: vec![entity(x, velocity, flags)] }
    }
    fn test_smoother() -> RemoteSmoothing {
        RemoteSmoothing::default()
    }
    #[test]
    fn fractional_clock_keeps_advancing_with_integer_game_time() {
        let now = Instant::now();
        let mut clock = VisualClock::default();
        let first = clock.sample(100.0, now);
        let next = clock.sample(100.0, now + std::time::Duration::from_micros(400));
        assert!(next > first, "high-FPS frames must not repeat integer timestamps");
        assert!(next < first + 1.0, "sub-ms progression should stay sub-ms");
    }
    #[test]
    fn linear_stop_extrapolation_respects_trajectory_end() {
        let mut remote = test_smoother();
        let mut frame = snapshot(100, 1, 0.0, 100.0, 0);
        let state = &mut frame.entities[0];
        for (name, value) in [("pos.trType", super::super::TR_LINEAR_STOP as u32),
            ("pos.trTime", 100), ("pos.trDuration", 10)] {
            let index = ENTITY_FIELDS.iter().position(|(candidate, _)| *candidate == name).unwrap();
            state.fields[index] = value;
        }
        remote.insert_snapshot(&frame);
        let at = remote.sample_player(2, 125.0, 2).unwrap();
        assert_eq!(at.source, Source::Extrapolated);
        assert!((at.origin[0] - 1.0).abs() < 0.01);
    }
    #[test]
    fn interpolation_and_bounded_extrapolation() {
        let mut remote = test_smoother();
        remote.insert_snapshot(&snapshot(100, 1, 0.0, 100.0, 0));
        remote.insert_snapshot(&snapshot(150, 2, 5.0, 100.0, 0));
        let mid = remote.sample_player(2, 125.0, 2).unwrap();
        assert_eq!(mid.source, Source::Interpolated);
        assert!((mid.origin[0] - 2.5).abs() < 0.01);
        let extrap = remote.sample_player(2, 175.0, 2).unwrap();
        assert_eq!(extrap.source, Source::Extrapolated);
        assert!((extrap.origin[0] - 7.5).abs() < 0.01);
        assert_eq!(remote.sample_player(2, 210.0, 2).unwrap().source, Source::Held);
    }
    #[test]
    fn teleports_never_lerp_or_hermite_across_positions() {
        let mut remote = test_smoother();
        remote.insert_snapshot(&snapshot(100, 1, 0.0, 200.0, 0));
        remote.insert_snapshot(&snapshot(150, 2, 450.0, 0.0, EF_TELEPORT_BIT));
        let result = remote.sample_player(2, 125.0, 3).unwrap();
        assert_eq!(result.source, Source::Discontinuity);
        assert_eq!(result.origin[0], 450.0);
    }
    #[test]
    fn disappearance_does_not_bridge_missing_pvs_snapshots() {
        let mut remote = test_smoother();
        remote.insert_snapshot(&snapshot(100, 1, 0.0, 50.0, 0));
        let mut absent = snapshot(150, 2, 5.0, 50.0, 0);
        absent.entities.clear();
        remote.insert_snapshot(&absent);
        remote.insert_snapshot(&snapshot(200, 3, 10.0, 50.0, 0));
        assert!(remote.sample_player(2, 125.0, 3).is_none());
        assert!(remote.sample_player(2, 175.0, 3).is_none());
    }
    #[test]
    fn server_restart_discards_old_timeline() {
        let mut remote = test_smoother();
        remote.insert_snapshot(&snapshot(100, 1, 0.0, 0.0, 0));
        let mut restarted = snapshot(50, 2, 500.0, 0.0, 0);
        restarted.snap_flags = SNAPFLAG_SERVERCOUNT;
        remote.insert_snapshot(&restarted);
        assert_eq!(remote.frames.len(), 1);
        assert!(remote.sample_player(2, 100.0, 2).is_some());
        assert!(remote.sample_player(2, 49.0, 2).is_none());
    }
    #[test]
    fn mover_translation_only_applies_after_the_last_snapshot() {
        let mut remote = test_smoother();
        let mut frame = snapshot(100, 1, 10.0, 0.0, 0);
        let p = frame.entities.get_mut(0).unwrap();
        let ground = ENTITY_FIELDS.iter().position(|(name, _)| *name == "groundEntityNum").unwrap();
        p.fields[ground] = 5;
        let mut mover = EntityState { number: 5, fields: [0; ENTITY_FIELDS.len()] };
        for (name, bits) in [("eType", super::super::ET_MOVER as u32),
            ("pos.trType", super::super::TR_LINEAR as u32), ("pos.trTime", 100),
            ("pos.trDelta[0]", 50f32.to_bits())] {
            let index = ENTITY_FIELDS.iter().position(|(candidate, _)| *candidate == name).unwrap();
            mover.fields[index] = bits;
        }
        frame.entities.push(mover);
        remote.insert_snapshot(&frame);
        let at = remote.sample_player(2, 120.0, 2).unwrap();
        assert!((at.origin[0] - 11.0).abs() < 0.01);
    }
    fn follow_snapshot(time: i32, msg: i32, target: i32, x: f32,
                       velocity: f32, command_time: i32) -> Snapshot {
        let mut frame = snapshot(time, msg, x, velocity, 0);
        frame.entities.clear(); // followed player is sent as playerState, not entityState
        let ps = &mut frame.player_state;
        for (name, bits) in [("pm_flags", 4096), ("clientNum", target as u32),
            ("commandTime", command_time as u32),
            ("origin[0]", x.to_bits()), ("velocity[0]", velocity.to_bits())] {
            ps.set_field_bits(name, bits);
        }
        frame
    }

    #[test]
    fn spectator_follow_reads_playerstate_not_packet_entity() {
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 95));
        smooth.insert_snapshot(&follow_snapshot(150, 2, 3, 5.0, 100.0, 145));
        let mid = smooth.sample_follow(125.0, 3, 1).unwrap();
        assert_eq!(mid.source, Source::Interpolated);
        assert!((mid.origin[0] - 2.5).abs() < 0.01);
        assert!(smooth.sample_player(3, 125.0, 2).is_none());
    }

    #[test]
    fn spectator_extrapolation_caps_position_without_snapping_back() {
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 95));
        smooth.insert_snapshot(&follow_snapshot(150, 2, 3, 5.0, 100.0, 145));
        let predicted = smooth.sample_follow(160.0, 3, 2).unwrap();
        assert_eq!(predicted.source, Source::Extrapolated);
        assert!((predicted.origin[0] - 6.0).abs() < 0.01);
        let capped = smooth.sample_follow(235.0, 3, 2).unwrap();
        assert_eq!(capped.source, Source::Held);
        assert!((capped.origin[0] - 8.5).abs() < 0.01);
        assert_eq!(smooth.sample_follow(235.0, 3, 1).unwrap().origin[0], 5.0);
    }

    #[test]
    fn spectator_follow_switch_spawn_and_teleport_never_blend() {
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 95));
        smooth.insert_snapshot(&follow_snapshot(150, 2, 4, 50.0, 100.0, 145));
        assert!(smooth.sample_follow(125.0, 3, 4).is_none());
        assert!(smooth.sample_follow(125.0, 4, 4).is_none());
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 95));
        let mut teleported = follow_snapshot(150, 2, 3, 500.0, 100.0, 145);
        teleported.player_state.set_field_bits("eFlags", EF_TELEPORT_BIT as u32);
        smooth.insert_snapshot(&teleported);
        let mid = smooth.sample_follow(125.0, 3, 3).unwrap();
        assert_eq!(mid.source, Source::Discontinuity);
        assert_eq!(mid.origin[0], 0.0); // stay before jump until snapshot is due
        assert_eq!(smooth.sample_follow(150.0, 3, 3).unwrap().origin[0], 500.0);
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 95));
        let mut respawn = follow_snapshot(150, 2, 3, 5.0, 100.0, 145);
        respawn.player_state.persistant[4] = 1;
        smooth.insert_snapshot(&respawn);
        assert_eq!(smooth.sample_follow(125.0, 3, 2).unwrap().source, Source::Discontinuity);
    }

    #[test]
    fn spectator_jamme_inspired_mode_preserves_endpoints_and_falls_back() {
        let mut smooth = test_smoother();
        let mut a = follow_snapshot(100, 1, 3, 0.0, 100.0, 95);
        a.player_state.set_field_bits("velocity[0]", 200.0f32.to_bits());
        let b = follow_snapshot(150, 2, 3, 10.0, 0.0, 135);
        smooth.insert_snapshot(&a);
        smooth.insert_snapshot(&b);
        let mid = smooth.sample_follow(125.0, 3, 4).unwrap();
        assert_eq!(mid.source, Source::Interpolated);
        assert!(mid.used_command_time);
        assert!((0.0..=10.0).contains(&mid.origin[0]));
        assert_eq!(smooth.sample_follow(100.0, 3, 4).unwrap().origin[0], 0.0);
        assert_eq!(smooth.sample_follow(150.0, 3, 4).unwrap().origin[0], 10.0);
        let mut smooth = test_smoother();
        smooth.insert_snapshot(&follow_snapshot(100, 1, 3, 0.0, 100.0, 0));
        smooth.insert_snapshot(&follow_snapshot(150, 2, 3, 10.0, 100.0, 0));
        let fallback = smooth.sample_follow(125.0, 3, 4).unwrap();
        assert!(!fallback.used_command_time);
        assert!((fallback.origin[0] - 5.0).abs() < 0.01);
    }

    #[test]
    fn hermite_is_bounded_on_reversal() {
        let a = Pose::new(&entity(0.0, -2000.0, 0), 100).unwrap();
        let b = Pose::new(&entity(10.0, 2000.0, 0), 150).unwrap();
        for t in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let p = hermite(&a, &b, t, 0.05);
            assert!((0.0..=10.0).contains(&p[0]));
        }
    }
}
