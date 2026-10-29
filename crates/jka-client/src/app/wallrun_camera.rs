//! Vertical wallrun camera experiments, presentation only.
//!
//! JKA's `PM_AdjustAngleForWallRunUp` aligns the view yaw to the current
//! forward `trace.plane.normal` every PMove tick. A single unstable contact
//! normal can jump the view in a perfectly straight vertical wallrun.
//! These modes never touch predicted/playerState angles, usercmd, or DM26 data.

#[derive(Clone, Copy, Debug)]
struct LastCameraYaw {
    client: i32,
    time_ms: f64,
    yaw: f32,
    running: bool,
    mode: u8,
    candidate: Option<f32>,
    candidate_count: u8,
}

#[derive(Default)]
pub(super) struct WallrunCameraYaw {
    last: Option<LastCameraYaw>,
}

/// Only the vertical run-up; the left/right wallruns use a different JKA
/// movement function and are deliberately excluded from this experiment.
pub(super) fn is_wall_running(anim: i32) -> bool {
    static WALLRUN_UP: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    let id = *WALLRUN_UP.get_or_init(|| {
        jka_assets::animation::animation_index("BOTH_FORCEWALLRUNFLIP_START")
            .and_then(|id| i32::try_from(id).ok()).unwrap_or(-1)
    });
    (anim & !2048) == id
}

fn angle_delta(from: f32, to: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

impl WallrunCameraYaw {
    /// 0 = raw, 1 = hold initial yaw, 2 = 85 deg/s, 3 = isolated spike reject.
    pub(super) fn apply(
        &mut self, yaw: f32, anim: i32, client: i32,
        presentation_time_ms: f64, teleported: bool, mode: u8,
    ) -> f32 {
        if mode == 0 || !yaw.is_finite() || !presentation_time_ms.is_finite() || teleported {
            self.last = None;
            return yaw;
        }
        let running = is_wall_running(anim);
        let mut presented = yaw;
        let mut candidate = None;
        let mut candidate_count = 0;
        if let Some(last) = self.last {
            let dt = presentation_time_ms - last.time_ms;
            // Start each vertical run at its first measured angle. Never carry
            // an old wall's yaw across teleports, cuts, seeks, or a fresh run.
            if running && last.running && last.client == client && last.mode == mode
                && dt >= 0.0 && dt <= 150.0
            {
                // Render Studio can sample two camera keys at the same time
                // for a transition. Do not count that as two independent
                // measurements (especially in the spike-rejection mode).
                if dt == 0.0 { return last.yaw; }
                let delta = angle_delta(last.yaw, yaw);
                presented = match mode {
                    1 => last.yaw, // lock at the first wallrun sample
                    2 => last.yaw + delta.clamp(-(dt as f32 * 0.085), dt as f32 * 0.085),
                    3 if delta.abs() > 25.0 => {
                        // A truly different wall angle must persist in three
                        // consecutive snapshots before we accept it. One-off
                        // plane switches never get into the camera.
                        if last.candidate.is_some_and(|c| angle_delta(c, yaw).abs() <= 10.0) {
                            candidate_count = last.candidate_count.saturating_add(1);
                        } else {
                            candidate_count = 1;
                        }
                        if candidate_count >= 3 {
                            candidate_count = 0;
                            yaw
                        } else {
                            candidate = Some(yaw);
                            last.yaw
                        }
                    }
                    _ => yaw,
                };
            }
        }
        self.last = Some(LastCameraYaw {
            client, time_ms: presentation_time_ms, yaw: presented,
            running, mode, candidate, candidate_count,
        });
        presented
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn up() -> i32 {
        jka_assets::animation::animation_index("BOTH_FORCEWALLRUNFLIP_START").unwrap() as i32
    }
    fn sideways() -> i32 {
        jka_assets::animation::animation_index("BOTH_WALL_RUN_RIGHT").unwrap() as i32
    }
    #[test]
    fn vertical_only_raw_and_lock() {
        let mut filter = WallrunCameraYaw::default();
        assert!(is_wall_running(up() | 2048));
        assert!(!is_wall_running(sideways()));
        assert_eq!(filter.apply(20.0, up(), 1, 100.0, false, 0), 20.0);
        assert_eq!(filter.apply(150.0, up(), 1, 116.0, false, 0), 150.0);
        assert_eq!(filter.apply(20.0, up(), 1, 130.0, false, 1), 20.0);
        assert_eq!(filter.apply(150.0, up(), 1, 146.0, false, 1), 20.0);
        assert_eq!(filter.apply(50.0, 0, 1, 162.0, false, 1), 50.0);
        assert_eq!(filter.apply(90.0, up(), 1, 178.0, false, 1), 90.0);
    }
    #[test]
    fn speed_limit_and_short_arc() {
        let mut filter = WallrunCameraYaw::default();
        filter.apply(359.0, up(), 1, 100.0, false, 2);
        assert!((filter.apply(1.0, up(), 1, 120.0, false, 2) - 360.7).abs() < 0.01);
        assert_eq!(filter.apply(120.0, up(), 1, 90.0, false, 2), 120.0);
        assert_eq!(filter.apply(80.0, up(), 2, 100.0, false, 2), 80.0);
        assert_eq!(filter.apply(40.0, up(), 2, 110.0, true, 2), 40.0);
    }
    #[test]
    fn spike_rejection_waits_for_persistent_change() {
        let mut filter = WallrunCameraYaw::default();
        filter.apply(30.0, up(), 1, 100.0, false, 3);
        assert_eq!(filter.apply(170.0, up(), 1, 116.0, false, 3), 30.0);
        assert_eq!(filter.apply(30.0, up(), 1, 132.0, false, 3), 30.0);
        assert_eq!(filter.apply(170.0, up(), 1, 148.0, false, 3), 30.0);
        assert_eq!(filter.apply(171.0, up(), 1, 164.0, false, 3), 30.0);
        assert_eq!(filter.apply(170.0, up(), 1, 180.0, false, 3), 170.0);
        // Mode changes clear the previous candidate/filter state.
        assert_eq!(filter.apply(15.0, up(), 1, 196.0, false, 1), 15.0);
    }
}
