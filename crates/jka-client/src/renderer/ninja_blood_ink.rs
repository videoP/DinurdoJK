//! Overgrowth-style authored density input adapter, never a replacement fluid
//! solver or a static decal. The alpha silhouette is injected into Ninja.
use crate::{fx::template::Rng, ninja_blood::{BloodPool, BloodSmear, BloodStamp, SplatKind}};
use glam::Vec2;
use jka_ninja::BLOOD_INK_RES;
use jka_assets::pk3::AssetSearchPath;

// A single authored RGBA splat is accepted directly; if absent, use the
// legacy four-splat atlas. No game-specific decal artwork is bundled.
const SINGLE_QPATH: &str = "textures/japro/blood_splatter.png";
const ATLAS_QPATH: &str = "textures/japro/blood_splatter_01.png";
const VEFECTS_RADIAL: &str = "textures/dinurdo/blood/vefects_radial.png";
const VEFECTS_DIRECTIONAL: &str = "textures/dinurdo/blood/vefects_directional.png";
const VEFECTS_DIRECTIONAL_FULL: &str = "textures/dinurdo/blood/vefects_directional_02.png";
const VEFECTS_SCATTER: [(&str, f32); 3] = [
    ("textures/dinurdo/blood/vefects_radial_02.png", 1.0),
    ("textures/dinurdo/blood/vefects_radial_03.png", 1.0),
    ("textures/dinurdo/blood/vefects_directional_03.png", 2.0),
];
const N: usize = BLOOD_INK_RES as usize;

/// Texel rectangle `[x0, y0, x1, y1)` of the ink image.
pub(super) type InkRect = [u32; 4];

fn texel_rect(lo: Vec2, hi: Vec2) -> Option<InkRect> {
    (lo.x <= hi.x && lo.y <= hi.y)
        .then(|| [lo.x as u32, lo.y as u32, hi.x as u32 + 1, hi.y as u32 + 1])
}

pub(super) fn union(a: Option<InkRect>, b: Option<InkRect>) -> Option<InkRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
    }
}

