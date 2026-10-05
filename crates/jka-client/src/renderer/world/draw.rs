//! World draw.
use crate::renderer::WorldBatch;

/// Whether the FFT ocean owns this authored water batch, so the ordinary passes
/// must not draw it. The promoted face draws through its clipmap; sibling
/// stages, undersides and sides are dropped on purpose. Water that qualified
/// for promotion but got no clipmap (past the surface cap) is the one case the
/// ocean does not own: it keeps its authored draw instead of vanishing.
pub(in crate::renderer) fn ocean_suppresses_batch(ocean_enabled: bool, batch: &WorldBatch) -> bool {
    ocean_enabled
        && batch.source.water
        && !(batch.source.water_primary && batch.ocean_clipmap.is_none())
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
        let start = batch.indexed_range.start.checked_add(offset)?;
        let count = source_vertices.end - source_vertices.start;
        Some(start..start.checked_add(count)?)
    })
}

#[allow(clippy::too_many_arguments)]
/// Linear average colour of a map's skybox faces.
///
/// Each face is read from the smallest mip its decode produced, which the mip
/// chain has already reduced to (very nearly) that face's mean, so this costs a
/// handful of texel reads rather than a full-resolution scan. Stored bytes are
/// sRGB-encoded for colour textures, so they are linearised before averaging;
/// averaging them encoded would bias the result bright.
pub(in crate::renderer) fn average_skybox_color(
    texture_data: &[crate::materials::TextureData],
    faces: &[usize],
) -> [f32; 3] {
    // Matches the reference's overcast daylight sky, used when a map has no
    // skybox at all (fog-only or fully enclosed levels).
    const FALLBACK: [f32; 3] = [0.45, 0.55, 0.75];
    if faces.is_empty() {
        return FALLBACK;
    }

    let mut total = [0.0f64; 3];
    let mut counted = 0u32;
    for &index in faces {
        let Some(data) = texture_data.get(index) else {
            continue;
        };
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
            continue;
        }

        let mut face = [0.0f64; 3];
        for texel in 0..texels {
            for channel in 0..3 {
                let raw = data.rgba[offset + texel * 4 + channel] as f32 / 255.0;
                let linear = if data.srgb {
                    if raw <= 0.04045 {
                        raw / 12.92
                    } else {
                        ((raw + 0.055) / 1.055).powf(2.4)
                    }
                } else {
                    raw
                };
                face[channel] += f64::from(linear);
            }
        }
        for channel in 0..3 {
            total[channel] += face[channel] / texels as f64;
        }
        counted += 1;
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
