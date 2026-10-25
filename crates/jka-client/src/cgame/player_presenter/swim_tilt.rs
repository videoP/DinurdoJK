//! Client-only swim orientation layered over OpenJK's normal player axis.
//! There is no player waterlevel field in remote entityState, so the water
//! collision volume estimates surface proximity; the swim legs animation is
//! the reliable shared gate for local and remote bodies.
use jka_assets::animation::AnimationSet;
use jka_movement::TraceWorld;

const MAX_FORWARD_PITCH: f32 = 82.0;
const MAX_BACKWARD_PITCH: f32 = 62.0;
const MAX_STRAFE_ROLL: f32 = 22.0;
const MAX_TURN_BANK: f32 = 60.0;
const SURFACE_PITCH_LIFT: f32 = 12.0;
const WATER_SURFACE_BLEND_DISTANCE: f32 = 48.0;
const CONTENTS_WATER: i32 = 0x4;

pub(super) fn swimming_legs(raw_anim: i32) -> bool {
    // The animation toggle bit is not part of the index (JKA 2048).
    matches!(
        AnimationSet::name(raw_anim & !super::physics::ANIM_TOGGLEBIT),
        Some("BOTH_SWIM_IDLE1" | "BOTH_SWIMFORWARD" | "BOTH_SWIMBACKWARD")
    )
}

/// Output pitch (+ forward/nose-down) and roll, degrees. Stationary swimmers
/// retain the stock upright tread animation; moving bodies transition flat.
pub(super) fn swim_target_angles(
    legs_anim: i32, yaw_degrees: f32, velocity: [f32; 3], strength: f32,
    surface_proximity: f32,
) -> [f32; 2] {
    if strength <= 0.0 || !swimming_legs(legs_anim) || !velocity.iter().all(|v| v.is_finite()) {
        return [0.0; 2];
    }
    let horizontal = velocity[0].hypot(velocity[1]);
    let motion = ((horizontal - 20.0) / 120.0).clamp(0.0, 1.0);
    if motion == 0.0 { return [0.0; 2]; }
    let (sin, cos) = yaw_degrees.to_radians().sin_cos();
    let forward = (velocity[0] * cos + velocity[1] * sin) / horizontal;
    let sideways = (-velocity[0] * sin + velocity[1] * cos) / horizontal;
    let base_pitch = if forward >= 0.0 {
        MAX_FORWARD_PITCH * forward + 27.0 * sideways.abs()
    } else {
        MAX_BACKWARD_PITCH * forward + 27.0 * sideways.abs()
    };
    // Pitch slightly toward diving/ascending without making vertical tread
    // point the head straight down or flip the body upside-down.
    let vertical = velocity[2] / horizontal.hypot(velocity[2]).max(1.0);
    let surface_lift = SURFACE_PITCH_LIFT * surface_proximity.clamp(0.0, 1.0);
    let pitch = (base_pitch - 23.0 * vertical * forward.max(0.0) - surface_lift)
        .clamp(-67.0, 102.0);
    [pitch * motion * strength, -MAX_STRAFE_ROLL * sideways * motion * strength]
}

/// Estimate how close a swimmer is to the top of its water volume. Sample from
/// torso height upward so the head-up pose eases in while the body approaches
/// the surface. The swim animation remains the gate; this also works for
/// remote players, whose network entity state does not include waterlevel.
pub(super) fn swim_surface_proximity<W: TraceWorld>(
    world: &mut W,
    origin: [f32; 3],
    pass_entity: i32,
) -> f32 {
    if !origin.iter().all(|value| value.is_finite()) {
        return 0.0;
    }
    let torso = [origin[0], origin[1], origin[2] + 24.0];
    let torso_wet = world.point_contents(torso, pass_entity) & CONTENTS_WATER != 0;
    if !torso_wet {
        return if world.point_contents(origin, pass_entity) & CONTENTS_WATER != 0 {
            1.0
        } else {
            0.0
        };
    }

    for height in (8..=48).step_by(8) {
        let sample = [torso[0], torso[1], torso[2] + height as f32];
        if world.point_contents(sample, pass_entity) & CONTENTS_WATER == 0 {
            let distance = height as f32 / WATER_SURFACE_BLEND_DISTANCE;
            let smooth = distance * distance * (3.0 - 2.0 * distance);
            return 1.0 - smooth;
        }
    }
    0.0
}

/// Velocity-derived swim turn bank. Local axis 2 rotates the swimmer about
/// the post-pitch head-to-feet axis (visible back toward inside of the turn).
/// Pure camera yaw does not create a false swim bank. Snapshot-held velocity
/// is tolerated, and old turn impulse relaxes instead of freezing forever.
#[derive(Clone, Debug, Default)]
pub(super) struct SwimTurnMotion {
    last_heading: Option<f32>,
    last_heading_time_ms: i32,
    last_change_time_ms: i32,
    target: f32,
}
impl SwimTurnMotion {
    pub fn reset(&mut self) { *self = Self::default(); }
    pub fn target(&self) -> f32 { self.target }

