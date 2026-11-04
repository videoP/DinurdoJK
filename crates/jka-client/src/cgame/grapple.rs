//! Visual-only rope presentation for jaPRO / JA+ grapples (+button12).
//!
//! TaystJK CG_Missile / CG_GrappleTrail define the network discriminants and
//! hand/endpoint pairing. DestroHook's rope-wiggle idea is adapted here to
//! GPU-generated joined ribbons rather than importing its Godot movement spring:
//! **no** grapple force, collision, usercmd or movement data is ever changed.

use super::{ClientGameState, PresentedEntity, ET_MISSILE};
use super::weapon_fx::MissileModel;
use crate::fx::system::FxDraw;
use std::sync::Arc;

const WP_STUN_BATON: i32 = 1;
const WP_BRYAR_PISTOL: i32 = 4;
const MAX_CLIENTS: i32 = 32;
const ROPE_SHADER: &str = "$grapple_rope";

/// Matches TaystJK's two CG_Missile grapple branches. Normal Bryar shots,
/// stun baton users on vanilla servers and generic missiles aren't hooks.
/// Ownership differs between jaPRO (clientNum) and JA+ (otherEntityNum).
pub(crate) fn grapple_owner(entity: &PresentedEntity, game: &ClientGameState) -> Option<u16> {
    if entity.entity_type != ET_MISSILE { return None }
    let state = &entity.state;
    let weapon = state.field_i32("weapon").unwrap_or(-1);
    let owner = if game.is_japro() && weapon == WP_BRYAR_PISTOL
        && state.field_i32("saberInFlight").unwrap_or(0) != 0
    {
        state.field_i32("clientNum")?
    } else if game.is_japlus() && weapon == WP_STUN_BATON {
        state.field_i32("otherEntityNum")?
    } else {
        return None;
    };
    (0..MAX_CLIENTS).contains(&owner).then_some(owner as u16)
}

/// TaystJK `CG_GrappleEndpoint` model (registered in `cg_main.c`).
/// Unlike the usual Bryar/Stun missile this is the grappling head, not a shot FX.
const GRAPPLE_MODEL: &str = "models/items/grapple.md3";
const TR_STATIONARY: i32 = 0;
const TR_INTERPOLATE: i32 = 1;
const EF_MISSILE_STICK: i32 = 1 << 22;

#[inline]
fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = magnitude(v);
    (length.is_finite() && length > 0.0001).then(|| scale(v, 1.0 / length))
}

/// TaystJK's `CG_GrappleEndpoint` orientation/roll, with a bounded 4-unit
/// head offset. Its original `VectorMA` uses raw server `angles` as a vector;
/// normalizing the offset protects against mods writing Euler degrees there.
/// The rope still terminates at the *unshifted* networked tip (the rear of
/// the model), just as `CG_GrappleTrail` ends at `cent->lerpOrigin`.
///
/// In particular, zero `pos.trDelta` means embedded, not "point upwards":
/// follow TaystJK's negative snapshot `angles` direction in that case.
pub(crate) fn grapple_endpoint_model(entity: &PresentedEntity, time: i32) -> MissileModel {
    let snapshot_angles = super::entity_vec3(&entity.state, "angles").unwrap_or([0.0; 3]);
    let delta = super::entity_vec3(&entity.state, "pos.trDelta").unwrap_or([0.0; 3]);
    let forward = unit(delta)
        .or_else(|| unit(scale(snapshot_angles, -1.0)))
        .unwrap_or([0.0, 0.0, 1.0]);
    let tr_type = entity.state.field_i32("pos.trType").unwrap_or(TR_STATIONARY);
    let stick = entity.state.field_i32("eFlags").unwrap_or(0) & EF_MISSILE_STICK != 0;
    let axis = if entity.state.field_i32("apos.trType").unwrap_or(TR_STATIONARY)
        == TR_INTERPOLATE
    {
        // CG_GrappleEndpoint: AnglesToAxis(cent->lerpAngles, axis).
        // PresentedEntity already contains the interpolated angles.
        super::weapon_fx::angles_to_axis(entity.angles)
    } else {
        // CG_GrappleEndpoint: RotateAroundDirection; stationary hooks preserve
        // the server-provided roll instead of spinning indefinitely.
        let roll = if tr_type != TR_STATIONARY {
            time as f32 * if stick { 0.5 } else { 0.25 }
        } else if stick {
            entity.state.field_i32("pos.trTime").unwrap_or(0) as f32 * 0.5
        } else {
            entity.state.field_i32("time").unwrap_or(0) as f32
        };
        jka_movement::rotate_around_direction(forward, roll)
    };

    // The source uses `startPos += 4 * currentState.angles`, after negating
    // angles twice. On normal hooks this vector is already normalized.
    let offset_direction = unit(snapshot_angles).filter(|_| magnitude(snapshot_angles) <= 1.5)
        .unwrap_or(forward);
    MissileModel {
        qpath: GRAPPLE_MODEL.to_owned(),
        origin: add(entity.origin, scale(offset_direction, 4.0)),
        axis,
    }
}

