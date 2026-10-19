//! Presentation-only airborne strafejump/bunnyhop leaning.
//! Grounded or non-PM_NORMAL movement is deliberately NEVER modified.
//! Speed is horizontal UPS; longitudinal and lateral acceleration are
//! derived from world-space horizontal velocity differences, not yaw changes.
//! GLA animation, collision and movement remain unchanged.

#[derive(Clone, Debug, Default)]
pub(super) struct MoveLeanState {
    pub pitch: f32,
    pub roll: f32,
    last_planar_velocity: Option<[f32; 2]>,
    last_sample_ms: i32,
    last_frame_ms: i32,
    sampled_acceleration: f32,
    filtered_acceleration: f32,
    sampled_lateral_acceleration: f32,
    filtered_lateral_acceleration: f32,
}

/// Only actual jump/in-air animations, and only when groundEntityNum is NONE.
/// Run/walk anims are never accepted, even across a bunnyhop transition.
pub(super) fn allowed_animation(raw_anim: i32, on_ground: bool) -> bool {
    if on_ground { return false; }
    let anim = raw_anim & !super::physics::ANIM_TOGGLEBIT;
    matches!(jka_assets::animation::AnimationSet::name(anim),
        Some("BOTH_JUMP1" | "BOTH_INAIR1"
            | "BOTH_JUMPBACK1" | "BOTH_INAIRBACK1"
            | "BOTH_JUMPLEFT1" | "BOTH_INAIRLEFT1"
            | "BOTH_JUMPRIGHT1" | "BOTH_INAIRRIGHT1"
            | "BOTH_FORCEJUMP1" | "BOTH_FORCEINAIR1"
            | "BOTH_FORCEJUMPBACK1" | "BOTH_FORCEINAIRBACK1"
            | "BOTH_FORCEJUMPLEFT1" | "BOTH_FORCEINAIRLEFT1"
            | "BOTH_FORCEJUMPRIGHT1" | "BOTH_FORCEINAIRRIGHT1")
    )
}

impl MoveLeanState {
    pub fn reset(&mut self, time_ms: i32) {
        *self = Self {
            last_frame_ms: time_ms, last_sample_ms: time_ms, ..Self::default()
        };
    }

