//! Zone-guided sunlight removal ("Auto (Contour Guided)").
//!
//! The authored sun is traced into the original lightmap charts (unrefined
//! flux). Wherever that predicted sun has a shadow edge, the lightmap is
//! compared across the edge to measure how much direct sun the compiler really
//! baked there. Measurements are pooled in world-space zones (ZONE units, not
//! per chart), so a floor without its own shadow edge borrows from nearby
//! walls/floors on other charts. Zone strengths fade into each other through a
//! smooth world-space field, and the removal mask is `k(x) * visibility(x)`.
//!
//! Short spacings (2-4 luxels) are deliberate. Longer pairs also measure the
//! ambient/bounce falloff beside the occluding wall (shadow side darker because
//! the wall blocks sky), which a global fit misreads as stronger sun. A zone
//! whose lightmap shows no edge where the trace predicts one (geometry the
//! compiler saw differently) measures ~0 and removes ~nothing there.
use super::{compiled_lightmap_photons, median_f32};
use crate::renderer::{StaticAoTexelSample, TextureData, Vec3};
use crate::scene::BakedSunEncoding;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// World-space zone edge length.
const ZONE: f32 = 128.0;
/// Field kernel radius around a zone's evidence centroid.
const RADIUS: f32 = 2.0 * ZONE;
/// Weight of the map-wide prior at every field node. Dominates only where no
/// zone evidence exists within RADIUS.
const PRIOR_WEIGHT: f32 = 0.05;
const SPACINGS: [usize; 3] = [2, 3, 4];
const MIN_FLUX_STEP: f32 = 0.3;
const MAX_STRENGTH: f32 = 2.5;
/// The mask encodes up to this many times the authored direct flux.
pub(super) const ENCODE_SCALE: f32 = super::CONTOUR_MASK_SCALE;
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

#[derive(Clone, Copy)]
struct Zone {
    centroid: Vec3,
    strength: f32,
    confidence: f32,
}

#[derive(Clone, Copy)]
struct PairSample {
    cell: [i32; 3],
    ratio: f32,
    weight: f32,
    mid: Vec3,
}

pub(super) struct ZoneField {
    zones: Vec<Zone>,
    /// Zones bucketed by the cell holding their centroid.
    cells: HashMap<[i32; 3], Vec<u32>>,
    prior: f32,
}

fn cell_of(position: Vec3) -> [i32; 3] {
    let c = (position / ZONE).floor();
    [c.x as i32, c.y as i32, c.z as i32]
}

fn compatible(a: &StaticAoTexelSample, b: &StaticAoTexelSample) -> bool {
    let normal = Vec3::from_array(a.normal);
    normal.dot(Vec3::from_array(b.normal)) >= 0.995
        && normal.dot(Vec3::from_array(b.position) - Vec3::from_array(a.position)).abs() <= 0.75
}

fn weighted_median(values: &mut [(f32, f32)]) -> f32 {
    values.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let half = values.iter().map(|v| v.1).sum::<f32>() * 0.5;
    let mut accumulated = 0.0;
    for &(value, weight) in values.iter() {
        accumulated += weight;
        if accumulated >= half { return value; }
    }
    values.last().map_or(0.0, |v| v.0)
}

