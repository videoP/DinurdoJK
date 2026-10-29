//! Selected MF_TextureAdvection: bracketed three-phase flow and WPO attenuation.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
//! See wave-advection-reference.json. Requires the native FFT detail texture;
//! this module does not manufacture a replacement for its normals/alpha height.

pub const SHADER: &str = include_str!("wave_advection.wgsl");
/// Material functions connected as the selected live-grid vertex path. The
/// caller supplies the actual native FFT detail texture, frame-phase inputs,
/// native material outputs and samplers; there is no fallback texture/shader.
pub fn vertex_shader() -> String {
    [
        super::material::SHADER,
        SHADER,
        include_str!("wave_vertex.wgsl"),
    ]
    .join("\n")
}
const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = 2.0 * PI;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub angle_degrees: f32,
    pub magnitude_delta: f32,
    pub pulse_length: f32,
    pub texture_scale: f32,
    pub speed: f32,
    pub vertex_mip: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            angle_degrees: 20.0,
            magnitude_delta: 70.0,
            pulse_length: 2.0,
            texture_scale: 0.0005,
            speed: 1.0,
            vertex_mip: 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub velocities: [[f32; 2]; 4],
    pub angle_blend: f32,
    pub magnitude_blend: f32,
    pub times: [f32; 3],
    pub weights: [f32; 3],
    pub cycles: [f32; 3],
    pub offsets: [f32; 3],
}

/// Native PseudoRandom uses frac, not Rust's signed fract. Do not substitute a
/// different hash or seed table. CPU dot/contraction is an ideal f32 reference;
/// device-specific low bits can change the randomized phase offsets.
pub fn pseudo_random(cycle: f32) -> f32 {
    let frac = |x: f32| x - x.floor();
    let x = frac(cycle / 128.0) * 128.0 - 64.340622;
    let y = -72.465622f32;
    frac((x * x) * 20.390625 + (y * y) * 60.703125 + (x * y) * 2.4281209)
}

impl Plan {
    pub fn new(velocity: [f32; 2], normal: [f32; 3], time: f32, s: Settings) -> Self {
        Self::with_random(velocity, normal, time, s, pseudo_random)
    }

    /// Independent device-math oracles may supply native PseudoRandom values
    /// for CPU/GPU checks without guessing a different hash or rounding mode.
    pub fn with_random(
        velocity: [f32; 2],
        normal: [f32; 3],
        time: f32,
        s: Settings,
        mut random: impl FnMut(f32) -> f32,
    ) -> Self {
        let scaled = [velocity[0] * s.speed, velocity[1] * s.speed, 0.0];
        let dot = scaled[0] * normal[0] + scaled[1] * normal[1] + scaled[2] * normal[2];
        let projected = [scaled[0] - dot * normal[0], scaled[1] - dot * normal[1]];
        let (dir0, dir1, angle_blend) = if s.angle_degrees > 1e-5 {
            let delta = s.angle_degrees * PI / 180.0;
            let mut angle = projected[1].atan2(projected[0]);
            if angle < 0.0 {
                angle += 2.0 * PI;
            }
            let fraction = angle % delta;
            let angle0 = angle - fraction;
            let dir = [angle0.cos(), angle0.sin()];
            let c = delta.cos();
            let sin = delta.sin();
            (
                dir,
                [dir[0] * c - dir[1] * sin, dir[0] * sin + dir[1] * c],
                fraction / delta,
            )
        } else {
            let length = (projected[0] * projected[0] + projected[1] * projected[1]).sqrt();
            let dir = projected.map(|v| v / length);
            (dir, dir, 0.0)
        };
        let magnitude = (projected[0] * projected[0] + projected[1] * projected[1]).sqrt();
        let (m0, m1, magnitude_blend) = if s.magnitude_delta > 1e-5 {
            let fraction = magnitude % s.magnitude_delta;
            let m0 = magnitude - fraction;
            (m0, m0 + s.magnitude_delta, fraction / s.magnitude_delta)
        } else {
            (magnitude, magnitude, 0.0)
        };
        let time_scale = TWO_PI / s.pulse_length;
        let weights = std::array::from_fn(|i| {
            (0.5 - 0.5 * (time_scale * time + i as f32 * TWO_PI / 3.0).cos()) / 1.5
        });
        let phase = std::array::from_fn::<_, 3, _>(|i| time + s.pulse_length * i as f32 / 3.0);
        let times = phase.map(|v| v % s.pulse_length);
        let cycles = phase.map(|v| (v / s.pulse_length).floor());
        Self {
            velocities: [
                dir0.map(|v| v * m0),
                dir0.map(|v| v * m1),
                dir1.map(|v| v * m0),
                dir1.map(|v| v * m1),
            ],
            angle_blend,
            magnitude_blend,
            times,
            weights,
            cycles,
            offsets: cycles.map(|v| random(v) * 1024.0),
        }
    }

