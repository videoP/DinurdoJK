//! Experimental, conservative contour-guided sunlight reconstruction.
//!
//! Contours are evidence, not partition boundaries: local contrast fits are
//! interpolated on a chart-aware continuous field. The *additional* bounce
//! estimate only uses measured shadow-side residual falloff, not a guessed
//! absolute ambient baseline. One offline pass; no gameplay-frame work.
use super::{compiled_lightmap_photons, median_f32};
use crate::renderer::{StaticAoTexelSample, TextureData, Vec3};
use crate::scene::BakedSunEncoding;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

const TILE: usize = 32;
const GRID: usize = 8;
/// The alternate mask encodes up to three times the authored direct flux.
/// Matching strength 3.0 is applied by the existing lightmap compositor.
pub(super) const ENCODE_SCALE: f32 = super::CONTOUR_MASK_SCALE;
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

#[derive(Clone, Copy)]
struct Anchor {
    owner: u32,
    xy: [f32; 2],
    position: Vec3,
    normal: Vec3,
    strength: f32,
    bounce: f32,
    confidence: f32,
}

#[derive(Clone, Copy)]
struct Node {
    owner: u32,
    position: Vec3,
    normal: Vec3,
    strength: f32,
    bounce: f32,
}
impl Default for Node {
    fn default() -> Self {
        Self { owner: 0, position: Vec3::ZERO, normal: Vec3::ZERO, strength: 0.0, bounce: 0.0 }
    }
}

struct Page<'a> {
    image: &'a TextureData,
    edge_flux: &'a [u8],
    samples: &'a [Option<StaticAoTexelSample>],
    owners: &'a [u32],
    width: usize,
    height: usize,
    tiles_w: usize,
    tiles_h: usize,
    tiles: Vec<Vec<Anchor>>,
    /// Independently repaired visibility for this mode only; never modifies Auto.
    mask_flux: Vec<u8>,
}

fn compatible(a: &StaticAoTexelSample, b: &StaticAoTexelSample) -> bool {
    let normal = Vec3::from_array(a.normal);
    normal.dot(Vec3::from_array(b.normal)) >= 0.995
        && normal.dot(Vec3::from_array(b.position) - Vec3::from_array(a.position)).abs() <= 0.75
}

fn valid_pixel(page: &Page<'_>, index: usize) -> bool {
    page.image.rgba[index * 4..index * 4 + 3].iter().all(|&v| v < 250)
}

fn residual(page: &Page<'_>, index: usize, sun_y: f32, ratio: f32, encoding: BakedSunEncoding) -> f32 {
    let p = &page.image.rgba[index * 4..index * 4 + 4];
    compiled_lightmap_photons(p, encoding).dot(LUMA) - sun_y * (page.edge_flux[index] as f32 / 255.0) * ratio
}

/// Shade-side falloff is a *lower-confidence* bounce hint, not an ambient-light
/// decomposition. Require samples to remain in the same shadow on one plane.
fn bounce_sample(
    page: &Page<'_>, lit: (usize, usize), shadow: (usize, usize),
    ratio: f32, sun_y: f32, encoding: BakedSunEncoding,
) -> Option<f32> {
    let si = shadow.1 * page.width + shadow.0;
    let s = page.samples[si].as_ref()?;
    let shadow_flux = page.edge_flux[si] as f32 / 255.0;
    if shadow_flux > 0.05 { return None; }
    let (dx, dy) = (shadow.0 as i32 - lit.0 as i32, shadow.1 as i32 - lit.1 as i32);
    let near = residual(page, si, sun_y, ratio, encoding);
    let mut far = f32::INFINITY;
    let mut found = 0;
    for scale in [2, 3] {
        let x = shadow.0 as i32 + dx * scale;
        let y = shadow.1 as i32 + dy * scale;
        if x < 0 || y < 0 || x >= page.width as i32 || y >= page.height as i32 { continue; }
        let idx = y as usize * page.width + x as usize;
        if page.owners[idx] != page.owners[si] || !valid_pixel(page, idx)
            || page.samples[idx].as_ref().is_none_or(|sample| !compatible(s, sample))
            || page.edge_flux[idx] as f32 / 255.0 > shadow_flux + 0.025
        { continue; }
        far = far.min(residual(page, idx, sun_y, ratio, encoding));
        found += 1;
    }
    if found < 2 || near <= far { return None; }
    // Strong unrelated gradients (lamps, textured illumination) are kept from
    // being interpreted as giant sun bounce. This is intentionally conservative.
    Some((near - far).min(sun_y * 0.12).min(near.max(0.0) * 0.15).max(0.0))
}

