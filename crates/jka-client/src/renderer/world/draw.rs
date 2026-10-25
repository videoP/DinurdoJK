//! World draw.
use crate::renderer::WorldBatch;

/// Whether promoted water owns this authored water batch, so ordinary and wireframe passes
/// must not draw it. Suppress sibling passes only when their face actually
/// received a replacement. Rejected faces and faces past the cap keep every
/// authored stage, so enabling the feature cannot remove unpromoted water.
pub(in crate::renderer) fn ocean_suppresses_batch(ocean_enabled: bool, batch: &WorldBatch) -> bool {
    ocean_enabled && batch.ocean_replaced
}

/// Render-only cvar filter for static BSP surfaces whose *original* shader qpath
/// starts with models/.  Keep this single predicate shared by the normal world
/// pass, its indirect/replay paths, wireframe, depth, raster shadows and
/// reflections. Remapping to `clear` is a
/// separate, resolved-material check (`contributes_to_world_wireframe`).
#[inline]
pub(in crate::renderer) fn hidden_bsp_model_surface(
    draw_bsp_model_surfaces: bool,
    batch: &WorldBatch,
    materials: &crate::scene::MaterialDebugInfo,
) -> bool {
    bsp_model_surface_name_hidden(
        draw_bsp_model_surfaces,
        batch.is_inline_entity,
        materials.entries
            .get(batch.source.material_debug_index as usize)
            .map(|entry| entry.name.as_str()),
    )
}

#[inline]
fn bsp_model_surface_name_hidden(
    enabled: bool,
    is_inline_entity: bool,
    original_shader_name: Option<&str>,
) -> bool {
    !enabled && !is_inline_entity
        && original_shader_name.is_some_and(|name| name.starts_with("models/"))
}

#[cfg(test)]
mod bsp_model_surface_tests {
    use super::bsp_model_surface_name_hidden;

    #[test]
    fn toggling_bsp_model_surfaces_hides_only_static_models_materials() {
        assert!(bsp_model_surface_name_hidden(false, false, Some("models/map_objects/tree")));
        assert!(!bsp_model_surface_name_hidden(true, false, Some("models/map_objects/tree")));
        assert!(!bsp_model_surface_name_hidden(false, true, Some("models/map_objects/tree")));
        assert!(!bsp_model_surface_name_hidden(false, false, Some("textures/stone/floor")));
        assert!(!bsp_model_surface_name_hidden(false, false, None));
    }
}

/// Every face merged into one ocean surface points at the same clipmap, so a
/// pass must draw it once. `drawn` is a per-pass bit set of surface ids; batches
/// without a clipmap always pass.
pub(in crate::renderer) fn claim_ocean_clipmap(drawn: &mut u8, batch: &WorldBatch) -> bool {
    if batch.ocean_clipmap.is_none() || batch.ocean_clipmap_id >= 8 {
        return true;
    }
    let bit = 1u8 << batch.ocean_clipmap_id;
    let first = *drawn & bit == 0;
    *drawn |= bit;
    first
}

pub(in crate::renderer) fn draw_world_batch<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    batch: &WorldBatch,
    instances: std::ops::Range<u32>,
    ocean_quality: Option<usize>,
) {
    let range = match (ocean_quality, batch.ocean_clipmap.as_ref()) {
        (Some(quality), Some(clipmap)) => &clipmap[quality.min(1)],
        _ => &batch.indexed_range,
    };
    pass.draw_indexed(range.clone(), 0, instances);
}

/// Secondary cameras/shadows use immutable full-detail patch geometry. This
/// prevents the main camera's coarse selection from degrading another view.
pub(in crate::renderer) fn secondary_batch_range(batch: &WorldBatch) -> &std::ops::Range<u32> {
    if batch.patch_geometry.is_some() && !batch.patch_suppressed { &batch.full_indexed_range }
    else { &batch.indexed_range }
}

