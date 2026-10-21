//! Horizontal acceleration or strafe-angle feedback, sampled on the simulation clock;
//! one local oscillator runs on the audio thread, without queued beep assets.
use super::{connect_player, Mixer, Player, Source};
use std::{
    f32::consts::TAU,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const STALE: Duration = Duration::from_millis(250);
const DEADBAND: f32 = 12.0; // horizontal units / second / second

#[derive(Clone, Copy)]
pub(crate) struct MotionSample {
    pub time_ms: i32,
    pub velocity: [f32; 3],
    pub origin: [f32; 3],
    pub client: i32,
    pub teleport_bit: i32,
    /// None selects horizontal acceleration; Some selects signed guide error.
    pub angle_error: Option<f32>,
    /// Degrees either side of the optimal angle that stay silent.
    pub angle_dead_zone: f32,
    /// Sensitivity multiplier; 1 reaches the full tone range 15 degrees past the dead zone.
    pub angle_scale: f32,
}

impl MotionSample {
    fn speed(self) -> f32 {
        self.velocity[0].hypot(self.velocity[1])
    }
    fn valid(self) -> bool {
        self.velocity[..2]
            .iter()
            .chain(self.origin.iter())
            .all(|v| v.is_finite())
            && self.speed().is_finite()
            && self.angle_error.is_none_or(f32::is_finite)
    }
}

#[derive(Default)]
struct MotionTracker {
    previous: Option<MotionSample>,
    acceleration: f32,
}

impl MotionTracker {
    /// None means no new simulation sample yet. Some(0) resets audible feedback.
    fn update(&mut self, sample: MotionSample) -> Option<f32> {
        if !sample.valid() {
            *self = Self::default();
            return Some(0.0);
        }
        let Some(previous) = self.previous else {
            self.previous = Some(sample);
            return Some(0.0);
        };
        let elapsed = i64::from(sample.time_ms) - i64::from(previous.time_ms);
        let distance = sample
            .origin
            .iter()
            .zip(previous.origin)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt();
        let plausible_distance = 64.0
            + sample
                .speed()
                .max(previous.speed())
                .max(sample.velocity[2].abs())
                .max(previous.velocity[2].abs())
                * elapsed.max(0) as f32
                * 0.002;
        if elapsed < 0
            || elapsed > 250
            || sample.client != previous.client
            || sample.angle_error.is_some() != previous.angle_error.is_some()
            || sample.teleport_bit != previous.teleport_bit
            || distance > plausible_distance
        {
            self.previous = Some(sample);
            self.acceleration = 0.0;
            return Some(0.0);
        }
        // Average tiny command steps before differentiating. This also avoids
        // dividing repeated snapshot speeds by rendered-frame durations.
        if elapsed < 32 {
            return None;
        }
        let dt = elapsed as f32 * 0.001;
        let raw = if let Some(error) = sample.angle_error {
            // Configurable dead zone. Under = low/falling, over = high/rising;
            // 15 degrees beyond the dead zone reaches the full tone range.
            let magnitude = (error.abs() - sample.angle_dead_zone.max(0.0)).max(0.0);
            if magnitude == 0.0 {
                0.0
            } else {
                error.signum() * (DEADBAND + magnitude * sample.angle_scale.max(0.0) * 800.0 / 15.0)
            }
        } else {
            (sample.speed() - previous.speed()) / dt
        }
        .clamp(-8000.0, 8000.0);
        self.acceleration += (raw - self.acceleration) * (1.0 - (-dt / 0.080).exp());
        self.previous = Some(sample);
        Some(self.acceleration)
    }
}

#[derive(Default)]
struct ToneControl {
    acceleration: AtomicU32,
    volume: AtomicU32,
    revision: AtomicU32,
    style: AtomicU8,
    stopping: AtomicBool,
}

pub(super) struct Variometer {
    tracker: MotionTracker,
    last_fresh: Instant,
    control: Arc<ToneControl>,
    player: Player,
}

impl Variometer {
    pub(super) fn new(mixer: &Mixer, sample_rate: u32) -> Self {
        let control = Arc::new(ToneControl::default());
        let player = connect_player(mixer, ToneSource::new(Arc::clone(&control), sample_rate));
        Self {
            tracker: MotionTracker::default(),
            last_fresh: Instant::now(),
            control,
            player,
        }
    }

    pub(super) fn update(&mut self, sample: MotionSample, volume: f32, style: u8) {
        self.control
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.control.style.store(style.min(2), Ordering::Relaxed);
        if self.last_fresh.elapsed() > STALE {
            self.tracker = MotionTracker::default();
        }
        if let Some(acceleration) = self.tracker.update(sample) {
            self.control
                .acceleration
                .store(acceleration.to_bits(), Ordering::Relaxed);
            self.control.revision.fetch_add(1, Ordering::Release);
            self.last_fresh = Instant::now();
        }
    }

    pub(super) fn stop(self) {
        self.control.stopping.store(true, Ordering::Relaxed);
        self.player.detach(); // oscillator finishes after a short release
    }
}

struct ToneSource {
    control: Arc<ToneControl>,
    rate: rodio::SampleRate,
    phase: f32,
    pulse: f32,
    frequency: f32,
    level: f32,
    acceleration: f32,
    volume: f32,
    revision: u32,
    stale_frames: u32,
    frames: u32,
    style: u8,
}

impl ToneSource {
    fn new(control: Arc<ToneControl>, sample_rate: u32) -> Self {
        Self {
            control,
            rate: rodio::SampleRate::new(sample_rate).expect("valid output rate"),
            phase: 0.0,
            pulse: 0.0,
            frequency: 650.0,
            level: 0.0,
            acceleration: 0.0,
            volume: 0.0,
            revision: 0,
            stale_frames: 0,
            frames: 0,
            style: 0,
        }
    }
}

impl Iterator for ToneSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let rate = self.rate.get() as f32;
        // Atomics are read once per small block; no locks or allocations in DSP.
        if self.frames % 128 == 0 {
            let revision = self.control.revision.load(Ordering::Acquire);
            if revision != self.revision {
                self.revision = revision;
                self.stale_frames = 0;
            }
            self.acceleration = f32::from_bits(self.control.acceleration.load(Ordering::Relaxed));
            self.volume = f32::from_bits(self.control.volume.load(Ordering::Relaxed));
            let style = self.control.style.load(Ordering::Relaxed);
            if style != self.style {
                self.pulse = 0.0;
            }
            self.style = style;
        }
        self.frames = self.frames.wrapping_add(1);
        self.stale_frames = self.stale_frames.saturating_add(1);
        let stopping = self.control.stopping.load(Ordering::Relaxed);
        let active = !stopping && self.stale_frames < self.rate.get() / 4;
        let (frequency, gate, cadence) = tone_parameters(self.style, self.acceleration, self.pulse);
        self.frequency += (frequency - self.frequency) * (1.0 / (0.015 * rate));
        // Restart the beep cycle from its audible half when feedback wakes up.
        if !active {
            self.pulse = 0.0;
        }
        let audible = active && gate;
        let target = if audible { self.volume * 0.22 } else { 0.0 };
        // A 5 ms envelope keeps both beep boundaries and live volume changes smooth.
        self.level += (target - self.level) * (1.0 / (0.005 * rate));
        if stopping && self.level < 0.00001 {
            return None;
        }
        let sample = (self.phase * TAU).sin() * self.level;
        self.phase = (self.phase + self.frequency / rate).fract();
        self.pulse = (self.pulse + cadence / rate).fract();
        Some(sample)
    }
}