    pub fn update(
        &mut self, time_ms: i32, legs_anim: i32, on_ground: bool, normal_pm_movement: bool, yaw_degrees: f32,
        velocity: [f32; 3], strength: f32,
    ) -> [f32; 2] {
        let dt_ms = time_ms.saturating_sub(self.last_frame_ms);
        if time_ms < self.last_frame_ms || dt_ms > 250 {
            self.reset(time_ms);
        }
        self.last_frame_ms = time_ms;

        // Hard gate: do not apply or retain air lean outside PM_NORMAL
        // (noclip, spectator, dead, vehicle modes) or when grounded.
        if on_ground || !normal_pm_movement {
            self.reset(time_ms);
            return [0.0, 0.0];
        }
        let valid = allowed_animation(legs_anim, on_ground)
            && strength > 0.0
            && yaw_degrees.is_finite() && velocity.iter().all(|v| v.is_finite());
        if !valid {
            self.last_planar_velocity = None;
            self.last_sample_ms = time_ms;
            self.sampled_acceleration = 0.0;
            self.filtered_acceleration = 0.0;
            self.sampled_lateral_acceleration = 0.0;
            self.filtered_lateral_acceleration = 0.0;
            // A transition to a different midair anim can ease out, but on
            // touching ground we reset immediately (above).
            self.pitch = smooth(self.pitch, 0.0, dt_ms, 100.0);
            self.roll = smooth(self.roll, 0.0, dt_ms, 100.0);
            return [self.pitch, self.roll];
        }

        let planar_speed = velocity[0].hypot(velocity[1]);
        let (sin, cos) = yaw_degrees.to_radians().sin_cos();
        let forward = velocity[0]*cos + velocity[1]*sin;
        let left = -velocity[0]*sin + velocity[1]*cos;
        let fwd_fraction = forward / planar_speed.max(1.0);
        let left_fraction = left / planar_speed.max(1.0);

        // Work in actual world-space horizontal velocity, not a delta of
        // view-relative projected speed (which would mistake yaw as acceleration).
        // Sample over >=24ms to suppress 1ms render-frame / snapshot noise.
        if let Some(old) = self.last_planar_velocity {
            let elapsed = time_ms.saturating_sub(self.last_sample_ms);
            if elapsed >= 24 {
                let dv = [velocity[0] - old[0], velocity[1] - old[1]];
                // Large discontinuities (teleport, seek, correction) reset
                // rather than making the character fold in half.
                let valid = elapsed <= 180 && dv[0].hypot(dv[1]) < 300.0;
                let accel = if valid { 1000.0 / elapsed as f32 } else { 0.0 };
                let old_speed = old[0].hypot(old[1]);
                self.sampled_acceleration = if valid {
                    ((planar_speed - old_speed) * accel).clamp(-4000.0, 4000.0)
                } else {0.0};
                self.sampled_lateral_acceleration =
                    ((-dv[0] * sin + dv[1] * cos) * accel).clamp(-4000.0, 4000.0);
                self.last_planar_velocity = Some([velocity[0], velocity[1]]);
                self.last_sample_ms = time_ms;
            }
        } else {
            self.last_planar_velocity = Some([velocity[0], velocity[1]]);
            self.last_sample_ms = time_ms;
            self.sampled_acceleration = 0.0;
            self.sampled_lateral_acceleration = 0.0;
        }
        self.filtered_acceleration = smooth(
            self.filtered_acceleration, self.sampled_acceleration, dt_ms, 65.0);
        self.filtered_lateral_acceleration = smooth(
            self.filtered_lateral_acceleration, self.sampled_lateral_acceleration, dt_ms, 65.0);
        const AIR_SPEED_SCALE_UPS: f32 = 1200.0;
        // No lean through 500 UPS, then ease in ALL lean (speed and
        // acceleration/braking) to full strength by 800 UPS. A smoothstep
        // avoids a visible kink at either threshold.
        let t = ((planar_speed - 500.0) / 300.0).clamp(0.0, 1.0);
        let speed_gate = t * t * (3.0 - 2.0 * t);
        let speed_fraction = ((planar_speed - 60.0)/(AIR_SPEED_SCALE_UPS - 60.0)).clamp(0.0,1.0);
        let gain = strength * speed_gate;
        // Speed is intentionally only a subtle cruising posture, not the main
        // source of lean. Even at 1500 UPS steady speed yields ~6° pitch.
        let speed_pitch = 6.0 * speed_fraction * fwd_fraction;
        let speed_roll = -4.0 * speed_fraction * left_fraction;
        // Distinguish slow vs fast air acceleration. Braking is deliberately
        // stronger and reverses the lean even while travelling rapidly.
        let a = (self.filtered_acceleration / 900.0).clamp(-1.0, 1.0);
        let dynamic_pitch = if a >= 0.0 { 28.0 * a } else { 46.0 * a };
        // Lateral acceleration from changing world velocity creates a turn
        // lean, even with little sideways *speed* relative to the camera.
        let a_side = (self.filtered_lateral_acceleration / 1000.0).clamp(-1.0, 1.0);
        let pitch_target = ((speed_pitch + dynamic_pitch * fwd_fraction) * gain).clamp(-42.0, 36.0);
        let roll_target = ((speed_roll - 25.0 * a_side) * gain).clamp(-29.0, 29.0);
        let response = if a < 0.0 { 65.0 } else if a > 0.0 { 85.0 } else { 135.0 };
        self.pitch = smooth(self.pitch, pitch_target, dt_ms, response);
        self.roll = smooth(self.roll, roll_target, dt_ms, 95.0);
        if planar_speed <= 500.0 {
            // The speed gate is absolute, not merely a smoothed target.
            self.pitch = 0.0;
            self.roll = 0.0;
        }
        [self.pitch, self.roll]
    }
}

