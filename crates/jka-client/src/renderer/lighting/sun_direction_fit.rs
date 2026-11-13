//! One-shot, CPU-only search for a lost/incorrect q3map_sun direction.
//! Compares low-resolution *local* lightmap gradients against traced shadow
//! discontinuities; never compares whole-map brightness or modifies GPU state.
use crate::renderer::{
    collect_static_ao_lightmap_triangles, collect_static_ao_render_triangles, scene, DrawBatch,
    GpuVertex, StaticAoBvh, StaticAoTexelSample, Vec3,
};
use crate::materials::TextureData;
use std::sync::atomic::{AtomicBool, Ordering};

const DOWNSAMPLE: u32 = 4;
const MAX_PAIRS: usize = 1800;
const MAX_PER_TILE: usize = 3;
const MIN_PAIRS: usize = 48;
const MIN_TRANSITIONS: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DirectionFit {
    pub azimuth: f32,
    pub elevation: f32,
    pub confidence: f32,
    pub transitions: usize,
}

#[derive(Clone, Copy)]
struct Pair {
    a: StaticAoTexelSample,
    b: StaticAoTexelSample,
    observed: f32,
    tile: usize,
}

fn luma(image: &TextureData, x: usize, y: usize) -> Option<f32> {
    let i = (y * image.width as usize + x).checked_mul(4)?;
    let pixel = image.rgba.get(i..i + 3)?;
    if pixel.iter().any(|&c| c >= 250) { return None; }
    let linear = |v: u8| {
        let c = v as f32 / 255.0;
        if image.srgb {
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        } else { c }
    };
    Some(0.2126 * linear(pixel[0]) + 0.7152 * linear(pixel[1]) + 0.0722 * linear(pixel[2]))
}

fn gather_pairs(
    vertices: &[GpuVertex], batches: &[DrawBatch], lightmaps: &[TextureData],
    cancelled: &AtomicBool,
) -> Result<Vec<Pair>, String> {
    let triangles = collect_static_ao_lightmap_triangles(vertices, batches);
    let sizes: Vec<_> = lightmaps.iter().map(|image| [
        image.width.div_ceil(DOWNSAMPLE), image.height.div_ceil(DOWNSAMPLE),
    ]).collect();
    let texels = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(&triangles, &sizes, 1);
    let owners = crate::renderer::static_ao::bvh::rasterize_lightmap_owners(&triangles, &sizes, 1);
    let mut tiles = Vec::<Vec<Pair>>::new();
    let mut tile_base = 0usize;
    for (page, image) in lightmaps.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) { return Err("cancelled".into()); }
        if image.rgba16f.is_some() || image.mip_level_count != 1 || image.width < 16 || image.height < 16 {
            continue;
        }
        let w = sizes[page][0] as usize;
        let h = sizes[page][1] as usize;
        let tiles_w = w.div_ceil(8);
        let tiles_h = h.div_ceil(8);
        tiles.resize_with(tile_base + tiles_w * tiles_h, Vec::new);
        for y in 0..h.saturating_sub(1) {
            if y % 32 == 0 && cancelled.load(Ordering::Relaxed) { return Err("cancelled".into()); }
            for x in 0..w.saturating_sub(1) {
                let i = y * w + x;
                if owners[page][i] == 0 { continue; }
                let Some(a) = texels[page][i] else { continue; };
                let Some(la) = luma(image,
                    (x * DOWNSAMPLE as usize + 2).min(image.width as usize - 1),
                    (y * DOWNSAMPLE as usize + 2).min(image.height as usize - 1)) else { continue; };
                for (dx, dy) in [(1, 0), (0, 1)] {
                    let j = (y + dy) * w + x + dx;
                    if owners[page][j] != owners[page][i] { continue; }
                    let Some(b) = texels[page][j] else { continue; };
                    let na = Vec3::from_array(a.normal);
                    let nb = Vec3::from_array(b.normal);
                    if na.dot(nb) < 0.995 ||
                        na.dot(Vec3::from_array(b.position) - Vec3::from_array(a.position)).abs() > 1.0 {
                        continue;
                    }
                    let Some(lb) = luma(image,
                        ((x + dx) * DOWNSAMPLE as usize + 2).min(image.width as usize - 1),
                        ((y + dy) * DOWNSAMPLE as usize + 2).min(image.height as usize - 1)) else { continue; };
                    let observed = lb - la;
                    // Do not let nearly uniform textures or smooth ambient
                    // changes dominate the directional shadow-edge signal.
                    if observed.abs() < 0.035 { continue; }
                    let tile = tile_base + (y / 8) * tiles_w + x / 8;
                    let bucket = &mut tiles[tile];
                    bucket.push(Pair { a, b, observed, tile });
                    if bucket.len() > MAX_PER_TILE {
                        bucket.sort_unstable_by(|a, b| b.observed.abs().total_cmp(&a.observed.abs()));
                        bucket.truncate(MAX_PER_TILE);
                    }
                }
            }
        }
        tile_base += tiles_w * tiles_h;
    }
    // Spatial balancing: choose at most three highest-contrast edges per
    // atlas tile, then a fixed bounded set across the whole level.
    let mut pairs = Vec::new();
    for bucket in tiles { pairs.extend(bucket); }
    if pairs.len() < MIN_PAIRS { return Err("too few reliable lightmap shadow gradients".into()); }
    // A deterministic stride preserves coverage rather than taking only the
    // most contrast-heavy region from a single room.
    if pairs.len() > MAX_PAIRS {
        let len = pairs.len();
        pairs = (0..MAX_PAIRS).map(|i| pairs[i * len / MAX_PAIRS]).collect();
    }
    Ok(pairs)
}