    pub fn update(&mut self, time_ms: i32, anim: i32, velocity: [f32; 3],
                  tilt_strength: f32, bank_strength: f32) -> f32 {
        let speed = velocity[0].hypot(velocity[1]);
        if !swimming_legs(anim) || tilt_strength <= 0.0 || bank_strength <= 0.0
            || !velocity.iter().all(|v| v.is_finite()) || speed < 25.0
        {
            self.reset();
            return 0.0;
        }
        let heading = velocity[1].atan2(velocity[0]).to_degrees();
        let Some(old_heading) = self.last_heading else {
            self.last_heading = Some(heading);
            self.last_heading_time_ms = time_ms;
            self.last_change_time_ms = time_ms;
            return 0.0;
        };
        let elapsed = time_ms.saturating_sub(self.last_heading_time_ms);
        if time_ms < self.last_heading_time_ms || elapsed > 250 {
            self.reset();
            self.last_heading = Some(heading);
            self.last_heading_time_ms = time_ms;
            self.last_change_time_ms = time_ms;
            return 0.0;
        }
        let delta = (heading - old_heading + 180.0).rem_euclid(360.0) - 180.0;
        // Velocity comes from discrete snapshots; avoid tiny interpolation
        // deltas over 1ms, but do not discard a meaningful 40-50ms change.
        if delta.abs() >= 0.2 && elapsed >= 12 {
            self.last_heading = Some(heading);
            self.last_heading_time_ms = time_ms;
            self.last_change_time_ms = time_ms;
            if delta.abs() > 75.0 {
                self.target = 0.0; // teleport/seek discontinuity
            } else {
                let turn_rate = delta * 1000.0 / elapsed as f32;
                let turn_fraction = ((turn_rate.abs() - 8.0) / 105.0).clamp(0.0, 1.0);
                let motion = ((speed - 25.0) / 95.0).clamp(0.0, 1.0);
                // Positive heading delta is a left curve, so negative bank;
                // right/D curve is positive bank. At full rate -> ±60°.
                self.target = -turn_rate.signum() * MAX_TURN_BANK * turn_fraction
                    * motion * tilt_strength * bank_strength;
            }
        } else if time_ms.saturating_sub(self.last_change_time_ms) > 100 {
            // No continuing curve: fade old target once repeated same-velocity
            // snapshots have ended. Outer axis smoothing handles easing.
            self.target *= (-(time_ms.saturating_sub(self.last_change_time_ms) - 100) as f32 / 75.0).exp();
            self.last_change_time_ms = time_ms - 100;
            if self.target.abs() < 0.05 { self.target = 0.0; }
        }
        self.target
    }
}

/// Frame-rate independent relaxation; rejoin the original upright pose when
/// swimming stops. Reset on seek/teleport or large unpresented gaps.
pub(super) fn swim_smooth(previous: [f32; 2], target: [f32; 2], dt_ms: i32) -> [f32; 2] {
    if dt_ms < 0 || dt_ms > 250 { return target; }
    let time_constant = if target[0].abs() + target[1].abs() > previous[0].abs() + previous[1].abs() {
        160.0
    } else { 230.0 };
    let alpha = 1.0 - (-(dt_ms as f32) / time_constant).exp();
    [previous[0] + (target[0] - previous[0]) * alpha,
     previous[1] + (target[1] - previous[1]) * alpha]
}

/// Apply local pitch and roll to the full Ghoul2 axis. This leaves the shared
/// skinning pose, renderer scale, weapon/saber bolt positions and shadows in
/// the *same* world transform (rather than tilting just the visible mesh).
pub(super) fn swim_rotate_axis(axis: [[f32; 3]; 3], pitch_roll: [f32; 2]) -> [[f32; 3]; 3] {
    let (sp, cp) = pitch_roll[0].to_radians().sin_cos();
    let (sr, cr) = pitch_roll[1].to_radians().sin_cos();
    let mut pitched_forward = [0.0; 3];
    let mut pitched_up = [0.0; 3];
    let mut new_left = [0.0; 3];
    let mut new_up = [0.0; 3];
    for k in 0..3 {
        pitched_forward[k] = cp * axis[0][k] - sp * axis[2][k];
        pitched_up[k] = sp * axis[0][k] + cp * axis[2][k];
        new_left[k] = cr * axis[1][k] + sr * pitched_up[k];
        new_up[k] = cr * pitched_up[k] - sr * axis[1][k];
    }
    [pitched_forward, new_left, new_up]
}

