//! Renderer-free lightning, ported from the web game's `sky/weather/lightning.ts`: a seeded Poisson scheduler, the
//! return-stroke flicker, the scene flash and the thunder cue. The GPU side (`shaders/bolt.wgsl`) draws what this
//! decides; the channel's shape is [`crate::bolt`].
//!
//! Intensity convention: the channel's radiant intensity, scaled so a point `r` metres from the strike's `position`
//! receives an irradiance of `intensity × (1000 / r)²` in the sea's units (the sun above the air gives about 6). A
//! return stroke peaks at 40.

use glam::Vec3;

/// Cloud-to-ground strikes draw a bolt to the sea; intra-cloud flashes only light the clouds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrikeKind {
    Ground,
    Cloud,
}

/// One return stroke, or one pulse of an intra-cloud flash. Times in seconds from the first stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub time: f64,
    pub peak: f64,
    pub decay: f64,
    pub glow: f64,
    pub glow_decay: f64,
}

pub const STRIKE_RANGE: [f64; 2] = [2000.0, 25000.0];
pub const CLOSE_SHARE: f64 = 0.07;
pub const CLOSE_RANGE: [f64; 2] = [700.0, 2000.0];
pub const GROUND_SHARE: f64 = 0.5;
pub const FLASH_PEAK: f64 = 1.5;
pub const FLASH_RANGE: f64 = 2200.0;
pub const SOUND_SPEED: f64 = 343.0;
pub const THUNDER_NEAR: f64 = 1500.0;
pub const THUNDER_AUDIBLE: [f64; 2] = [14000.0, 22000.0];
const DARK: f64 = 1e-3;

pub fn stroke_intensity(kind: StrikeKind) -> f64 {
    match kind {
        StrikeKind::Ground => 40.0,
        StrikeKind::Cloud => 20.0,
    }
}

fn flash_share(kind: StrikeKind) -> f64 {
    match kind {
        StrikeKind::Ground => 1.0,
        StrikeKind::Cloud => 0.6,
    }
}

/// A small seeded generator (mulberry32): the same seed gives the same storm.
#[derive(Clone, Debug)]
pub struct Random {
    state: u32,
}