/// One cheaply integrated visual spring per missile entity. Reinitialized on
/// entity reuse, demo seek, movement teleport or presentation gap.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GrappleRopeState {
    owner: u16,
    generation: i32,
    start: [f32; 3],
    tip: [f32; 3],
    offset: [f32; 3],
    velocity: [f32; 3],
    phase: f32,
    pub last_time: i32,
    initialized: bool,
}

impl Default for GrappleRopeState {
    fn default() -> Self {
        Self {
            owner: 0, generation: 0, start: [0.0; 3], tip: [0.0; 3],
            offset: [0.0; 3], velocity: [0.0; 3], phase: 0.0,
            last_time: 0, initialized: false,
        }
    }
}

#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0]+b[0], a[1]+b[1], a[2]+b[2]]
}
#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0]-b[0], a[1]-b[1], a[2]-b[2]]
}
#[inline]
fn scale(v: [f32; 3], f: f32) -> [f32; 3] {
    [v[0]*f, v[1]*f, v[2]*f]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0]*b[0] + a[1]*b[1] + a[2]*b[2]
}
#[inline]
fn magnitude(a: [f32; 3]) -> f32 { dot(a, a).sqrt() }
#[inline]
fn capped(v: [f32; 3], max_length: f32) -> [f32; 3] {
    let size = magnitude(v);
    if size > max_length && size > 0.0 { scale(v, max_length / size) } else { v }
}

impl GrappleRopeState {
    fn advance(
        &mut self, owner: u16, generation: i32, time: i32,
        start: [f32; 3], tip: [f32; 3], hook_speed: f32,
    ) {
        let elapsed = time.saturating_sub(self.last_time);
        // A new hook must not inherit a previous missile's oscillation.
        if !self.initialized || self.owner != owner || self.generation != generation
            || elapsed < 0 || elapsed > 200
            || magnitude(sub(tip, self.tip)) > 512.0
            || magnitude(sub(start, self.start)) > 512.0
        {
            *self = Self {
                owner, generation, start, tip, last_time: time,
                initialized: true, ..Self::default()
            };
            return;
        }
        let dt = (elapsed as f32 * 0.001).clamp(0.0, 0.05);
        if dt > 0.0 {
            // Disturb the spring from *observed* end motion rather than from
            // a hardcoded hook velocity. The server's actual hookspeed drives
            // propagation; this integrates the shape only, never player force.
            let movement = add(scale(sub(tip, self.tip), 0.7), scale(sub(start, self.start), 0.3));
            self.velocity = sub(self.velocity, scale(capped(movement, 128.0), 0.7));
            let acceleration = sub(scale(self.offset, -47.0), scale(self.velocity, 9.5));
            self.velocity = add(self.velocity, scale(acceleration, dt));
            self.offset = add(self.offset, scale(self.velocity, dt));
            self.offset = capped(self.offset, 14.0);
            self.phase = (self.phase + dt * (4.0 + hook_speed * 0.006).min(26.0))
                .rem_euclid(std::f32::consts::TAU);
        }
        self.start = start;
        self.tip = tip;
        self.last_time = time;
    }
}