fn smooth(from: f32, to: f32, dt_ms: i32, response_ms: f32) -> f32 {
    if dt_ms < 0 || dt_ms > 250 { return to; }
    let alpha = 1.0 - (-(dt_ms.max(0) as f32) / response_ms).exp();
    from + (to-from)*alpha
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_assets::animation::animation_index;
    fn anim(s: &str) -> i32 { animation_index(s).unwrap() as i32 }
    fn air() -> i32 { anim("BOTH_INAIR1") }
    fn step(s: &mut MoveLeanState, t: i32, vx: f32, vy: f32) -> [f32; 2] {
        s.update(t, air(), false, true, 0.0, [vx,vy,300.0], 1.0)
    }
    #[test]
    fn no_ground_run_or_walk_changes_ever() {
        let run=anim("BOTH_RUN1");
        let walk=anim("BOTH_WALK1");
        assert!(!allowed_animation(run,true));
        assert!(!allowed_animation(walk,true));
        assert!(!allowed_animation(run,false));
        assert!(!allowed_animation(walk,false));
        assert!(!allowed_animation(air(),true));
        let mut s=MoveLeanState::default();s.reset(0);
        for t in (16..=320).step_by(16) { step(&mut s,t,1400.0,400.0); }
        assert!(s.pitch.abs()>1.0);
        assert_eq!(s.update(336,run,true,true,0.0,[1600.0,0.0,0.0],1.0),[0.0,0.0]);
        assert_eq!(s.pitch,0.0);
    }
    #[test]
    fn fast_airborne_strafejump_uses_full_planar_ups() {
        let mut s=MoveLeanState::default();s.reset(0);
        for t in (16..=480).step_by(16) { step(&mut s,t,1100.0,1000.0); }
        assert!(s.pitch>2.0 && s.pitch < 9.0,"steady speed is subtle: {}",s.pitch);
        assert!(s.roll < -1.0 && s.roll > -5.0,"roll={}",s.roll);
        assert!(!allowed_animation(anim("BOTH_FLIP_F"),false));
        assert!(!allowed_animation(anim("BOTH_SWIMFORWARD"),false));
    }
    #[test]
    fn air_acceleration_dominates_cruise_and_braking_reverses_at_speed() {
        fn run(rate: f32) -> f32 {
            let mut s=MoveLeanState::default();s.reset(0);
            for t in (16..=400).step_by(16) {step(&mut s,t,1000.0,0.0);}
            for i in 1..=9 {step(&mut s,400 + i*16,1000.0 + rate * i as f32,0.0);}
            s.pitch
        }
        let cruise = run(0.0);
        let slow = run(4.0);
        let fast = run(25.0);
        let brake = run(-25.0);
        assert!(fast > slow + 8.0,"slow={slow} fast={fast}");
        assert!(slow > cruise + 2.0,"slow={slow} cruise={cruise}");
        assert!(brake < -10.0,"brake={brake}");
    }
    #[test]
    fn lateral_acceleration_changes_roll_without_lateral_cruise_speed() {
        let mut s=MoveLeanState::default();s.reset(0);
        for t in (16..=400).step_by(16) {step(&mut s,t,1200.0,0.0);}
        let baseline=s.roll;
        for (i,y) in [20.0,60.0,100.0,140.0,180.0,220.0].into_iter().enumerate() {
            step(&mut s,416 + (i as i32)*16,1200.0,y);
        }
        assert!(s.roll < baseline - 5.0,"lateral accel roll={}, baseline={baseline}",s.roll);
    }
    #[test]
    fn single_move_lean_strength_scales_air_and_stays_off_on_ground() {
        fn sample(strength: f32) -> [f32; 2] {
            let mut state = MoveLeanState::default();
            state.reset(0);
            for t in (16..=400).step_by(16) {
                state.update(t, air(), false, true, 0.0, [1200.0, 200.0, 300.0], strength);
            }
            state.update(416, air(), false, true, 0.0, [1260.0, 230.0, 300.0], strength)
        }
        let off = sample(0.0);
        let half = sample(0.5);
        let full = sample(1.0);
        assert_eq!(off, [0.0, 0.0]);
        assert!(full[0].abs() > half[0].abs());
        assert!((half[0] - full[0] * 0.5).abs() < 0.1);
        assert!((half[1] - full[1] * 0.5).abs() < 0.1);
    }
    #[test]
    fn air_lean_requires_normal_pm_even_during_fast_inair_anim() {
        let mut state=MoveLeanState::default(); state.reset(0);
        for t in (16..=480).step_by(16) {
            state.update(t, air(), false, true, 0.0, [1500.0, 0.0, 0.0], 1.0);
        }
        assert!(state.pitch.abs() > 1.0);
        // PM_NOCLIP immediately clears even an established lean.
        assert_eq!(state.update(496,air(),false,false,0.0,[2000.0,0.0,0.0],1.0),[0.0,0.0]);
        assert_eq!(state.pitch,0.0);
        assert_eq!(state.roll,0.0);
    }
    #[test]
    fn threshold_disables_below_500_and_ramps_to_full_at_800_ups() {
        fn sustained(v: f32) -> [f32; 2] {
            let mut state=MoveLeanState::default(); state.reset(0);
            for t in (16..=720).step_by(16) {
                state.update(t, air(), false, true, 0.0, [v, 0.0, 0.0], 1.0);
            }
            [state.pitch,state.roll]
        }
        assert_eq!(sustained(450.0),[0.0,0.0]);
        assert_eq!(sustained(500.0),[0.0,0.0]);
        let at_650=sustained(650.0)[0];
        let at_800=sustained(800.0)[0];
        assert!(at_650 > 0.0 && at_650 < at_800,"650={at_650}, 800={at_800}");
    }
    #[test]
    fn speed_up_and_brake_in_air_and_ignore_yaw_only() {
        let mut s=MoveLeanState::default();s.reset(0);
        for t in (16..=480).step_by(16) { step(&mut s,t,1100.0,0.0); }
        let steady=s.pitch;
        for (t,v) in [(496,1180.0),(512,1250.0),(528,1350.0)] {step(&mut s,t,v,0.0);}
        assert!(s.pitch>steady,"accelerating={}; steady={steady}",s.pitch);
        // Sustain the deceleration long enough to overcome the smoothed
        // acceleration and previous forward lean, while still above 500 UPS.
        for i in 1..=18 {
            step(&mut s, 528 + i*16, 1350.0 - i as f32 * 40.0, 0.0);
        }
        assert!(s.pitch < 0.0,"braking at 630 UPS should tip back: {}",s.pitch);
        step(&mut s, 832, 480.0, 0.0);
        assert_eq!(s.pitch,0.0,"below the 500 UPS threshold there must be no lean");
        // Give the sampler two stable observations before a yaw-only change.
        step(&mut s, 848, 480.0, 0.0);
        let old=s.sampled_acceleration;
        s.update(880,air(),false,true,90.0,[480.0,0.0,0.0],1.0);
        assert_eq!(s.sampled_acceleration,0.0);
        assert!(old.is_finite());
    }
}