fn page_pairs(
    image: &TextureData, flux: &[u8], samples: &[Option<StaticAoTexelSample>], owners: &[u32],
    sun_y: f32, encoding: BakedSunEncoding, cancelled: &AtomicBool,
) -> Vec<PairSample> {
    let (width, height) = (image.width as usize, image.height as usize);
    // Unclipped, chart-owned luxels only. Clipped bytes are lower bounds and
    // say nothing about the sun's share.
    let luma: Vec<f32> = (0..width * height).into_par_iter().map(|i| {
        let pixel = &image.rgba[i * 4..i * 4 + 4];
        if owners[i] == 0 || samples[i].is_none() || pixel[..3].iter().any(|&c| c >= 250) {
            f32::NAN
        } else {
            compiled_lightmap_photons(pixel, encoding).dot(LUMA)
        }
    }).collect();
    (0..height).into_par_iter().flat_map_iter(|y| {
        let mut out = Vec::new();
        if cancelled.load(Ordering::Relaxed) { return out.into_iter(); }
        for x in 0..width {
            let i = y * width + x;
            if luma[i].is_nan() { continue; }
            let a = samples[i].as_ref().unwrap();
            let flux_a = flux[i] as f32 / 255.0;
            for d in SPACINGS {
                for (nx, ny) in [(x + d, y), (x, y + d)] {
                    if nx >= width || ny >= height { continue; }
                    let j = ny * width + nx;
                    if luma[j].is_nan() || owners[j] != owners[i] { continue; }
                    let b = samples[j].as_ref().unwrap();
                    if !compatible(a, b) { continue; }
                    let step = flux[j] as f32 / 255.0 - flux_a;
                    if step.abs() < MIN_FLUX_STEP { continue; }
                    let ratio = (luma[j] - luma[i]) / (sun_y * step);
                    // Negative ratios stay: they are evidence that the trace
                    // and the compiled lightmap disagree in this zone.
                    if !ratio.is_finite() || !(-0.5..=MAX_STRENGTH).contains(&ratio) { continue; }
                    let mid = (Vec3::from_array(a.position) + Vec3::from_array(b.position)) * 0.5;
                    out.push(PairSample { cell: cell_of(mid), ratio, weight: step * step, mid });
                }
            }
        }
        out.into_iter()
    }).collect()
}

/// Measure local direct-sun strength per world zone. `flux` must be the
/// UNREFINED traced visibility; RGB-refined flux would make the fit circular.
pub(super) fn fit_zones(
    lightmaps: &[TextureData], flux: &[Vec<u8>], samples: &[Vec<Option<StaticAoTexelSample>>],
    owners: &[Vec<u32>], sun_y: f32, encoding: BakedSunEncoding, cancelled: &AtomicBool,
) -> Option<ZoneField> {
    if sun_y <= 0.001 || !sun_y.is_finite() { return None; }
    let mut pairs = Vec::new();
    for (((image, flux), texels), chart) in lightmaps.iter().zip(flux).zip(samples).zip(owners) {
        if cancelled.load(Ordering::Relaxed) { return None; }
        let count = image.width as usize * image.height as usize;
        if count == 0 || image.rgba16f.is_some() || image.rgba.len() != count * 4
            || texels.len() != count || chart.len() != count || flux.len() != count { continue; }
        pairs.extend(page_pairs(image, flux, texels, chart, sun_y, encoding, cancelled));
    }
    if cancelled.load(Ordering::Relaxed) { return None; }
    pairs.par_sort_unstable_by_key(|pair| pair.cell);
    let zones: Vec<Zone> = pairs
        .chunk_by(|a, b| a.cell == b.cell)
        .filter_map(|group| {
            let total: f32 = group.iter().map(|p| p.weight).sum();
            if group.len() < 6 || total < 1.5 { return None; }
            let mut weighted: Vec<(f32, f32)> = group.iter().map(|p| (p.ratio, p.weight)).collect();
            let strength = weighted_median(&mut weighted).clamp(0.0, MAX_STRENGTH);
            let mut ratios: Vec<f32> = group.iter().map(|p| p.ratio).collect();
            ratios.sort_unstable_by(f32::total_cmp);
            let iqr = ratios[ratios.len() * 3 / 4] - ratios[ratios.len() / 4];
            let agreement = (1.0 - iqr / (0.3 + strength)).clamp(0.2, 1.0);
            let centroid = group.iter().fold(Vec3::ZERO, |sum, p| sum + p.mid * p.weight) / total;
            Some(Zone { centroid, strength, confidence: total / (total + 3.0) * agreement })
        })
        .collect();
    if zones.len() < 4 {
        println!("[BAKED SUN ZONES] only {} zones with shadow-edge evidence ({} pairs); unavailable", zones.len(), pairs.len());
        return None;
    }
    let mut votes: Vec<(f32, f32)> = zones.iter().map(|z| (z.strength, z.confidence)).collect();
    let prior = weighted_median(&mut votes);
    let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for (index, zone) in zones.iter().enumerate() {
        cells.entry(cell_of(zone.centroid)).or_default().push(index as u32);
    }
    let mut strengths: Vec<f32> = zones.iter().map(|z| z.strength).collect();
    let median = median_f32(&mut strengths);
    println!("[BAKED SUN ZONES] {} zones from {} edge pairs: confidence-weighted {:.3}x, plain median {:.3}x, range {:.3}-{:.3}",
        zones.len(), pairs.len(), prior, median, strengths[0], strengths[strengths.len() - 1]);
    Some(ZoneField { zones, cells, prior })
}

