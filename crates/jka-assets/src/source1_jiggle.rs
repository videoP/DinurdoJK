//! Secondary motion for procedural Source 1 StudioMDL jigglebones.
//!
//! Unlike a baked animation, these are persistent per-character spring states.
//! The Source rest rig is already rebound into JKA geometry; these *additional*
//! palette matrices deform the preserved native Source jigglebone weights.
//! This is a lightweight Source-compatible spring approximation, not a bit-for-
//! bit port of Valve's proprietary per-branch solver (especially boing).
use crate::ghoul2::Matrix3x4;
use crate::source1::{
    JIGGLE_HAS_ANGLE_CONSTRAINT, JIGGLE_HAS_BASE_SPRING, JIGGLE_HAS_PITCH_CONSTRAINT,
    JIGGLE_HAS_YAW_CONSTRAINT, JIGGLE_IS_FLEXIBLE,
};
use crate::source1_rig::SourceJiggleBinding;

fn ident() -> Matrix3x4 {
    [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]
}
fn mul(a: &Matrix3x4, b: &Matrix3x4) -> Matrix3x4 {
    let mut out = ident();
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
        out[r][3] = a[r][3] + (0..3).map(|k| a[r][k] * b[k][3]).sum::<f32>();
    }
    out
}
fn transform(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r][3] + (0..3).map(|c| m[r][c] * p[c]).sum::<f32>())
}
fn rotate(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| (0..3).map(|c| m[r][c] * p[c]).sum::<f32>())
}
fn pos(m: &Matrix3x4) -> [f32; 3] { [m[0][3], m[1][3], m[2][3]] }
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { std::array::from_fn(|i| a[i] - b[i]) }
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { std::array::from_fn(|i| a[i] + b[i]) }
fn scale(a: [f32; 3], s: f32) -> [f32; 3] { a.map(|c| c * s) }
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { (0..3).map(|i| a[i] * b[i]).sum() }
fn norm(a: [f32; 3]) -> f32 { dot(a, a).sqrt() }
fn unit(a: [f32; 3]) -> [f32; 3] { scale(a, 1.0 / norm(a).max(1.0e-8)) }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
/// Works for rigid transforms and uniform scale (actor model scale).
fn inverse_similarity(m: &Matrix3x4) -> Matrix3x4 {
    let mut out = ident();
    let scale2 = (0..3).map(|r| m[r][0] * m[r][0]).sum::<f32>().max(1.0e-8);
    for r in 0..3 {
        for c in 0..3 { out[r][c] = m[c][r] / scale2; }
        out[r][3] = -(0..3).map(|k| out[r][k] * m[k][3]).sum::<f32>();
    }
    out
}
fn from_axis(axis: [[f32; 3]; 3], origin: [f32; 3]) -> Matrix3x4 {
    std::array::from_fn(|r| [axis[0][r], axis[1][r], axis[2][r], origin[r]])
}
/// Shortest rotation that takes the default tip direction to the solved tip.
fn rotate_between(from: [f32; 3], to: [f32; 3]) -> Matrix3x4 {
    let a = unit(from);
    let b = unit(to);
    let cosine = dot(a, b).clamp(-1.0, 1.0);
    if cosine > 1.0 - 1.0e-6 { return ident(); }
    let axis = if cosine < -1.0 + 1.0e-6 {
        unit(cross(a, if a[0].abs() < 0.8 { [1., 0., 0.] } else { [0., 1., 0.] }))
    } else { unit(cross(a, b)) };
    let sine = (1.0 - cosine * cosine).max(0.0).sqrt();
    let mut out = ident();
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = if row == col { cosine } else { 0.0 }
                + axis[row] * axis[col] * (1.0 - cosine)
                + sine * match (row, col) {
                    (0, 1) => -axis[2], (0, 2) => axis[1],
                    (1, 0) => axis[2], (1, 2) => -axis[0],
                    (2, 0) => -axis[1], (2, 1) => axis[0], _ => 0.0,
                };
        }
    }
    out
}

#[derive(Clone, Default)]
struct JiggleState {
    initialized: bool,
    tip: [f32; 3],
    tip_velocity: [f32; 3],
    base: [f32; 3],
    base_velocity: [f32; 3],
}

/// A simulation belongs to one visible player instance, not to its cached model.
#[derive(Clone, Default)]
pub struct SourceJiggleAnimator {
    states: Vec<JiggleState>,
    last_time_ms: Option<i32>,
}

impl SourceJiggleAnimator {
    pub fn reset(&mut self) { self.states.clear(); self.last_time_ms = None; }

