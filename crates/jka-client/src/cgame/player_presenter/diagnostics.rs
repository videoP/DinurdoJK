//! Diagnostics.
use crate::cgame::player_presenter::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct Ghoul2PerfStats {
    pub pose_ms: f64,
    pub motion_pose_ms: f64,
    pub skin_ms: f64,
    pub bolt_ms: f64,
    pub pose_evals: u32,
    pub motion_pose_evals: u32,
    pub bolt_queries: u32,
    pub surfaces_skinned: u32,
    pub vertices_skinned: u64,
    pub frustum_tests: u32,
    pub frustum_culled: u32,
    pub lod_counts: [u32; 4],
}

pub(in crate::cgame::player_presenter) fn record_pose_eval(
    perf: &mut Ghoul2PerfStats,
    started: Instant,
) {
    perf.pose_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.pose_evals = perf.pose_evals.saturating_add(1);
}

pub(in crate::cgame::player_presenter) fn record_motion_pose_eval(
    perf: &mut Ghoul2PerfStats,
    started: Instant,
) {
    // Keep the lazy pre-angle Motion chain separate from full final skeleton
    // evaluation. This makes the profiler prove that the old second full pose
    // is gone instead of reporting both requests under g2_pose.
    perf.motion_pose_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.motion_pose_evals = perf.motion_pose_evals.saturating_add(1);
}
