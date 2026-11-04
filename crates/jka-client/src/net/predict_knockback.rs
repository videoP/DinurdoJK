//! Speculative *self* rocket knockback for jaPRO racemode.
//!
//! TaystJK PR #344 behavior, using the same trajectory constructor as
//! `cg_simulatedProjectiles`, with bounded per-shot replay and server reconciliation.
//! There is one impact collision trace per new committed rocket; the cosmetic
//! missile renderer has no collision simulation, so it does not duplicate this.
//! Nothing runs when the option is off.

use jka_movement::{PlayerState as NativePlayerState, PlayerView, TraceQuery, TraceWorld};
use jka_protocol::{entity_event::{EntityEvent, EV_EVENT_BITS}, server::Snapshot};
use crate::cgame::weapon_fx::{
    local_projectile_muzzle, LocalMissileFlight, RACE_ROCKET_SPEED, RACE_ROCKET_PRESTEP_MS,
};

const WP_ROCKET_LAUNCHER: i32 = 11;
const ROCKET_RADIUS: f32 = 160.0;
const ROCKET_SIZE: f32 = 1.0; // TaystJK CG_AddMissile [-1,1]; collision trace matches its local-missile path.
const MAX_FLIGHT_MS: i32 = 700;
const MAX_PENDING_SHOTS: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Shot {
    command_number: i32,
    fire_time: i32,
    impact_time: i32,
    impact: [f32; 3],
    reported: bool,
    confirmed: bool,
}

#[derive(Default)]
pub(super) struct KnockbackPrediction {
    shots: Vec<Shot>,
    last_seen_command: i32,
    last_observed_snapshot: i32,
    enabled: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Hit {
    pub command_number: i32,
    pub impact_time: i32,
    pub impulse: [f32; 3],
    pub impact: [f32; 3],
    pub knockback: i32,
    pub pre_velocity: [f32; 3],
}

impl KnockbackPrediction {
    /// Disabling, leaving race, discontinuity or switching servers must never
    /// resurrect an old rocket when replay starts from a different snapshot.
    pub fn begin_frame(&mut self, active: bool, current_cmd: i32, teleported: bool, base_cmd_time: i32) {
        if !active || teleported || current_cmd < self.last_seen_command {
            self.shots.clear();
            self.last_seen_command = current_cmd;
            self.last_observed_snapshot = -1;
            self.enabled = active;
            return;
        }
        if !self.enabled {
            self.last_seen_command = current_cmd;
            self.enabled = true;
        }
        // An authoritative playerstate at/after the impact has already taken
        // responsibility for the impulse. Never replay it on top of that state.
        self.shots.retain(|shot| !shot.confirmed && shot.impact_time > base_cmd_time);
    }

    /// TaystJK EV_MISSILE_MISS / EV_MISSILE_MISS_METAL handoff: a server
    /// explosion owned by this player is authoritative. Match by position,
    /// never retire a different, overlapping rocket merely because its weapon
    /// type matches. Snapshot identity avoids reprocessing the event each frame.
    /// Return diagnostics only when requested, otherwise do not allocate.
    pub fn observe_server_explosions(&mut self, snap: &Snapshot, owner: i32, debug: u8) -> Vec<String> {
        if !self.enabled || snap.message_num <= self.last_observed_snapshot {
            return Vec::new();
        }
        self.last_observed_snapshot = snap.message_num;
        let mut messages = Vec::new();
        for entity in &snap.entities {
            if entity.field_i32("weapon") != Some(WP_ROCKET_LAUNCHER)
                || entity.field_i32("owner") != Some(owner) { continue; }
            let kind = entity.field_i32("eType").unwrap_or(-1);
            let event = if kind >= crate::cgame::ET_EVENTS {
                kind - crate::cgame::ET_EVENTS
            } else { entity.field_i32("event").unwrap_or(0) & !EV_EVENT_BITS };
            if event != EntityEvent::EV_MISSILE_MISS.as_i32()
                && event != EntityEvent::EV_MISSILE_MISS_METAL.as_i32() { continue; }
            let Some(position) = crate::cgame::evaluate_entity_trajectory(entity, "pos", snap.server_time) else {
                continue;
            };
            // Pick one speculative rocket, even when several are in flight.
            let matching = self.shots.iter().enumerate()
                .filter(|(_, shot)| !shot.confirmed && snap.server_time >= shot.fire_time)
                .min_by(|a, b| distance_sq(a.1.impact, position).total_cmp(&distance_sq(b.1.impact, position)));
            if let Some((index, shot)) = matching {
                let discrepancy = distance_sq(shot.impact, position).sqrt();
                // A far-away rocket belonging to the same client must not
                // accidentally clear a newer, unrelated speculative shot.
                if discrepancy > 192.0 { continue; }
                if debug >= 2 {
                    messages.push(format!(
                        "^6PK-SERVER^7 cmd={} predicted={:?} actual={:?} delta={:.1} predictedT={} serverT={}",
                        shot.command_number, shot.impact, position, discrepancy,
                        shot.impact_time, snap.server_time,
                    ));
                }
                self.shots[index].confirmed = true;
            }
        }
        messages
    }

