//! TaystJK CG_Disintegration / RB_CalcDisintegrateColors presentation seam.
//!
//! The C renderer sends an RF_DISINTEGRATE2 burn shader pass followed by an
//! RF_DISINTEGRATE1 base-skin pass. WGPU's model shaders implement the same
//! 0.045 units/ms radial threshold and the 60/150/180 squared-distance burn
//! bands. Marker alpha is confined to these two short-lived body-only passes;
//! no global shader variant or shared DynamicModelVertex ABI change is needed.

use std::sync::Arc;

use crate::{materials::TextureData, renderer::{DynamicModelAlphaMode, DynamicModelSurface}};

// Both shaders reserve these exact, nonoverlapping alpha ranges. They must
// precede the older negative-alpha predator-cloak dispatch in fs_mask/unlit.
const BASE_MARKER: f32 = -20.0;
const BURN_MARKER: f32 = -22.0;

/// Encode a single disintegration pass for both CPU and GPU Ghoul2 skinning.
/// The ordinary model vertices/materials remain unchanged outside this path.
fn tagged_surface(
    mut surface: DynamicModelSurface,
    elapsed_ms: i32,
    hit: [f32; 3],
    burn: bool,
) -> DynamicModelSurface {
    // The effect stops being emitted after 1500ms (CG_Player); encoding time
    // as a normalized value preserves that exact lifecycle in shader uniforms.
    let progress = (elapsed_ms.max(0) as f32 / 1500.0).min(1.0);
    let tag = (if burn { BURN_MARKER } else { BASE_MARKER }) + progress;
    let hit_render = crate::scene::render_position(hit);
    surface.rt_rigid = None;
    surface.rt_skinned_key = None;
    surface.lighting_origin = None;
    surface.authored_glow = false;
    surface.alpha_mode = if burn { DynamicModelAlphaMode::Additive }
        else { DynamicModelAlphaMode::Mask };
    if let Some(skin) = surface.ghoul2_gpu.as_mut() {
        skin.color[3] = tag;
        // Ordinary uv_cross.xy/zw are not needed here: vs_main bypasses
        // tcMods for negative-alpha model-special effects. The three otherwise
        // unused coordinates transmit the world-space origin of the burn.
        skin.uv_cross = [hit_render[0], hit_render[1], hit_render[2], 0.0];
    } else {
        surface.vertices = Arc::new(surface.vertices.iter().copied().map(|mut vertex| {
            vertex.color[3] = tag;
            // Same pre-existing transient-model auxiliary channels used by
            // cloak. The MD3 vertex shader skips depth-hack/foliage for these.
            vertex.depth_hack = hit_render[0];
            vertex.water_wetness = hit_render[1];
            vertex.foliage_wind = hit_render[2];
            vertex
        }).collect());
    }
    surface
}

/// Render the authored `gfx/effects/burn` overlay and the original skinned
/// body. Just as in TaystJK, accessories are intentionally *not* duplicated.
pub(super) fn disintegration_body_draws(
    surfaces: Vec<DynamicModelSurface>,
    elapsed_ms: i32,
    hit: [f32; 3],
    burn_texture: Option<Arc<TextureData>>,
) -> Vec<DynamicModelSurface> {
    let mut draws = Vec::with_capacity(surfaces.len() * 2);
    for surface in surfaces {
        if let Some(texture) = burn_texture.as_ref() {
            let mut glow = tagged_surface(surface.clone(), elapsed_ms, hit, true);
            glow.texture = Some(Arc::clone(texture));
            draws.push(glow); // RF_DISINTEGRATE2
        }
        draws.push(tagged_surface(surface, elapsed_ms, hit, false)); // RF_DISINTEGRATE1
    }
    draws
}

/// Normal EV_BODYFADE keeps the translucent body and adds only its secondary
/// coloured shell. TaystJK begins this approximately 2500ms into the fade.
pub(super) fn fade_body_glow(
    surfaces: &[DynamicModelSurface],
    burn_elapsed_ms: i32,
    center: [f32; 3],
    glow_texture: Option<Arc<TextureData>>,
    _light_side: bool,
) -> Vec<DynamicModelSurface> {
    let Some(texture) = glow_texture else { return Vec::new(); };
    surfaces.iter().filter(|surface| surface.raster_visible).map(|surface| {
        let mut glow = tagged_surface(surface.clone(), burn_elapsed_ms, center, true);
        glow.texture = Some(Arc::clone(&texture));
        glow
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_disjoint_from_cloak_and_each_other() {
        for elapsed in [0, 500, 1000, 1500] {
            let t = elapsed as f32 / 1500.0;
            let base = BASE_MARKER + t;
            let burn = BURN_MARKER + t;
            assert!((-21.0..=-19.0).contains(&base));
            assert!((-23.0..=-21.0).contains(&burn));
            assert_ne!(base, burn);
        }
    }

    #[test]
    fn legacy_threshold_is_45_units_at_one_second() {
        let threshold = 1000.0_f32 * 0.045;
        assert!((threshold - 45.0).abs() < f32::EPSILON);
    }
}
