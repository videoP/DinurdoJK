//! `r_oceanType 5`'s packed pattern texture (`blue_water_gpu::godot_water::GodotPatterns`), baked once per process:
//!
//! * r: godot-water-shader's `textures/sea_foam.png` (sampled raw, as upstream's non-`source_color` uniform);
//! * g: its seamless cellular NoiseTexture2D (`ocean_noise.tres`, `erosion_noise.tres` and FoamScene's far-foam noise
//!   are all FastNoiseLite `noise_type = 2` at Godot's defaults, so one texture serves all three);
//! * b: its `i_noise_texture`, a default FastNoiseLite (smooth simplex).
//!
//! Godot bakes each noise at NoiseTexture2D's default 512² (normalized, seamless over a 10% blend skirt); that is
//! reproduced here and resampled to sea_foam.png's 1024², whose repeat covers the same 512 noise texels.
use fastnoise_lite::{CellularDistanceFunction, CellularReturnType, FastNoiseLite, FractalType, NoiseType};
use std::sync::{Arc, OnceLock};

const SEA_FOAM_PNG: &[u8] = include_bytes!("../../../../vendor/godot-water-shader/textures/sea_foam.png");

/// NoiseTexture2D's default size and seamless blend skirt.
const NOISE_SIZE: usize = 512;
const BLEND_SKIRT: f32 = 0.1;

/// Godot's FastNoiseLite resource defaults (seed 0, frequency 0.01, FBM 5 octaves, Euclidean cellular distance).
fn godot_noise(noise_type: NoiseType) -> FastNoiseLite {
    let mut noise = FastNoiseLite::with_seed(0);
    noise.set_noise_type(Some(noise_type));
    noise.set_frequency(Some(0.01));
    noise.set_fractal_type(Some(FractalType::FBm));
    noise.set_fractal_octaves(Some(5));
    noise.set_fractal_lacunarity(Some(2.0));
    noise.set_fractal_gain(Some(0.5));
    noise.set_fractal_weighted_strength(Some(0.0));
    noise.set_cellular_distance_function(Some(CellularDistanceFunction::Euclidean));
    noise.set_cellular_return_type(Some(CellularReturnType::Distance));
    noise.set_cellular_jitter(Some(1.0));
    noise
}

/// NoiseTexture2D with `seamless` and `normalize`: the noise over `NOISE_SIZE` plus the skirt, normalized to 0..1, its
/// far edge cross-faded over the near one so the texture repeats, quantized to 8 bits.
fn seamless_noise(noise: &FastNoiseLite) -> Vec<u8> {
    let skirt = ((NOISE_SIZE as f32 * BLEND_SKIRT) as usize).max(1);
    let source_size = NOISE_SIZE + skirt;
    let mut source: Vec<f32> = (0..source_size * source_size)
        .map(|i| noise.get_noise_2d((i % source_size) as f32, (i / source_size) as f32))
        .collect();
    let (low, high) = source.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| (l.min(*v), h.max(*v)));
    let range = (high - low).max(1e-6);
    for value in &mut source {
        *value = (*value - low) / range;
    }
    let base_share = |i: usize| {
        if i >= skirt { return 1.0; }
        let t = ((i as f32 / skirt as f32 - 0.1) / 0.8).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let at = |x: usize, y: usize| source[y * source_size + x];
    (0..NOISE_SIZE * NOISE_SIZE).map(|i| {
        let (x, y) = (i % NOISE_SIZE, i / NOISE_SIZE);
        let (tx, ty) = (base_share(x), base_share(y));
        let (wx, wy) = (x + NOISE_SIZE * usize::from(x < skirt), y + NOISE_SIZE * usize::from(y < skirt));
        let value = at(x, y) * tx * ty + at(wx, y) * (1.0 - tx) * ty + at(x, wy) * tx * (1.0 - ty)
            + at(wx, wy) * (1.0 - tx) * (1.0 - ty);
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    }).collect()
}

