//! Diagnostics.
use crate::cgame::player_presenter::Instant;

#[cfg(test)]
impl super::PlayerPresenter {
    pub(crate) fn swing_probe_animation(&self, entity: u16, time: i32) -> serde_json::Value {
        let Some(runtime) = self.entities.get(&entity) else { return serde_json::Value::Null };
        let gla = &runtime.model.animation_gla;
        let mut animator = runtime.animation.animator.clone();
        let bones: Vec<_> = ["model_root", "lower_lumbar", "Motion"].into_iter().map(|name| {
            let index = jka_assets::ghoul2::Ghoul2Animator::bone_index(gla, name).unwrap();
            let state = animator.bone_anim(index).copied().unwrap();
            serde_json::json!({"name": name, "frame": animator.bone_frame(gla, index, time).unwrap(),
                "start_frame": state.start_frame, "end_frame": state.end_frame,
                "start_time": state.start_time, "speed": state.anim_speed, "flags": state.flags,
                "blend_start": state.blend_start, "blend_time": state.blend_time,
                "blend_frame": state.blend_frame, "blend_lerp_frame": state.blend_lerp_frame})
        }).collect();
        // Evaluate a clone without procedural bone angles. This isolates the
        // authored GLA + transition blend from BG_G2PlayerAngles and smoothing.
        animator.clear_bone_angle_overrides();
        let raw_pose = animator.evaluate_pose_openjk_root(gla, time).unwrap();
        let raw_joints: serde_json::Map<String, serde_json::Value> =
            ["rhand", "lhand", "lower_lumbar", "thoracic", "Motion"].into_iter().filter_map(|name| {
                let index = jka_assets::ghoul2::Ghoul2Animator::bone_index(gla, name)?;
                Some((name.into(), serde_json::json!(raw_pose[index])))
            }).collect();
        serde_json::json!({"model": runtime.model_key, "bones": bones, "raw_joints": raw_joints})
    }
}

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