    pub fn uv(
        self,
        world_xy: [f32; 2],
        settings: Settings,
        bracket: usize,
        phase: usize,
    ) -> [f32; 2] {
        std::array::from_fn(|i| {
            ((world_xy[i] - self.velocities[bracket][i] * self.times[phase]) + self.offsets[phase])
                * settings.texture_scale
        })
    }

    pub fn sample(
        self,
        world_xy: [f32; 2],
        settings: Settings,
        mips: [f32; 3],
        mut sample: impl FnMut([f32; 2], f32) -> [f32; 4],
    ) -> [f32; 4] {
        let mut samples = [[0.0f32; 4]; 4];
        for (i, value) in samples.iter_mut().enumerate() {
            let taps = std::array::from_fn::<_, 3, _>(|j| {
                sample(self.uv(world_xy, settings, i, j), mips[j])
            });
            *value = std::array::from_fn(|c| {
                taps[0][c] * self.weights[0]
                    + taps[1][c] * self.weights[1]
                    + taps[2][c] * self.weights[2]
            });
        }
        std::array::from_fn(|c| {
            let a = samples[0][c] + self.magnitude_blend * (samples[1][c] - samples[0][c]);
            let b = samples[2][c] + self.magnitude_blend * (samples[3][c] - samples[2][c]);
            a + self.angle_blend * (b - a)
        })
    }
}

/// Fragment footprint uses unscaled/unprojected velocity in the native custom
/// node, not the bracketed velocity. Callers provide derivatives of those UVs.
pub fn fragment_mip(
    dx: [f32; 2],
    dy: [f32; 2],
    dimensions: [u32; 2],
    bias: f32,
    levels: u32,
) -> f32 {
    let x = std::array::from_fn::<_, 2, _>(|i| dx[i] * dimensions[i] as f32);
    let y = std::array::from_fn::<_, 2, _>(|i| dy[i] * dimensions[i] as f32);
    let max = (x[0] * x[0] + x[1] * x[1]).max(y[0] * y[0] + y[1] * y[1]);
    (0.5 * max.max(1e-8).log2() + bias).clamp(0.0, levels as f32)
}

#[derive(Clone, Copy, Debug)]
pub struct Height {
    pub low_velocity_scale: f32,
    pub velocity_min: f32,
    pub velocity_max: f32,
    pub power: f32,
    pub multiplier: f32,
    pub camera_falloff: f32,
}
impl Default for Height {
    fn default() -> Self {
        Self {
            low_velocity_scale: 0.1,
            velocity_min: 0.0,
            velocity_max: 400.0,
            power: 3.0,
            multiplier: 5.0,
            camera_falloff: 200.0,
        }
    }
}
impl Height {
    /// Sampled alpha, raw river velocity magnitude and high precision camera
    /// distance are upstream graph inputs. The native camera smoothstep grows
    /// from falloff to falloff+50; do not reverse it to invent a near-camera LOD.
    pub fn evaluate(self, alpha: f32, magnitude: f32, camera_distance: f32) -> f32 {
        let smooth = |a: f32, b: f32, v: f32| {
            let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let attenuation = 1.0
            + (1.0 - self.low_velocity_scale.clamp(0.0, 1.0))
                * (smooth(self.velocity_min, self.velocity_max, magnitude) - 1.0);
        let powered = if attenuation <= 2.980233e-8 {
            0.0
        } else {
            attenuation.powf(self.power)
        };
        (alpha * (powered * self.multiplier))
            * smooth(
                self.camera_falloff,
                self.camera_falloff + 50.0,
                camera_distance,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_three_phase_flow_keeps_remainder_cycles_and_constant_texture() {
        for time in [-0.125, 0.0, 1.999, 2.0, 7.5] {
            let p = Plan::new([180.0, -23.0], [0.0, 0.0, 1.0], time, Settings::default());
            assert!((p.weights.iter().sum::<f32>() - 1.0).abs() < 2e-6);
            let value = p.sample([12.0, -34.0], Settings::default(), [2.0; 3], |_, _| {
                [1.0, -2.0, 0.5, 3.0]
            });
            for (a, b) in value.into_iter().zip([1.0, -2.0, 0.5, 3.0]) {
                assert!((a - b).abs() < 1e-5);
            }
        }
        assert!(Plan::new([1.0, 1.0], [0.0, 0.0, 1.0], -0.125, Settings::default()).times[0] < 0.0);
        for cycle in -5..5 {
            assert_eq!(
                pseudo_random(cycle as f32),
                pseudo_random((cycle + 128) as f32)
            );
        }
    }
    #[test]
    fn river_wave_height_retains_native_power_and_camera_direction() {
        let h = Height::default();
        assert_eq!(h.evaluate(2.0, 400.0, 199.0), 0.0);
        assert_eq!(h.evaluate(2.0, 400.0, 225.0), 5.0);
        assert_eq!(h.evaluate(2.0, 400.0, 250.0), 10.0);
        assert!((h.evaluate(2.0, 0.0, 250.0) - 0.01).abs() < 1e-7);
        assert_eq!(fragment_mip([0.0; 2], [0.0; 2], [512; 2], 2.0, 10), 0.0);
    }
}