    /// Take only the first encounter with an actual *committed* usercmd.
    /// Pmove adds EV_FIRE_WEAPON to eventSequence; unsent preview commands
    /// are intentionally excluded by the caller.
    pub fn note_command(&mut self, command_number: i32) -> bool {
        if command_number <= self.last_seen_command {
            return false;
        }
        self.last_seen_command = command_number;
        self.enabled
    }

    /// Reproduce the centered jaPRO race muzzle, snapped network trajectory,
    /// ballistic speed and 50ms missile prestep. Only solid impact
    /// candidates are stored; all distance/impulse evaluation occurs later,
    /// at the replayed player's actual predicted location near impact.
    pub fn note_rocket_fire(&mut self, command_number: i32, fire_time: i32, owner_client: i32, player: PlayerView, world: &mut impl TraceWorld) -> Option<(i32, [f32; 3])> {
        if player.weapon != WP_ROCKET_LAUNCHER || player.pm_type != 0 {
            return None;
        }
        // Race muzzle is centered in TaystJK and in our visual projectile
        // implementation. Both use identical AngleVectors, speed and prestep.
        let (mut muzzle, forward) = local_projectile_muzzle(
            player.origin, player.view_angles, player.view_height as f32,
            WP_ROCKET_LAUNCHER, true,
        );
        let flight = LocalMissileFlight::new(
            muzzle, forward, RACE_ROCKET_SPEED,
            fire_time.saturating_sub(RACE_ROCKET_PRESTEP_MS), false,
        ).snapped_to_network();
        muzzle = flight.origin;
        let velocity = flight.velocity;
        let end = flight.position(fire_time.saturating_add(MAX_FLIGHT_MS));
        let trace = world.trace(TraceQuery {
            start: muzzle, end,
            mins: [-ROCKET_SIZE; 3], maxs: [ROCKET_SIZE; 3],
            pass_entity: owner_client, mask: 1, // TaystJK CG_Trace: CONTENTS_SOLID.
        });
        // A point-blank rocket may start inside the wall. The server
        // still detonates it; do not discard startsolid/allsolid traces.
        if !trace.fraction.is_finite() || trace.fraction >= 1.0 {
            return None;
        }
        let impact = snap_towards(trace.end, muzzle);
        let distance = magnitude(sub(impact, muzzle));
        let actual_speed = magnitude(velocity);
        if !actual_speed.is_finite() || actual_speed <= 1.0 { return None; }
        let flight_ms = (distance / actual_speed * 1000.0) as i32;
        let impact_time = fire_time.saturating_add((flight_ms - RACE_ROCKET_PRESTEP_MS).max(0));
        self.shots.push(Shot { command_number, fire_time, impact_time, impact, reported: false, confirmed: false });
        if self.shots.len() > MAX_PENDING_SHOTS {
            self.shots.remove(0);
        }
        Some((impact_time, impact))
    }

