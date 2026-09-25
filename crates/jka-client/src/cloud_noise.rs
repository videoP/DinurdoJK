//! Tileable Perlin / Worley noise baking for the volumetric cloud renderer.
//!
//! This is a CPU port of the noise pipeline in `evroon/bevy-volumetric-clouds`
//! (`src/shaders/common.wgsl` plus the `init` entry point of
//! `clouds_compute.wgsl`), which in turn follows the Horizon Zero Dawn and
//! Frostbite cloud references.
//!
//! Two products are baked once, lazily, the first time clouds are enabled:
//!
//! * a tileable 2D *weather map* driving the cloud silhouette. `r` is the
//!   Perlin-Worley base field, `g` is a spatially varying contrast floor that
//!   the density remap divides by, `b` is a per-region cloud-height field so
//!   towering and flat cloud masses can coexist in one sky, and `a` is a
//!   low-frequency base-offset field the reference has no equivalent for.
//! * a tileable 3D *Worley detail volume* that erodes the silhouette edges into
//!   the cauliflower billows the base field alone cannot produce.
//!
//! Both are stored as f16. The `g` channel is negative by construction, and the
//! coverage ramp resolves a window only a fifth of the field's range wide, so
//! the 8-bit volume this replaces banded visibly across every soft cloud edge.

/// Edge length of the square tileable weather map, in texels.
pub const WEATHER_SIZE: u32 = 1024;
/// Edge length of the cubic tileable Worley detail volume, in texels.
pub const DETAIL_SIZE: u32 = 64;

fn fract(v: f32) -> f32 {
    v - v.floor()
}

fn fract3(v: [f32; 3]) -> [f32; 3] {
    [fract(v[0]), fract(v[1]), fract(v[2])]
}

/// Euclidean modulo, always returning a value in `[0, m)`.
///
/// The shader reference wraps lattice cells with `%`, whose sign follows the
/// dividend. The Worley neighbour loop reaches cell -1 at the low edge of the
/// tile, so `%` hashes it as -1 there but as `tile - 1` at the high edge, and
/// the field does not actually meet across the wrap. That seam is invisible in
/// the reference, which point-samples a single tile, but this port repeats the
/// map across the whole sky, where it would draw a straight line through it.
fn modulo(v: f32, m: f32) -> f32 {
    v - m * (v / m).floor()
}

/// Hash without Sine by Dave Hoskins (<https://www.shadertoy.com/view/4djSRW>).
fn hash13(p: [f32; 3]) -> f32 {
    let q = fract3([p[0] * 1031.1031, p[1] * 1031.1031, p[2] * 1031.1031]);
    let d = q[0] * (q[1] + 19.19) + q[1] * (q[2] + 19.19) + q[2] * (q[0] + 19.19);
    let q = [q[0] + d, q[1] + d, q[2] + d];
    fract((q[0] + q[1]) * q[2])
}

