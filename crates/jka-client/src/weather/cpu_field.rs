//! CPU copy of the weather field and of the puddle shape.
//!
//! The GPU decides where standing water is (`weather_surface.wgsl`); the CPU needs
//! the same answer in a few places: whether the camera is under cover (the
//! wet-weather grade fades out indoors) and whether a footstep landed in water
//! (only then does it make a splash). `puddle_at` is a line-for-line port of
//! `weather_sample_field` + `weather_puddle_shape`; keep them in step, and see the
//! reference-value tests below.

use super::rain::{WeatherOcclusionInfo, WEATHER_NO_SURFACE_HEIGHT};

pub(crate) struct CpuField {
    info: WeatherOcclusionInfo,
    texels: Vec<[f32; 4]>,
}

/// Standing water at a point, as the shaders draw it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PuddleSample {
    /// 0..1: how much of the point is under water (soft shoreline).
    pub coverage: f32,
    /// 0..1: 0 at the shore, 1 in the deepest water.
    pub depth: f32,
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn fract(value: f32) -> f32 {
    value - value.floor()
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn hash12(x: f32, y: f32) -> f32 {
    let p = [fract(x * 0.1031), fract(y * 0.1031), fract(x * 0.1031)];
    let d = p[0] * (p[1] + 33.33) + p[1] * (p[2] + 33.33) + p[2] * (p[0] + 33.33);
    let q = [p[0] + d, p[1] + d, p[2] + d];
    fract((q[0] + q[1]) * q[2])
}

pub(crate) fn value_noise(x: f32, y: f32) -> f32 {
    let (cx, cy) = (x.floor(), y.floor());
    let (fx, fy) = (x - cx, y - cy);
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let n00 = hash12(cx, cy);
    let n10 = hash12(cx + 1.0, cy);
    let n01 = hash12(cx, cy + 1.0);
    let n11 = hash12(cx + 1.0, cy + 1.0);
    mix(mix(n00, n10, ux), mix(n01, n11, ux), uy)
}

fn rotate(x: f32, y: f32) -> (f32, f32) {
    (0.8 * x - 0.6 * y, 0.6 * x + 0.8 * y)
}

pub(crate) fn relief(x: f32, z: f32) -> f32 {
    let (mut px, mut py) = (x / 210.0, z / 210.0);
    let n0 = value_noise(px, py);
    (px, py) = rotate(px, py);
    (px, py) = (px * 2.37 + 17.3, py * 2.37 + 5.1);
    let n1 = value_noise(px, py);
    (px, py) = rotate(px, py);
    (px, py) = (px * 2.41 + 3.7, py * 2.41 + 11.9);
    let n2 = value_noise(px, py);
    0.62 * n0 + 0.26 * n1 + 0.12 * n2
}

pub(crate) fn cluster(x: f32, z: f32) -> f32 {
    value_noise(x / 900.0 + 31.7, z / 900.0 + 7.3)
}

/// What the field says about the ground under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FieldSample {
    pub exposure: f32,
    pub flat_area: f32,
    pub basin: f32,
}

/// `weather_puddle_shape`. Returns `(coverage, depth)`.
pub(crate) fn puddle_shape(
    field: FieldSample,
    x: f32,
    z: f32,
    accumulation: f32,
    scatter_amount: f32,
) -> (f32, f32) {
    let mut level = 0.0_f32;
    if field.basin > 0.001 {
        level = field.basin * (0.14 + 1.6 * accumulation);
    }
    if field.flat_area > 0.02 && scatter_amount > 0.01 {
        let eligibility = smoothstep(0.05, 0.6, field.flat_area);
        let amount = smoothstep(0.0, 1.0, scatter_amount).sqrt();
        let scatter_level =
            amount * (0.235 + 0.15 * accumulation + (cluster(x, z) - 0.5) * 0.14);
        level = level.max(scatter_level * eligibility);
    }
    if level <= 0.001 {
        return (0.0, 0.0);
    }
    let submerged = level - relief(x, z);
    (
        smoothstep(0.0, 0.22, submerged),
        (submerged / 0.35).clamp(0.0, 1.0),
    )
}

