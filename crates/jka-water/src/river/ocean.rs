//! Selected Grid2D_OceanPatch spectrum and native Stockham FFT reference.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
//! See ocean-reference.json; ocean-noise.g8 is extracted native source R.
pub const SHADER: &str = include_str!("ocean.wgsl");
pub const NOISE: &[u8; 4096] = include_bytes!("ocean-noise.g8");
pub const SIZE: u32 = 512;
const PI: f32 = std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Parameters {
    pub gravity: f32,
    pub amplitude: f32,
    pub time_scale: f32,
    pub length: f32,
    pub wind_speed: f32,
    pub wind_angle: f32,
    pub alignment: i32,
    pub slosh: f32,
}
impl Default for Parameters {
    fn default() -> Self {
        Self {
            gravity: 980.0,
            amplitude: 3.0,
            time_scale: 2.5,
            length: 250.0,
            wind_speed: 55.0,
            wind_angle: 180.0,
            alignment: 0,
            slosh: 1.0,
        }
    }
}
/// Native wrapper passes scanline as kx, thread as ky. Do not transpose it
/// to match the pin names. Backward seeds add 0.25 to both components.
pub fn noise_uv(scanline: u32, thread: u32) -> [[f32; 2]; 4] {
    let half = SIZE as i32 / 2;
    let fold = |v: i32| if v > half { SIZE as i32 - v } else { v };
    let a = [
        fold(scanline as i32) as f32 / half as f32,
        fold(thread as i32) as f32 / half as f32,
    ];
    let b = [fold(scanline as i32 + half) as f32 / half as f32, a[1]];
    [a, b, a.map(|x| x + 0.25), b.map(|x| x + 0.25)]
}
/// Ideal f32 raw G8 SampleLevel. Hardware filtering can be supplied separately
/// in spectrum diagnostics rather than guessing device interpolation rounding.
pub fn sample_noise(uv: [f32; 2]) -> [f32; 4] {
    let p = uv.map(|v| v * 64.0 - 0.5);
    let floor = p.map(f32::floor);
    let alpha = [p[0] - floor[0], p[1] - floor[1]];
    let at =
        |x: i32, y: i32| NOISE[(x.rem_euclid(64) + 64 * y.rem_euclid(64)) as usize] as f32 / 255.0;
    let [x, y] = floor.map(|v| v as i32);
    let a = at(x, y) + (at(x + 1, y) - at(x, y)) * alpha[0];
    let b = at(x, y + 1) + (at(x + 1, y + 1) - at(x, y + 1)) * alpha[0];
    [a + (b - a) * alpha[1], 0.0, 0.0, 1.0]
}
/// Two local coefficients share native sign/mask state across the r loop.
pub fn spectrum_pair(
    scanline: u32,
    thread: u32,
    time: f32,
    p: Parameters,
    noise: [[f32; 4]; 4],
) -> [[f32; 2]; 2] {
    let half = SIZE as i32 / 2;
    let two_pi = 2.0 * PI;
    let factor = two_pi / p.length;
    let speed = (factor * p.gravity).sqrt();
    let scaled_time = p.time_scale * time;
    let mut angle = p.wind_angle % 360.0;
    if angle < 0.0 {
        angle += 360.0;
    }
    let radians = -(PI / 180.0) * angle;
    let wind = [radians.cos(), radians.sin()];
    let principal = if p.wind_speed.abs() > 0.001 {
        p.wind_speed * p.wind_speed * two_pi / p.gravity
    } else {
        1.0
    };
    let mut sign = 1.0;
    if p.wind_angle >= 180.0 {
        sign *= -1.0;
    }
    if p.wind_speed < 0.0 {
        sign *= -1.0;
    }
    let mut mask = 1.0;
    let mut k2 = scanline as i32;
    if k2 == half {
        mask = 0.0;
    } else if k2 > half {
        k2 = -(SIZE as i32 - k2);
        sign *= -1.0;
    }
    std::array::from_fn(|r| {
        let mut k1 = thread as i32 + r as i32 * half;
        if k1 == half {
            mask = 0.0;
        }
        if k1 > half {
            k1 = -(SIZE as i32 - k1);
            if k2 == 0 {
                sign *= -1.0;
            }
        }
        let k = [k1 as f32 * factor, k2 as f32 * factor];
        let magnitude = (k[0] * k[0] + k[1] * k[1]).sqrt();
        let mut amplitude = 0.0;
        if k1 != 0 || k2 != 0 {
            let kl = magnitude * principal;
            let dir = k.map(|v| v / magnitude);
            let cos = dir[0] * wind[0] + dir[1] * wind[1];
            let power = (cos * cos).powf(p.alignment.clamp(1, 20) as f32);
            let ksqr = (k1 * k1 + k2 * k2) as f32;
            amplitude =
                (p.amplitude.abs() * power / ((1.0 / (kl * kl)).exp() * (ksqr * ksqr))).sqrt();
        }
        let phase = |n: [f32; 4]| {
            let length = n.iter().map(|v| v * v).sum::<f32>().sqrt();
            let random = 20.0 * length;
            let random = random - random.floor();
            sign * speed * magnitude.sqrt() * scaled_time + two_pi * random
        };
        let a = phase(noise[r]);
        let b = phase(noise[r + 2]);
        let other = amplitude * p.slosh;
        [
            mask * (other * b.cos() + amplitude * a.cos()),
            mask * (other * (-b.sin()) + amplitude * a.sin()),
        ]
    })
}
fn complex(a: [f32; 2], theta: f32) -> [f32; 2] {
    let c = theta.cos();
    let s = theta.sin();
    [a[0] * c - a[1] * s, a[0] * s + a[1] * c]
}
/// Native positive-sign, unnormalized radix-two scanline. No grid half stores
/// occur inside this transform; the caller quantizes between the X/Y stages.
pub fn fft_line(input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = input.len();
    assert!(n >= 2 && n.is_power_of_two());
    let half = n / 2;
    let mut v = input.to_vec();
    let mut shared = vec![[0.0; 2]; n];
    let mut ns = 1;
    while ns < half {
        for t in 0..half {
            let theta = 2.0 * PI * (t % ns) as f32 / (ns * 2) as f32;
            let b = complex(v[t + half], theta);
            let a = v[t];
            let d = (t / ns) * ns * 2 + t % ns;
            shared[d] = [a[0] + b[0], a[1] + b[1]];
            shared[d + ns] = [a[0] - b[0], a[1] - b[1]];
        }
        v.copy_from_slice(&shared);
        ns *= 2;
    }
    for t in 0..half {
        let b = complex(v[t + half], 2.0 * PI * (t % half) as f32 / n as f32);
        let a = v[t];
        shared[t] = [a[0] + b[0], a[1] + b[1]];
        shared[t + half] = [a[0] - b[0], a[1] - b[1]];
    }
    shared
}
pub fn normal(height: f32, right: f32, up: f32) -> [f32; 4] {
    let dx = 150.0 / SIZE as f32;
    // Preserve the native cross rather than substituting slope normalization.
    let a = [dx, 0.0, right - height];
    let b = [0.0, dx, up - height];
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let length = cross.iter().map(|v| v * v).sum::<f32>().sqrt();
    let n = if length < 1e-10 {
        [0.0, 0.0, 1.0]
    } else {
        cross.map(|v| v / length)
    };
    [n[0], n[1], n[2], height]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_ocean_stockham_matches_independent_positive_dft() {
        let input: Vec<_> = (0..8)
            .map(|i| [i as f32 - 3.0, (i % 3) as f32 - 1.0])
            .collect();
        let actual = fft_line(&input);
        for (k, a) in actual.iter().enumerate() {
            let mut expected = [0.0f64; 2];
            for (j, v) in input.iter().enumerate() {
                let theta = 2.0 * std::f64::consts::PI * (k * j) as f64 / 8.0;
                expected[0] += v[0] as f64 * theta.cos() - v[1] as f64 * theta.sin();
                expected[1] += v[0] as f64 * theta.sin() + v[1] as f64 * theta.cos();
            }
            for c in 0..2 {
                assert!((a[c] as f64 - expected[c]).abs() < 0.00002);
            }
        }
    }
    #[test]
    fn river_ocean_preserves_seed_call_order_nyquist_and_shared_mask() {
        assert_eq!(noise_uv(511, 255)[1], [-255.0 / 256.0, 255.0 / 256.0]);
        let p = Parameters::default();
        let noise = [[0.5, 0.0, 0.0, 1.0]; 4];
        assert_eq!(spectrum_pair(256, 123, 0.3, p, noise), [[0.0; 2]; 2]);
        assert_eq!(spectrum_pair(0, 0, 0.3, p, noise), [[0.0; 2]; 2]);
        assert_eq!(normal(12.0, 12.0, 12.0), [0.0, 0.0, 1.0, 12.0]);
        assert_eq!(sample_noise([0.5 / 64.0; 2])[1..], [0.0, 0.0, 1.0]);
    }
}
