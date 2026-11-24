//! EchoField eap/emitter/SyrinxSynth.java (GPL-3.0), direct port.
//! Same eight species, ODE, RK4, dual tracheal delay lines and seeded variation.
use crate::random::JavaRandom;
use std::f64::consts::PI;
pub const SAMPLE_RATE: u32 = 44100;
const STATE_CLAMP: f32 = 2.0;

#[derive(Clone, Copy)]
enum Shape {
    Descending,
    Ascending,
    UpDown,
    UShape,
    Sinusoidal,
    Zigzag,
    Step,
    Complex,
}
struct Species {
    low: f32,
    high: f32,
    count: i32,
    duration: f32,
    gap: f32,
    shape: Shape,
    pressure_scale: f32,
    beta1: f32,
    beta2: f32,
    beta3: f32,
    p_min: f32,
    p_max: f32,
    k_min: f32,
    k_max: f32,
    delay: usize,
    reflection: f32,
}
impl Species {
    fn new(variant: i32, rng: &mut JavaRandom) -> Self {
        use Shape::*;
        let pv = 0.88 + rng.next_float() * 0.24;
        let tv = 0.75 + rng.next_float() * 0.50;
        let prv = 0.75 + rng.next_float() * 0.50;
        let (
            low,
            high,
            count,
            duration,
            gap,
            shape,
            beta1,
            beta2,
            beta3,
            p_min,
            p_max,
            k_min,
            k_max,
            delay,
            reflection,
        ) = match variant & 7 {
            0 => (
                2500.0,
                5500.0,
                3 + rng.next_int(5),
                0.12,
                0.08,
                Descending,
                -0.12,
                0.12,
                0.10,
                0.01,
                0.08,
                0.3,
                0.8,
                8 + rng.next_int(12),
                0.25 + rng.next_float() * 0.3,
            ),
            1 => (
                2000.0,
                6000.0,
                (2 + rng.next_int(4)) * 2,
                0.15,
                0.06,
                UpDown,
                -0.10,
                0.10,
                0.11,
                0.01,
                0.08,
                0.3,
                0.8,
                12 + rng.next_int(15),
                0.3 + rng.next_float() * 0.3,
            ),
            2 => (
                4500.0,
                9000.0,
                12 + rng.next_int(15),
                0.04,
                0.02,
                Sinusoidal,
                -0.15,
                0.15,
                0.08,
                0.02,
                0.10,
                0.2,
                0.7,
                4 + rng.next_int(6),
                0.15 + rng.next_float() * 0.25,
            ),
            3 => (
                1500.0,
                4500.0,
                3 + rng.next_int(4),
                0.30,
                0.15,
                UShape,
                -0.09,
                0.09,
                0.12,
                0.008,
                0.06,
                0.35,
                0.85,
                18 + rng.next_int(20),
                0.35 + rng.next_float() * 0.25,
            ),
            4 => (
                3000.0,
                7500.0,
                5 + rng.next_int(6),
                0.06,
                0.03,
                Zigzag,
                -0.13,
                0.14,
                0.09,
                0.015,
                0.09,
                0.25,
                0.75,
                6 + rng.next_int(8),
                0.2 + rng.next_float() * 0.3,
            ),
            5 => (
                2500.0,
                10000.0,
                6 + rng.next_int(8),
                0.08,
                0.04,
                Complex,
                -0.11,
                0.11,
                0.10,
                0.01,
                0.10,
                0.2,
                0.8,
                5 + rng.next_int(10),
                0.2 + rng.next_float() * 0.35,
            ),
            6 => (
                2500.0,
                6500.0,
                if rng.next_float() < 0.3 { 3 } else { 2 },
                0.35,
                0.10,
                Step,
                -0.10,
                0.13,
                0.11,
                0.01,
                0.07,
                0.3,
                0.7,
                7 + rng.next_int(8),
                0.25 + rng.next_float() * 0.25,
            ),
            _ => (
                1800.0,
                5500.0,
                2 + rng.next_int(4),
                0.25,
                0.20,
                Ascending,
                -0.08,
                0.08,
                0.13,
                0.005,
                0.05,
                0.4,
                0.85,
                20 + rng.next_int(25),
                0.4 + rng.next_float() * 0.25,
            ),
        };
        Self {
            low: low * pv,
            high: high * pv,
            count,
            duration: duration * tv,
            gap: gap * tv,
            shape,
            pressure_scale: prv,
            beta1,
            beta2,
            beta3,
            p_min,
            p_max,
            k_min,
            k_max,
            delay: delay.max(1) as usize,
            reflection,
        }
    }