pub(in crate::renderer) fn draw_world_batch_secondary<'a>(
    pass: &mut wgpu::RenderPass<'a>, batch: &WorldBatch,
    instances: std::ops::Range<u32>, ocean_quality: Option<usize>,
) {
    let range=match (ocean_quality,batch.ocean_clipmap.as_ref()) {
        (Some(quality),Some(clipmap))=>&clipmap[quality.min(1)],
        _=>secondary_batch_range(batch),
    };
    pass.draw_indexed(range.clone(),0,instances);
}

pub(in crate::renderer) fn inspector_index_range(
    batches: &[WorldBatch],
    source_vertices: &std::ops::Range<u32>,
) -> Option<std::ops::Range<u32>> {
    batches.iter().find_map(|batch| {
        if source_vertices.start < batch.source.vertices.start
            || source_vertices.end > batch.source.vertices.end
        {
            return None;
        }
        let offset = source_vertices.start - batch.source.vertices.start;
        let start = batch.full_indexed_range.start.checked_add(offset)?;
        let count = source_vertices.end - source_vertices.start;
        Some(start..start.checked_add(count)?)
    })
}

#[allow(clippy::too_many_arguments)]
/// Linear average colour of the *primary authored skybox* (six face slots).
/// Independent of BSP draw-batch count, PVS mode, and camera position.
///
/// Each face is read from the smallest mip its decode produced, which the mip
/// chain has already reduced to (very nearly) that face's mean, so this costs a
/// handful of texel reads rather than a full-resolution scan. Stored bytes are
/// sRGB-encoded for colour textures, so they are linearised before averaging;
/// averaging them encoded would bias the result bright.
pub(in crate::renderer) fn average_skybox_color(
    texture_data: &[crate::materials::TextureData],
    skybox: Option<[usize; 6]>,
) -> [f32; 3] {
    // Matches the reference's overcast daylight sky, used when a map has no
    // skybox at all (fog-only or fully enclosed levels).
    const FALLBACK: [f32; 3] = [0.45, 0.55, 0.75];
    let Some(faces) = skybox else {
        return FALLBACK;
    };

    // A skybox has exactly six slots, regardless of PVS granularity. An image
    // used in multiple slots contributes to each slot, but is only scanned
    // once. Use an sRGB transfer LUT for faces without generated small mips.
    let srgb_to_linear: [f64; 256] = std::array::from_fn(|byte| {
        let raw = byte as f32 / 255.0;
        let linear = if raw <= 0.04045 {
            raw / 12.92
        } else {
            ((raw + 0.055) / 1.055).powf(2.4)
        };
        f64::from(linear)
    });
    let mut face_cache = std::collections::HashMap::<usize, Option<[f64; 3]>>::new();
    let mut total = [0.0f64; 3];
    let mut counted = 0u32;
    for index in faces {
        let cached = face_cache.entry(index).or_insert_with(|| {
            let data = texture_data.get(index)?;
            // Walk the packed mip chain to the start of the smallest level.
            let mut offset = 0usize;
            let mut width = data.width.max(1);
            let mut height = data.height.max(1);
            for _ in 1..data.mip_level_count.max(1) {
                offset += (width * height * 4) as usize;
                width = (width / 2).max(1);
                height = (height / 2).max(1);
            }
            let texels = (width * height) as usize;
            if texels == 0 || offset + texels * 4 > data.rgba.len() {
                return None;
            }
            let mut average = [0.0f64; 3];
            for texel in 0..texels {
                let at = offset + texel * 4;
                for (channel, sum) in average.iter_mut().enumerate() {
                    let value = data.rgba[at + channel];
                    *sum += if data.srgb {
                        srgb_to_linear[value as usize]
                    } else {
                        f64::from(value as f32 / 255.0)
                    };
                }
            }
            for component in &mut average {
                *component /= texels as f64;
            }
            Some(average)
        });
        if let Some(average) = cached.as_ref() {
            // Different face slots have equal weight, even when two slots
            // reference the same image. BSP/PVS draw counts have no influence.
            for channel in 0..3 {
                total[channel] += average[channel];
            }
            counted += 1;
        }
    }

    if counted == 0 {
        return FALLBACK;
    }
    let scale = f64::from(counted);
    [
        (total[0] / scale) as f32,
        (total[1] / scale) as f32,
        (total[2] / scale) as f32,
    ]
}