#[derive(Clone, Copy, Default)]
struct Score {
    correlation: f32,
    transitions: usize,
    regions: usize,
}

#[derive(Default, Clone, Copy)]
struct Evidence {
    dot: f64,
    observed2: f64,
    predicted2: f64,
    transitions: usize,
}

fn evaluate(pairs: &[Pair], bvh: &StaticAoBvh, azimuth: f32, elevation: f32, cancelled: &AtomicBool) -> Score {
    let sun = scene::DirectionalSun::from_q3_angles([1.0; 3], 350.0, azimuth, elevation);
    let toward = -Vec3::from_array(sun.direction).normalize_or_zero();
    // Independent interleaved tiles act as a cheap spatial holdout. A few
    // accidentally matching lamp edges cannot validate their own direction.
    let mut bins = [Evidence::default(); 2];
    let mut regions = 0usize;
    let mut last_tile = usize::MAX;
    for (index, pair) in pairs.iter().enumerate() {
        if index % 128 == 0 && cancelled.load(Ordering::Relaxed) { return Score::default(); }
        let normal = Vec3::from_array(pair.a.normal);
        let cosine = normal.dot(toward);
        if cosine < 0.15 { continue; }
        let visible = |sample: StaticAoTexelSample| {
            let origin = Vec3::from_array(sample.position) + Vec3::from_array(sample.normal) * 1.5 + toward * 1.5;
            bvh.trace(origin, toward, 131072.0).is_none()
        };
        let a = visible(pair.a);
        let b = visible(pair.b);
        if a == b { continue; }
        let predicted = (f32::from(b as u8) - f32::from(a as u8)) * cosine;
        let observed = pair.observed;
        let bin = &mut bins[pair.tile % 2];
        bin.dot += f64::from(predicted * observed);
        bin.observed2 += f64::from(observed * observed);
        bin.predicted2 += f64::from(predicted * predicted);
        bin.transitions += 1;
        if pair.tile != last_tile { regions += 1; last_tile = pair.tile; }
    }
    let transitions = bins.iter().map(|bin| bin.transitions).sum();
    if transitions < MIN_TRANSITIONS || regions < 8 || bins.iter().any(|b| b.transitions < 5 || b.observed2 <= 1e-5 || b.predicted2 <= 1e-5) {
        return Score::default();
    }
    let correlation = bins.iter()
        .map(|b| (b.dot / (b.observed2 * b.predicted2).sqrt()) as f32)
        .sum::<f32>() * 0.5;
    // Evidence support saturates gradually, but does not dilute good sun
    // boundaries with thousands of unrelated local lamp/lightmap gradients.
    let support = (transitions as f32 / (transitions as f32 + 24.0)).sqrt();
    Score { correlation: correlation * support, transitions, regions }
}

