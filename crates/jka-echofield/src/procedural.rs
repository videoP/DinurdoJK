//! ProceduralBufferFactory.java (4a595e7): PCM generation only. Source pool,
//! sample selection and OpenAL buffer lifetimes belong to the audio host.
use crate::{noise::PerlinNoise, random::JavaRandom, terrain_audio::crossfade};
use std::f64::consts::PI;
pub const RATE: usize = 44100;
pub const LOOP_SAMPLES: usize = RATE * 6;
const SR: f32 = 44100.;
fn white(r: &mut JavaRandom) -> f32 {
    r.next_float() * 2. - 1.
}
fn alpha(f: f32) -> f32 {
    f / (f + SR / (2. * std::f32::consts::PI))
}
fn t(i: usize) -> f32 {
    i as f32 / SR
}
fn sin_cycle(p: f32) -> f32 {
    (2. * PI * f64::from(p)).sin() as f32
}
fn dc_block(out: &mut [f32]) {
    let r = 1. - 2. * std::f32::consts::PI * 10. / SR;
    let (mut prev_in, mut prev_out) = (0., 0.);
    for s in out {
        let x = *s;
        prev_out = x - prev_in + r * prev_out;
        prev_in = x;
        *s = prev_out;
    }
}
#[derive(Default)]
struct TwoPole {
    a: f32,
    b: f32,
}
impl TwoPole {
    fn lp(&mut self, x: f32, k: f32) -> f32 {
        self.a += (x - self.a) * k;
        self.b += (self.a - self.b) * k;
        self.b
    }
    fn hp(&mut self, x: f32, k: f32) -> f32 {
        self.a += (x - self.a) * k;
        let x = x - self.a;
        self.b += (x - self.b) * k;
        x - self.b
    }
}
#[derive(Default)]
struct Resonator {
    z1: f32,
    z2: f32,
}
impl Resonator {
    fn coefficients(freq: f32, q: f32) -> [f32; 3] {
        let w = (2. * PI * f64::from(freq) / f64::from(SR)) as f32;
        let a = (f64::from(w).sin() / (2. * f64::from(q))) as f32;
        [a / (1. + a), -2. * w.cos() / (1. + a), (1. - a) / (1. + a)]
    }
    fn step(&mut self, x: f32, [b, a1, a2]: [f32; 3], cap: f32) -> f32 {
        let y = b * x - a1 * self.z1 - a2 * self.z2;
        self.z2 = self.z1.clamp(-cap, cap);
        self.z1 = y.clamp(-cap, cap);
        y
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Category {
    Bird,
    Insect,
    Frog,
    Bat,
    WindLeaf,
    WindGrass,
    WindWhistle,
    WaterFlow,
    WaterDrip,
    WaterRain,
    WaterStill,
    Lava,
    CaveAmbient,
    CaveDrone,
    Mechanical,
}
impl Category {
    pub fn info(self) -> (f32, u8, bool) {
        use Category::*;
        match self {
            Bird => (32., 4, true),
            Insect => (28., 3, true),
            Frog => (24., 4, true),
            Bat => (20., 2, true),
            WindLeaf => (24., 1, false),
            WindGrass => (20., 1, false),
            WindWhistle => (16., 2, false),
            WaterFlow => (32., 4, false),
            WaterDrip => (24., 3, true),
            WaterRain => (28., 3, true),
            WaterStill => (24., 5, true),
            Lava => (40., 5, false),
            CaveAmbient => (48., 3, true),
            CaveDrone => (32., 3, false),
            Mechanical => (16., 3, true),
        }
    }
}
pub fn generate_loop(category: Category, variant: u32, nonce: i64) -> Option<Vec<f32>> {
    if category.info().2 || variant >= 4 {
        return None;
    }
    let mut out = vec![0.; LOOP_SAMPLES];
    let mut r = JavaRandom::new(nonce ^ ((category as i64) << 32) ^ ((variant as i64) << 48));
    match category {
        Category::WindLeaf => generate_rustle(&mut out, &mut r, false),
        Category::WindGrass => generate_rustle(&mut out, &mut r, true),
        Category::WindWhistle => generate_whistle(&mut out, &mut r),
        Category::WaterFlow => generate_water_flow(&mut out, &mut r),
        Category::Lava => generate_lava(&mut out, &mut r),
        Category::CaveDrone => generate_cave_drone(&mut out, &mut r),
        _ => return None,
    };
    crossfade(&mut out, RATE / 5);
    Some(out)
}
pub fn normalized_pcm16(out: &[f32]) -> Vec<i16> {
    let peak = out.iter().map(|s| s.abs()).fold(0.001, f32::max);
    let norm = 0.75 / peak;
    out.iter().map(|x| (x * norm * 32767.) as i16).collect()
}
fn wind_envelope(r: &mut JavaRandom) -> Vec<f32> {
    let noise = PerlinNoise::new(r.next_long());
    (0..LOOP_SAMPLES)
        .map(|i| {
            let mut value = 0.;
            for (f, w) in [0.08, 0.25, 0.6, 1.5].into_iter().zip([0.5, 1., 0.6, 0.12]) {
                value += noise.sample(t(i) * f) * w;
            }
            let raw = (value * 0.40 + 0.35).clamp(0., 1.);
            raw * raw * raw
        })
        .collect()
}
fn generate_rustle(out: &mut [f32], r: &mut JavaRandom, grass: bool) {
    let wind = wind_envelope(r);
    let mut hp = TwoPole::default();
    let mut lp = TwoPole::default();
    let mut smooth = 0.;
    let k = 1. / (1. + SR * if grass { 0.025 } else { 0.020 });
    for (i, s) in out.iter_mut().enumerate() {
        smooth += (wind[i] - smooth) * k;
        let (hf, lf, gain) = if grass {
            (80. - smooth * 40., 600. + smooth * 800., 0.45)
        } else {
            (150. - smooth * 80., 800. + smooth * 1400., 0.55)
        };
        *s = lp.lp(hp.hp(white(r), alpha(hf)), alpha(lf)) * smooth * gain;
    }
    let (window, base, extra) = if grass {
        (RATE / 3, 60, 250.)
    } else {
        (RATE / 4, 120, 500.)
    };
    for w in 0..LOOP_SAMPLES / window {
        let ws = w * window;
        let local = wind[ws..ws + window].iter().sum::<f32>() / window as f32;
        let num = base + (extra * local) as usize;
        for _ in 0..num {
            let start = ws + r.next_int(window as i32) as usize;
            if start >= out.len() {
                continue;
            }
            let lw = wind[start];
            let (len, amp, cutoff) = if grass {
                (
                    (660. * (1. - lw * 0.3)) as usize + 220,
                    (0.004 + r.next_float() * 0.008) * (0.2 + lw * 0.8),
                    400. + lw * 600.,
                )
            } else {
                (
                    (440. * (1. - lw * 0.5)) as usize + 50,
                    (0.006 + r.next_float() * 0.010) * (0.3 + lw * 0.7),
                    600. + lw * 1200.,
                )
            };
            let mut filter = TwoPole::default();
            let k = alpha(cutoff);
            for (i, s) in out[start..].iter_mut().take(len).enumerate() {
                let phase = i as f32 / len as f32;
                let win = if grass {
                    if phase < 0.4 {
                        0.5 * (1. - (PI * f64::from(phase) / f64::from(0.4_f32)).cos() as f32)
                    } else {
                        0.5 * (1. + (PI * f64::from(phase - 0.4) / f64::from(0.6_f32)).cos() as f32)
                    }
                } else {
                    0.5 * (1. - (2. * PI * i as f64 / len as f64).cos() as f32)
                };
                *s += amp * win * filter.lp(white(r), k);
            }
        }
    }
}
fn generate_whistle(out: &mut [f32], r: &mut JavaRandom) {
    let wind = wind_envelope(r);
    let diameter = 0.005 + r.next_float() * 0.015;
    let turbulence = PerlinNoise::new(r.next_long());
    let mut res = Resonator::default();
    for (i, s) in out.iter_mut().enumerate() {
        let wind = wind[i];
        if wind <= 0.6 {
            *s = 0.;
            continue;
        }
        let effective = (wind - 0.6) / (1. - 0.6);
        let base = 0.2 * (wind * 20.) / diameter;
        let freq = (base * (1. + turbulence.sample(t(i) * 3.5) * 0.05)).clamp(100., 4000.);
        let co = Resonator::coefficients(freq, 15. + effective * 10.);
        *s = res.step(white(r) * effective, co, 2.) * effective * 0.7;
    }
}
pub fn minnaert(
    out: &mut [f32],
    start: usize,
    radius_mm: f32,
    decay: f32,
    amp: f32,
    chirp: f32,
    rho: f32,
) {
    let a = radius_mm / 1000.;
    let freq = ((3_f64 * 1.4 * 101325. / f64::from(rho)).sqrt() / (2. * PI * f64::from(a))) as f32;
    let dur = ((5. / decay * SR) as usize).min(RATE);
    if let Some(tail) = out.get_mut(start..) {
        for (i, s) in tail.iter_mut().take(dur).enumerate() {
            let time = t(i);
            let env = amp * (-decay * time).exp();
            let f = freq * (1. - chirp * time * decay / 30.);
            *s += env * (2. * PI * f64::from(f) * f64::from(time)).sin() as f32;
        }
    }
}
fn generate_water_flow(out: &mut [f32], r: &mut JavaRandom) {
    let noise = PerlinNoise::new(r.next_long());
    for (pass, (hi, lo, gain)) in [(30., 250., 0.45), (200., 800., 0.20)]
        .into_iter()
        .enumerate()
    {
        let mut hp = TwoPole::default();
        let mut lp = TwoPole::default();
        let (ha, la) = (alpha(hi), alpha(lo));
        for (i, s) in out.iter_mut().enumerate() {
            let m = if pass == 0 {
                0.6 + 0.4 * noise.sample(t(i) * 1.5)
            } else {
                0.5 + 0.5 * noise.sample(t(i) * 2.5)
            };
            let v = lp.lp(hp.hp(white(r), ha), la) * gain * m;
            if pass == 0 {
                *s = v
            } else {
                *s += v
            }
        }
    }
    for _ in 0..180 {
        let start = r.next_int(out.len() as i32) as usize;
        let radius = (2_f64.ln() + 0.6 * r.next_gaussian()).exp() as f32;
        let decay = 40. + r.next_float() * 100.;
        let amp = 0.2 + r.next_float() * 0.3;
        minnaert(out, start, radius.clamp(0.3, 8.), decay, amp, 0.05, 998.);
    }
    for _ in 0..18 {
        let start = r.next_int(out.len() as i32) as usize;
        let dur = (SR * (0.005 + r.next_float() * 0.015)) as usize;
        let amp = 0.05 + r.next_float() * 0.1;
        for (i, s) in out[start..].iter_mut().take(dur).enumerate() {
            *s += white(r) * amp * (1. - i as f32 / dur as f32);
        }
    }
}
pub fn generate_water_drip(out: &mut [f32], r: &mut JavaRandom) {
    let radius = 1. + r.next_float() * 3.;
    let decay = 80. + r.next_float() * 120.;
    let amp = 0.6 + r.next_float() * 0.3;
    minnaert(out, 0, radius, decay, amp, 0.05, 998.);
}
pub fn generate_rain(out: &mut [f32], r: &mut JavaRandom, count: usize) {
    assert!(count <= out.len());
    if count == 0 {
        return;
    }
    for _ in 0..(200. * t(count)) as usize {
        let start = r.next_int(count as i32) as usize;
        let diameter = ((-(1. - r.next_double()).ln() / 4.1) as f32).clamp(0.5, 5.);
        let radius = 0.5 + r.next_float() * 1.5;
        let decay = 100. + r.next_float() * 150.;
        minnaert(
            out,
            start,
            radius,
            decay,
            0.01 + diameter * 0.008,
            0.03,
            998.,
        );
        let len = (SR * (0.001 + diameter * 0.0005)) as usize;
        let amp = 0.005 + diameter * 0.004;
        for (i, s) in out[start..].iter_mut().take(len).enumerate() {
            *s += amp * (1. - i as f32 / len as f32) * white(r);
        }
    }
}
fn generate_lava(out: &mut [f32], r: &mut JavaRandom) {
    let mut state = 0_f32;
    for (i, s) in out.iter_mut().enumerate() {
        state += white(r) * 0.01;
        state = (state * 0.999).clamp(-1., 1.);
        let time = t(i);
        *s = state * 0.3;
        for (f, a) in [(25., 0.15), (38., 0.10), (55., 0.06)] {
            *s += a * (2. * PI * f * f64::from(time)).sin() as f32;
        }
    }
    for _ in 0..1800 {
        let start = r.next_int(out.len() as i32) as usize;
        let radius = ((15_f64.ln() + 0.6 * r.next_gaussian()).exp() as f32).clamp(8., 25.);
        let decay = 5. + r.next_float() * 15.;
        let amp = 0.03 + r.next_float() * 0.06;
        minnaert(out, start, radius, decay, amp, 0.03, 2500.);
    }
}
pub fn generate_bird(out: &mut [f32], r: &mut JavaRandom, species: i32) -> usize {
    let n = crate::syrinx::generate(out, species, r);
    out[n..].fill(0.);
    n
}
pub fn generate_cricket(out: &mut [f32], r: &mut JavaRandom, temp: f32, count: usize) {
    assert!(count <= out.len());
    let carrier = 2000. + r.next_float() * 3000.;
    let pulses = 3 + r.next_int(6);
    let c = temp * 30. - 5.;
    let f = c * 9. / 5. + 32.;
    let chirps = (f - 40.).max(0.);
    if temp < 0.25 {
        out[..count].fill(0.);
        return;
    }
    let interval = if chirps > 0.5 { 60. / chirps } else { 999. };
    let period = 1. / (10. + c * 0.28).clamp(5., 25.);
    let duration = period * 0.6;
    let ramp = (0.002 * SR) as i32;
    let (mut phase, mut inc) = (0., carrier / SR);
    let mut next = r.next_float() * 0.5;
    let mut amp = 0.7 + r.next_float() * 0.3;
    inc *= 1. + (r.next_float() - 0.5) * 0.04;
    let drift = PerlinNoise::new(r.next_long());
    for (i, s) in out[..count].iter_mut().enumerate() {
        let time = t(i);
        let drift_amp = 0.3 + 0.7 * (0.5 + 0.5 * drift.sample(time * 0.2));
        let elapsed = time - next;
        let mut sample = 0.;
        if elapsed >= 0. {
            if elapsed < pulses as f32 * period {
                let p = elapsed % period;
                if p < duration {
                    let index = (p * SR) as i32;
                    let end = (duration * SR) as i32 - index;
                    let mut env = 1.;
                    if index < ramp {
                        env = index as f32 / ramp as f32;
                    }
                    if end < ramp {
                        env = env.min(end as f32 / ramp as f32);
                    }
                    sample = sin_cycle(phase) * env * amp;
                }
            } else {
                next += interval + (r.next_float() - 0.5) * interval * 0.15;
                amp = 0.4 + r.next_float() * 0.6;
            }
        }
        phase += inc;
        if phase > 1. {
            phase -= 1.;
        }
        *s = sample * drift_amp * 0.75;
    }
}
pub fn generate_cicada(out: &mut [f32], r: &mut JavaRandom, temp: f32, count: usize) {
    assert!(count <= out.len());
    if temp < 0.4 {
        out[..count].fill(0.);
        return;
    }
    let volume = if temp < 0.6 {
        (temp - 0.4) / 0.2 * 0.1
    } else {
        0.1 + (temp.min(1.5) - 0.6) / 0.9 * 0.9
    };
    let rate = 100. + r.next_float() * 100.;
    let freq = 4000. + r.next_float() * 6000.;
    let trem_rate = 8. + r.next_float() * 4.;
    let depth = 0.4 + r.next_float() * 0.3;
    let co = Resonator::coefficients(freq, 8. + r.next_float() * 7.);
    let mut res = Resonator::default();
    let (mut phase, mut inc) = (0., rate / SR);
    let mut trem = r.next_float();
    let drift = PerlinNoise::new(r.next_long());
    for (i, s) in out[..count].iter_mut().enumerate() {
        phase += inc;
        let mut excitation = 0.;
        if phase >= 1. {
            phase -= 1.;
            excitation = 0.8 + r.next_float() * 0.4;
        }
        excitation += white(r) * 0.02;
        let filtered = res.step(excitation, co, 4.);
        trem += trem_rate / SR;
        if trem > 1. {
            trem -= 1.;
        }
        let mod_amp = 1. - depth * (0.5 + 0.5 * sin_cycle(trem));
        inc = rate * (1. + drift.sample(t(i) * 0.5) * 0.03) / SR;
        *s = filtered * mod_amp * volume * 0.5;
    }
    let fade = ((0.1 * SR) as usize).min(count / 4);
    for i in 0..fade {
        let f = i as f32 / fade as f32;
        out[i] *= f;
        let end = count - fade + i;
        out[end] = out[end] * (1. - f) + out[i] * f;
    }
}
pub fn generate_frog(out: &mut [f32], r: &mut JavaRandom, count: usize) {
    assert!(count <= out.len());
    let fundamental = 500. + r.next_float() * 500.;
    let whine = ((0.3 + r.next_float() * 0.2) * SR) as usize;
    let chuck = r.next_float() < 0.3;
    let _unused_q = 5. + r.next_float() * 5.;
    let mut phase = [0.; 4];
    for (i, s) in out[..count].iter_mut().take(whine).enumerate() {
        let p = i as f32 / whine as f32;
        let f = fundamental * (1. - 0.2 * p * p);
        let mut v = 0.;
        for (j, (phase, amp)) in phase.iter_mut().zip([1., 0.6, 0.3, 0.15]).enumerate() {
            *phase += f * (j + 1) as f32 / SR;
            v += sin_cycle(*phase) * amp;
        }
        let env = if p < 0.03 {
            p / 0.03
        } else if p > 0.8 {
            (1. - p) / 0.2
        } else {
            1.
        };
        *s = v * env * 0.3;
    }
    let dur = (0.05 * SR) as usize;
    if chuck {
        let freq = 200. + r.next_float() * 50.;
        let mut phase = [0.; 3];
        if let Some(tail) = out[..count].get_mut(whine..) {
            for (i, s) in tail.iter_mut().take(dur).enumerate() {
                let mut v = 0.;
                for (j, (p, a)) in phase.iter_mut().zip([1., 0.7, 0.4]).enumerate() {
                    *p += freq * (j + 1) as f32 / SR;
                    v += sin_cycle(*p) * a;
                }
                *s = v * (-40. * t(i)).exp() * 0.4;
            }
        }
    }
    out[(whine + if chuck { dur } else { 0 }).min(count)..count].fill(0.);
}
pub fn generate_cave_ambient(out: &mut [f32], r: &mut JavaRandom, room: f32, count: usize) {
    assert!(count <= out.len());
    if count == 0 {
        return;
    }
    let fundamental = 343. / (2. * room.clamp(3., 64.));
    let mut brown = 0_f32;
    for s in &mut out[..count] {
        brown += white(r) * 0.012;
        brown = (brown * 0.9997).clamp(-1., 1.);
        *s = brown * 0.015;
    }
    let n = if fundamental < 20. { 4 } else { 3 };
    for h in 1..=n {
        let f = fundamental * h as f32;
        if f > SR / 2. {
            break;
        }
        let q = 12. / h as f32 + 2.;
        let amp = 0.08 / h as f32;
        let co = Resonator::coefficients(f, q);
        let mut res = Resonator::default();
        for s in &mut out[..count] {
            *s += res.step(*s, co, 4.) * amp;
        }
    }
    // Upstream adds Minnaert drips here. They are omitted in DinurdoJK: cave
    // ambience also plays in dry enclosed rooms, where drips are wrong. The
    // RNG draws are kept so the remaining layers match upstream.
    let drip_rate = 0.5 + r.next_float() * 1.5;
    for _ in 0..((drip_rate * t(count)) as usize).max(1) {
        r.next_int(count as i32);
        r.next_float();
        r.next_float();
        r.next_float();
    }
    let noise = PerlinNoise::new(r.next_long());
    let lfo = 1. / (2. + r.next_float() * 2.);
    let freq = 80. + r.next_float() * 120.;
    let q = 3. + r.next_float() * 2.;
    let co = Resonator::coefficients(freq, q);
    let mut res = Resonator::default();
    for (i, s) in out[..count].iter_mut().enumerate() {
        let filtered = res.step(white(r), co, 4.);
        *s += filtered * (0.5 + 0.5 * noise.sample(t(i) * lfo)) * 0.10;
    }
    let noise = PerlinNoise::new(r.next_long());
    let freq = 1200. + r.next_float() * 800.;
    let q = 2. + r.next_float() * 2.;
    let co = Resonator::coefficients(freq, q);
    let mut res = Resonator::default();
    for (i, s) in out[..count].iter_mut().enumerate() {
        let filtered = res.step(white(r), co, 4.);
        let lfo = 0.4 + 0.6 * noise.sample(t(i) * (0.3 + r.next_float() * 0.5));
        *s += filtered * lfo * 0.08;
    }
    dc_block(&mut out[..count]);
    out[count..].fill(0.);
}
fn generate_cave_drone(out: &mut [f32], r: &mut JavaRandom) {
    let noise = PerlinNoise::new(r.next_long());
    let mut states: [f32; 8] = std::array::from_fn(|_| white(r));
    let a = alpha(3000. + r.next_float() * 1500.);
    let mut lp = [0.; 4];
    for (i, s) in out.iter_mut().enumerate() {
        let mut sum = 0.;
        for (o, v) in states.iter_mut().enumerate() {
            if i % (1 << o) == 0 {
                *v = white(r);
            }
            sum += *v;
        }
        let mut v = sum / 8.;
        for x in &mut lp {
            *x += (v - *x) * a;
            v = *x;
        }
        *s = v * (0.5 + 0.5 * noise.sample(t(i) * 0.15)) * 0.10;
    }
    dc_block(out);
}
pub fn generate_fire(out: &mut [f32], r: &mut JavaRandom, count: usize) {
    assert!(count <= out.len());
    if count == 0 {
        return;
    }
    let rate = 5. + r.next_float() * 10.;
    for _ in 0..((rate * t(count)) as usize).max(1) {
        let start = r.next_int(count as i32) as usize;
        let len = ((0.005 + r.next_float() * 0.025) * SR) as usize;
        let freq = 100. + r.next_float() * 900.;
        let amp = 0.15 + r.next_float() * 0.25;
        let q = 3. + r.next_float() * 4.;
        let co = Resonator::coefficients(freq, q);
        let mut res = Resonator::default();
        for (i, s) in out[start..count].iter_mut().take(len).enumerate() {
            let window = 0.5 * (1. - (2. * PI * f64::from(i as f32 / len as f32)).cos() as f32);
            *s += res.step(white(r), co, 2.) * window * amp;
        }
    }
    let a = alpha(3000.);
    let mut state = 0.;
    for s in &mut out[..count] {
        let x = white(r);
        state += (x - state) * a;
        *s += (x - state) * 0.04;
    }
    let noise = PerlinNoise::new(r.next_long());
    let rate = 1. + r.next_float() * 2.;
    for (i, s) in out[..count].iter_mut().enumerate() {
        *s *= 0.55 + 0.45 * noise.sample(t(i) * rate);
    }
    out[count..].fill(0.);
}