impl ZoneField {
    /// Map-wide strength: every zone votes once, so one huge floor or a few
    /// pages packed with edges cannot dominate.
    pub(super) fn global(&self) -> Option<f32> {
        (self.zones.len() >= 8).then_some(self.prior)
    }

    pub(super) fn len(&self) -> usize { self.zones.len() }

    fn node(&self, corner: [i32; 3]) -> f32 {
        let point = Vec3::new(corner[0] as f32, corner[1] as f32, corner[2] as f32) * ZONE;
        let mut numerator = PRIOR_WEIGHT * self.prior;
        let mut weights = PRIOR_WEIGHT;
        let reach = (RADIUS / ZONE).ceil() as i32;
        for dz in -reach..=reach {
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let Some(list) = self.cells.get(&[corner[0] + dx, corner[1] + dy, corner[2] + dz]) else { continue; };
                    for &index in list {
                        let zone = self.zones[index as usize];
                        let d2 = zone.centroid.distance_squared(point);
                        if d2 >= RADIUS * RADIUS { continue; }
                        let weight = zone.confidence * (1.0 - d2 / (RADIUS * RADIUS)).powi(2);
                        numerator += weight * zone.strength;
                        weights += weight;
                    }
                }
            }
        }
        numerator / weights
    }

    /// Per-luxel strength, trilinear between zone-lattice nodes so adjacent
    /// zones fade into each other (also across chart borders).
    pub(super) fn strength_maps(&self, samples: &[Vec<Option<StaticAoTexelSample>>]) -> Vec<Vec<f32>> {
        let corners: HashSet<[i32; 3]> = samples.iter().flatten().flatten()
            .flat_map(|sample| {
                let base = cell_of(Vec3::from_array(sample.position));
                (0..8).map(move |k| [base[0] + (k & 1), base[1] + ((k >> 1) & 1), base[2] + ((k >> 2) & 1)])
            })
            .collect();
        let nodes: HashMap<[i32; 3], f32> = corners.into_par_iter()
            .map(|corner| (corner, self.node(corner)))
            .collect();
        samples.par_iter().map(|page| page.iter().map(|sample| {
            let Some(sample) = sample else { return self.prior; };
            let scaled = Vec3::from_array(sample.position) / ZONE;
            let base = scaled.floor();
            let f = scaled - base;
            let b = [base.x as i32, base.y as i32, base.z as i32];
            let mut value = 0.0;
            for k in 0..8 {
                let (ox, oy, oz) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
                let w = if ox == 1 { f.x } else { 1.0 - f.x }
                    * if oy == 1 { f.y } else { 1.0 - f.y }
                    * if oz == 1 { f.z } else { 1.0 - f.z };
                value += w * nodes.get(&[b[0] + ox, b[1] + oy, b[2] + oz]).copied().unwrap_or(self.prior);
            }
            value
        }).collect()).collect()
    }
}

