//! Reversible A/B experiments for saber split/full-body animation boundaries.
use jka_movement::PlayerAngleResult;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// Mode 2 was confirmed visually on the supplied swing. Keep the alternatives
// session-only for reference comparisons.
static MODE: AtomicU8 = AtomicU8::new(2);
static ANIM_BLEND: AtomicBool = AtomicBool::new(true);
pub(crate) fn saber_swing_fix() -> u8 { MODE.load(Ordering::Relaxed) }
pub(crate) fn set_saber_swing_fix(mode: u8) { MODE.store(mode.min(2), Ordering::Relaxed); }
pub(crate) fn animation_blend_enabled() -> bool { ANIM_BLEND.load(Ordering::Relaxed) }
pub(crate) fn set_animation_blend(enabled: bool) { ANIM_BLEND.store(enabled, Ordering::Relaxed); }
// Diagnostic: log JK2 CG_G2PlayerAngles inputs/outputs every frame.
static JK2_ANGLE_DEBUG: AtomicBool = AtomicBool::new(false);
pub(crate) fn jk2_angle_debug() -> bool { JK2_ANGLE_DEBUG.load(Ordering::Relaxed) }
pub(crate) fn set_jk2_angle_debug(enabled: bool) { JK2_ANGLE_DEBUG.store(enabled, Ordering::Relaxed); }

pub(super) fn split_boundary(previous: (i32, i32), next: (i32, i32)) -> bool {
    previous.0 >= 0 && previous.1 >= 0
        && (previous.0 != previous.1) != (next.0 != next.1)
}

const SPINE: [&str; 3] = ["lower_lumbar", "upper_lumbar", "thoracic"];
fn angle_delta(from: f32, to: f32) -> f32 { (from - to + 180.0).rem_euclid(360.0) - 180.0 }

#[derive(Clone, Default)]
pub(super) struct SpineTransition {
    previous: [Option<[f32; 3]>; 3],
    residual: [[f32; 3]; 3],
    started: i32,
    duration: i32,
    last_time: Option<i32>,
    mode: u8,
}

impl SpineTransition {
    /// Candidate B carries only the procedural spine-angle discontinuity into
    /// the new animation's blend window. Aim and native angle state still
    /// advance normally; the residual reaches exactly zero at the blend end.
    pub(super) fn apply(&mut self, result: &mut PlayerAngleResult, time: i32,
        mode: u8, eligible: bool, boundary: bool, duration: i32) {
        if !eligible || self.mode != mode || self.last_time.is_some_and(|last|
            time < last || time.saturating_sub(last) > 250) {
            *self = Self { mode, ..Self::default() };
        }
        if !eligible { return; }
        let count = result.commands().len();
        if mode == 2 && boundary && self.last_time.is_some() {
            self.started = time;
            self.duration = duration.max(1);
            self.residual = [[0.0; 3]; 3];
            for command in &result.commands[..count] {
                if let Some(i) = SPINE.iter().position(|name| *name == command.bone_name()) {
                    if let Some(previous) = self.previous[i] {
                        self.residual[i] = std::array::from_fn(|axis|
                            angle_delta(previous[axis], command.angles[axis]));
                    }
                }
            }
        }
        let weight = if mode == 2 && self.duration > 0 {
            1.0 - (time.saturating_sub(self.started) as f32 / self.duration as f32).clamp(0.0, 1.0)
        } else { 0.0 };
        self.previous = [None; 3];
        for command in &mut result.commands[..count] {
            if let Some(i) = SPINE.iter().position(|name| *name == command.bone_name()) {
                for axis in 0..3 { command.angles[axis] += self.residual[i][axis] * weight; }
                self.previous[i] = Some(command.angles);
            }
        }
        self.last_time = Some(time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(angle: f32) -> PlayerAngleResult {
        let mut result = PlayerAngleResult::default();
        result.command_count = 1;
        result.commands[0].bone_name[..12].copy_from_slice(b"lower_lumbar");
        result.commands[0].angles = [0.0, angle, 0.0];
        result
    }
    #[test]
    fn detects_only_split_full_body_boundaries() {
        assert!(split_boundary((851,330),(281,281)));
        assert!(split_boundary((281,281),(851,330)));
        assert!(!split_boundary((851,330),(851,331)));
        assert!(!split_boundary((281,281),(338,338)));
        assert!(!split_boundary((-1,-1),(281,281)));
    }
    #[test]
    fn preserves_start_and_reaches_native_angles_at_blend_end() {
        let mut state = SpineTransition::default();
        state.apply(&mut result(50.0), 1000, 2, true, false, 100);
        for (time, expected) in [(1001,50.0),(1051,25.0),(1101,0.0),(1102,0.0)] {
            let mut frame = result(0.0);
            state.apply(&mut frame, time, 2, true, time == 1001, 100);
            assert!((frame.commands[0].angles[1] - expected).abs() < 0.001);
        }
    }
    #[test]
    fn uses_source_time_and_keeps_new_aim_changes() {
        let mut state = SpineTransition::default();
        state.apply(&mut result(50.0), 1000, 2, true, false, 100);
        state.apply(&mut result(0.0), 1001, 2, true, true, 100);
        let mut frame = result(10.0);
        state.apply(&mut frame, 1051, 2, true, false, 100);
        assert_eq!(frame.commands[0].angles[1], 35.0);
        let mut repeated = result(10.0);
        state.apply(&mut repeated, 1051, 2, true, false, 100);
        assert_eq!(frame, repeated);
    }
    #[test]
    fn resets_on_rewind_gap_ineligible_and_mode_switch() {
        for (time, mode, eligible) in [(999,2,true),(1300,2,true),(1001,0,true),(1001,2,false)] {
            let mut state = SpineTransition::default();
            state.apply(&mut result(50.0), 1000, 2, true, false, 100);
            let mut frame = result(0.0);
            state.apply(&mut frame, time, mode, eligible, true, 100);
            assert_eq!(frame.commands[0].angles[1], 0.0);
        }
    }
    #[test]
    fn short_arc_and_interrupted_blends_stay_continuous() {
        let mut state = SpineTransition::default();
        state.apply(&mut result(179.0), 1000, 2, true, false, 100);
        let mut frame = result(-179.0);
        state.apply(&mut frame, 1001, 2, true, true, 100);
        assert_eq!(frame.commands[0].angles[1], -181.0);
        let mut next = result(0.0);
        state.apply(&mut next, 1002, 2, true, true, 100);
        assert_eq!(angle_delta(next.commands[0].angles[1], frame.commands[0].angles[1]), 0.0);
    }
}