/// A rasterized ray can falsely classify one or two texels beside brush
/// intersections as shadow, although the *original* compiled lightmap is still
/// brightly sunlit there. Extend flux only where measured RGB supports the
/// same direct-sun component as nearby, coplanar, fully lit texels. This is
/// intentionally NOT a morphological dilation: real dark shadow edges must
/// remain shadowed, and unrelated packed charts may never supply a sample.
fn recover_missing_edge_flux(
    page: &Page<'_>, sun_rgb: Vec3, sun_y: f32, strength: f32,
    encoding: BakedSunEncoding,
) -> Vec<u8> {
    let strength = strength.clamp(0.10, 2.5);
    let traced: Vec<u8> = page.image.rgba.chunks_exact(4).map(|p| p[3]).collect();
    let normal_sun = sun_rgb / sun_y;
    (0..traced.len()).into_par_iter().map(|index| {
        let original = traced[index];
        if original >= 240 { return original; }
        let Some(sample) = page.samples[index].as_ref() else { return original; };
        let owner = page.owners[index];
        if owner == 0 || !valid_pixel(page, index) { return original; }
        let x = index % page.width;
        let y = index / page.width;
        // Most fully shaded texels have no nearby lit neighbor. Avoid photon
        // decoding those (especially expensive when baked gamma != 1).
        let mut target_rgb = None;
        // Fixed-size thread-local stack buffers: millions of luxels must not
        // perform millions of tiny heap allocations during the background bake.
        let mut estimates = [0.0f32; 49];
        let mut count = 0usize;
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                if dx == 0 && dy == 0 || dx * dx + dy * dy > 10 { continue; }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= page.width as i32 || ny >= page.height as i32 { continue; }
                let n = ny as usize * page.width + nx as usize;
                if page.owners[n] != owner || !valid_pixel(page, n) { continue; }
                let Some(other) = page.samples[n].as_ref() else { continue; };
                if !compatible(sample, other) { continue; }
                let source_flux = traced[n] as f32 / 255.0;
                // Only a strongly lit *traced* neighbor is evidence. Avoid
                // expanding partially shadowed mask samples into real shadows.
                if source_flux < 0.32 || source_flux < original as f32 / 255.0 + 0.12 { continue; }
                let source_rgb = compiled_lightmap_photons(
                    &page.image.rgba[n * 4..n * 4 + 4], encoding);
                let baseline = source_rgb - sun_rgb * (source_flux * strength);
                if baseline.min_element() < -sun_y * 0.02 { continue; }
                let target = *target_rgb.get_or_insert_with(|| compiled_lightmap_photons(
                    &page.image.rgba[index * 4..index * 4 + 4], encoding));
                let delta = target - baseline;
                let candidate = delta.dot(LUMA) / (sun_y * strength);
                if candidate <= 0.08 || candidate > source_flux + 0.15 { continue; }
                // The *color* of the inferred difference should resemble the
                // authored sun, not arbitrary nearby lamps or colored bounce.
                let predicted = normal_sun * delta.dot(LUMA);
                if (delta - predicted).length() > sun_y * 0.17 { continue; }
                estimates[count] = candidate.min(source_flux);
                count += 1;
            }
        }
        if count < 2 { return original; }
        let candidate = median_f32(&mut estimates[..count]);
        let mut deviations = [0.0f32; 49];
        for (i, value) in estimates[..count].iter().enumerate() {
            deviations[i] = (value - candidate).abs();
        }
        if median_f32(&mut deviations[..count]) > 0.14 { return original; }
        // This was a false ray shadow only if RGB independently supports it;
        // do not invent light beyond the measured sun-facing cosine.
        let recovered = candidate.clamp(0.0, 1.0);
        original.max((recovered * 255.0).round() as u8)
    }).collect()
}