    fn omega(&self, t: f32, syllable: i32) -> f32 {
        use Shape::*;
        let frac = syllable as f32 / (self.count - 1).max(1) as f32;
        let (lo, hi) = (self.low, self.high);
        let range = hi - lo;
        let base = match self.shape {
            Descending => (hi - range * frac) - range * 0.15 * t,
            Ascending => (lo + range * frac) + range * 0.15 * t,
            UpDown => {
                if syllable % 2 == 0 {
                    lo + range * t
                } else {
                    hi - range * t
                }
            }
            UShape => {
                let b = hi - range * 2.0 * (t - 0.5).abs();
                lo + (hi - b)
            }
            Sinusoidal => {
                (lo + hi) * 0.5 + range * 0.5 * (2.0 * PI * f64::from(t) * 2.0).sin() as f32
            }
            Zigzag => {
                let z = (t * 4.0) % 1.0;
                if z < 0.5 {
                    lo + range * z * 2.0
                } else {
                    hi - range * (z - 0.5) * 2.0
                }
            }
            Step => {
                if syllable == 0 {
                    hi
                } else {
                    lo
                }
            }
            Complex => {
                if t < 0.3 {
                    lo + range * (t / 0.3)
                } else if t < 0.6 {
                    hi
                } else {
                    let decay = (t - 0.6) / 0.4;
                    hi - range * 0.6 * decay
                }
            }
        };
        base * (2.0 * PI) as f32 / SAMPLE_RATE as f32
    }
    fn pressure(&self, t: f32, syllable: i32) -> f32 {
        use Shape::*;
        let scale = self.pressure_scale;
        let lo = self.p_min;
        let hi = self.p_max;
        let range = (hi - lo) * scale;
        let div = (self.count - 1).max(1) as f32;
        let base = match self.shape {
            Descending => (hi * scale - range * 0.3 * syllable as f32 / div) * (1.0 - 0.2 * t),
            Ascending => (lo * scale + range * 0.5 * syllable as f32 / div) + range * 0.3 * t,
            UpDown => lo * scale + range * (PI * f64::from(t)).sin() as f32,
            UShape => hi * scale - range * 0.5 * (PI * f64::from(t)).sin() as f32,
            Sinusoidal => {
                (lo + hi) * 0.5 * scale + range * 0.4 * (2.0 * PI * f64::from(t) * 3.0).sin() as f32
            }
            Zigzag => {
                let z = (t * 3.0) % 1.0;
                lo * scale + range * if z < 0.5 { z * 2.0 } else { 2.0 - z * 2.0 }
            }
            Step => {
                if syllable == 0 {
                    hi * scale * 0.8
                } else {
                    hi * scale * 0.6
                }
            }
            Complex => {
                (lo + hi) * 0.5 * scale + range * 0.4 * (2.0 * PI * f64::from(t)).cos() as f32
            }
        };
        base.max(0.0)
    }
    fn kappa(&self, t: f32, syllable: i32) -> f32 {
        use Shape::*;
        let (lo, hi) = (self.k_min, self.k_max);
        let range = hi - lo;
        match self.shape {
            Descending => lo + range * (0.3 + 0.4 * t),
            Ascending => hi - range * 0.4 * t,
            UpDown => lo + range * (0.5 + 0.3 * (PI * f64::from(t)).cos() as f32),
            UShape => lo + range * (PI * f64::from(t)).sin() as f32,
            Sinusoidal => {
                lo + range * (0.5 + 0.4 * (2.0 * PI * f64::from(t) * 3.0 + 1.5).sin() as f32)
            }
            Zigzag => {
                let z = (t * 3.0 + 0.25) % 1.0;
                lo + range * if z < 0.5 { z * 2.0 } else { 2.0 - z * 2.0 }
            }
            Step => {
                if syllable == 0 {
                    lo + range * 0.3
                } else {
                    lo + range * 0.6
                }
            }
            Complex => lo + range * (0.5 + 0.4 * (2.0 * PI * f64::from(t)).sin() as f32),
        }
    }
    fn rhs(&self, x: f32, y: f32, omega: f32, p: f32, kappa: f32) -> f32 {
        let damping = (self.beta1 + self.beta2 * x * x + self.beta3 * y * y) * y;
        let forcing = (p - kappa * x * x) * x;
        -omega * omega * x - damping + forcing
    }
}
fn envelope(t: f32) -> f32 {
    if t < 0.1 {
        t / 0.1
    } else if t > 0.85 {
        (1.0 - t) / 0.15
    } else {
        1.0
    }
}