impl CpuField {
    pub(crate) fn new(info: WeatherOcclusionInfo, texels: Vec<[f32; 4]>) -> Self {
        debug_assert_eq!(texels.len(), info.width as usize * info.height as usize);
        Self { info, texels }
    }

    fn texel(&self, x: i32, z: i32) -> [f32; 4] {
        let x = x.clamp(0, self.info.width as i32 - 1) as usize;
        let z = z.clamp(0, self.info.height as i32 - 1) as usize;
        self.texels[z * self.info.width as usize + x]
    }

    fn uv(&self, position: [f32; 3]) -> Option<(f32, f32)> {
        let u = (position[0] - self.info.min_xz[0]) * self.info.inv_extent_xz[0];
        let v = (position[2] - self.info.min_xz[1]) * self.info.inv_extent_xz[1];
        (u > 0.0 && u < 1.0 && v > 0.0 && v < 1.0).then_some((u, v))
    }

    /// 1 in the open, 0 fully under cover, read from the nearest texel: the rule
    /// the surface shaders use, without the bilinear blend.
    pub(crate) fn exposure_at(&self, position: [f32; 3]) -> f32 {
        let Some((u, v)) = self.uv(position) else { return 1.0 };
        let x = (u * self.info.width as f32) as i32;
        let z = (v * self.info.height as f32) as i32;
        exposure_from_cover(self.texel(x, z)[0], position[1])
    }

    /// `weather_sample_field`.
    pub(crate) fn sample(&self, position: [f32; 3]) -> FieldSample {
        let open = FieldSample { exposure: 1.0, flat_area: 0.0, basin: 0.0 };
        let Some((u, v)) = self.uv(position) else { return open };
        let px = u * self.info.width as f32 - 0.5;
        let pz = v * self.info.height as f32 - 0.5;
        let (bx, bz) = (px.floor() as i32, pz.floor() as i32);
        let (fx, fz) = (px - bx as f32, pz - bz as f32);
        let corners = [
            self.texel(bx, bz),
            self.texel(bx + 1, bz),
            self.texel(bx, bz + 1),
            self.texel(bx + 1, bz + 1),
        ];
        let y = position[1];
        let bilinear = |f: &dyn Fn(&[f32; 4]) -> f32| {
            let [a, b, c, d] = [
                f(&corners[0]),
                f(&corners[1]),
                f(&corners[2]),
                f(&corners[3]),
            ];
            mix(mix(a, b, fx), mix(c, d, fx), fz)
        };
        let height_match = |t: &[f32; 4]| 1.0 - smoothstep(6.0, 20.0, (t[1] - y).abs());
        FieldSample {
            exposure: bilinear(&|t| exposure_from_cover(t[0], y)),
            flat_area: bilinear(&|t| t[2] * height_match(t)),
            basin: bilinear(&|t| t[3] * height_match(t)),
        }
    }

    /// Standing water under a point, exactly as the shaders would draw it.
    pub(crate) fn puddle_at(&self, position: [f32; 3], accumulation: f32, scatter: f32) -> PuddleSample {
        if accumulation <= 0.001 {
            return PuddleSample::default();
        }
        let field = self.sample(position);
        if field.exposure <= 0.001 {
            return PuddleSample::default();
        }
        let (coverage, depth) = puddle_shape(field, position[0], position[2], accumulation, scatter);
        PuddleSample { coverage: coverage * field.exposure, depth }
    }
}