fn value_hash(p: [f32; 3]) -> f32 {
    let q = fract3([p[0] * 0.1031, p[1] * 0.1031, p[2] * 0.1031]);
    let d = q[0] * (q[1] + 19.19) + q[1] * (q[2] + 19.19) + q[2] * (q[0] + 19.19);
    let q = [q[0] + d, q[1] + d, q[2] + d];
    fract((q[0] + q[1]) * q[2])
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Tileable trilinear value noise. `tile` must be the integer lattice period so
/// the field wraps exactly at the texture boundary.
fn hash_based_noise(x: [f32; 3], tile: f32) -> f32 {
    let p = [x[0].floor(), x[1].floor(), x[2].floor()];
    let raw = [x[0] - p[0], x[1] - p[1], x[2] - p[2]];
    let f = [
        raw[0] * raw[0] * (3.0 - 2.0 * raw[0]),
        raw[1] * raw[1] * (3.0 - 2.0 * raw[1]),
        raw[2] * raw[2] * (3.0 - 2.0 * raw[2]),
    ];

    let h = |dx: f32, dy: f32, dz: f32| {
        value_hash([
            modulo(p[0] + dx, tile),
            modulo(p[1] + dy, tile),
            modulo(p[2] + dz, tile),
        ])
    };

    lerp(
        lerp(
            lerp(h(0.0, 0.0, 0.0), h(1.0, 0.0, 0.0), f[0]),
            lerp(h(0.0, 1.0, 0.0), h(1.0, 1.0, 0.0), f[0]),
            f[1],
        ),
        lerp(
            lerp(h(0.0, 0.0, 1.0), h(1.0, 0.0, 1.0), f[0]),
            lerp(h(0.0, 1.0, 1.0), h(1.0, 1.0, 1.0), f[0]),
            f[1],
        ),
        f[2],
    )
}

/// Tileable Worley/cellular noise. The feature point inside each cell is offset
/// by a single scalar hash broadcast to all three axes, exactly as the shader
/// reference does; the resulting diagonal point distribution is part of the
/// look being reproduced, not an oversight of this port.
fn voronoi(x: [f32; 3], tile: f32) -> f32 {
    let p = [x[0].floor(), x[1].floor(), x[2].floor()];
    let f = [x[0] - p[0], x[1] - p[1], x[2] - p[2]];

    let mut res = 100.0f32;
    for k in -1..=1 {
        for j in -1..=1 {
            for i in -1..=1 {
                let b = [i as f32, j as f32, k as f32];
                let c = [
                    modulo(p[0] + b[0], tile),
                    modulo(p[1] + b[1], tile),
                    modulo(p[2] + b[2], tile),
                ];
                let offset = hash13(c);
                let r = [
                    b[0] - f[0] + offset,
                    b[1] - f[1] + offset,
                    b[2] - f[2] + offset,
                ];
                let d = r[0] * r[0] + r[1] * r[1] + r[2] * r[2];
                res = res.min(d);
            }
        }
    }

    1.0 - res
}

fn tilable_voronoi(p: [f32; 3], octaves: u32, base_freq: f32) -> f32 {
    let mut freq = base_freq;
    let mut amplitude = 1.0f32;
    let mut noise = 0.0f32;
    let mut weight = 0.0f32;

    for _ in 0..octaves {
        noise += amplitude * voronoi([p[0] * freq, p[1] * freq, p[2] * freq], freq);
        freq *= 2.0;
        weight += amplitude;
        amplitude *= 0.5;
    }

    noise / weight
}

fn tilable_perlin_fbm(p: [f32; 3], octaves: u32, base_freq: f32) -> f32 {
    let mut freq = base_freq;
    let mut amplitude = 1.0f32;
    let mut noise = 0.0f32;
    let mut weight = 0.0f32;

    for _ in 0..octaves {
        noise += amplitude * hash_based_noise([p[0] * freq, p[1] * freq, p[2] * freq], freq);
        freq *= 2.0;
        weight += amplitude;
        amplitude *= 0.5;
    }

    noise / weight
}

/// Measured centre and usable width of the raw base-offset field, used to
/// stretch it across the full 0..1 range. See `report_variation_field_spread`.
const BASE_FIELD_MEAN: f32 = 0.559;
const BASE_FIELD_SPREAD: f32 = 0.30;

/// One weather-map texel. See the module docs for what each channel drives.
fn weather_texel(uv: [f32; 2]) -> [f32; 4] {
    let coord = [uv[0], uv[1], 0.5];
    let shifted = [coord[0] + 0.5, coord[1] + 0.5, coord[2] + 0.5];

    const MFBM: f32 = 0.9;
    const MVOR: f32 = 0.7;

    let base = lerp(1.0, tilable_perlin_fbm(coord, 7, 4.0), MFBM)
        * lerp(1.0, tilable_voronoi(coord, 8, 9.0), MVOR);

    let contrast = 0.625 * tilable_voronoi(coord, 3, 15.0)
        + 0.250 * tilable_voronoi(coord, 3, 19.0)
        + 0.125 * tilable_voronoi(coord, 3, 23.0)
        - 1.0;

    let height = 1.0 - tilable_voronoi(shifted, 6, 9.0);

    // Low-frequency base-offset field. Every term the reference uses to shape
    // the bottom of the layer is a pure function of normalized height, so cloud
    // bases all land on one exact plane. Three octaves from a very low base
    // frequency gives undulation over regions thousands of units across, which
    // is the scale a real condensation level varies on.
    //
    // Averaging octaves pulls the raw field into a narrow band around its mean
    // (measured p5..p95 of 0.401..0.679), so used directly it acts as a constant
    // offset rather than variation. Normalise it about that mean to fill 0..1;
    // the shader then centres it so bases both rise and fall.
    let base_offset = ((tilable_perlin_fbm(coord, 3, 2.0) - BASE_FIELD_MEAN)
        / BASE_FIELD_SPREAD
        + 0.5)
        .clamp(0.0, 1.0);

    [base, contrast, height, base_offset]
}

/// One detail-volume texel: three Worley octave bands folded into an inverted
/// billow field, so high values sit inside the cell bodies rather than on the
/// cell walls.
fn detail_texel(coord: [f32; 3]) -> f32 {
    let r = tilable_voronoi(coord, 16, 3.0);
    let g = tilable_voronoi(coord, 4, 8.0);
    let b = tilable_voronoi(coord, 4, 16.0);
    (1.0 - (r + g * 0.5 + b * 0.25) / 1.75).max(0.0)
}

/// Encode to IEEE binary16. Values outside the half range clamp to the largest
/// finite half, and magnitudes below the smallest normal flush to zero; neither
/// case is reachable from the bounded noise fields baked here.
fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let abs_bits = bits & 0x7fff_ffff;

    if abs_bits >= 0x4780_0000 {
        return sign | 0x7bff;
    }
    if abs_bits < 0x3880_0000 {
        return sign;
    }

    let exp = ((abs_bits >> 23) as i32) - 127 + 15;
    let mantissa = (abs_bits & 0x007f_ffff) >> 13;
    sign | ((exp as u16) << 10) | (mantissa as u16)
}

fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16)
}

/// Bake the tileable weather map as tightly packed `Rgba16Float` texels.
pub fn bake_weather_map() -> Vec<u8> {
    let size = WEATHER_SIZE;
    let row_bytes = (size * 4 * 2) as usize;
    let mut data = vec![0u8; row_bytes * size as usize];

    let rows_per_band = size.div_ceil(worker_count() as u32).max(1);
    let band_bytes = row_bytes * rows_per_band as usize;

    std::thread::scope(|scope| {
        for (band, chunk) in data.chunks_mut(band_bytes).enumerate() {
            let y0 = band as u32 * rows_per_band;
            scope.spawn(move || {
                for (row, out) in chunk.chunks_mut(row_bytes).enumerate() {
                    let y = y0 + row as u32;
                    let v = (y as f32 + 0.5) / size as f32;
                    for x in 0..size {
                        let u = (x as f32 + 0.5) / size as f32;
                        let texel = weather_texel([u, v]);
                        let offset = (x * 8) as usize;
                        for (channel, value) in texel.into_iter().enumerate() {
                            let half = f32_to_f16(value).to_le_bytes();
                            out[offset + channel * 2] = half[0];
                            out[offset + channel * 2 + 1] = half[1];
                        }
                    }
                }
            });
        }
    });

    data
}