/// Pack `rect` of the full-size scratch into `upload` (tightly), then zero
/// it so the scratch is clean for the next pool without a full clear.
pub(super) fn take(scratch: &mut [u8], rect: InkRect, upload: &mut Vec<u8>) {
    let [x0, y0, x1, y1] = rect.map(|v| v as usize);
    upload.clear();
    upload.reserve((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        let row = &mut scratch[y * N + x0..y * N + x1];
        upload.extend_from_slice(row);
        row.fill(0);
    }
}

pub(super) struct InkMasks {
    radial: Atlas,
    directional: Option<Atlas>,
    full_directional: Option<Atlas>,
    scatter: Vec<(Atlas, f32)>,
}

impl InkMasks {
    pub(super) fn load(search: &mut AssetSearchPath) -> Option<Self> {
        if let Some(radial) = Atlas::load(search, VEFECTS_RADIAL, 1) {
            let directional = Atlas::load(search, VEFECTS_DIRECTIONAL, 1);
            let full_directional = Atlas::load(search, VEFECTS_DIRECTIONAL_FULL, 1);
            let scatter = VEFECTS_SCATTER.into_iter().filter_map(|(path, padding)| {
                Atlas::load(search, path, 1).map(|atlas| (atlas, padding))
            }).collect();
            return Some(Self { radial, directional, full_directional, scatter });
        }
        for (path, tiles) in [(SINGLE_QPATH, 1), (ATLAS_QPATH, 4)] {
            if let Some(radial) = Atlas::load(search, path, tiles) {
                return Some(Self { radial, directional: None, full_directional: None, scatter: Vec::new() });
            }
        }
        log::warn!("[NINJA BLOOD] no authored splatter mask found; using Ninja brush seeds");
        None
    }

    fn for_impact(&self, kind: SplatKind, glancing: bool, rng: &mut Rng) -> (&Atlas, f32) {
        if matches!(kind, SplatKind::Scatter) && !self.scatter.is_empty() {
            let (atlas, padding) = &self.scatter[rng.irand(0, self.scatter.len() as i32 - 1) as usize];
            return (atlas, *padding);
        }
        let directional = if matches!(kind, SplatKind::SpurtImpact) {
            &self.full_directional
        } else {
            &self.directional
        };
        if let Some(mask) = directional.as_ref().filter(|_| glancing) {
            // The directional converter centers the impact on a canvas twice
            // the source width. Compensate for the added transparent padding.
            (mask, 2.0)
        } else {
            (&self.radial, 1.0)
        }
    }
}

struct Atlas {
    image: image::RgbaImage,
    tile_size: u32,
    columns: u32,
    tiles: usize,
    fullest_tile: usize,
    // An opaque blood texture has no alpha. Derive transparency from its
    // BACKGROUND, not its red channel (white backgrounds are also red=255).
    backgrounds: [[u8; 3]; 4],
    has_alpha: [bool; 4],
}
impl Atlas {
    fn load(search: &mut AssetSearchPath, path: &str, tiles: usize) -> Option<Self> {
        let asset = match search.read(path, 32 * 1024 * 1024) {
            Ok(Some(asset)) => asset,
            Ok(None) => return None,
            Err(error) => {
                log::warn!("[NINJA BLOOD] cannot read {path}: {error}");
                return None;
            }
        };
        let image = match image::load_from_memory(&asset.bytes) {
            Ok(im) => im.into_rgba8(),
            Err(error) => {
                log::warn!("[NINJA BLOOD] cannot decode {path}: {error}");
                return None;
            }
        };
        let Some(atlas) = Self::from_image(image, tiles) else {
            log::warn!("[NINJA BLOOD] invalid splatter artwork {path}; expected a square, >=256px single image or >=512px 2x2 atlas");
            return None;
        };
        log::info!("[NINJA BLOOD] using authored blood splatter mask {path}: {}x{}, {} variants",
            atlas.image.width(), atlas.image.height(), atlas.tiles);
        Some(atlas)
    }

    fn from_image(image: image::RgbaImage, tiles: usize) -> Option<Self> {
        if !matches!(tiles, 1 | 4) || image.width() != image.height() {
            return None;
        }
        let columns = if tiles == 4 { 2 } else { 1 };
        if image.width() < columns * 256 || image.width() % columns != 0 {
            return None;
        }
        let tile_size = image.width() / columns;
        let mut backgrounds = [[0; 3]; 4];
        let mut has_alpha = [false; 4];
        for tile in 0..tiles {
            let left = (tile as u32 % columns) * tile_size;
            let top = (tile as u32 / columns) * tile_size;
            // Median corners resist one splat overlapping the edge of a tile.
            for channel in 0..3 {
                let mut c = [
                    image.get_pixel(left, top)[channel],
                    image.get_pixel(left + tile_size - 1, top)[channel],
                    image.get_pixel(left, top + tile_size - 1)[channel],
                    image.get_pixel(left + tile_size - 1, top + tile_size - 1)[channel],
                ];
                c.sort_unstable();
                backgrounds[tile][channel] = ((u16::from(c[1]) + u16::from(c[2])) / 2) as u8;
            }
            has_alpha[tile] = (top..top + tile_size).any(|y| {
                (left..left + tile_size).any(|x| image.get_pixel(x, y)[3] != 255)
            });
        }
        let mut atlas = Self { image, tile_size, columns, tiles, fullest_tile: 0, backgrounds, has_alpha };
        if tiles > 1 {
            atlas.fullest_tile = (0..tiles).max_by_key(|&tile| {
                (0..tile_size).flat_map(|y| (0..tile_size).map(move |x| (x, y)))
                    .filter(|&(x, y)| atlas.pixel_alpha(tile, x, y) > 0.25)
                    .count()
            }).unwrap_or(0);
        }
        Some(atlas)
    }

    fn pixel_alpha(&self, tile: usize, x: u32, y: u32) -> f32 {
        let p = self.image.get_pixel(
            (tile as u32 % self.columns) * self.tile_size + x,
            (tile as u32 / self.columns) * self.tile_size + y,
        );
        if self.has_alpha[tile] {
            return p[3] as f32 / 255.0;
        }
        // Authoring-independent opaque PNGs: white-on-black and red-on-white
        // both work. The tile background must be distinct from splat pigment.
        // This only interprets the authored silhouette; it does not invent it.
        let background = &self.backgrounds[tile];
        let difference = (0..3)
            .map(|i| u8::abs_diff(p[i], background[i]) as f32 / 255.0)
            .fold(0.0, f32::max);
        ((difference - 0.02) / 0.98).clamp(0.0, 1.0)
    }

    // Bilinear sampling confined to each authored tile, with border samples
    // outside the tile treated as transparent rather than bleeding neighbors.
    fn alpha(&self, tile: usize, uv: Vec2) -> f32 {
        if uv.min_element() < 0.0 || uv.max_element() > 1.0 { return 0.0; }
        let edge = self.tile_size as i32;
        let p = uv * self.tile_size as f32 - Vec2::splat(0.5);
        let x = p.x.floor() as i32;
        let y = p.y.floor() as i32;
        let fx = p.x - x as f32;
        let fy = p.y - y as f32;
        let read = |x: i32, y: i32| -> f32 {
            if x < 0 || x >= edge || y < 0 || y >= edge { return 0.0; }
            self.pixel_alpha(tile, x as u32, y as u32)
        };
        let a = read(x, y) * (1.0 - fx) + read(x+1, y) * fx;
        let b = read(x, y+1) * (1.0 - fx) + read(x+1, y+1) * fx;
        a * (1.0 - fy) + b * fy
    }
}

fn stamp(out: &mut [u8], masks: Option<&InkMasks>, hit: BloodStamp, extent: Vec2) -> Option<InkRect> {
    let BloodStamp { uv, seed, stretch, kind } = hit;
    let center = Vec2::from_array(uv);
    let mut rng = Rng::new(seed.variation);
    let d = Vec2::from_array(seed.direction_uv);
    let (atlas, padding) = masks.map_or((None, 1.0), |masks| {
        let glancing = stretch > 1.35 && d.length_squared() > 1e-6;
        let (atlas, padding) = masks.for_impact(kind, glancing, &mut rng);
        (Some(atlas), padding)
    });
    let tile = atlas.map_or(0, |a| {
        if matches!(kind, SplatKind::SpurtImpact) { a.fullest_tile }
        else { rng.irand(0, a.tiles as i32 - 1) as usize }
    });
    let angle = if d.length_squared() > 1.0e-6 {d.y.atan2(d.x)} else {rng.flrand(0.0,std::f32::consts::TAU)};
    let (sin,cos) = angle.sin_cos();
    let size = (seed.point_size_cm * 1.5).clamp(2.0, extent.min_element() * 0.25) * padding;
    let scale = Vec2::new((size * stretch).min(extent.x * 0.65),size);
    let span = Vec2::new(cos.abs()*scale.x+sin.abs()*scale.y,sin.abs()*scale.x+cos.abs()*scale.y)*0.5/extent;
    let lo = ((center-span) * N as f32).floor().max(Vec2::ZERO);
    let hi = ((center+span) * N as f32).ceil().min(Vec2::splat((N-1) as f32));
    for y in lo.y as usize..=hi.y as usize { for x in lo.x as usize..=hi.x as usize {
        let q = (Vec2::new(x as f32+0.5,y as f32+0.5)/N as f32-center)*extent;
        let uv = Vec2::new(cos*q.x+sin*q.y,-sin*q.x+cos*q.y)/scale+Vec2::splat(0.5);
        // Only secondary deposits use the simple fallback. Initial impacts
        // without an authored atlas still use the existing Ninja brush seed.
        let alpha = atlas.map_or_else(
            || (1.0-(uv-Vec2::splat(0.5)).length()*2.0).clamp(0.0,1.0),
            |a| a.alpha(tile,uv));
        out[y*N+x] = out[y*N+x].max((alpha*255.0+0.5) as u8);
    }}
    texel_rect(lo, hi)
}

/// Draws only the not-yet-consumed primitives into `out`, which must be an
/// all-zero `BLOOD_INK_RES`² scratch (see [`take`]). Returns the touched rect.
pub(super) fn fill(out: &mut Vec<u8>, pool: &BloodPool, atlas: Option<&InkMasks>, initial: bool,
    trail_start: usize, stamp_start: usize, smear_start: usize) -> Option<InkRect> {
    out.resize(N*N,0);
    let [w,h,_] = jka_ninja::blood_desc().expect("blood field available").extent_cm;
    let extent = Vec2::new(w,h);
    let mut rect = None;
    if initial {
        let hit = BloodStamp { uv: [0.5; 2], seed: pool.seed, stretch: pool.stretch, kind: pool.kind };
        rect = union(rect, stamp(out, atlas, hit, extent));
    }
    for hit in pool.stamps.iter().skip(stamp_start) {
        rect = union(rect, stamp(out, atlas, *hit, extent));
    }
    for smear in pool.smears.iter_from(smear_start) {
        rect = union(rect, smear_stroke(out, *smear, extent));
    }
    // Preserve Overgrowth's thin gravity-driven trails. Unlike wall impact
    // silhouettes, these are deposited by its triangle walker over time.
    for segment in pool.trail.iter().skip(trail_start) {
        let a = Vec2::from_array(segment[0])*N as f32-Vec2::splat(0.5);
        let b = Vec2::from_array(segment[1])*N as f32-Vec2::splat(0.5);
        let delta = b-a; let len = delta.length_squared();
        let lo = (a.min(b)-Vec2::splat(2.0)).floor().max(Vec2::ZERO);
        let hi = (a.max(b)+Vec2::splat(2.0)).ceil().min(Vec2::splat((N-1) as f32));
        rect = union(rect, texel_rect(lo, hi));
        for y in lo.y as usize..=hi.y as usize { for x in lo.x as usize..=hi.x as usize {
            let p = Vec2::new(x as f32,y as f32);
            let t = if len > 1.0e-8 {(p-a).dot(delta)/len} else {0.0};
            let coverage = (1.5-p.distance(a+delta*t.clamp(0.0,1.0))).clamp(0.0,1.0);
            out[y*N+x] = out[y*N+x].max((coverage*255.0+0.5) as u8);
        }}
    }
    rect
}

// 1D value noise, deterministic and continuous in `x`.
fn value_noise(x: f32) -> f32 {
    let cell = x.floor();
    let hash = |i: i32| {
        let mut h = (i as u32).wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
        h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12;
        (h & 0xFFFF) as f32 / 65535.0
    };
    let f = x - cell;
    let f = f * f * (3.0 - 2.0 * f);
    let i = cell as i32;
    hash(i) * (1.0 - f) + hash(i.wrapping_add(1)) * f
}

/// Round-capped stroke with a feathered edge and drag streaks running along
/// the motion. Streaks are keyed to the lateral field coordinate, so the
/// consecutive segments of one straight contact path continue the same lines.
/// Composited with `max`, so overlapping caps never double the density.
fn smear_stroke(out: &mut [u8], smear: BloodSmear, extent: Vec2) -> Option<InkRect> {
    let a = Vec2::from_array(smear.a) * extent;
    let b = Vec2::from_array(smear.b) * extent;
    let half = (smear.width_cm * 0.5).max(0.5);
    if !a.is_finite() || !b.is_finite() || !half.is_finite() { return None; }
    let delta = b - a;
    let len2 = delta.length_squared();
    let perp = if len2 > 1.0e-6 { Vec2::new(-delta.y, delta.x) / len2.sqrt() } else { Vec2::Y };
    let texel = extent / N as f32;
    let lo = ((a.min(b) - Vec2::splat(half)) / texel).floor().max(Vec2::ZERO);
    let hi = ((a.max(b) + Vec2::splat(half)) / texel).ceil().min(Vec2::splat((N - 1) as f32));
    let rect = texel_rect(lo, hi)?;
    let strength = smear.strength.clamp(0.0, 1.0);
    for y in lo.y as usize..=hi.y as usize { for x in lo.x as usize..=hi.x as usize {
        let p = (Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) * texel;
        let t = if len2 > 1.0e-6 { ((p - a).dot(delta) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let lateral = p.dot(perp);
        // Ragged edge: the local half-width wanders with the streak pattern.
        let edge_noise = value_noise(lateral / 5.0 + 17.0);
        let radius = half * (0.82 + 0.3 * edge_noise);
        let d = p.distance(a + delta * t) / radius;
        if d >= 1.0 { continue; }
        let edge = ((1.0 - d) / 0.35).clamp(0.0, 1.0);
        let edge = edge * edge * (3.0 - 2.0 * edge);
        let streak = 0.6 * value_noise(lateral / 1.6) + 0.4 * value_noise(lateral / 6.0 + 41.0);
        let alpha = strength * edge * (0.4 + 0.6 * streak);
        let v = (alpha * 255.0 + 0.5) as u8;
        out[y*N+x] = out[y*N+x].max(v);
    }}
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_background_is_not_filled_with_blood() {
        let mut image = image::RgbaImage::from_pixel(512, 512, image::Rgba([255,255,255,255]));
        image.put_pixel(128, 128, image::Rgba([120,0,0,255]));
        let atlas = Atlas::from_image(image, 4).unwrap();
        assert_eq!(atlas.alpha(0, Vec2::new(0.05,0.05)), 0.0);
        assert!(atlas.pixel_alpha(0, 128,128) > 0.95);
    }

    #[test]
    fn black_background_red_artwork_is_detected() {
        let mut image = image::RgbaImage::from_pixel(512, 512, image::Rgba([0,0,0,255]));
        image.put_pixel(128, 128, image::Rgba([220,0,0,255]));
        let atlas = Atlas::from_image(image, 4).unwrap();
        assert_eq!(atlas.pixel_alpha(0, 0,0), 0.0);
        assert!(atlas.pixel_alpha(0, 128,128) > 0.8);
    }

    #[test]
    fn single_high_resolution_image_is_one_splat() {
        let mut image = image::RgbaImage::from_pixel(1024,1024, image::Rgba([0,0,0,0]));
        image.put_pixel(512,512,image::Rgba([255,255,255,255]));
        let atlas = Atlas::from_image(image, 1).unwrap();
        assert_eq!(atlas.tiles,1);
        assert_eq!(atlas.tile_size,1024);
        assert!(atlas.pixel_alpha(0,512,512) > 0.99);
    }

    #[test]
    fn transparency_takes_precedence_over_rgb() {
        let mut image = image::RgbaImage::from_pixel(512, 512, image::Rgba([255,0,0,0]));
        image.put_pixel(128,128,image::Rgba([255,0,0,255]));
        let atlas = Atlas::from_image(image, 4).unwrap();
        assert_eq!(atlas.pixel_alpha(0,0,0),0.0);
        assert_eq!(atlas.pixel_alpha(0,128,128),1.0);
    }
}