/// Adds one joined red rope with halo. Both ends stay at exact network/
/// posed coordinates; spring, sag and motion waves only modify interior nodes.
pub(crate) fn append_grapple_rope(
    draws: &mut Vec<FxDraw>,
    state: &mut GrappleRopeState,
    owner: u16,
    generation: i32,
    time: i32,
    start: [f32; 3],
    tip: [f32; 3],
    tr_delta: [f32; 3],
) {
    let span = sub(tip, start);
    let length = magnitude(span);
    if !length.is_finite() || length < 2.0 || length > 32768.0 { return }
    let speed = magnitude(tr_delta).min(5000.0);
    state.advance(owner, generation, time, start, tip, speed);

    // A *single* FX request for the entire rope, not 16-56 independent
    // RT_LINE quads. Its two layers are expanded into connected ribbons by
    // the GPU vertex shader. The reference FX mode uses an indexed CPU mesh.
    let forward = scale(span, 1.0 / length);
    let spring = sub(state.offset, scale(forward, dot(state.offset, forward)));
    draws.push(FxDraw::Rope {
        start, end: tip, spring, phase: state.phase, speed,
        shader: Arc::from(ROPE_SHADER),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook_for_model(fields: &[(&str, u32)]) -> PresentedEntity {
        use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};
        let mut state = EntityState { number: 41, fields: [0; ENTITY_FIELDS.len()] };
        for &(key, bits) in fields {
            let index = ENTITY_FIELDS.iter().position(|(field, _)| *field == key)
                .expect("test netfield exists");
            state.fields[index] = bits;
        }
        PresentedEntity {
            number: 41, entity_type: ET_MISSILE,
            origin: [100.0, 200.0, 300.0], angles: [0.0; 3], state,
        }
    }

    #[test]
    fn grapple_head_uses_taystjk_md3_and_flight_direction() {
        let hook = hook_for_model(&[
            ("pos.trType", 2),
            ("pos.trDelta[0]", 1000.0_f32.to_bits()),
            ("angles[0]", 1.0_f32.to_bits()),
        ]);
        let model = grapple_endpoint_model(&hook, 1000);
        assert_eq!(model.qpath, "models/items/grapple.md3");
        assert_eq!(model.origin, [104.0, 200.0, 300.0]);
        assert!((model.axis[0][0] - 1.0).abs() < 0.0001);
        assert!(model.axis[0][1].abs() < 0.0001);
    }

    #[test]
    fn grapple_head_stationary_roll_is_frozen_and_zero_velocity_uses_wall_direction() {
        let hook = hook_for_model(&[
            ("angles[0]", 1.0_f32.to_bits()),
            ("pos.trTime", 1234),
            ("eFlags", EF_MISSILE_STICK as u32),
        ]);
        let before = grapple_endpoint_model(&hook, 2000);
        let later = grapple_endpoint_model(&hook, 4000);
        assert_eq!(before.axis, later.axis);
        assert!((before.axis[0][0] + 1.0).abs() < 0.0001);
        assert_eq!(before.origin, [104.0, 200.0, 300.0]);
    }

    #[test]
    fn grapple_head_uses_lerped_angles_and_never_applies_euler_degrees_as_an_offset() {
        let mut hook = hook_for_model(&[
            ("apos.trType", TR_INTERPOLATE as u32),
            ("pos.trDelta[0]", 1000.0_f32.to_bits()),
            ("angles[1]", 180.0_f32.to_bits()),
        ]);
        hook.angles = [0.0, 90.0, 0.0];
        let model = grapple_endpoint_model(&hook, 1000);
        assert!((model.axis[0][0]).abs() < 0.0001);
        assert!((model.axis[0][1] - 1.0).abs() < 0.0001);
        assert_eq!(model.origin, [104.0, 200.0, 300.0]);
    }

    #[test]
    fn endpoints_are_exact_despite_sag_and_wiggle() {
        let mut state = GrappleRopeState::default();
        let mut draws = Vec::new();
        let start = [1.0, 2.0, 25.0];
        let end = [220.0, 65.0, 100.0];
        append_grapple_rope(&mut draws, &mut state, 1, 18, 1000, start, end, [2000.0, 0.0, 0.0]);
        assert!(!draws.is_empty());
        assert_eq!(draws.len(), 1, "one draw request per rope");
        assert!(matches!(draws.first(), Some(FxDraw::Rope { start: p, .. }) if *p == start));
        assert!(matches!(draws.first(), Some(FxDraw::Rope { end: p, .. }) if *p == end));
    }

    #[test]
    fn reusing_entity_resets_visual_spring() {
        let mut state = GrappleRopeState::default();
        let mut draws = Vec::new();
        append_grapple_rope(&mut draws, &mut state, 1, 18, 1000,
            [0.0, 0.0, 24.0], [120.0, 0.0, 24.0], [1600.0, 0.0, 0.0]);
        draws.clear();
        append_grapple_rope(&mut draws, &mut state, 1, 18, 1016,
            [0.0, 0.0, 24.0], [140.0, 0.0, 24.0], [1600.0, 0.0, 0.0]);
        assert!(state.velocity.iter().any(|component| component.abs() > 0.01));
        append_grapple_rope(&mut draws, &mut state, 2, 19, 1032,
            [0.0, 0.0, 24.0], [100.0, 0.0, 24.0], [0.0; 3]);
        assert_eq!(state.velocity, [0.0; 3]);
        assert_eq!(state.owner, 2);
    }
}
