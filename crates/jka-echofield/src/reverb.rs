//! EchoField 4a595e7 EapSystem.overrideReverbParams, ReverbParams,
//! PerSourceDRProcessor and EarlyReflectionProcessor calculations.
use crate::{acoustic::reverb_send_multiplier, geometry::Vec3, profile::*};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbSlot {
    pub density: f32,
    pub diffusion: f32,
    pub gain: f32,
    pub gain_hf: f32,
    pub gain_lf: f32,
    pub decay_time: f32,
    pub decay_hf_ratio: f32,
    pub decay_lf_ratio: f32,
    pub reflections_gain: f32,
    pub reflections_delay: f32,
    pub reflections_pan: [f32; 3],
    pub late_gain: f32,
    pub late_delay: f32,
    pub late_pan: [f32; 3],
    pub echo_time: f32,
    pub echo_depth: f32,
    pub modulation_time: f32,
    pub modulation_depth: f32,
    pub air_absorption_gain_hf: f32,
    pub hf_reference: f32,
    pub lf_reference: f32,
    pub room_rolloff: f32,
}
/// SPR base settings, including the settings not overridden by EAP.
pub fn base_slots(reverb_gain: f32, brightness: f32, attenuation: f32) -> [ReverbSlot; 4] {
    std::array::from_fn(|i| ReverbSlot {
        density: [0., 0., 0.1, 0.5][i],
        diffusion: 1.,
        gain: [0.2, 0.3, 0.5, 0.4][i] * (0.7 * reverb_gain) * 0.85,
        gain_hf: [0.99, 0.99, 0.99, 0.89][i],
        gain_lf: 1.,
        decay_time: [0.15, 0.55, 1.68, 4.142][i],
        decay_hf_ratio: [0.6, 0.7, 0.7, 0.7][i] * brightness,
        decay_lf_ratio: 1.,
        reflections_gain: [2.5, 0.2, 0., 0.][i],
        // SPR setReverbParams does not write REFLECTIONS_DELAY: retain EFX default.
        reflections_delay: 0.007,
        reflections_pan: [0.; 3],
        late_gain: 1.26,
        late_delay: [0.011, 0.011, 0.021, 0.021][i],
        late_pan: [0.; 3],
        echo_time: 0.25,
        echo_depth: 0.,
        modulation_time: 0.25,
        modulation_depth: 0.,
        air_absorption_gain_hf: 0.994,
        hf_reference: 5000.,
        lf_reference: 250.,
        room_rolloff: [0.16, 0.15, 0.13, 0.11][i] * attenuation,
    })
}
/// `yaw` is the upstream signed player yaw (degrees), not JKA yaw.
pub fn override_slots(
    p: &EnvironmentProfile,
    yaw: f32,
    mut slots: [ReverbSlot; 4],
) -> [ReverbSlot; 4] {
    let [low, mid, high] = p.spectral.map(|a| a.max(0.01));
    let hf_ratio = ((1. - mid).ln() / (1. - high).ln()).clamp(0.1, 1.);
    let lf_ratio = ((1. - mid).ln() / (1. - low).ln()).clamp(0.1, 2.);
    let volume = p.volume();
    let surface = 6. * volume.powf(2. / 3.);
    let eyring = 0.161 * volume / (-surface * (1. - p.absorption.clamp(0.01, 0.99)).ln());
    let rt = (0.3 + 2. * (1. + eyring).log10()).clamp(0.1, 4.);
    let mut pan = [0_f32; 3];
    let mut total_energy = 0_f32;
    let mut dist = 0_f32;
    let mut count = 0;
    for t in p.taps.iter().filter(|t| t.order == 1) {
        let e = t.energy as f32;
        for (component, d) in pan.iter_mut().zip(t.direction) {
            *component += d as f32 * e;
        }
        total_energy += e;
        dist += t.distance as f32;
        count += 1;
    }
    let mag = pan.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.001 {
        pan = pan.map(|x| x / mag * (total_energy * 2.).min(1.));
    }
    let (sin, cos) = (-yaw).to_radians().sin_cos();
    let [x, y, z] = pan;
    pan = [x * cos - z * sin, y, x * sin + z * cos];
    let avg = if count > 0 { dist / count as f32 } else { 0. };
    let base_delay = (avg / 343.).clamp(0., 0.3);
    let refl_gain = (total_energy * 2.5).clamp(0., 3.16 * p.enclosure);
    let room_echo = (2. * avg / 343.).clamp(0.04, 0.25);
    let mut echo_time = room_echo;
    let mut echo_depth = 0_f32;
    let hard = |t: &Tap| {
        t.order == 1
            && (1.0..=40.0).contains(&t.distance)
            && t.material.spectral == SpectralCategory::Hard
    };
    for (i, a) in p.taps.iter().enumerate().filter(|(_, t)| hard(t)) {
        for (j, b) in p.taps.iter().enumerate().filter(|(_, t)| hard(t)) {
            if i == j || dot(a.direction, b.direction) >= -0.8 {
                continue;
            }
            let candidate = ((a.distance + b.distance) as f32 / 343.).clamp(0.04, 0.25);
            let avg_abs = ((1. - a.energy) + (1. - b.energy)) as f32 / 2.;
            let depth = ((1. - avg_abs) * 0.9).clamp(0., 1.);
            if depth > echo_depth {
                echo_depth = depth;
                echo_time = room_echo.max(candidate);
            }
        }
    }
    let volume_factor = (1. - 0.15 * (1. + volume / 1000.).log10()).clamp(0.5, 1.);
    for (i, s) in slots.iter_mut().enumerate() {
        s.decay_time = (rt * [0.15, 0.35, 0.65, 1.][i]).clamp(0.1, 20.);
        s.decay_hf_ratio = hf_ratio;
        s.decay_lf_ratio = lf_ratio;
        s.gain_lf = (1. - low * 2.).clamp(0.5, 1.);
        s.density = ((p.scatter * 1.5 + 0.2) * volume_factor).clamp(0.2, 1.);
        s.diffusion = (p.scatter * 1.2 + 0.3).clamp(0., 1.);
        s.late_gain = (0.5 + p.enclosure * 1.5).clamp(0., 5.);
        s.gain_hf = (1. - high).clamp(0.1, 0.99);
        s.hf_reference = 2500.;
        s.lf_reference = 250.;
        s.air_absorption_gain_hf = (1. - p.average_return_distance * 0.003).clamp(0.892, 1.);
        s.reflections_delay = (base_delay * [1., 1.5, 2.5, 4.][i]).clamp(0., 0.3);
        s.reflections_gain = refl_gain;
        s.late_delay = (base_delay * [3., 4.5, 7.5, 12.][i]).clamp(0., 0.1);
        s.reflections_pan = if i <= 1 { pan } else { [0.; 3] };
        s.late_pan = [0.; 3];
        s.echo_time = echo_time;
        s.echo_depth = echo_depth;
        s.modulation_time = (0.25 + p.scatter * 1.5).clamp(0.04, 4.);
        s.modulation_depth = (p.scatter * 0.15).clamp(0., 1.);
    }
    slots
}
pub fn smooth_multiplier(current: f32, target: f32) -> f32 {
    let mut s = current + (target - current) * 0.5;
    if current > 0.001 {
        let ratio = s / current;
        if ratio > 1.413 {
            s = current * 1.413;
        } else if ratio < 1. / 1.413 {
            s = current / 1.413;
        }
    }
    s.clamp(0., 1.)
}
/// Upstream 64 source limit, 100 tick expiry, and per-source smoothing.
#[derive(Default)]
pub struct DrProcessor {
    states: std::collections::HashMap<u64, (f32, u64)>,
}
impl DrProcessor {
    pub fn update(&mut self, id: u64, distance: f32, critical: f32, tick: u64) -> Option<f32> {
        if self.states.len() >= 64 && !self.states.contains_key(&id) {
            return None;
        }
        let state = self.states.entry(id).or_insert((0.5, tick));
        state.0 = smooth_multiplier(state.0, reverb_send_multiplier(distance, critical));
        state.1 = tick;
        Some(state.0)
    }
    pub fn cleanup(&mut self, tick: u64) {
        self.states.retain(|_, v| tick.saturating_sub(v.1) <= 100);
    }
    pub fn clear(&mut self) {
        self.states.clear();
    }
}
pub fn reflection_filter(t: &Tap) -> [f32; 2] {
    let [l, m, h] = t.material.spectral.absorption();
    [
        (1. - (l + m + h) / 3.).clamp(0., 1.),
        if l >= 1. {
            0.
        } else {
            ((1. - h) / (1. - l)).clamp(0., 1.)
        },
    ]
}
pub fn lateral_fraction(pos: Vec3, listener: Vec3, forward: Vec3) -> f64 {
    let to = normalize(sub(pos, listener));
    let f = normalize([forward[0], 0., forward[2]]);
    if dot(f, f) < 0.001 {
        0.5
    } else {
        1. - (to[0] * f[0] + to[2] * f[2]).abs()
    }
}
/// Ordered candidates; actual 8-slot pool stealing/delayed playback is owned by the host.
pub fn reflection_candidates(p: &EnvironmentProfile, listener: Vec3, forward: Vec3) -> Vec<Tap> {
    let mut taps: Vec<_> = p
        .taps
        .iter()
        .copied()
        .filter(|t| (1.0..=50.0).contains(&(t.delay * 1000.)))
        .collect();
    taps.sort_by(|a, b| b.energy.total_cmp(&a.energy));
    let score = |t: &Tap| t.energy * 0.7 + lateral_fraction(t.position, listener, forward) * 0.3;
    taps.sort_by(|a, b| score(b).total_cmp(&score(a)));
    taps.into_iter().take_while(|t| t.energy >= 0.001).collect()
}
