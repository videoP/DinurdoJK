//! Underwater gameplay bus, after Steam Audio/EAL and before the master limiter.
//!
//! The low-pass is the LOWPASS biquad coefficient calculation from Godot's
//! `servers/audio/audio_filter_sw.cpp` (MIT license; Godot Engine contributors):
//! https://github.com/godotengine/godot/blob/master/servers/audio/audio_filter_sw.cpp
//! The per-channel transposed-direct-form-II state and coefficient glide are
//! the corresponding stereo streaming implementation for Rodio's f32 Source.
//! Music lives on a different mixer and never passes through this stage.
use std::sync::{atomic::{AtomicU8, Ordering}, Arc};
use std::time::Duration;
use rodio::{mixer::MixerSource, Source};

/// One byte communicates both the saved preset and the listener's wet state.
/// 0 means dry/off, 1/2/3 mean submerged with subtle/normal/heavy filtering.
pub(super) type UnderwaterControl = Arc<AtomicU8>;

#[derive(Clone, Copy, Debug)]
struct Coefficients { b0: f32, b1: f32, b2: f32, a1: f32, a2: f32 }
impl Coefficients {
    fn lowpass(cutoff: f32, sample_rate: f32) -> Self {
        // Godot's 2nd-order lowpass (RBJ biquad), Butterworth Q = 1/sqrt(2).
        let omega = std::f32::consts::TAU * cutoff.min(sample_rate * 0.45) / sample_rate;
        let (sin, cos) = omega.sin_cos();
        let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
        let inv_a0 = 1.0 / (1.0 + alpha);
        Self {
            b0: (1.0 - cos) * 0.5 * inv_a0,
            b1: (1.0 - cos) * inv_a0,
            b2: (1.0 - cos) * 0.5 * inv_a0,
            a1: -2.0 * cos * inv_a0,
            a2: (1.0 - alpha) * inv_a0,
        }
    }

    fn glide(&mut self, target: Self, rate: f32) {
        self.b0 += (target.b0 - self.b0) * rate;
        self.b1 += (target.b1 - self.b1) * rate;
        self.b2 += (target.b2 - self.b2) * rate;
        self.a1 += (target.a1 - self.a1) * rate;
        self.a2 += (target.a2 - self.a2) * rate;
    }
}