impl Random {
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    pub fn draw(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

/// The flicker of one flash (`planStrokes`).
pub fn plan_strokes(kind: StrikeKind, random: &mut Random) -> Vec<Stroke> {
    let mut strokes = Vec::new();
    match kind {
        StrikeKind::Ground => {
            let count = 2 + (random.draw() * 3.0).floor() as usize;
            let mut time = 0.0;
            for i in 0..count {
                if i > 0 {
                    time += 0.035 + random.draw() * 0.085;
                }
                let last = i == count - 1;
                let peak = if i > 0 {
                    0.45 + random.draw() * 0.4
                } else {
                    1.0
                };
                let decay = 0.012 + random.draw() * 0.01;
                let glow = if last {
                    0.2 + random.draw() * 0.12
                } else {
                    0.05 + random.draw() * 0.1
                };
                let glow_decay = if last {
                    0.05 + random.draw() * 0.04
                } else {
                    0.03 + random.draw() * 0.03
                };
                strokes.push(Stroke {
                    time,
                    peak,
                    decay,
                    glow,
                    glow_decay,
                });
            }
        }
        StrikeKind::Cloud => {
            let count = 3 + (random.draw() * 5.0).floor() as usize;
            let mut time = 0.0;
            for i in 0..count {
                if i > 0 {
                    time += 0.03 + random.draw() * 0.13;
                }
                let peak = 0.35 + random.draw() * 0.65;
                let decay = 0.03 + random.draw() * 0.05;
                let glow = 0.15 + random.draw() * 0.15;
                let glow_decay = 0.08 + random.draw() * 0.08;
                strokes.push(Stroke {
                    time,
                    peak,
                    decay,
                    glow,
                    glow_decay,
                });
            }
        }
    }
    strokes
}

/// Brightness at `age` (s), relative to a stroke's peak.
pub fn stroke_brightness(strokes: &[Stroke], age: f64) -> f64 {
    let mut sum = 0.0;
    for s in strokes {
        if age < s.time {
            break;
        }
        let since = age - s.time;
        sum += s.peak * ((-since / s.decay).exp() + s.glow * (-since / s.glow_decay).exp());
    }
    sum
}

/// Mean brightness over the exposure `from`–`to` (s); a paused frame shows the instant.
pub fn stroke_exposure(strokes: &[Stroke], from: f64, to: f64) -> f64 {
    if to - from < 1e-6 {
        return stroke_brightness(strokes, to);
    }
    let exposed = |peak: f64, decay: f64, since: f64, until: f64| {
        peak * decay * ((-since / decay).exp() - (-until / decay).exp())
    };
    let mut sum = 0.0;
    for s in strokes {
        if to <= s.time {
            break;
        }
        let since = (from - s.time).max(0.0);
        let until = to - s.time;
        sum += exposed(s.peak, s.decay, since, until)
            + exposed(s.peak * s.glow, s.glow_decay, since, until);
    }
    sum / (to - from)
}

/// Seconds until a flash has gone dark.
pub fn stroke_duration(strokes: &[Stroke]) -> f64 {
    let n = strokes.len() as f64;
    let mut end: f64 = 0.0;
    for s in strokes {
        end = end
            .max(s.time + s.decay * (s.peak * n / DARK).max(1.0).ln())
            .max(s.time + s.glow_decay * (s.peak * s.glow * n / DARK).max(1.0).ln());
    }
    end
}

/// Thunder heard for a strike whose nearest channel lies `distance` metres away (`thunderFor`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thunder {
    pub distance: f64,
    pub delay: f64,
    pub loudness: f64,
    /// Absolute multiplayer serverTime of the strike. None on the original local clock.
    pub strike_time_ms: Option<i64>,
    /// A stable bundled sample index shared by clients for this strike.
    pub sample: Option<u8>,
}

pub fn thunder_for(kind: StrikeKind, distance: f64) -> Thunder {
    let [fade, silent] = THUNDER_AUDIBLE;
    let t = ((distance - fade) / (silent - fade)).clamp(0.0, 1.0);
    let audible = 1.0 - t * t * (3.0 - 2.0 * t);
    let loudness = (THUNDER_NEAR / distance.max(1.0)).min(1.0)
        * audible
        * if kind == StrikeKind::Cloud { 0.6 } else { 1.0 };
    Thunder {
        distance,
        delay: distance / SOUND_SPEED,
        loudness,
        strike_time_ms: None,
        sample: None,
    }
}

/// The scene flash (a multiple of the ambient fill) for a flash of brightness `brightness` `distance` metres away.
pub fn flash_at(kind: StrikeKind, brightness: f64, distance: f64) -> f64 {
    FLASH_PEAK * flash_share(kind) * brightness / (1.0 + (distance / FLASH_RANGE).powi(2))
}

/// One flash: where it is, how it flickers, and its state this frame.
#[derive(Clone, Debug)]
pub struct Strike {
    pub kind: StrikeKind,
    pub strokes: Vec<Stroke>,
    pub seed: u32,
    /// The channel's brightest point, inside the cloud base: what lights the clouds.
    pub position: Vec3,
    /// Where a ground stroke meets the sea.
    pub ground: Vec3,
    /// Top of the visible channel, at the cloud base.
    pub top: Vec3,
    pub age: f64,
    pub distance: f64,
    pub intensity: f64,
    pub brightness: f64,
    pub first_brightness: f64,
    pub duration: f64,
}

impl Strike {
    pub fn new(kind: StrikeKind, strokes: Vec<Stroke>, seed: u32) -> Self {
        let duration = stroke_duration(&strokes);
        Self {
            kind,
            strokes,
            seed,
            position: Vec3::ZERO,
            ground: Vec3::ZERO,
            top: Vec3::ZERO,
            age: 0.0,
            distance: 0.0,
            intensity: 0.0,
            brightness: 0.0,
            first_brightness: 0.0,
            duration,
        }
    }

    pub fn done(&self) -> bool {
        self.age >= self.duration
    }

    /// Advance by `dt` and expose the frame: `dt` 0 holds the instant.
    pub fn advance(&mut self, dt: f64) {
        let from = self.age;
        self.age += dt.max(0.0);
        self.expose(from, self.age);
    }

