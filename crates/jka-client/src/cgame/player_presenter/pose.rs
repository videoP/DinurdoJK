//! Pose.
use crate::cgame::player_presenter::{
    model_bolt_matrix_timed, skin_glm_surface, transform_jka_model_point, Ghoul2PerfStats,
    Ghoul2SkinnedSurface, GlmSurface, Instant, Matrix3x4, PlayerModelAsset,
};

/// `PM_FootSlopeTrace`'s two `G2API_GetBoltMatrix` queries, in model space
/// (scaled like G2's `scale` argument). `None` while the model has no foot tags.
pub(in crate::cgame::player_presenter) fn foot_bolts_timed(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    scale: f32,
) -> Option<[[f32; 3]; 2]> {
    let mut foot = |name| {
        model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, name)
            .ok()
            .flatten()
            .map(|m| [m[0][3] * scale, m[1][3] * scale, m[2][3] * scale])
    };
    Some([foot("*l_leg_foot")?, foot("*r_leg_foot")?])
}

pub(in crate::cgame::player_presenter) fn model_bolt_origin_timed(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    entity_origin: [f32; 3],
    name: &str,
) -> Result<Option<[f32; 3]>, String> {
    Ok(
        model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, name)?
            .map(|m| transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, entity_origin)),
    )
}

pub(in crate::cgame::player_presenter) fn skin_surface_timed(
    perf: &mut Ghoul2PerfStats,
    surface: &GlmSurface,
    pose: &[Matrix3x4],
) -> Result<Ghoul2SkinnedSurface, String> {
    let started = Instant::now();
    let result = skin_glm_surface(surface, pose);
    perf.skin_ms += started.elapsed().as_secs_f64() * 1000.0;
    if let Ok(skinned) = &result {
        perf.surfaces_skinned = perf.surfaces_skinned.saturating_add(1);
        perf.vertices_skinned = perf
            .vertices_skinned
            .saturating_add(skinned.vertices.len() as u64);
    }
    result
}