    /// Called exactly once per replayed (or provisional) usercmd, after native
    /// Pmove. 'before' is the previous commandTime. This same post-Pmove point
    /// is used whether the shot is newly observed or replayed, avoiding the
    /// one-frame ordering change in the original single-slot implementation.
    pub fn inject_crossed(&mut self, before: i32, after: i32, native: &mut NativePlayerState) -> Result<Vec<Hit>, String> {
        if !self.enabled || after <= before {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        for shot in &mut self.shots {
            if shot.confirmed || shot.impact_time <= before || shot.impact_time > after || shot.fire_time > after {
                continue;
            }
            let player = native.view();
            if player.pm_type != 0 || player.health <= 0 {
                continue;
            }
            let Some((impulse, knockback)) = splash_impulse(player, shot.impact) else {
                continue;
            };
            let velocity = add(player.velocity, impulse);
            // Damage uses knockback*2 (50..200ms). Preserve a longer active
            // knockback timer if two predicted explosions overlap.
            let duration = (knockback * 2).clamp(50, 200).max(player.pm_time);
            native.apply_knockback(velocity, duration.clamp(0, 200))?;
            if !shot.reported {
                shot.reported = true;
                hits.push(Hit {
                    command_number: shot.command_number,
                    impact_time: shot.impact_time,
                    impulse, impact: shot.impact, knockback,
                    pre_velocity: player.velocity,
                });
            }
        }
        Ok(hits)
    }
}

/// Quake G_RadiusDamage's edge-of-AABB distance plus G_Damage knockback:
/// damage = int(100*(1-distance/160)), impulse = direction * (1000*damage/200).
/// JKA adds 24 to Z before normalizing the outward direction.
fn splash_impulse(player: PlayerView, explosion: [f32; 3]) -> Option<([f32; 3], i32)> {
    let mut distance_sq = 0.0;
    for axis in 0..3 {
        let low = player.origin[axis] + player.mins[axis];
        let high = player.origin[axis] + player.maxs[axis];
        let edge = if explosion[axis] < low { low - explosion[axis] }
            else if explosion[axis] > high { explosion[axis] - high }
            else { 0.0 };
        distance_sq += edge * edge;
    }
    let distance = distance_sq.sqrt();
    if !distance.is_finite() || distance >= ROCKET_RADIUS {
        return None;
    }
    let points = (100.0 * (1.0 - distance / ROCKET_RADIUS)) as i32;
    if points <= 0 { return None; }
    let mut direction = sub(player.origin, explosion);
    direction[2] += 24.0;
    let len = magnitude(direction);
    if !len.is_finite() || len <= f32::EPSILON { return None; }
    Some((scale(direction, 1000.0 * points as f32 / (200.0 * len)), points))
}

fn snap_towards(mut v: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    for axis in 0..3 {
        v[axis] = if to[axis] <= v[axis] { v[axis].floor() } else { v[axis].ceil() };
    }
    v
}
fn scale(v: [f32; 3], factor: f32) -> [f32; 3] { [v[0]*factor, v[1]*factor, v[2]*factor] }
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]+b[0],a[1]+b[1],a[2]+b[2]] }
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]-b[0],a[1]-b[1],a[2]-b[2]] }
fn distance_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}
fn magnitude(a: [f32; 3]) -> f32 { (a[0]*a[0]+a[1]*a[1]+a[2]*a[2]).sqrt() }

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> PlayerView {
        PlayerView {
            mins: [-15.0,-15.0,-24.0], maxs: [15.0,15.0,32.0],
            view_height: 26, weapon: WP_ROCKET_LAUNCHER, health: 100,
            ..PlayerView::default()
        }
    }
    #[test]
    fn rocket_race_muzzle_shared_with_simulated_projectiles() {
        let ps = player();
        let (muzzle, forward) = local_projectile_muzzle(
            ps.origin, ps.view_angles, ps.view_height as f32, WP_ROCKET_LAUNCHER, true,
        );
        assert_eq!(muzzle, [12.0,0.0,26.0]);
        assert_eq!(forward, [1.0,0.0,0.0]);
    }
    #[test]
    fn splash_uses_bbox_edge_not_player_center() {
        let (impulse, kb) = splash_impulse(player(), [0.0,0.0,-30.0]).unwrap();
        assert_eq!(kb, 96); // 6 units below bbox, not 30 below origin.
        assert!(impulse[2] > 0.0);
    }
    #[test]
    fn splash_outside_radius_is_zero() {
        assert!(splash_impulse(player(), [300.0,0.0,0.0]).is_none());
    }
    #[test]
    fn snap_toward_source() {
        assert_eq!(snap_towards([1.7,-1.4,0.7],[0.0,0.0,1.0]),[1.0,-1.0,1.0]);
    }
}