/// Bake the tileable Worley detail volume as tightly packed `R16Float` texels.
pub fn bake_detail_volume() -> Vec<u8> {
    let size = DETAIL_SIZE;
    let slice_bytes = (size * size * 2) as usize;
    let mut data = vec![0u8; slice_bytes * size as usize];

    let slices_per_band = size.div_ceil(worker_count() as u32).max(1);
    let band_bytes = slice_bytes * slices_per_band as usize;

    std::thread::scope(|scope| {
        for (band, chunk) in data.chunks_mut(band_bytes).enumerate() {
            let z0 = band as u32 * slices_per_band;
            scope.spawn(move || {
                for (slice, out) in chunk.chunks_mut(slice_bytes).enumerate() {
                    let z = z0 + slice as u32;
                    let w = (z as f32 + 0.5) / size as f32;
                    for y in 0..size {
                        let v = (y as f32 + 0.5) / size as f32;
                        for x in 0..size {
                            let u = (x as f32 + 0.5) / size as f32;
                            let half = f32_to_f16(detail_texel([u, v, w])).to_le_bytes();
                            let offset = ((y * size + x) * 2) as usize;
                            out[offset] = half[0];
                            out[offset + 1] = half[1];
                        }
                    }
                }
            });
        }
    });

    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_encoding_matches_known_values() {
        assert_eq!(f32_to_f16(0.0), 0x0000);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-1.0), 0xbc00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(2.0), 0x4000);
    }

    #[test]
    fn weather_channels_stay_in_expected_ranges() {
        // The density remap divides by `g` and adds `b`; both going out of range
        // would silently invert the cloud silhouette rather than fail loudly.
        for i in 0..64 {
            let u = i as f32 / 64.0;
            let texel = weather_texel([u, 1.0 - u]);
            assert!((0.0..=1.5).contains(&texel[0]), "base {}", texel[0]);
            assert!((-1.0..=0.01).contains(&texel[1]), "contrast {}", texel[1]);
            assert!((-0.01..=1.0).contains(&texel[2]), "height {}", texel[2]);
            assert!((-0.01..=1.01).contains(&texel[3]), "base offset {}", texel[3]);
        }
    }

    #[test]
    fn weather_map_tiles_seamlessly() {
        // A field that does not agree across the wrap shows every cloud tile
        // edge as a straight line across the sky.
        for i in 0..16 {
            let v = i as f32 / 16.0;
            let a = weather_texel([0.0, v]);
            let b = weather_texel([1.0, v]);
            assert!(
                (a[0] - b[0]).abs() < 1e-3,
                "seam at v={v}: {} vs {}",
                a[0],
                b[0]
            );
        }
    }

    #[test]
    #[ignore]
    fn report_variation_field_spread() {
        // Both variation controls read fields that averaging pushes toward 0.5.
        // If the usable spread is small the sliders act as a constant offset,
        // which is exactly "every cloud starts at the same height".
        let mut base = Vec::new();
        let mut top = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                let t = weather_texel([x as f32 / 64.0, y as f32 / 64.0]);
                top.push(t[2]);
                base.push(t[3]);
            }
        }
        for (name, mut v) in [("cloud height (b)", top), ("base offset (a)", base)] {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = v.iter().sum::<f32>() / v.len() as f32;
            let p5 = v[v.len() / 20];
            let p95 = v[v.len() * 19 / 20];
            println!(
                "{name:18} min {:.3} p5 {:.3} mean {:.3} p95 {:.3} max {:.3}  (p5..p95 spread {:.3})",
                v[0], p5, mean, p95, v[v.len() - 1], p95 - p5
            );
        }
    }

    #[test]
    fn base_offset_field_spans_a_usable_range() {
        // A narrow field makes the base-variation slider a constant shift of the
        // whole deck instead of per-region variation, which is invisible.
        let mut values: Vec<f32> = (0..48)
            .flat_map(|y| {
                (0..48).map(move |x| weather_texel([x as f32 / 48.0, y as f32 / 48.0])[3])
            })
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p5 = values[values.len() / 20];
        let p95 = values[values.len() * 19 / 20];
        assert!(
            p95 - p5 > 0.55,
            "base offset field too narrow to vary anything: p5 {p5:.3} p95 {p95:.3}"
        );
    }

    #[test]
    fn detail_volume_is_non_degenerate() {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for i in 0..32 {
            let t = i as f32 / 32.0;
            let value = detail_texel([t, t * 0.5 + 0.25, 1.0 - t]);
            min = min.min(value);
            max = max.max(value);
        }
        assert!(min >= 0.0, "detail went negative: {min}");
        assert!(max - min > 0.05, "detail field is flat: {min}..{max}");
    }
}

#[cfg(test)]
mod bake_timing {
    /// Not part of the normal suite: run with
    /// `cargo test --release --bins -- --ignored bake_timing --nocapture`
    /// to check the lazy first-enable hitch on this machine.
    #[test]
    #[ignore]
    fn report_bake_cost() {
        let start = std::time::Instant::now();
        let weather = super::bake_weather_map();
        let weather_ms = start.elapsed().as_secs_f64() * 1000.0;

        let start = std::time::Instant::now();
        let detail = super::bake_detail_volume();
        let detail_ms = start.elapsed().as_secs_f64() * 1000.0;

        println!(
            "weather {}^2: {weather_ms:.0} ms, {} KiB | detail {}^3: {detail_ms:.0} ms, {} KiB",
            super::WEATHER_SIZE,
            weather.len() / 1024,
            super::DETAIL_SIZE,
            detail.len() / 1024
        );
    }
}