/// Two-luxel guard band for bilinear filtering at packed lightmap UV borders.
/// Fill only unowned texels with a unique nearest chart. Never overwrite
/// another chart or spread removal across a real geometric receiver.
pub(super) fn pad_chart_edges(
    mask: &mut [u8], width: usize, height: usize,
    samples: &[Option<StaticAoTexelSample>], owners: &[u32],
) {
    let unpadded = mask.to_vec();
    mask.par_chunks_mut(256).enumerate().for_each(|(block, pixels)| {
        for (local, dst) in pixels.iter_mut().enumerate() {
            let index = block * 256 + local;
            if owners[index] != 0 || samples[index].is_some() { continue; }
            let (x, y) = (index % width, index / width);
            let mut best = i32::MAX;
            let mut chosen = 0;
            let mut value = 0;
            let mut conflicted = false;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 { continue; }
                    let n = ny as usize * width + nx as usize;
                    let owner = owners[n];
                    if owner == 0 || samples[n].is_none() { continue; }
                    let d2 = dx * dx + dy * dy;
                    if d2 < best {
                        best = d2;
                        chosen = owner;
                        value = unpadded[n];
                        conflicted = false;
                    } else if d2 == best && owner != chosen {
                        conflicted = true;
                    }
                }
            }
            if !conflicted && chosen != 0 { *dst = value; }
        }
    });
}

/// Encode `strength * refined flux` in the contour mask's ENCODE_SCALE range.
pub(super) fn encode_zone_mask(flux: &[u8], strength: &[f32]) -> Vec<u8> {
    flux.iter().zip(strength).map(|(&flux, &k)| {
        (flux as f32 * k.clamp(0.0, ENCODE_SCALE) / ENCODE_SCALE).round().clamp(0.0, 255.0) as u8
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoding() -> BakedSunEncoding {
        BakedSunEncoding { exposure: 0.0, gamma: 1.0, compensate: 1.0, brightness: 1.0, deviance: 0.0, samples: 1 }
    }

    /// Two floors 1024 units apart with different baked sun strengths; each
    /// has a shadow edge every 32 luxels so every zone has evidence.
    fn fixture() -> (Vec<TextureData>, Vec<Vec<u8>>, Vec<Vec<Option<StaticAoTexelSample>>>, Vec<Vec<u32>>) {
        const W: usize = 256;
        const H: usize = 64;
        let mut rgba = Vec::with_capacity(W * H * 4);
        let mut flux = Vec::with_capacity(W * H);
        let mut samples = Vec::with_capacity(W * H);
        for y in 0..H {
            for x in 0..W {
                let lit = (x / 16) % 2 == 0;
                let far = x >= W / 2;
                let k = if far { 0.6 } else { 0.15 };
                let f = if lit { 1.0 } else { 0.0 };
                let value = (60.0 + 100.0 * k * f) as u8;
                rgba.extend_from_slice(&[value, value, value, 0]);
                flux.push((f * 255.0) as u8);
                let world_x = if far { 4096.0 + x as f32 * 8.0 } else { x as f32 * 8.0 };
                samples.push(Some(StaticAoTexelSample { position: [world_x, 0.0, y as f32 * 8.0], normal: [0.0, 1.0, 0.0] }));
            }
        }
        (vec![TextureData { label: "zones".into(), source: None, width: W as u32, height: H as u32,
            rgba: rgba.into(), rgba16f: None, mip_level_count: 1, clamp: true, srgb: true }],
         vec![flux], vec![samples], vec![vec![1; W * H]])
    }

    #[test]
    fn zones_follow_local_strength_without_a_global_floor() {
        let (images, flux, samples, owners) = fixture();
        let field = fit_zones(&images, &flux, &samples, &owners, 100.0, encoding(), &AtomicBool::new(false)).unwrap();
        let maps = field.strength_maps(&samples);
        assert!((maps[0][32 * 256 + 20] - 0.15).abs() < 0.05, "{}", maps[0][32 * 256 + 20]);
        assert!((maps[0][32 * 256 + 200] - 0.6).abs() < 0.08, "{}", maps[0][32 * 256 + 200]);
    }

    #[test]
    fn zones_fail_closed_without_edges() {
        let (images, mut flux, samples, owners) = fixture();
        flux[0].iter_mut().for_each(|f| *f = 255);
        assert!(fit_zones(&images, &flux, &samples, &owners, 100.0, encoding(), &AtomicBool::new(false)).is_none());
    }
}
