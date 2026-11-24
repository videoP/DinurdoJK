//! TerrainBufferFactory.java at 4a595e7. All 3 families x 4 variants,
//! independent of OpenAL ownership; exact upstream deterministic seed policy.
use crate::{noise::PerlinNoise, random::JavaRandom, terrain::Family};
pub const SAMPLE_RATE: usize = 44100;
pub const BUFFER_SAMPLES: usize = SAMPLE_RATE * 6;
pub fn normalize(samples: &mut [f32], ceiling: f32) {
    let max = samples.iter().map(|s| s.abs()).fold(0., f32::max);
    if max > 0. {
        let scale = ceiling / max;
        for s in samples {
            *s *= scale;
        }
    }
}
pub fn crossfade(samples: &mut [f32], count: usize) {
    assert!(count <= samples.len());
    let start = samples.len() - count;
    for i in 0..count {
        let t = i as f32 / count as f32;
        let a = f64::from(t) * std::f64::consts::PI * 0.5;
        samples[start + i] = samples[start + i] * (a.cos() as f32) + samples[i] * (a.sin() as f32);
    }
}
pub fn bandpass(samples: &mut [f32], frequency: f32, q: f32, rate: usize) {
    let w = (2. * std::f64::consts::PI * f64::from(frequency) / rate as f64) as f32;
    let a = w.sin() / (2. * q);
    let c = w.cos();
    let a0 = 1. + a;
    let (b0, b2, a1, a2) = (a / a0, -a / a0, -2. * c / a0, (1. - a) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0., 0., 0., 0.);
    for s in samples {
        let x = *s;
        let y = b0 * x + 0. * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = x;
        y2 = y1;
        y1 = y;
        *s = y;
    }
}
pub fn lowpass4(samples: &mut [f32], cutoff: f32, rate: usize) {
    let a = cutoff / (cutoff + rate as f32 / (2. * std::f32::consts::PI));
    let mut state = [0.; 4];
    for s in samples {
        let mut x = *s;
        for p in &mut state {
            *p += (x - *p) * a;
            x = *p;
        }
        *s = x;
    }
}
pub fn variant_for_position(x: i32, z: i32) -> usize {
    (x.wrapping_mul(374761393)
        .wrapping_add(z.wrapping_mul(668265263)) as u32
        >> 30) as usize
}
pub fn generate(family: Family, variant: usize) -> Vec<f32> {
    assert!(variant < 4);
    let family_id = match family {
        Family::Void => 0,
        Family::Surface => 1,
        Family::Ground => 2,
    };
    let seed = family_id * 100003 + variant as i64 * 7919;
    let mut rng = JavaRandom::new(seed);
    let mut samples: Vec<_> = (0..BUFFER_SAMPLES)
        .map(|_| rng.next_float() * 2. - 1.)
        .collect();
    let (offset, rate, db, ceiling) = match family {
        Family::Void => {
            bandpass(
                &mut samples,
                400.,
                if variant < 2 { 2. } else { 4. },
                SAMPLE_RATE,
            );
            (99991, 0.5, 3., 0.7)
        }
        Family::Surface => {
            // DinurdoJK omits upstream's constant -18 dB 110 Hz sine: under
            // terrain gain it reads as a steady hum.
            lowpass4(&mut samples, 1000., SAMPLE_RATE);
            (77773, 0.3, 2., 0.7)
        }
        Family::Ground => {
            bandpass(&mut samples, 280., 1.4, SAMPLE_RATE);
            (55547, 0.2, 1.5, 0.6)
        }
    };
    let noise = PerlinNoise::new(seed + offset);
    for (i, s) in samples.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let mod_db = noise.sample(t * rate) * db;
        *s *= 10_f64.powf(f64::from(mod_db) / 20.) as f32;
    }
    normalize(&mut samples, ceiling);
    crossfade(&mut samples, 4410);
    samples
}
pub fn pcm16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| (s * 32767.) as i16).collect()
}