/// Bilinear, repeating: the `NOISE_SIZE`² texture at `size`² texel centres.
fn resample(noise: &[u8], size: usize) -> Vec<u8> {
    let scale = NOISE_SIZE as f32 / size as f32;
    let wrap = |v: isize| v.rem_euclid(NOISE_SIZE as isize) as usize;
    (0..size * size).map(|i| {
        let u = ((i % size) as f32 + 0.5) * scale - 0.5;
        let v = ((i / size) as f32 + 0.5) * scale - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let at = |x: isize, y: isize| f32::from(noise[wrap(y) * NOISE_SIZE + wrap(x)]);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        (top * (1.0 - fy) + bottom * fy).round() as u8
    }).collect()
}

fn bake() -> Vec<u8> {
    let size = blue_water_gpu::godot_water::PATTERN_SIZE as usize;
    let mut foam = image::load_from_memory_with_format(SEA_FOAM_PNG, image::ImageFormat::Png)
        .expect("vendored sea_foam.png decodes");
    if foam.width() as usize != size || foam.height() as usize != size {
        foam = foam.resize_exact(size as u32, size as u32, image::imageops::FilterType::Triangle);
    }
    let foam = foam.to_rgb8();
    let cellular = resample(&seamless_noise(&godot_noise(NoiseType::Cellular)), size);
    let simplex = resample(&seamless_noise(&godot_noise(NoiseType::OpenSimplex2S)), size);
    let mut rgba = vec![255u8; size * size * 4];
    for (i, texel) in rgba.chunks_exact_mut(4).enumerate() {
        texel[0] = foam.as_raw()[i * 3];
        texel[1] = cellular[i];
        texel[2] = simplex[i];
    }
    rgba
}

/// The packed texels (`PATTERN_SIZE`² RGBA8), baked on first use (on a pipeline worker) and kept for the process.
pub(super) fn patterns() -> Arc<Vec<u8>> {
    static BAKED: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
    Arc::clone(BAKED.get_or_init(|| {
        let started = std::time::Instant::now();
        let baked = Arc::new(bake());
        rverbose!(1, "[GERSTNER] pattern texture baked in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
        baked
    }))
}

/// The Gerstner material's smooth simplex source, reduced to a small height/gradient
/// texture for lakes. Two octaves keep only broad relief that a cheap mesh resolves.
pub(super) fn lake_noise() -> Arc<Vec<[f32; 4]>> {
    static BAKED: OnceLock<Arc<Vec<[f32; 4]>>> = OnceLock::new();
    Arc::clone(BAKED.get_or_init(|| {
        let mut noise = godot_noise(NoiseType::OpenSimplex2S);
        noise.set_fractal_octaves(Some(2));
        const SIZE: usize = 128;
        const REPEAT: f32 = 32.0;
        let heights = resample(&seamless_noise(&noise), SIZE);
        let at = |x: usize, y: usize| heights[(y % SIZE) * SIZE + x % SIZE] as f32 / 127.5 - 1.0;
        Arc::new((0..SIZE * SIZE).map(|i| {
            let (x, y) = (i % SIZE, i / SIZE);
            let derivative = SIZE as f32 / (2.0 * REPEAT);
            [at(x,y), (at(x+1,y)-at(x+SIZE-1,y))*derivative,
                (at(x,y+1)-at(x,y+SIZE-1))*derivative, 0.0]
        }).collect())
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn noise_is_seamless_and_spans_the_range() {
        let noise = super::seamless_noise(&super::godot_noise(fastnoise_lite::NoiseType::Cellular));
        let n = super::NOISE_SIZE;
        let (low, high) = noise.iter().fold((255u8, 0u8), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(low < 30 && high > 225, "normalized range {low}..{high}");
        // Across the wrap a texel steps no further than between ordinary neighbours.
        let step = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
        let interior = (0..n).map(|y| step(noise[y * n + n / 2], noise[y * n + n / 2 + 1])).max().unwrap();
        let seam = (0..n).map(|y| step(noise[y * n + n - 1], noise[y * n])).max().unwrap();
        assert!(seam <= interior.max(8) + 4, "seam step {seam} vs interior {interior}");
    }
}
