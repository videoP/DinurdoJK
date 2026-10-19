//! Cosmetics.
use crate::cgame::player_presenter::{
    model_bolt_matrix, transform_jka_model_point, transform_jka_model_vector, Arc, CosmeticDraw,
    DynamicModelSurface, Ghoul2PerfStats, GlaAnimation, GlmModel, Instant, Matrix3x4,
    PlayerModelAsset, SaberModelAsset, Vec3,
};

/// This frame's animated vehicle pose, also used by the *driver rider tag.
#[derive(Clone)]
pub(in crate::cgame::player_presenter) struct VehicleSnap {
    /// Lower-case `.veh` name.
    pub(in crate::cgame::player_presenter) vehicle: String,
    pub(in crate::cgame::player_presenter) frame_time: i32,
    pub(in crate::cgame::player_presenter) owner: i32,
    pub(in crate::cgame::player_presenter) yaw: f32,
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

/// Profile is intentionally studio-lit rather than map-lightgrid-lit. Bake
/// an ambient bounce + front key/fill + restrained back rim into preview-only
/// vertex lighting. All terms multiply the *existing* material/tint color:
/// they must never add white or saber RGB over the chosen `char_color_*`.
/// Gameplay models continue through the configured lighting.
pub fn apply_profile_studio_light(draws: &mut [DynamicModelSurface], cinematic: bool) {
    let key_direction = Vec3::new(-0.35, 0.72, 0.60).normalize();
    let fill_direction = Vec3::new(0.55, 0.20, -0.35).normalize();
    // Opposite the front key, slightly elevated to pick out silhouette edges.
    let back_direction = Vec3::new(0.25, -0.90, 0.38).normalize();
    for surface in draws {
        let vertices = Arc::make_mut(&mut surface.vertices);
        for vertex in vertices.iter_mut() {
            let normal = Vec3::from_array(vertex.normal).normalize_or_zero();
            let key = normal.dot(key_direction).max(0.0);
            let fill = normal.dot(fill_direction).max(0.0);
            let (base_light, back_rim) = if cinematic {
                // Former 0.18 ambient / 0.83 ceiling left even white skin
                // visibly dark. Keep the key directional, but lift shadows.
                let base = 0.43 + 0.46 * key + 0.18 * fill;
                let rim = 0.20 * normal.dot(back_direction).max(0.0).powi(2);
                (base, rim)
            } else {
                (0.38 + 0.72 * key + 0.20 * fill, 0.0)
            };
            for channel in 0..3 {
                // A very subtle cool rim adds shape without washing out the
                // model's chosen RGB. Cap highlights; never add naked light RGB.
                let rim_tint = [0.86, 0.94, 1.0][channel];
                let light = (base_light + back_rim * rim_tint)
                    .min(if cinematic { 1.10 } else { 1.18 });
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
            model: cosmetic.model.to_owned(),
            origin: position,
            axis: columns,
            rgba,
            custom_shader,
        });
    }
    out
}

/// Non-jaPRO `CG_DrawCosmeticOnPlayer2`. Reuse the same authoritative
/// Ghoul2 bolt axes as the jaPRO path; TaystJK's custom offsets are absolute
/// XYZ additions after the -2-up adjustment (not relative to the bolt basis).
#[allow(clippy::too_many_arguments)]
pub(in crate::cgame::player_presenter) fn global_cosmetic_draw(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    slot: crate::global_cosmetics::Slot,
    name: &str,
    offset: [f32; 3],
    entity_num: u16,
    alpha: f32,
    custom_shader: Option<&'static str>,
) -> Option<CosmeticDraw> {
    let path = crate::global_cosmetics::model_path(slot, name)?;
    let tag = match slot {
        crate::global_cosmetics::Slot::Hat => "*head_top",
        crate::global_cosmetics::Slot::Cape => "*back",
    };
    let m = model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, tag).ok()??;
    let column = |index: usize, sign: f32| {
        transform_jka_model_vector(
            [sign * m[0][index], sign * m[1][index], sign * m[2][index]],
            axis,
        )
    };
    let columns = [column(1, -1.0), column(0, 1.0), column(2, 1.0)];
    let mut position = transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, origin);
    for i in 0..3 { position[i] = position[i] - 2.0 * columns[2][i] + offset[i]; }
    Some(CosmeticDraw {
        entity_num, model: path, origin: position, axis: columns,
        rgba: [1.0, 1.0, 1.0, alpha], custom_shader,
    })
}

impl super::PlayerPresenter {
    /// Lazily load TaystJK's per-model/per-skin JSON offsets only when an
    /// actual MD3 is visible; avoid VFS+JSON work on the draw hot path.
    pub(in crate::cgame::player_presenter) fn global_cosmetic_offsets(
        &mut self,
        slot: crate::global_cosmetics::Slot,
        name: &str,
        model: &str,
        skin: &str,
    ) -> [f32; 3] {
        let key = (slot as u8, name.to_ascii_lowercase(), model.to_owned(), skin.to_owned());
        if let Some(offset) = self.global_cosmetic_offsets.get(&key) { return *offset; }
        let path = format!("{}/{}.cosmetic", slot.settings_dir(), name);
        let offset = self.assets.read(&path, 256 * 1024).ok().flatten()
            .and_then(|asset| serde_json::from_slice::<serde_json::Value>(&asset.bytes).ok())
            .map(|json| crate::global_cosmetics::offsets(&json, model, skin))
            .unwrap_or([0.0; 3]);
        self.global_cosmetic_offsets.insert(key, offset);
        offset
    }
}