/// Swim banking is permanently on the post-pitch head-to-feet axis (2).
/// This rolls the swimmer's back to the side, rather than rotating around
/// the direction of travel as the original diagnostic axis 0 did.
pub(super) fn rotate_local_bank(axis: [[f32; 3]; 3], degrees: f32) -> [[f32; 3]; 3] {
    if degrees == 0.0 { return axis; }
    let (s, c) = degrees.to_radians().sin_cos();
    let mut result = axis;
    for k in 0..3 {
        result[0][k] = c * axis[0][k] + s * axis[1][k];
        result[1][k] = c * axis[1][k] - s * axis[0][k];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgame::player_presenter::angles_to_axis;
    use jka_assets::animation::animation_index;

    #[test]
    fn swim_direction_and_toggle() {
        let forward = animation_index("BOTH_SWIMFORWARD").unwrap() as i32;
        let backward = animation_index("BOTH_SWIMBACKWARD").unwrap() as i32;
        assert!(swimming_legs(forward | super::super::physics::ANIM_TOGGLEBIT));
        assert!(swim_target_angles(forward, 0.0, [200.0, 0.0, 0.0], 1.0, 0.0)[0] > 75.0);
        assert!(swim_target_angles(backward, 0.0, [-200.0, 0.0, 0.0], 1.0, 0.0)[0] < -50.0);
        assert_eq!(swim_target_angles(forward, 0.0, [200.0, 0.0, 0.0], 0.0, 0.0), [0.0; 2]);
        assert_eq!(swim_target_angles(0, 0.0, [200.0, 0.0, 0.0], 1.0, 0.0), [0.0; 2]);
    }

    #[test]
    fn axis_is_orthogonal_and_pitch_lays_swimmer_flat() {
        let axis = angles_to_axis([0.0, 20.0, 0.0]);
        let flat = swim_rotate_axis(axis, [90.0, 0.0]);
        assert!(flat[2][2].abs() < 0.001, "body up-axis should become horizontal");
        assert!(flat[0][2] < -0.999, "face should aim toward the floor");
        for i in 0..3 {
            let length = flat[i].iter().map(|x| x*x).sum::<f32>();
            assert!((length - 1.0).abs() < 0.0001);
        }
    }
    #[test]
    fn bank_axis_2_rotates_back_sideways_with_horizontal_swim() {
        let flat = swim_rotate_axis(angles_to_axis([0.0, 0.0, 0.0]), [90.0, 0.0]);
        let banked = rotate_local_bank(flat, -60.0);
        // Local forward is the chest-facing axis.  With the player flat,
        // a head-to-feet bank moves it sideways, while head-to-feet stays put.
        assert!(banked[0][1].abs() > 0.85, "body chest should turn sideways: {banked:?}");
        assert!((banked[2][0] - flat[2][0]).abs() < 0.001);
    }

    #[test]
    fn bank_tracks_velocity_curvature_not_camera_yaw() {
        let swim = animation_index("BOTH_SWIMFORWARD").unwrap() as i32;
        let mut left = SwimTurnMotion::default();
        left.update(100, swim, [200.0,0.0,0.0],1.0,1.0);
        let left_bank = left.update(150, swim, [180.0,60.0,0.0],1.0,1.0);
        assert!(left_bank < -40.0,"left curve must bank left: {left_bank}");
        let mut right = SwimTurnMotion::default();
        right.update(100,swim,[200.0,0.0,0.0],1.0,1.0);
        let right_bank = right.update(150,swim,[180.0,-60.0,0.0],1.0,1.0);
        assert!(right_bank > 40.0,"right curve must bank right: {right_bank}");
        assert!(right.update(166,swim,[180.0,-60.0,0.0],1.0,1.0)>0.0);
        for t in (266..=666).step_by(20) {right.update(t,swim,[180.0,-60.0,0.0],1.0,1.0);}
        assert!(right.target().abs() < 1.0,"old bank must fade: {}",right.target());
    }
    #[test]
    fn bank_does_not_spike_on_yaw_only_wrap_teleport_or_idle() {
        let swim = animation_index("BOTH_SWIMFORWARD").unwrap() as i32;
        let mut state=SwimTurnMotion::default();
        assert_eq!(state.update(100,swim,[200.0,0.0,0.0],1.0,1.0),0.0);
        assert_eq!(state.update(150,swim,[200.0,0.0,0.0],1.0,1.0),0.0);
        assert_eq!(state.update(200,swim,[-200.0,0.0,0.0],1.0,1.0),0.0);
        assert_eq!(state.update(220,swim,[0.0,0.0,0.0],1.0,1.0),0.0);
        assert_eq!(state.update(250,0,[200.0,0.0,0.0],1.0,1.0),0.0);
    }

    #[test]
    fn smooth_returns_to_upright() {
        let one = swim_smooth([80.0, 12.0], [0.0, 0.0], 16);
        assert!(one[0] > 0.0 && one[0] < 80.0);
        assert_eq!(swim_smooth(one, [0.0; 2], 251), [0.0; 2]);
    }
}