    /// Brightness over the exposure `from`–`to`.
    pub fn expose(&mut self, from: f64, to: f64) {
        self.brightness = stroke_exposure(&self.strokes, from, to);
        self.first_brightness = if self.strokes.is_empty() {
            0.0
        } else {
            stroke_exposure(&self.strokes[..1], from, to)
        };
        self.intensity = stroke_intensity(self.kind) * self.brightness;
    }
}

/// Where storms put their strikes: the cloud layer's base and depth (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrikeClouds {
    pub altitude: f64,
    pub thickness: f64,
}

/// Put a strike's channel under the cloud base (`placeChannel`).
pub fn place_channel(strike: &mut Strike, clouds: StrikeClouds, random: &mut Random) {
    let base = clouds.altitude.max(200.0);
    let depth = clouds.thickness.max(300.0);
    match strike.kind {
        StrikeKind::Ground => {
            let lean = random.draw() * 0.33 * base;
            let heading = random.draw() * 2.0 * std::f64::consts::PI;
            strike.top = Vec3::new(
                (f64::from(strike.ground.x) + heading.sin() * lean) as f32,
                base as f32,
                (f64::from(strike.ground.z) + heading.cos() * lean) as f32,
            );
            strike.position = Vec3::new(
                strike.top.x,
                (base + 250f64.min(0.1 * depth)) as f32,
                strike.top.z,
            );
        }
        StrikeKind::Cloud => {
            strike.top = Vec3::new(strike.ground.x, base as f32, strike.ground.z);
            strike.position = Vec3::new(
                strike.ground.x,
                (base + (0.25 + 0.35 * random.draw()) * depth) as f32,
                strike.ground.z,
            );
        }
    }
}

/// Distance from `from` to the nearest point of a strike's channel (`channelDistance`).
pub fn channel_distance(strike: &Strike, from: Vec3) -> f64 {
    if strike.kind == StrikeKind::Cloud {
        return f64::from(strike.position.distance(from));
    }
    let along = strike.top - strike.ground;
    let t = ((along.dot(from) - along.dot(strike.ground)) / along.length_squared().max(1e-6))
        .clamp(0.0, 1.0);
    f64::from((along * t + strike.ground).distance(from))
}

/// Strikes as a Poisson process at `rate` per minute from a seeded stream (`LightningScheduler`).
#[derive(Clone, Debug)]
pub struct LightningScheduler {
    pub rate: f64,
    random: Random,
    budget: f64,
}

impl LightningScheduler {
    pub fn new(seed: u32) -> Self {
        let mut random = Random::new(seed);
        let budget = -(1.0 - random.draw()).ln();
        Self {
            rate: 0.0,
            random,
            budget,
        }
    }

    /// Strikes that fell due during `dt` seconds.
    pub fn advance(&mut self, dt: f64) -> u32 {
        if dt <= 0.0 || self.rate <= 0.0 {
            return 0;
        }
        self.budget -= self.rate / 60.0 * dt;
        let mut due = 0;
        while self.budget <= 0.0 {
            due += 1;
            self.budget += -(1.0 - self.random.draw()).ln();
        }
        due
    }

    /// Deterministic, seekable multiplayer clock: one independent 25 ms slot.
    /// Each slot has an exponential/Poisson probability p = 1 - exp(-rate*dt).
    /// This has no frame-rate dependence, no accumulated RNG history, and is
    /// reproducible for late joins or demo seeks. At the 30/minute UI maximum,
    /// limiting a slot to one strike changes the expected rate by < 1%.
    pub fn at_server_slot(
        slot: i64,
        rate_per_minute: f64,
        origin: Vec3,
        clouds: StrikeClouds,
    ) -> Option<(i64, Strike)> {
        const SLOT_MS: i64 = 25;
        if rate_per_minute <= 0.0 { return None; }
        // SplitMix64 gives adjacent slots independent reproducible streams.
        let mut x = (slot as u64).wrapping_add(0x9e3779b97f4a7c15 ^ 1941);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        let seed = (x ^ (x >> 31)) as u32;
        let mut rng = Random::new(seed);
        let probability = -(-rate_per_minute.min(30.0) / 2400.0).exp_m1();
        if rng.draw() >= probability { return None; }
        let offset = (rng.draw() * SLOT_MS as f64) as i64;
        let mut scheduler = Self {
            rate: rate_per_minute,
            random: rng,
            budget: 0.0,
        };
        Some((slot * SLOT_MS + offset, scheduler.next(origin, clouds)))
    }