/// BeatDrop's README describes rising/falling/flat pairs and black-key notes:
/// https://github.com/kubotron/BeatDrop#what-is-bibip
/// This is an original synthesis mapping of that idea to horizontal acceleration.
fn tone_parameters(style: u8, acceleration: f32, pulse: f32) -> (f32, bool, f32) {
    let changing = acceleration.abs() > DEADBAND;
    let strength = ((acceleration.abs() - DEADBAND).max(0.0) / 800.0)
        .min(1.0)
        .sqrt();
    let gaining = acceleration > 0.0;
    if style == 1 {
        // Pentatonic black-key pitches, with a distinct two-note direction.
        const NOTES: [f32; 7] = [311.13, 369.99, 415.30, 466.16, 554.37, 622.25, 739.99];
        let step = (strength * 4.0).round() as usize;
        let second = pulse >= 0.20;
        let note = if !changing {
            3
        } else if gaining {
            1 + step + usize::from(second)
        } else {
            4 - step + usize::from(!second)
        };
        // Gap between notes, then a longer rest between pairs.
        let gate = pulse < 0.16 || (0.20..0.38).contains(&pulse);
        (
            NOTES[note],
            gate,
            if changing { 1.5 + 3.5 * strength } else { 1.3 },
        )
    } else {
        let frequency = if gaining {
            650.0 + 850.0 * strength
        } else {
            350.0 - 150.0 * strength
        };
        (
            frequency,
            changing && (style == 2 || !gaining || pulse < 0.48),
            2.0 + 7.0 * strength,
        )
    }
}

