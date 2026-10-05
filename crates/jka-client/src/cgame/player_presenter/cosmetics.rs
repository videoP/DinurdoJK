//! Cosmetics.
use crate::cgame::player_presenter::{
    model_bolt_matrix, transform_jka_model_point, transform_jka_model_vector, Arc, CosmeticDraw,
    DynamicModelSurface, Ghoul2PerfStats, GlaAnimation, GlmModel, Instant, Matrix3x4,
    PlayerModelAsset, SaberModelAsset, Vec3,
};

/// A vehicle's last presented (static) pose, kept for muzzle bolts.
#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct VehicleSnap {
    /// Lower-case `.veh` name.
    pub(in crate::cgame::player_presenter) vehicle: String,
    pub(in crate::cgame::player_presenter) model: Arc<SaberModelAsset>,
    pub(in crate::cgame::player_presenter) pose: Vec<Matrix3x4>,
    pub(in crate::cgame::player_presenter) axis: [[f32; 3]; 3],
    pub(in crate::cgame::player_presenter) origin: [f32; 3],
}

pub(in crate::cgame::player_presenter) fn model_bolt_matrix_timed(
    perf: &mut Ghoul2PerfStats,
    glm: &GlmModel,
    gla: &GlaAnimation,
    pose: &[Matrix3x4],
    name: &str,
) -> Result<Option<Matrix3x4>, String> {
    let started = Instant::now();
    let result = model_bolt_matrix(glm, gla, pose, name);
    perf.bolt_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.bolt_queries = perf.bolt_queries.saturating_add(1);
    result
}

/// Profile is intentionally studio-lit rather than map-lightgrid-lit. This bakes
/// a key + soft fill from vertex normals into preview-only shaderRGBA. Gameplay
/// models continue through the configured lighting.
pub fn apply_profile_studio_light(draws: &mut [DynamicModelSurface]) {
    for surface in draws {
        let vertices = Arc::make_mut(&mut surface.vertices);
        for vertex in vertices.iter_mut() {
            let normal = Vec3::from_array(vertex.normal).normalize_or_zero();
            let key = normal
                .dot(Vec3::new(-0.35, 0.72, 0.60).normalize())
                .max(0.0);
            let fill = normal
                .dot(Vec3::new(0.55, 0.20, -0.35).normalize())
                .max(0.0);
            let light = (0.38 + 0.72 * key + 0.20 * fill).min(1.18);
            for channel in 0..3 {
                vertex.color[channel] *= light;
            }
        }
        // Do not also sample the current BSP lightgrid in the isolated
        // preview; the studio lighting should be stable on every map.
        surface.lighting_origin = None;
    }
}

/// `CG_DrawCosmeticOnPlayer` for every cosmetic in `mask`: bolt the hat MD3s to
/// `*head_top` and the capes/carried items to `*back`.
///
/// The bolt matrix here is the internal (low) one. The public
/// `G2API_GetBoltMatrix` jaPRO reads post-multiplies it by a 270-degree yaw
/// ("lots of game code is written to assume this 90 degree offset thing"), so
/// forward is -column 1, the second axis is column 0 and up is column 2. Using
/// the raw columns turns every cosmetic 90 degrees.
#[allow(clippy::too_many_arguments)]
pub(in crate::cgame::player_presenter) fn cosmetic_draws_for_mask(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    mask: u32,
    entity_num: u16,
    rgba: [f32; 4],
    custom_shader: Option<&'static str>,
) -> Vec<CosmeticDraw> {
    use crate::japro_cg::{self, CosmeticSlot};
    let mut out = Vec::new();
    for cosmetic in japro_cg::cosmetics_to_draw(mask) {
        let tag = match cosmetic.slot {
            CosmeticSlot::Hat => "*head_top",
            CosmeticSlot::Back => "*back",
        };
        let Ok(Some(m)) = model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, tag) else {
            continue;
        };
        let column = |index: usize, sign: f32| {
            transform_jka_model_vector(
                [sign * m[0][index], sign * m[1][index], sign * m[2][index]],
                axis,
            )
        };
        let columns = [column(1, -1.0), column(0, 1.0), column(2, 1.0)];
        let mut position = transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, origin);
        // VectorMA(boltOrg, -2, re.axis[2], boltOrg)
        for (value, up) in position.iter_mut().zip(columns[2]) {
            *value -= 2.0 * up;
        }
        out.push(CosmeticDraw {
            entity_num,
            model: cosmetic.model,
            origin: position,
            axis: columns,
            rgba,
            custom_shader,
        });
    }
    out
}