    /// The next strike around `camera`.
    pub fn next(&mut self, camera: Vec3, clouds: StrikeClouds) -> Strike {
        let random = &mut self.random;
        let kind = if random.draw() < GROUND_SHARE {
            StrikeKind::Ground
        } else {
            StrikeKind::Cloud
        };
        let distance = if random.draw() < CLOSE_SHARE {
            CLOSE_RANGE[0] + (CLOSE_RANGE[1] - CLOSE_RANGE[0]) * random.draw()
        } else {
            STRIKE_RANGE[0] * (STRIKE_RANGE[1] / STRIKE_RANGE[0]).powf(random.draw())
        };
        let bearing = random.draw() * 2.0 * std::f64::consts::PI;
        let strokes = plan_strokes(kind, random);
        let seed = (random.draw() * 4_294_967_296.0).floor() as u32;
        let mut strike = Strike::new(kind, strokes, seed);
        strike.ground = Vec3::new(
            (f64::from(camera.x) + bearing.sin() * distance) as f32,
            0.0,
            (f64::from(camera.z) + bearing.cos() * distance) as f32,
        );
        place_channel(&mut strike, clouds, random);
        strike.distance = channel_distance(&strike, camera);
        strike
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_matches_javascript() {
        // `seededRandom(1941)` in Bun: the first three draws.
        let mut random = Random::new(1941);
        for expected in [0.7352093060035259, 0.6761041986756027, 0.6805352955125272] {
            assert_eq!(random.draw(), expected);
        }
    }

    #[test]
    fn server_slots_repeat_independent_of_frame_rate_or_join_time() {
        let clouds = StrikeClouds { altitude: 650.0, thickness: 900.0 };
        let anchor = Vec3::new(120.0, 0.0, 330.0);
        let slots: Vec<_> = (0..2400).filter_map(|slot| {
            LightningScheduler::at_server_slot(slot, 30.0, anchor, clouds)
                .map(|(time, strike)| (time, strike.seed, strike.ground))
        }).collect();
        assert!(!slots.is_empty());
        let replay: Vec<_> = (0..2400).filter_map(|slot| {
            LightningScheduler::at_server_slot(slot, 30.0, anchor, clouds)
                .map(|(time, strike)| (time, strike.seed, strike.ground))
        }).collect();
        assert_eq!(slots, replay);
        assert!(slots.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    /// The review page's forced strike (`forceStrike` at (2600, 0, 380), 6 ms, ground, seed 7, a 650 m base), as Bun
    /// computes it and the game recorded it.
    #[test]
    fn the_forced_strike_matches_the_web() {
        let mut random = Random::new(7);
        let strokes = plan_strokes(StrikeKind::Ground, &mut random);
        assert_eq!(strokes.len(), 2);
        assert!((strokes[1].time - 0.07932284782524221).abs() < 1e-15);
        let seed = (random.draw() * 4_294_967_296.0).floor() as u32;
        assert_eq!(seed, 3_134_562_008);
        let mut strike = Strike::new(StrikeKind::Ground, strokes, seed);
        strike.ground = Vec3::new(2600.0, 0.0, 380.0);
        place_channel(
            &mut strike,
            StrikeClouds {
                altitude: 650.0,
                thickness: 4200.0,
            },
            &mut random,
        );
        strike.expose(0.006, 0.006);
        assert!((strike.top - Vec3::new(2645.923, 650.0, 410.8109)).length() < 1e-2);
        assert!((strike.position.y - 900.0).abs() < 1e-3);
        assert!((strike.intensity - 30.115760515219428).abs() < 1e-9);
    }

    #[test]
    fn a_flash_goes_dark() {
        let mut random = Random::new(7);
        let strokes = plan_strokes(StrikeKind::Ground, &mut random);
        let duration = stroke_duration(&strokes);
        assert!(stroke_brightness(&strokes, duration + 0.1) < 1e-2);
        assert!(stroke_brightness(&strokes, 0.0) >= 1.0);
    }
}
