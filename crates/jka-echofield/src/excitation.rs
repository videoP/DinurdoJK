//! Environmental excitation from EchoField's ExcitationSourceManager.
use crate::{
    geometry::Vec3,
    profile::{add, scale, EnvironmentProfile},
    random::JavaRandom,
    terrain_audio::{crossfade, normalize},
};
use std::f32::consts::TAU;

pub const SAMPLE_RATE: usize = 44_100;
const SAMPLES: usize = SAMPLE_RATE * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Wind,
    Foliage,
    Grass,
    Water,
    Lava,
}

impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Wind,
        Self::Foliage,
        Self::Grass,
        Self::Water,
        Self::Lava,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wind => "wind",
            Self::Foliage => "foliage",
            Self::Grass => "grass",
            Self::Water => "water",
            Self::Lava => "lava",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Excitation {
    pub position: Vec3,
    pub gain: f32,
    pub gain_hf: f32,
}

pub fn targets(
    profile: &EnvironmentProfile,
    listener: Vec3,
    foliage: Option<(Vec3, usize)>,
    grass: Option<(Vec3, usize)>,
    water: Option<(Vec3, usize)>,
    lava: Option<(Vec3, usize)>,
) -> [Excitation; 5] {
    let wind = profile.wind_exposure;
    let (gain, gain_hf) = if profile.enclosure > 0.7 && profile.scatter < 0.3 {
        (0.8, 0.3)
    } else if profile.enclosure > 0.4 {
        (0.7, 0.5)
    } else {
        (0.9, 0.8)
    };
    let source = |contact: Option<(Vec3, usize)>, cap: f32, gain: f32, gain_hf| {
        let (position, count) = contact.unwrap_or((listener, 0));
        Excitation {
            position,
            gain: (count as f32 / cap).min(1.0) * gain,
            gain_hf,
        }
    };
    [
        Excitation {
            position: add(listener, scale(profile.most_open_sky, 20.0)),
            gain: wind * gain,
            gain_hf,
        },
        source(foliage, 10.0, wind * 0.8, 0.7),
        source(grass, 10.0, wind * 0.7, 0.6),
        source(water, 5.0, 0.2, 0.5),
        source(lava, 5.0, 0.3, 0.3),
    ]
}

pub fn generate(kind: Kind) -> Vec<f32> {
    let mut rng = JavaRandom::new(0x4264_6934 ^ (kind as i64));
    let mut out = vec![0.0; SAMPLES];
    match kind {
        Kind::Wind => {
            // DinurdoJK fallback when stock wind audio is unavailable. Upstream
            // mixes fixed 41-167 Hz sines (a musical hum), and its 0.37/0.83 Hz
            // gusts jump at every 2 s loop seam. This is broadband noise with
            // gust rates that complete whole cycles in the buffer.
            let (mut brown, mut low) = (0.0, 0.0);
            for (index, sample) in out.iter_mut().enumerate() {
                let time = index as f32 / SAMPLE_RATE as f32;
                let noise = white(&mut rng);
                brown = ((brown + noise * 0.015) * 0.997).clamp(-1.0, 1.0);
                low += (noise - low) * 0.02;
                let gust = 0.6 + 0.25 * (TAU * 0.5 * time).sin() + 0.15 * (TAU * 1.5 * time).sin();
                *sample = (brown * 0.7 + low * 0.3) * gust;
            }
        }
        Kind::Foliage | Kind::Grass => {
            let (count, minimum, extra, level, variation, pitch, spread) = if kind == Kind::Foliage
            {
                (4000, 88, 176, 0.01, 0.025, 1500.0, 3500.0)
            } else {
                (2500, 132, 264, 0.005, 0.012, 500.0, 1500.0)
            };
            for _ in 0..count {
                let start = rng.next_int(SAMPLES as i32) as usize;
                let length = minimum + rng.next_int(extra) as usize;
                let gain = level + rng.next_float() * variation;
                let frequency = pitch + rng.next_float() * spread;
                for (index, sample) in out[start..].iter_mut().take(length).enumerate() {
                    let time = index as f32 / SAMPLE_RATE as f32;
                    let window = 0.5 * (1.0 - (TAU * index as f32 / length as f32).cos());
                    *sample += gain * window * white(&mut rng) * (TAU * frequency * time).sin();
                }
            }
        }
        Kind::Water | Kind::Lava => {
            if kind == Kind::Lava {
                let mut brown = 0.0;
                for (index, sample) in out.iter_mut().enumerate() {
                    let time = index as f32 / SAMPLE_RATE as f32;
                    brown = ((brown + white(&mut rng) * 0.01) * 0.999).clamp(-1.0, 1.0);
                    *sample = 0.3 * brown
                        + 0.15 * (TAU * 25.0 * time).sin()
                        + 0.10 * (TAU * 38.0 * time).sin();
                }
            }
            let count = if kind == Kind::Water { 3000 } else { 600 };
            for _ in 0..count {
                let start = rng.next_int(SAMPLES as i32) as usize;
                let (frequency, decay, level, max_length) = if kind == Kind::Water {
                    (
                        80.0 + rng.next_float() * rng.next_float() * 2900.0,
                        30.0 + rng.next_float() * 120.0,
                        0.008 + rng.next_float() * 0.02,
                        SAMPLE_RATE / 10,
                    )
                } else {
                    (
                        20.0 + rng.next_float() * 180.0,
                        8.0 + rng.next_float() * 30.0,
                        0.03 + rng.next_float() * 0.06,
                        SAMPLE_RATE / 2,
                    )
                };
                let length = ((5.0 / decay * SAMPLE_RATE as f32) as usize).min(max_length);
                for (index, sample) in out[start..].iter_mut().take(length).enumerate() {
                    let time = index as f32 / SAMPLE_RATE as f32;
                    let chirp = if kind == Kind::Water {
                        1.0 - 0.003 * time * decay
                    } else {
                        1.0
                    };
                    *sample +=
                        level * (-decay * time).exp() * (TAU * frequency * chirp * time).sin();
                }
            }
        }
    }
    crossfade(&mut out, SAMPLE_RATE / 20);
    normalize(&mut out, 0.8);
    out
}

fn white(rng: &mut JavaRandom) -> f32 {
    rng.next_float() * 2.0 - 1.0
}