impl Source for ToneSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::nz!(1)
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time_ms: i32, velocity: [f32; 3]) -> MotionSample {
        MotionSample {
            time_ms,
            velocity,
            origin: [0.0; 3],
            client: 0,
            teleport_bit: 0,
            angle_error: None,
            angle_dead_zone: 0.5,
            angle_scale: 1.0,
        }
    }

    #[test]
    fn variometer_ignores_vertical_speed_and_constant_speed_turns() {
        let mut tracker = MotionTracker::default();
        assert_eq!(tracker.update(sample(0, [300.0, 400.0, 0.0])), Some(0.0));
        assert_eq!(
            tracker.update(sample(40, [-400.0, 300.0, 1200.0])),
            Some(0.0)
        );
        assert_eq!(tracker.update(sample(80, [0.0, -500.0, -900.0])), Some(0.0));
    }

    #[test]
    fn variometer_measures_signed_acceleration_on_simulation_time() {
        let mut tracker = MotionTracker::default();
        tracker.update(sample(0, [200.0, 0.0, 0.0]));
        for _ in 0..20 {
            assert_eq!(tracker.update(sample(0, [200.0, 0.0, 0.0])), None);
        }
        assert!(tracker.update(sample(40, [220.0, 0.0, 0.0])).unwrap() > DEADBAND);
        assert!(tracker.update(sample(80, [180.0, 0.0, 0.0])).unwrap() < -DEADBAND);
    }

    #[test]
    fn variometer_resets_at_teleports_time_jumps_and_subject_changes() {
        let mut tracker = MotionTracker::default();
        tracker.update(sample(1000, [300.0, 0.0, 0.0]));
        let mut teleported = sample(1040, [0.0; 3]);
        teleported.teleport_bit = 8;
        assert_eq!(tracker.update(teleported), Some(0.0));
        assert_eq!(tracker.update(sample(100, [2000.0, 0.0, 0.0])), Some(0.0));
        assert_eq!(tracker.update(sample(1000, [100.0, 0.0, 0.0])), Some(0.0));
        let mut other = sample(1040, [900.0, 0.0, 0.0]);
        other.client = 3;
        assert_eq!(tracker.update(other), Some(0.0));
        let mut moved = sample(1080, [0.0; 3]);
        moved.client = 3;
        moved.origin[0] = 10000.0;
        assert_eq!(tracker.update(moved), Some(0.0));
    }

    #[test]
    fn variometer_invalid_samples_do_not_poison_future_feedback() {
        let mut tracker = MotionTracker::default();
        tracker.update(sample(0, [f32::NAN, 0.0, 0.0]));
        assert_eq!(tracker.update(sample(40, [100.0, 0.0, 0.0])), Some(0.0));
        assert!(tracker
            .update(sample(80, [200.0, 0.0, 0.0]))
            .unwrap()
            .is_finite());
    }

    #[test]
    fn variometer_angle_input_distinguishes_under_over_and_dead_zone() {
        for (error, sign) in [(-5.0, -1.0), (5.0, 1.0), (0.4, 0.0)] {
            let mut tracker = MotionTracker::default();
            let mut state = sample(0, [700.0, 0.0, 0.0]);
            state.angle_error = Some(error);
            tracker.update(state);
            state.time_ms = 40;
            // Deliberate speed loss: angle input must take precedence.
            state.velocity[0] = 600.0;
            let feedback = tracker.update(state).unwrap();
            if sign == 0.0 {
                assert_eq!(feedback, 0.0);
            } else {
                assert_eq!(feedback.signum(), sign);
            }
            state.time_ms = 80;
            state.angle_error = None;
            assert_eq!(tracker.update(state), Some(0.0)); // switching inputs resets
        }
    }

    #[test]
    fn variometer_backend_stops_on_disable_mute_pause_and_clear() {
        let (mut backend, mut output) = super::super::AudioBackend::offline();
        backend.update_variometer(Some(sample(0, [200.0, 0.0, 0.0])), 1.0, 0);
        backend.update_variometer(Some(sample(40, [240.0, 0.0, 0.0])), 1.0, 0);
        assert!(backend.variometer.is_some());
        assert!(output.by_ref().take(8000).any(|s| s.abs() > 0.05));
        backend.update_variometer(None, 1.0, 0);
        assert!(backend.variometer.is_none());
        backend.update_variometer(Some(sample(80, [200.0, 0.0, 0.0])), 0.0, 0);
        assert!(backend.variometer.is_none());
        backend.update_variometer(Some(sample(120, [200.0, 0.0, 0.0])), 1.0, 1);
        backend.set_rate(0.0);
        assert!(backend.variometer.is_none());
        backend.update_variometer(Some(sample(160, [200.0, 0.0, 0.0])), 1.0, 1);
        assert!(backend.variometer.is_none());
        backend.set_rate(1.0);
        backend.update_variometer(Some(sample(200, [200.0, 0.0, 0.0])), 1.0, 2);
        assert!(backend.variometer.is_some());
        backend.clear();
        assert!(backend.variometer.is_none());
    }

    fn tone(acceleration: f32) -> ToneSource {
        let control = Arc::new(ToneControl::default());
        control
            .acceleration
            .store(acceleration.to_bits(), Ordering::Relaxed);
        control.volume.store(1.0f32.to_bits(), Ordering::Relaxed);
        control.revision.store(1, Ordering::Release);
        ToneSource::new(control, 48000)
    }

    #[test]
    fn variometer_pcm_is_bounded_silent_in_deadband_and_fades_when_stale() {
        assert!(tone(5.0).take(24000).all(|v| v == 0.0));
        for acceleration in [-1000.0, 1000.0] {
            let samples: Vec<_> = tone(acceleration).take(24000).collect();
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 0.221));
            assert!(samples[..8000].iter().any(|s| s.abs() > 0.1));
            assert!(samples[20000..].iter().all(|s| s.abs() < 0.00001));
        }
    }

    #[test]
    fn variometer_speed_gain_pulses_and_speed_loss_is_continuous() {
        let mut gain = tone(800.0);
        let mut loss = tone(-800.0);
        let mut gain_quiet = 0;
        let mut loss_quiet = 0;
        for _ in 0..10000 {
            gain.next();
            loss.next();
            if gain.level < 0.01 {
                gain_quiet += 1;
            }
            if loss.level < 0.01 {
                loss_quiet += 1;
            }
        }
        assert!(gain_quiet > 2000);
        assert!(loss_quiet < 100);
        gain.control.stopping.store(true, Ordering::Relaxed);
        assert!(gain.take(4800).count() < 4800);
    }

    #[test]
    fn variometer_presets_have_distinct_note_direction_and_gaps() {
        let first = |style, acceleration| tone_parameters(style, acceleration, 0.05);
        let second = |style, acceleration| tone_parameters(style, acceleration, 0.25);
        assert!(first(1, 400.0).0 < second(1, 400.0).0);
        assert!(first(1, -400.0).0 > second(1, -400.0).0);
        assert_eq!(first(1, 0.0).0, second(1, 0.0).0);
        assert!(first(1, 0.0).1); // BeatDrop's steady-speed flat pair
        assert!(!first(0, 0.0).1);
        assert!(!first(2, 0.0).1);
        assert!(!tone_parameters(0, 400.0, 0.7).1);
        assert!(tone_parameters(2, 400.0, 0.7).1);
        assert!(!tone_parameters(1, -400.0, 0.7).1);
        assert!(first(0, 800.0).0 > first(0, 40.0).0);
        assert!(first(0, 800.0).2 > first(0, 40.0).2);
    }

    #[test]
    fn variometer_every_preset_handles_live_changes_mute_and_stop() {
        let mut source = tone(400.0);
        for style in 0..=2 {
            source.control.style.store(style, Ordering::Relaxed);
            source.control.revision.fetch_add(1, Ordering::Release);
            let samples: Vec<_> = source.by_ref().take(8000).collect();
            assert_eq!(source.style, style);
            assert!(samples.iter().any(|s| s.abs() > 0.1));
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 0.221));
        }
        source
            .control
            .volume
            .store(0.0f32.to_bits(), Ordering::Relaxed);
        assert!(source
            .by_ref()
            .skip(6000)
            .take(1000)
            .all(|s| s.abs() < 0.00001));
        source.control.stopping.store(true, Ordering::Relaxed);
        assert_eq!(source.next(), None);
    }
}