struct UnderwaterFilter {
    control: UnderwaterControl,
    last_mode: u8,
    current: Coefficients,
    target: Coefficients,
    // Independent state is essential: never cross-feed L/R samples.
    z1: [f32; 2],
    z2: [f32; 2],
    wet: f32,
    target_wet: f32,
    gain: f32,
    target_gain: f32,
    glide: f32,
    channel: usize,
}
impl UnderwaterFilter {
    fn new(control: UnderwaterControl, sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        let coeff = Coefficients::lowpass(3_500.0, sample_rate);
        Self {
            control, last_mode: 0, current: coeff, target: coeff,
            z1: [0.0; 2], z2: [0.0; 2], wet: 0.0, target_wet: 0.0,
            gain: 1.0, target_gain: 1.0,
            // ~200ms transition at the output sample rate, independent of FPS.
            glide: 1.0 - (-1.0 / (0.2 * sample_rate)).exp(),
            channel: 0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32, sample_rate: u32) -> f32 {
        let channel = self.channel;
        self.channel = (channel + 1) & 1;
        if channel == 0 {
            let mode = self.control.load(Ordering::Relaxed).min(3);
            if mode != self.last_mode {
                self.last_mode = mode;
                self.target_wet = if mode == 0 { 0.0 } else { 1.0 };
                if mode != 0 {
                    let (cutoff, db) = match mode {
                        1 => (3_500.0, -2.0_f32),
                        2 => (1_400.0, -5.0_f32),
                        _ => (650.0, -8.0_f32),
                    };
                    self.target = Coefficients::lowpass(cutoff, sample_rate as f32);
                    self.target_gain = 10.0_f32.powf(db / 20.0);
                }
            }
            self.wet += (self.target_wet - self.wet) * self.glide;
            self.gain += (self.target_gain - self.gain) * self.glide;
            // Coefficients glide as well as gain: switching presets submerged
            // cannot introduce a click or cause the cutoff to jump mid-block.
            self.current.glide(self.target, self.glide);
        }
        if self.wet <= 0.00001 && self.target_wet == 0.0 {
            // Exact dry bypass with zero filter history. Re-entering the water
            // cannot replay the tail of an old sound from a previous submersion.
            self.z1[channel] = 0.0;
            self.z2[channel] = 0.0;
            return x;
        }
        let y = self.current.b0 * x + self.z1[channel];
        self.z1[channel] = self.current.b1 * x - self.current.a1 * y + self.z2[channel];
        self.z2[channel] = self.current.b2 * x - self.current.a2 * y;
        // Guard against denormals in long, silent underwater passages.
        if self.z1[channel].abs() < 1.0e-20 { self.z1[channel] = 0.0; }
        if self.z2[channel].abs() < 1.0e-20 { self.z2[channel] = 0.0; }
        x + (self.gain * y - x) * self.wet
    }
}

/// Music is on an unfiltered bus, but it still must stay attached when the
/// mixer temporarily has no track. Rodio detaches exhausted MixerSources.
pub(super) struct DryMusicSource(MixerSource);
impl DryMusicSource {
    pub(super) fn new(inner: MixerSource) -> Self { Self(inner) }
}
impl Iterator for DryMusicSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> { Some(self.0.next().unwrap_or(0.0)) }
}
impl Source for DryMusicSource {
    fn current_span_len(&self) -> Option<usize> { None }
    fn channels(&self) -> rodio::ChannelCount { self.0.channels() }
    fn sample_rate(&self) -> rodio::SampleRate { self.0.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { None }
}

pub(super) struct UnderwaterSource {
    inner: MixerSource,
    filter: UnderwaterFilter,
    sample_rate: u32,
}
impl UnderwaterSource {
    pub(super) fn new(inner: MixerSource, control: UnderwaterControl) -> Self {
        let sample_rate = inner.sample_rate().get();
        Self { inner, filter: UnderwaterFilter::new(control, sample_rate), sample_rate }
    }
}
impl Iterator for UnderwaterSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        // Keep the bus alive even while all 32 game voices are empty. A Rodio
        // MixerSource otherwise ends and future one-shots become inaudible.
        Some(self.filter.process(self.inner.next().unwrap_or(0.0), self.sample_rate))
    }
}
impl Source for UnderwaterSource {
    fn current_span_len(&self) -> Option<usize> { None }
    fn channels(&self) -> rodio::ChannelCount { rodio::nz!(2) }
    fn sample_rate(&self) -> rodio::SampleRate { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn filter(mode: u8) -> (UnderwaterControl, UnderwaterFilter) {
        let control = Arc::new(AtomicU8::new(mode));
        (Arc::clone(&control), UnderwaterFilter::new(control, 48_000))
    }
    #[test]
    fn dry_passthrough_is_exact_and_has_no_cross_channel_leak() {
        let (_, mut f) = filter(0);
        for n in 0..10_000 {
            let left = (n as f32 / 17.0).sin();
            assert_eq!(f.process(left, 48_000), left);
            assert_eq!(f.process(0.0, 48_000), 0.0);
        }
    }
    #[test]
    fn heavy_reduces_high_frequency_and_preserves_low_frequency() {
        fn rms(freq: f32) -> f64 {
            let (_, mut f) = filter(3);
            let mut square = 0.0_f64;
            for n in 0..96_000 {
                let x = (std::f32::consts::TAU * freq * n as f32 / 48_000.0).sin();
                let y = f.process(x, 48_000);
                f.process(x, 48_000); // feed both stereo channels
                if n >= 48_000 { square += (y * y) as f64; }
            }
            (square / 48_000.0).sqrt()
        }
        assert!(rms(8_000.0) < rms(150.0) * 0.1);
    }
    #[test]
    fn wet_dry_preset_changes_are_continuous() {
        let (control, mut f) = filter(0);
        let mut previous = 0.0;
        for n in 0..96_000 {
            if n == 4_000 { control.store(2, Ordering::Relaxed); }
            if n == 21_000 { control.store(3, Ordering::Relaxed); }
            if n == 35_000 { control.store(0, Ordering::Relaxed); }
            let y = f.process(0.25, 48_000);
            f.process(0.25, 48_000);
            assert!(y.is_finite());
            assert!((y - previous).abs() < 0.15, "discontinuity at {n}: {y} vs {previous}");
            previous = y;
        }
        assert!((previous - 0.25).abs() < 0.02);
    }
}