/// Two-luxel guard band for bilinear filtering at packed lightmap UV borders.
/// Fill only unowned texels with a unique nearest chart. Never overwrite
/// another chart or spread removal across a real geometric receiver.
fn pad_chart_edges(mask: &mut [u8], page: &Page<'_>) {
    let unpadded = mask.to_vec();
    mask.par_chunks_mut(256).enumerate().for_each(|(block, pixels)| {
        for (local, dst) in pixels.iter_mut().enumerate() {
            let index = block * 256 + local;
            if page.owners[index] != 0 || page.samples[index].is_some() { continue; }
            let (x, y) = (index % page.width, index / page.width);
            let mut best = i32::MAX;
            let mut chosen = 0;
            let mut value = 0;
            let mut conflicted = false;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= page.width as i32 || ny >= page.height as i32 { continue; }
                    let n = ny as usize * page.width + nx as usize;
                    let owner = page.owners[n];
                    if owner == 0 || page.samples[n].is_none() { continue; }
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

fn tile_anchors(page: &Page<'_>, tx: usize, ty: usize, sun_y: f32, encoding: BakedSunEncoding) -> Vec<Anchor> {
    struct Pair { ratio: f32, bounce: Option<f32>, xy: [f32; 2], position: Vec3, normal: Vec3 }
    let mut by_chart = std::collections::BTreeMap::<u32, Vec<Pair>>::new();
    let x_end = ((tx + 1) * TILE).min(page.width.saturating_sub(8));
    let y_end = ((ty + 1) * TILE).min(page.height.saturating_sub(8));
    for y in (ty * TILE..y_end).step_by(4) {
        for x in (tx * TILE..x_end).step_by(4) {
            let i = y * page.width + x;
            let owner = page.owners[i];
            if owner == 0 || !valid_pixel(page, i) { continue; }
            let Some(a) = page.samples[i].as_ref() else { continue; };
            let pa = &page.image.rgba[i * 4..i * 4 + 4];
            let flux_a = page.edge_flux[i] as f32 / 255.0;
            let rgb_a = compiled_lightmap_photons(pa, encoding);
            for (dx, dy) in [(4, 0), (0, 4), (8, 0), (0, 8)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= page.width || ny >= page.height { continue; }
                let j = ny * page.width + nx;
                if page.owners[j] != owner || !valid_pixel(page, j) { continue; }
                let Some(b) = page.samples[j].as_ref() else { continue; };
                if !compatible(a, b) { continue; }
                let pb = &page.image.rgba[j * 4..j * 4 + 4];
                let df = page.edge_flux[j] as f32 / 255.0 - flux_a;
                if df.abs() < 0.18 { continue; }
                let diff = (compiled_lightmap_photons(pb, encoding) - rgb_a).dot(LUMA);
                let ratio = diff / (sun_y * df);
                if !ratio.is_finite() || !(0.10..=2.35).contains(&ratio) { continue; }
                let (lit, shade) = if df < 0.0 { ((x, y), (nx, ny)) } else { ((nx, ny), (x, y)) };
                let bounce = bounce_sample(page, lit, shade, ratio, sun_y, encoding);
                by_chart.entry(owner).or_default().push(Pair {
                    ratio, bounce,
                    xy: [(x + nx) as f32 * 0.5, (y + ny) as f32 * 0.5],
                    position: (Vec3::from_array(a.position) + Vec3::from_array(b.position)) * 0.5,
                    normal: Vec3::from_array(a.normal),
                });
            }
        }
    }
    by_chart.into_iter().filter_map(|(owner, pairs)| {
        if pairs.len() < 4 { return None; }
        let mut ratios: Vec<_> = pairs.iter().map(|p| p.ratio).collect();
        let strength = median_f32(&mut ratios);
        let mut deviations: Vec<_> = pairs.iter().map(|p| (p.ratio - strength).abs()).collect();
        let mad = median_f32(&mut deviations);
        if mad > 0.12 + 0.20 * strength { return None; }
        let consistent: Vec<_> = pairs.iter().filter(|p| (p.ratio - strength).abs() <= (0.15 + mad * 2.0)).collect();
        if consistent.len() < 4 { return None; }
        let scale = 1.0 / consistent.len() as f32;
        let mut bounce: Vec<f32> = consistent.iter().filter_map(|p| p.bounce).collect();
        // At least three independent observations before trusting a gradient.
        let bounce = if bounce.len() >= 3 { median_f32(&mut bounce) } else { 0.0 };
        Some(Anchor {
            owner,
            xy: [consistent.iter().map(|p| p.xy[0]).sum::<f32>() * scale,
                 consistent.iter().map(|p| p.xy[1]).sum::<f32>() * scale],
            position: consistent.iter().fold(Vec3::ZERO, |sum, p| sum + p.position) * scale,
            normal: consistent[0].normal,
            strength,
            bounce,
            confidence: (consistent.len() as f32 / 12.0).min(1.0) * (1.0 - mad / 1.5).clamp(0.2, 1.0),
        })
    }).collect()
}

fn node_at(page: &Page<'_>, x: usize, y: usize, prior: f32) -> Node {
    // A packed atlas cell can intersect several unrelated charts. Choose the
    // nearest valid sample to the cell center; actual output taps check owner.
    let cx = (x * GRID + GRID / 2).min(page.width - 1);
    let cy = (y * GRID + GRID / 2).min(page.height - 1);
    let mut target = None;
    'search: for radius in 0..=GRID / 2 {
        for dy in -(radius as i32)..=radius as i32 {
            for dx in -(radius as i32)..=radius as i32 {
                let nx = cx as i32 + dx;
                let ny = cy as i32 + dy;
                if nx < 0 || ny < 0 || nx >= page.width as i32 || ny >= page.height as i32 { continue; }
                let idx = ny as usize * page.width + nx as usize;
                if page.owners[idx] > 0 {
                    if let Some(sample) = page.samples[idx].as_ref() { target = Some((idx, sample)); break 'search; }
                }
            }
        }
    }
    let Some((idx, sample)) = target else { return Node::default(); };
    let owner = page.owners[idx];
    let position = Vec3::from_array(sample.position);
    let normal = Vec3::from_array(sample.normal);
    let mut numerator = 0.0;
    let mut weights = 0.0;
    let mut bounce_sum = 0.0;
    let mut bounce_weights = 0.0;
    let tx = cx / TILE;
    let ty = cy / TILE;
    for yy in ty.saturating_sub(4)..=(ty + 4).min(page.tiles_h - 1) {
        for xx in tx.saturating_sub(4)..=(tx + 4).min(page.tiles_w - 1) {
            for anchor in &page.tiles[yy * page.tiles_w + xx] {
                if anchor.owner != owner || normal.dot(anchor.normal) < 0.985
                    || normal.dot(anchor.position - position).abs() > 2.0 { continue; }
                let dx = anchor.xy[0] - cx as f32;
                let dy = anchor.xy[1] - cy as f32;
                let d2 = dx * dx + dy * dy;
                let influence = (1.0 - d2 / (96.0 * 96.0)).max(0.0).powi(2) * anchor.confidence;
                numerator += influence * anchor.strength;
                weights += influence;
                let bounce_weight = (1.0 - d2 / (48.0 * 48.0)).max(0.0).powi(2) * anchor.confidence;
                bounce_sum += bounce_weight * anchor.bounce;
                bounce_weights += bounce_weight;
            }
        }
    }
    Node {
        owner, position, normal,
        strength: (numerator + 0.18 * prior) / (weights + 0.18),
        bounce: bounce_sum / (bounce_weights + 0.7),
    }
}

fn sample_node(nodes: &[Node], grid_w: usize, grid_h: usize, x: usize, y: usize,
               owner: u32, sample: &StaticAoTexelSample, prior: f32) -> (f32, f32) {
    let gx = (x as f32 + 0.5) / GRID as f32 - 0.5;
    let gy = (y as f32 + 0.5) / GRID as f32 - 0.5;
    let ax = gx.floor() as i32;
    let ay = gy.floor() as i32;
    let fx = gx - ax as f32;
    let fy = gy - ay as f32;
    let position = Vec3::from_array(sample.position);
    let normal = Vec3::from_array(sample.normal);
    let mut total = 0.0;
    let mut strength = 0.0;
    let mut bounce = 0.0;
    for dy in 0..=1 {
        for dx in 0..=1 {
            let nx = (ax + dx).clamp(0, grid_w as i32 - 1) as usize;
            let ny = (ay + dy).clamp(0, grid_h as i32 - 1) as usize;
            let node = nodes[ny * grid_w + nx];
            if node.owner != owner || normal.dot(node.normal) < 0.985
                || normal.dot(node.position - position).abs() > 2.0 { continue; }
            let wx = if dx == 0 { 1.0 - fx } else { fx };
            let wy = if dy == 0 { 1.0 - fy } else { fy };
            let weight = wx * wy;
            total += weight;
            strength += weight * node.strength;
            bounce += weight * node.bounce;
        }
    }
    if total < 0.001 { (prior, 0.0) } else { (strength / total, bounce / total) }
}

/// Returns an alternative alpha-only flux encoding. The immutable original
/// mask remains available for regular Auto and sun-visibility diagnostics.
/// None means there is insufficient trustworthy contour evidence: fail closed.
pub(super) fn build_contour_masks(
    lightmaps: &[TextureData], edge_flux: &[Vec<u8>], samples: &[Vec<Option<StaticAoTexelSample>>],
    owners: &[Vec<u32>], sun_rgb: Vec3, sun_y: f32, auto_strength: Option<f32>,
    encoding: BakedSunEncoding, cancelled: &AtomicBool,
) -> Option<(Vec<Vec<u8>>, usize)> {
    if sun_y <= 0.001 || !sun_y.is_finite() { return None; }
    let mut pages = Vec::new();
    for (((image, edges), texels), chart) in lightmaps.iter().zip(edge_flux).zip(samples).zip(owners) {
        if cancelled.load(Ordering::Relaxed) { return None; }
        let (width, height) = (image.width as usize, image.height as usize);
        if width == 0 || height == 0 || image.rgba16f.is_some()
            || image.rgba.len() != width.checked_mul(height)?.checked_mul(4)?
            || texels.len() != width * height || chart.len() != width * height || edges.len() != width * height { return None; }
        let (tiles_w, tiles_h) = (width.div_ceil(TILE), height.div_ceil(TILE));
        let mut page = Page { image, edge_flux: edges, samples: texels, owners: chart, width, height,
            tiles_w, tiles_h, tiles: Vec::new(), mask_flux: Vec::new() };
        let tiles = (0..tiles_w * tiles_h).into_par_iter()
            .map(|index| {
                if cancelled.load(Ordering::Relaxed) { return Vec::new(); }
                tile_anchors(&page, index % tiles_w, index / tiles_w, sun_y, encoding)
            }).collect();
        page.tiles = tiles;
        pages.push(page);
    }
    if cancelled.load(Ordering::Relaxed) { return None; }
    let anchors: Vec<_> = pages.iter().flat_map(|p| p.tiles.iter().flatten().copied()).collect();
    // The global strength is only the *prior* in unknown regions; every valid
    // contour contributes its own local measurement without a global MAD gate.
    if anchors.len() < 4 { return None; }
    let mut ratios: Vec<_> = anchors.iter().map(|a| a.strength).collect();
    let prior = auto_strength.filter(|v| v.is_finite() && (0.03..=2.5).contains(v))
        .unwrap_or_else(|| median_f32(&mut ratios).clamp(0.1, 2.3));
    // A local estimate must never make a reliably calibrated Auto result worse
    // by reducing direct-sun subtraction. Contours may add evidence, not
    // discard the good global solution in areas with sparse observations.
    for page in &mut pages {
        page.mask_flux = recover_missing_edge_flux(page, sun_rgb, sun_y, prior, encoding);
    }
    let recovered_texels: usize = pages.iter().map(|page| {
        page.mask_flux.iter().enumerate().filter(|(i, value)|
            **value > page.image.rgba[*i * 4 + 3]).count()
    }).sum();
    let mut padded_texels = 0usize;
    let mut output = Vec::with_capacity(pages.len());
    for page in &pages {
        if cancelled.load(Ordering::Relaxed) { return None; }
        let gw = page.width.div_ceil(GRID);
        let gh = page.height.div_ceil(GRID);
        let nodes: Vec<Node> = (0..gw * gh).into_par_iter().map(|index| {
            node_at(page, index % gw, index / gw, prior)
        }).collect();
        let mut mask = vec![0u8; page.width * page.height];
        mask.par_chunks_mut(256).enumerate().for_each(|(block, pixels)| {
            if cancelled.load(Ordering::Relaxed) { return; }
            for (local, byte) in pixels.iter_mut().enumerate() {
                let idx = block * 256 + local;
                let x = idx % page.width;
                let y = idx / page.width;
                let flux = page.mask_flux[idx] as f32 / 255.0;
                // Padding is populated separately; chart owner is mandatory.
                let Some(sample) = page.samples[idx].as_ref() else { continue; };
                let (strength, bounce) = if page.owners[idx] != 0 {
                    sample_node(&nodes, gw, gh, x, y, page.owners[idx], sample, prior)
                } else { (prior, 0.0) };
                let strength = strength.max(auto_strength.unwrap_or(0.0));
                let direct = flux * strength * sun_y;
                let raw = compiled_lightmap_photons(&page.image.rgba[idx * 4..idx * 4 + 4], encoding).dot(LUMA);
                // Only the *extra* bounce term is bounded by remaining light.
                // Capping direct to 95% of RGB was regressing ordinary Auto:
                // on low-ambient maps direct sunlight can be >95% of raw.
                let shadow = (1.0 - page.edge_flux[idx] as f32 / 255.0 / 0.35).clamp(0.0, 1.0);
                let bounce = (bounce * shadow).min(sun_y * 0.08)
                    .min((raw - direct).max(0.0) * 0.30);
                let sun_photons = direct + bounce;
                *byte = (sun_photons / (sun_y * ENCODE_SCALE) * 255.0).clamp(0.0, 255.0).round() as u8;
            }
        });
        pad_chart_edges(&mut mask, page);
        padded_texels += mask.iter().enumerate().filter(|(i, byte)|
            page.owners[*i] == 0 && **byte > 0).count();
        output.push(mask);
    }
    if cancelled.load(Ordering::Relaxed) { return None; }
    println!("[BAKED SUN CONTOUR] {} local regions, {:.3}x Auto floor, {} pages, {} RGB-verified edge texels recovered, {} UV-pad texels filled; shadow-only bounce", anchors.len(), prior, output.len(), recovered_texels, padded_texels);
    Some((output, anchors.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoding() -> BakedSunEncoding {
        BakedSunEncoding {
            exposure: 0.0, gamma: 1.0, compensate: 1.0, brightness: 1.0,
            deviance: 0.0, samples: 1,
        }
    }

    fn fixture(with_edge: bool) -> (Vec<TextureData>, Vec<Vec<u8>>,
        Vec<Vec<Option<StaticAoTexelSample>>>, Vec<Vec<u32>>) {
        const W: usize = 160;
        const H: usize = 128;
        let mut rgba = Vec::with_capacity(W * H * 4);
        let mut edges = Vec::with_capacity(W * H);
        let mut samples = Vec::with_capacity(W * H);
        let mut owners = Vec::with_capacity(W * H);
        for y in 0..H {
            for x in 0..W {
                let flux = if with_edge && x < 64 { 0.0 } else { 1.0 };
                // Two areas have genuinely different direct-sun scale; a
                // shadow-side residual gradient is only close to the edge.
                let direct = if y < H / 2 { 0.5 } else { 1.1 };
                let bounce = if x < 64 { ((x as f32 - 38.0) / 26.0).clamp(0.0, 1.0) * 10.0 } else { 10.0 };
                let value = (50.0 + 80.0 * direct * flux + bounce).round() as u8;
                rgba.extend_from_slice(&[value, value, value, (flux * 255.0) as u8]);
                edges.push((flux * 255.0) as u8);
                samples.push(Some(StaticAoTexelSample {
                    position: [x as f32, 0.0, y as f32], normal: Vec3::Y.to_array(),
                }));
                owners.push(1);
            }
        }
        (vec![TextureData {
            label: "contour test".into(), source: None, width: W as u32, height: H as u32,
            rgba: rgba.into(), rgba16f: None, mip_level_count: 1, clamp: true, srgb: true,
        }], vec![edges], vec![samples], vec![owners])
    }

    #[test]
    fn contour_mode_follows_local_direct_sun_and_infers_only_near_shadow_bounce() {
        let (images, edges, samples, owners) = fixture(true);
        let (masks, count) = build_contour_masks(&images, &edges, &samples, &owners, Vec3::splat(80.0), 80.0, Some(0.5),
            encoding(), &AtomicBool::new(false)).expect("valid local shadow boundaries");
        assert!(count >= 4);
        let at = |x: usize, y: usize| masks[0][y * 160 + x] as f32 / 255.0 * ENCODE_SCALE;
        // Stronger local direct sunlight is removed from the lower part.
        assert!(at(90, 94) > at(90, 30) + 0.15);
        // No invented bounce far away from the contour.
        assert!(at(12, 30) < 0.015);
        assert!(at(60, 30) > 0.0);
        // No hard tile/region seam across the middle of the same plane.
        assert!((at(90, 63) - at(90, 64)).abs() < 0.15);
    }

    #[test]
    fn contour_mode_fails_closed_without_reliable_transitions() {
        let (images, edges, samples, owners) = fixture(false);
        assert!(build_contour_masks(&images, &edges, &samples, &owners, Vec3::splat(80.0), 80.0, Some(0.5),
            encoding(), &AtomicBool::new(false)).is_none());
        let (images, edges, samples, owners) = fixture(true);
        assert!(build_contour_masks(&images, &edges, &samples, &owners, Vec3::splat(80.0), 80.0, Some(0.5),
            encoding(), &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn contour_recovers_missing_raycast_light_without_erasing_real_shadows() {
        let (mut images, edges, samples, owners) = fixture(true);
        // Fully sunlit RGB, but the geometry ray unexpectedly reports shadow
        // over a narrow strip next to a brush intersection.
        for y in 20..28 {
            for x in 75..=76 {
                images[0].rgba[(y * 160 + x) * 4 + 3] = 0;
            }
        }
        let (masks, _) = build_contour_masks(
            &images, &edges, &samples, &owners,
            Vec3::splat(80.0), 80.0, Some(0.5), encoding(), &AtomicBool::new(false),
        ).expect("surrounding contour anchors remain valid");
        let read = |x: usize, y: usize| masks[0][y * 160 + x] as f32 / 255.0 * ENCODE_SCALE;
        assert!(read(75, 24) > 0.35, "lightmap-supported sun should bridge a false ray hole");
        assert!(read(40, 24) < 0.02, "genuine deep shade must not be dilated");
        assert!(read(90, 24) >= 0.49, "Contour Guided must preserve the Auto direct-sun floor");
    }

    #[test]
    fn padded_chart_has_no_cross_chart_coverage() {
        let (images, edges, mut samples, mut owners) = fixture(true);
        const W: usize = 160;
        let y = 90;
        // Simulate a narrow atlas gutter between distinct surface owners.
        for x in 78..82 {
            let i = y * W + x;
            samples[0][i] = None;
            owners[0][i] = 0;
        }
        // Split the two sides into unrelated UV charts so the guard band
        // cannot propagate from one side into the other at equal distance.
        for yy in 0..128 {
            for xx in 82..160 {
                owners[0][yy * W + xx] = 2;
            }
        }
        let page = Page {
            image: &images[0], edge_flux: &edges[0], samples: &samples[0], owners: &owners[0],
            width: W, height: 128, tiles_w: 5, tiles_h: 4,
            tiles: Vec::new(), mask_flux: Vec::new(),
        };
        let mut mask = vec![0u8; W * 128];
        mask[y * W + 77] = 70;
        mask[y * W + 82] = 230;
        pad_chart_edges(&mut mask, &page);
        assert_eq!(mask[y * W + 78], 70);
        assert_eq!(mask[y * W + 81], 230);
    }

}