fn exposure_from_cover(cover_y: f32, surface_y: f32) -> f32 {
    if cover_y <= WEATHER_NO_SURFACE_HEIGHT + 1.0 {
        return 1.0;
    }
    1.0 - smoothstep(6.0, 24.0, cover_y - surface_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values from an independent float32 port of weather_surface.wgsl
    // (hash12 / value noise / relief / cluster).
    #[test]
    fn noise_matches_the_wgsl_reference_values() {
        for ((x, y), expected) in [
            ((3.0, 7.0), 0.572_753_9),
            ((-12.5, 41.25), 0.124_511_72),
            ((123.5, -77.25), 0.437_988_28),
        ] {
            let actual = hash12(x, y);
            assert!((actual - expected).abs() < 0.006, "hash12({x}, {y}) = {actual}, expected {expected}");
        }
        for ((x, z), expected_relief, expected_cluster) in [
            ((0.0, 0.0), 0.253_720_88, 0.580_809_65),
            ((150.0, 300.0), 0.594_604_6, 0.556_327_3),
            ((-420.0, 910.0), 0.558_993_5, 0.626_831_5),
            ((1234.5, -987.25), 0.601_138_95, 0.412_768_8),
        ] {
            let r = relief(x, z);
            let c = cluster(x, z);
            assert!((r - expected_relief).abs() < 0.012, "relief({x}, {z}) = {r}, expected {expected_relief}");
            assert!((c - expected_cluster).abs() < 0.012, "cluster({x}, {z}) = {c}, expected {expected_cluster}");
        }
    }

    #[test]
    fn relief_is_bounded_with_the_expected_mean() {
        let mut sum = 0.0;
        let mut count = 0.0;
        for i in 0..64 {
            for j in 0..64 {
                let r = relief(i as f32 * 60.0, j as f32 * 60.0);
                assert!((0.0..=1.0).contains(&r));
                sum += r;
                count += 1.0;
            }
        }
        let mean = sum / count;
        assert!((0.45..0.58).contains(&mean), "relief mean {mean}");
    }

    fn info() -> WeatherOcclusionInfo {
        WeatherOcclusionInfo { min_xz: [0.0, 0.0], inv_extent_xz: [1.0 / 800.0; 2], width: 100, height: 100 }
    }

    fn plaza(cover: f32) -> CpuField {
        // Open flat ground at y = 0 everywhere.
        CpuField::new(info(), vec![[cover, 0.0, 1.0, 0.0]; 100 * 100])
    }

    #[test]
    fn dry_or_unmapped_ground_has_no_puddle() {
        let field = plaza(WEATHER_NO_SURFACE_HEIGHT);
        assert_eq!(field.puddle_at([400.0, 0.0, 400.0], 0.0, 1.0), PuddleSample::default());
        assert_eq!(field.puddle_at([400.0, 0.0, 400.0], 1.0, 0.0), PuddleSample::default());
        assert_eq!(field.puddle_at([-50.0, 0.0, 400.0], 1.0, 1.0), PuddleSample::default());
    }

    #[test]
    fn heavy_rain_floods_a_share_of_flat_ground_and_cover_stops_it() {
        let open = plaza(WEATHER_NO_SURFACE_HEIGHT);
        let mut wet = 0;
        let mut total = 0;
        for i in 0..80 {
            for j in 0..80 {
                let position = [40.0 + i as f32 * 9.0, 0.0, 40.0 + j as f32 * 9.0];
                total += 1;
                if open.puddle_at(position, 1.0, 1.0).coverage > 0.5 {
                    wet += 1;
                }
            }
        }
        let share = wet as f32 / total as f32;
        assert!((0.05..0.35).contains(&share), "flooded share {share}");

        // The same ground under a roof 100 units up stays dry.
        let roofed = plaza(100.0);
        for i in 0..40 {
            let position = [40.0 + i as f32 * 19.0, 0.0, 400.0];
            assert_eq!(roofed.puddle_at(position, 1.0, 1.0).coverage, 0.0);
        }
        assert!(roofed.exposure_at([400.0, 0.0, 400.0]) < 0.01);
        assert!(open.exposure_at([400.0, 0.0, 400.0]) > 0.99);
    }

    #[test]
    fn basins_flood_before_flat_ground_does() {
        let basin = CpuField::new(info(), vec![[WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0, 1.0]; 100 * 100]);
        let flat = plaza(WEATHER_NO_SURFACE_HEIGHT);
        let count = |field: &CpuField| {
            (0..60)
                .flat_map(|i| (0..60).map(move |j| [40.0 + i as f32 * 12.0, 0.0, 40.0 + j as f32 * 12.0]))
                .filter(|position| field.puddle_at(*position, 0.22, 1.0).coverage > 0.5)
                .count()
        };
        assert!(count(&basin) > 2 * count(&flat));
    }
}