    /// Update the Source-only extension of the pose. `pose[..gla_bones]` is
    /// untouched. The `actor_axis` has the same basis/scale as the final draw;
    /// simulation happens in world space so walking and turning create inertia.
    pub fn update(
        &mut self,
        bindings: &[SourceJiggleBinding],
        gla_bones: usize,
        pose: &mut [Matrix3x4],
        actor_axis: [[f32; 3]; 3],
        actor_origin: [f32; 3],
        time_ms: i32,
    ) {
        if bindings.is_empty() || pose.len() < gla_bones + bindings.len() { return; }
        if self.states.len() != bindings.len() {
            self.reset();
            self.states.resize_with(bindings.len(), JiggleState::default);
        }
        // A 250ms visibility gap, demo seek, or clock rewind invalidates
        // velocity history. Avoid huge impulses when a teleported player reappears.
        let delta_ms = self.last_time_ms.map_or(0, |last| time_ms - last);
        let reset = !(0..=250).contains(&delta_ms);
        if reset { self.states.iter_mut().for_each(|state| state.initialized = false); }
        self.last_time_ms = Some(time_ms);
        let dt = (delta_ms.max(0) as f32 * 0.001).min(0.1);
        let substeps = (dt / (1.0 / 120.0)).ceil().clamp(1.0, 12.0) as usize;
        let step = dt / substeps as f32;
        let actor = from_axis(actor_axis, actor_origin);
        let inverse_actor = inverse_similarity(&actor);
        let actor_scale = norm(actor_axis[0]).clamp(0.01, 100.0);
        let mut goals = Vec::with_capacity(bindings.len());
        let mut simulated = Vec::with_capacity(bindings.len());
        for (index, binding) in bindings.iter().enumerate() {
            if binding.parent_gla >= gla_bones { continue; }
            let mut goal = mul(&actor, &mul(&pose[binding.parent_gla], &binding.rest_frame));
            // A ponytail's second spring inherits its first spring's physical
            // rotation, *not* just the head animation. Preserve intermediate
            // static Source rest offsets with the baked rest frames.
            if let Some(parent) = binding.parent_jiggle.filter(|&p| p < index) {
                let delta = mul(&simulated[parent], &inverse_similarity(&goals[parent]));
                goal = mul(&delta, &goal);
            }
            let anchor = pos(&goal);
            let length = (binding.length * actor_scale).max(0.001);
            let target_tip = transform(&goal, [0., 0., binding.length]);
            let state = &mut self.states[index];
            if !state.initialized || norm(sub(state.tip, target_tip)) > length * 8.0 + 50.0 {
                state.tip = target_tip;
                state.base = anchor;
                state.tip_velocity = [0.; 3];
                state.base_velocity = [0.; 3];
                state.initialized = true;
            }
            let p = &binding.parameters;
            for _ in 0..substeps {
                if step == 0.0 { break; }
                // Source authored yaw/pitch spring coefficients determine the
                // restoring rates. tipMass predominantly controls gravity.
                let stiff = if p.flags & JIGGLE_IS_FLEXIBLE != 0 {
                    ((p.yaw_stiffness + p.pitch_stiffness) * 0.5).max(0.0)
                } else { 4.0 };
                let damping = if p.flags & JIGGLE_IS_FLEXIBLE != 0 {
                    ((p.yaw_damping + p.pitch_damping) * 0.5).max(0.0)
                } else { 1.0 };
                let acceleration = scale(sub(target_tip, state.tip), stiff);
                state.tip_velocity = add(state.tip_velocity, scale(acceleration, step));
                // Gravity is JKA world -Z, not camera coordinates.
                state.tip_velocity[2] -= 800.0 * (p.tip_mass / 1000.0).clamp(0.0, 2.0) * step;
                state.tip_velocity = scale(state.tip_velocity, (-damping * step).exp());
                state.tip = add(state.tip, scale(state.tip_velocity, step));
                if p.flags & JIGGLE_HAS_BASE_SPRING != 0 {
                    state.base_velocity = add(state.base_velocity,
                        scale(sub(anchor, state.base), p.base_stiffness.max(0.0) * step));
                    state.base_velocity[2] -= 800.0 * (p.base_mass / 1000.0).clamp(0.0, 2.0) * step;
                    state.base_velocity = scale(state.base_velocity, (-p.base_damping.max(0.0) * step).exp());
                    state.base = add(state.base, scale(state.base_velocity, step));
                    let inv_goal = inverse_similarity(&goal);
                    let local = transform(&inv_goal, state.base);
                    let limited = [
                        local[0].clamp(p.base_min_left.min(p.base_max_left), p.base_min_left.max(p.base_max_left)),
                        local[1].clamp(p.base_min_up.min(p.base_max_up), p.base_min_up.max(p.base_max_up)),
                        local[2].clamp(p.base_min_forward.min(p.base_max_forward), p.base_min_forward.max(p.base_max_forward)),
                    ];
                    let bounded = transform(&goal, limited);
                    if norm(sub(bounded, state.base)) > 0.0001 { state.base_velocity = [0.; 3]; }
                    state.base = bounded;
                } else {
                    state.base = anchor;
                    state.base_velocity = [0.; 3];
                }
                // Aim a rigid, fixed-length +Z segment. The Source compiled
                // flags contain cones and yaw/pitch limits in radians.
                let mut local = rotate(&inverse_similarity(&goal), sub(state.tip, state.base));
                if norm(local) < 1.0e-5 { local = [0., 0., binding.length]; }
                local = scale(unit(local), binding.length);
                if p.flags & JIGGLE_HAS_ANGLE_CONSTRAINT != 0 {
                    let max_angle = p.angle_limit.clamp(0.0, std::f32::consts::PI);
                    let angle = (local[2] / binding.length).clamp(-1.0, 1.0).acos();
                    if angle > max_angle {
                        let xy = unit([local[0], local[1], 0.]);
                        local = [xy[0] * binding.length * max_angle.sin(),
                            xy[1] * binding.length * max_angle.sin(), binding.length * max_angle.cos()];
                    }
                }
                if p.flags & (JIGGLE_HAS_PITCH_CONSTRAINT | JIGGLE_HAS_YAW_CONSTRAINT) != 0 {
                    let mut yaw = local[0].atan2(local[2]);
                    let mut pitch = (-local[1]).atan2(local[2]);
                    if p.flags & JIGGLE_HAS_YAW_CONSTRAINT != 0 {
                        yaw = yaw.clamp(p.min_yaw.min(p.max_yaw), p.min_yaw.max(p.max_yaw));
                    }
                    if p.flags & JIGGLE_HAS_PITCH_CONSTRAINT != 0 {
                        pitch = pitch.clamp(p.min_pitch.min(p.max_pitch), p.min_pitch.max(p.max_pitch));
                    }
                    local = unit([yaw.tan(), -pitch.tan(), 1.]);
                    local = scale(local, binding.length);
                }
                let bounded_tip = add(state.base, rotate(&goal, local));
                if norm(sub(bounded_tip, state.tip)) > 0.001 {
                    // Project velocity onto the constraint tangent; do not
                    // continuously accumulate outward acceleration.
                    let normal = unit(sub(bounded_tip, state.base));
                    state.tip_velocity = sub(state.tip_velocity, scale(normal, dot(state.tip_velocity, normal)));
                }
                state.tip = bounded_tip;
            }
            let tip_direction = sub(state.tip, state.base);
            let orientation = mul(&rotate_between(rotate(&goal, [0., 0., 1.]), tip_direction), &goal);
            let mut world = orientation;
            for r in 0..3 { world[r][3] = state.base[r]; }
            // Convert world-space bone motion into the existing JKA model's
            // baked bind coordinates, just like an ordinary skin matrix.
            pose[gla_bones + index] = mul(&inverse_actor,
                &mul(&world, &inverse_similarity(&binding.rest_frame)));
            goals.push(goal);
            simulated.push(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source1::{JIGGLE_IS_FLEXIBLE, SourceJiggleParams};

    fn params() -> SourceJiggleParams {
        // All fields are present in the actual compiled structure.
        let mut raw = [0u8; 140];
        raw[0..4].copy_from_slice(&JIGGLE_IS_FLEXIBLE.to_le_bytes());
        raw[4..8].copy_from_slice(&5.0f32.to_le_bytes());
        crate::source1::test_parse_jiggle_for_animator(&raw)
    }

    #[test]
    fn animated_parent_moves_static_extra_palette_and_hair_does_not_bleed() {
        let binding = SourceJiggleBinding {
            name: "hair".into(), parent_gla: 0, parent_jiggle: None,
            rest_frame: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 10.]],
            parameters: params(), length: 5.0,
        };
        let mut pose = vec![ident(), ident()];
        let mut sim = SourceJiggleAnimator::default();
        sim.update(&[binding.clone()], 1, &mut pose,
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 0.], 1000);
        assert!((pose[1][2][3]).abs() < 0.001);
        pose[0][0][3] = 8.0;
        sim.update(&[binding], 1, &mut pose,
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 0.], 1016);
        assert_eq!(pose[0][0][3], 8.0);
        // The dynamic tip must lag the animated head, not snap to it.
        assert!((pose[1][0][0] - 1.0).abs() > 0.0001 || pose[1][0][3] != 8.0);
    }
}