fn probe(
    measured: &mut std::collections::BTreeMap<(i32, i32), Score>,
    pairs: &[Pair], bvh: &StaticAoBvh, yaw: f32, pitch: f32, cancelled: &AtomicBool,
) {
    let yaw = ((yaw * 4.0).round() as i32).rem_euclid(1440);
    let pitch = ((pitch * 4.0).round() as i32).clamp(4, 356);
    if measured.contains_key(&(yaw, pitch)) { return; }
    let score = evaluate(pairs, bvh, yaw as f32 / 4.0, pitch as f32 / 4.0, cancelled);
    measured.insert((yaw, pitch), score);
}

pub(super) fn search(
    vertices: &[GpuVertex], batches: &[DrawBatch], lightmaps: &[TextureData],
    cancelled: &AtomicBool,
) -> Result<DirectionFit, String> {
    if cancelled.load(Ordering::Relaxed) { return Err("cancelled".into()); }
    let pairs = gather_pairs(vertices, batches, lightmaps, cancelled)?;
    let casters = collect_static_ao_render_triangles(vertices, batches);
    if casters.is_empty() { return Err("no usable opaque BSP shadow casters".into()); }
    let bvh = StaticAoBvh::build(casters.into()).map_err(|e| format!("sun-search BVH: {e:?}"))?;
    let mut measured = std::collections::BTreeMap::<(i32, i32), Score>::new();
    // Coarse upper hemisphere, then local angular refinement of several
    // independent promising basins (not just a single potentially false peak).
    for pitch in (5..=85).step_by(10) {
        for yaw in (0..360).step_by(20) {
            probe(&mut measured, &pairs, &bvh, yaw as f32, pitch as f32, cancelled);
        }
    }
    for step in [7.0, 2.0, 0.5] {
        if cancelled.load(Ordering::Relaxed) { return Err("cancelled".into()); }
        let mut best: Vec<_> = measured.iter().map(|(&(yaw, pitch), score)| (yaw, pitch, score.correlation)).collect();
        best.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        let mut centers = Vec::<(f32, f32)>::new();
        for (yaw, pitch, _) in best {
            let (yaw, pitch) = (yaw as f32 / 4.0, pitch as f32 / 4.0);
            if centers.iter().all(|&(cy, cp)| {
                let dy = (yaw - cy).abs(); dy.min(360.0 - dy) > step * 1.5 || (pitch - cp).abs() > step * 1.5
            }) {
                centers.push((yaw, pitch));
            }
            if centers.len() == 5 { break; }
        }
        for (yaw, pitch) in centers {
            for dp in [-1.0, 0.0, 1.0] {
                for da in [-1.0, 0.0, 1.0] {
                    probe(&mut measured, &pairs, &bvh, yaw + da * step, pitch + dp * step, cancelled);
                }
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) { return Err("cancelled".into()); }
    let Some((&(yaw, pitch), score)) = measured.iter()
        .max_by(|a, b| a.1.correlation.total_cmp(&b.1.correlation)) else {
        return Err("no candidate directions".into());
    };
    // Refuse low-confidence maps; never silently subtract arbitrary light.
    if score.correlation < 0.35 || score.transitions < MIN_TRANSITIONS || score.regions < 8 {
        return Err(format!("ambiguous shadows (best correlation {:.3}, {} crossing pairs in {} regions)",
            score.correlation, score.transitions, score.regions));
    }
    Ok(DirectionFit {
        azimuth: yaw as f32 / 4.0,
        elevation: pitch as f32 / 4.0,
        confidence: score.correlation,
        transitions: score.transitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uniform_lightmap_contains_no_directional_shadow_evidence() {
        let image = TextureData {
            label: "sun test".into(), source: None, width: 32, height: 32,
            rgba: vec![128u8, 128, 128, 255].repeat(1024).into(),
            rgba16f: None, mip_level_count: 1, clamp: true, srgb: false,
        };
        assert!(luma(&image, 5, 5).is_some());
        assert_eq!(luma(&image, 5, 5), luma(&image, 10, 10));
        assert!(search(&[], &[], &[image], &AtomicBool::new(false)).is_err());
    }
}