/// Mono float PCM at 44100 Hz; returns actual length. Remaining samples are
/// not modified, as in Java. The caller owns normalization and OpenAL upload.
pub fn generate(out: &mut [f32], species_variant: i32, rng: &mut JavaRandom) -> usize {
    let sp = Species::new(species_variant, rng);
    let (mut x, mut y) = (0.0001_f32, 0.0_f32);
    let dt = 1.0_f32;
    let delay_len = sp.delay;
    let mut delay = vec![0.0_f32; delay_len];
    let mut di = 0;
    let delay2_len = (delay_len / 3 + rng.next_int((delay_len / 4).max(1) as i32) as usize).max(1);
    let mut delay2 = vec![0.0_f32; delay2_len];
    let mut di2 = 0;
    let reflection2 = sp.reflection * 0.4 + rng.next_float() * 0.2;
    let mut sample_idx = 0;
    for syl in 0..sp.count {
        if sample_idx >= out.len() {
            break;
        }
        let syllable_samples =
            ((sp.duration * SAMPLE_RATE as f32) as usize).min(out.len() - sample_idx);
        for i in 0..syllable_samples {
            let t = i as f32 / syllable_samples as f32;
            let p = sp.pressure(t, syl);
            let k = sp.kappa(t, syl);
            let omega = sp.omega(t, syl);
            let k1x = y;
            let k1y = sp.rhs(x, y, omega, p, k);
            let x2 = x + 0.5 * dt * k1x;
            let y2 = y + 0.5 * dt * k1y;
            let k2x = y2;
            let k2y = sp.rhs(x2, y2, omega, p, k);
            let x3 = x + 0.5 * dt * k2x;
            let y3 = y + 0.5 * dt * k2y;
            let k3x = y3;
            let k3y = sp.rhs(x3, y3, omega, p, k);
            let x4 = x + dt * k3x;
            let y4 = y + dt * k3y;
            let k4x = y4;
            let k4y = sp.rhs(x4, y4, omega, p, k);
            x += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
            y += dt / 6.0 * (k1y + 2.0 * k2y + 2.0 * k3y + k4y);
            x = x.clamp(-STATE_CLAMP, STATE_CLAMP);
            y = y.clamp(-STATE_CLAMP, STATE_CLAMP);
            let trach1 = sp.reflection * delay[di];
            delay[di] = x;
            di = (di + 1) % delay_len;
            let trach2 = reflection2 * delay2[di2];
            delay2[di2] = x;
            di2 = (di2 + 1) % delay2_len;
            out[sample_idx] = (x + trach1 + trach2) * envelope(t);
            sample_idx += 1;
        }
        let jitter = 0.6 + rng.next_float() * 0.8;
        let gap = ((sp.gap * SAMPLE_RATE as f32 * jitter) as usize).min(out.len() - sample_idx);
        for _ in 0..gap {
            x *= 0.999;
            y *= 0.999;
            out[sample_idx] = 0.0;
            sample_idx += 1;
        }
    }
    sample_idx
}
