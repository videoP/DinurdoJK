//! Per-source PCM transport to native EAX. The device callback samples the same
//! decoded audio/clocks as the legacy voices; only the worker calls native DSP.
use super::{
    loop_seam_sample, Gains, Listener, RegisteredSound, GAIN_GLIDE_SECONDS, STOP_FADE_SECONDS,
};
use crate::{
    eal_native::{SceneFrame, MAX_SOURCES, PREROLL_BLOCKS},
    eal_worker::{EalWorker, BLOCK_FRAMES, LATENCY_BLOCKS},
};
use rodio::Source;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
struct Control {
    stopped: AtomicBool,
    paused: AtomicBool,
    speed: AtomicU32,
    frames: AtomicU64,
}
pub(super) struct EalVoiceHandle {
    control: Arc<Control>,
    detached: bool,
}
impl EalVoiceHandle {
    pub fn stop(&self) {
        self.control.stopped.store(true, Ordering::Release);
    }
    pub fn pause(&self) {
        self.control.paused.store(true, Ordering::Release);
    }
    pub fn play(&self) {
        self.control.paused.store(false, Ordering::Release);
    }
    pub fn set_speed(&self, speed: f32) {
        self.control.speed.store(speed.to_bits(), Ordering::Relaxed);
    }
    pub fn detach(mut self) {
        self.detached = true;
    }
    #[cfg(test)]
    pub fn speed(&self) -> f32 {
        f32::from_bits(self.control.speed.load(Ordering::Relaxed))
    }
    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        self.control.paused.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub fn get_pos(&self) -> Duration {
        Duration::from_nanos(self.control.frames.load(Ordering::Relaxed))
    }
}
impl Drop for EalVoiceHandle {
    fn drop(&mut self) {
        if !self.detached {
            self.stop();
        }
    }
}

struct Stream {
    id: u64,
    name: [u8; 96],
    sound: Arc<RegisteredSound>,
    gains: Arc<Gains>,
    control: Arc<Control>,
    cursor: f64,
    synced: bool,
    looping: bool,
    loop_start: usize,
    level: f32,
    fade: f32,
    first_submitted: Option<u64>,
    ended: Option<u64>,
    seam: usize,
}
impl Stream {
    fn fill(&mut self, pcm: &mut [f32; BLOCK_FRAMES], rate: u32) -> bool {
        if self.control.stopped.load(Ordering::Acquire) {
            return false;
        }
        if !self.synced {
            let position = f64::from_bits(self.gains.eal_position.load(Ordering::Acquire));
            if !position.is_finite() {
                return true;
            }
            self.cursor = position;
            self.synced = true;
        }
        if self.control.paused.load(Ordering::Acquire) {
            return true;
        }
        let speed = f32::from_bits(self.control.speed.load(Ordering::Relaxed)).max(0.0);
        let step = self.sound.sample_rate.get() as f64 / rate as f64 * speed as f64;
        let glide = 1.0 - (-1.0 / (GAIN_GLIDE_SECONDS * rate as f32)).exp();
        let fade_step = 1.0 / (STOP_FADE_SECONDS * rate as f32).max(1.0);
        let target = self.gains.hrtf().0;
        let len = self.sound.samples.len();
        for sample in pcm {
            if self.cursor >= len as f64 {
                if !self.looping {
                    return false;
                }
                self.cursor = self.loop_start as f64
                    + (self.cursor - len as f64).rem_euclid((len - self.loop_start) as f64);
            }
            if self.gains.stopping.load(Ordering::Relaxed) {
                self.fade = (self.fade - fade_step).max(0.0);
                if self.fade == 0.0 {
                    return false;
                }
            }
            let first = self.cursor as usize;
            let next = if first + 1 < len {
                first + 1
            } else if self.looping {
                self.loop_start
            } else {
                first
            };
            let fraction = (self.cursor - first as f64) as f32;
            self.level += (target - self.level) * glide;
            *sample = (loop_seam_sample(&self.sound.samples, first, self.loop_start, self.seam)
                + (loop_seam_sample(&self.sound.samples, next, self.loop_start, self.seam)
                    - loop_seam_sample(&self.sound.samples, first, self.loop_start, self.seam))
                    * fraction)
                * self.level
                * self.fade;
            self.cursor += step;
            self.control
                .frames
                .fetch_add((1_000_000_000.0 / rate as f64) as u64, Ordering::Relaxed);
        }
        true
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        if self.gains.eal_owner.load(Ordering::Acquire) == self.id {
            self.gains
                .eal_native_mix
                .store(0.0f32.to_bits(), Ordering::Release);
        }
    }
}

struct ListenerState {
    origin: [AtomicU32; 3],
    ahead: [AtomicU32; 3],
    up: [AtomicU32; 3],
    hrtf: AtomicBool,
}
impl ListenerState {
    fn new(listener: Listener) -> Self {
        Self {
            origin: listener.origin.map(|v| AtomicU32::new(v.to_bits())),
            ahead: listener.ahead.map(|v| AtomicU32::new(v.to_bits())),
            up: listener.up.map(|v| AtomicU32::new(v.to_bits())),
            hrtf: AtomicBool::new(false),
        }
    }
    fn update(&self, listener: Listener, hrtf: bool) {
        for (slots, values) in [
            (&self.origin, listener.origin),
            (&self.ahead, listener.ahead),
            (&self.up, listener.up),
        ] {
            for (slot, value) in slots.iter().zip(values) {
                slot.store(value.to_bits(), Ordering::Relaxed);
            }
        }
        self.hrtf.store(hrtf, Ordering::Relaxed);
    }
}
pub(super) struct EalBus {
    sender: mpsc::SyncSender<Stream>,
    listener: Arc<ListenerState>,
    status: Arc<crate::eal_worker::WorkerStatus>,
}
impl EalBus {
    pub fn new(
        worker: EalWorker,
        listener: Listener,
        enabled: Arc<AtomicBool>,
        debug_mode: Arc<AtomicU32>,
        rate: u32,
        offline: bool,
    ) -> (Self, EalSceneSource) {
        let (sender, receiver) = mpsc::sync_channel(MAX_SOURCES * 2);
        let listener = Arc::new(ListenerState::new(listener));
        let status = Arc::clone(&worker.status);
        let bus = Self {
            sender,
            listener: listener.clone(),
            status,
        };
        let source = EalSceneSource {
            worker,
            receiver,
            listener,
            enabled,
            debug_mode,
            streams: std::array::from_fn(|_| None),
            rate,
            offline,
            output: [0.0; BLOCK_FRAMES * 2],
            cursor: BLOCK_FRAMES * 2,
            blend: 0.0,
            sequence: 0,
        };
        (bus, source)
    }
    pub fn update_listener(&self, listener: Listener, hrtf: bool) {
        self.listener.update(listener, hrtf);
    }
    pub fn add(
        &self,
        name: &str,
        sound: Arc<RegisteredSound>,
        gains: Arc<Gains>,
        looping: bool,
        loop_start: usize,
        phase: Option<usize>,
    ) -> Option<EalVoiceHandle> {
        if sound.samples.is_empty() || loop_start >= sound.samples.len() {
            return None;
        }
        let control = Arc::new(Control {
            stopped: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            speed: AtomicU32::new(1.0f32.to_bits()),
            frames: AtomicU64::new(0),
        });
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut path = [0u8; 96];
        let n = name.len().min(95);
        path[..n].copy_from_slice(&name.as_bytes()[..n]);
        let cursor = phase.unwrap_or(0) % sound.samples.len();
        gains.eal_owner.store(id, Ordering::Release);
        let seam = if looping && loop_start > 0 {
            (sound.sample_rate.get() as usize / 200).min((sound.samples.len() - loop_start) / 8)
        } else {
            0
        };
        let stream = Stream {
            first_submitted: None,
            ended: None,
            seam,
            id,
            name: path,
            sound,
            gains,
            control: control.clone(),
            cursor: cursor as f64,
            synced: phase.is_some(),
            looping,
            loop_start,
            level: 0.0,
            fade: 1.0,
        };
        if self.sender.try_send(stream).is_err() {
            self.status.overflows.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(EalVoiceHandle {
            control,
            detached: false,
        })
    }
    #[cfg(test)]
    pub fn mock(listener: Listener, enabled: Arc<AtomicBool>) -> (Self, EalSceneSource) {
        Self::new(EalWorker::test_silent(), listener, enabled, Arc::new(AtomicU32::new(0)), 48000, true)
    }
}

pub(super) struct EalSceneSource {
    worker: EalWorker,
    receiver: mpsc::Receiver<Stream>,
    listener: Arc<ListenerState>,
    enabled: Arc<AtomicBool>,
    debug_mode: Arc<AtomicU32>,
    streams: [Option<Stream>; MAX_SOURCES],
    rate: u32,
    offline: bool,
    output: [f32; BLOCK_FRAMES * 2],
    cursor: usize,
    blend: f32,
    sequence: u64,
}
impl EalSceneSource {
    fn fill(&mut self) {
        for _ in 0..MAX_SOURCES * 2 {
            let Some(slot) = self.streams.iter_mut().find(|s| s.is_none()) else {
                break;
            };
            let Ok(mut stream) = self.receiver.try_recv() else {
                break;
            };
            let actual = f64::from_bits(stream.gains.eal_position.load(Ordering::Acquire));
            if actual.is_finite() {
                stream.cursor = actual;
                stream.synced = true;
            }
            *slot = Some(stream);
        }
        let sequence = self.sequence;
        self.sequence += 1;
        let mut frame = SceneFrame::silent();
        frame.listener = std::array::from_fn(|i| {
            f32::from_bits(self.listener.origin[i].load(Ordering::Relaxed))
        });
        frame.ahead =
            std::array::from_fn(|i| f32::from_bits(self.listener.ahead[i].load(Ordering::Relaxed)));
        frame.up =
            std::array::from_fn(|i| f32::from_bits(self.listener.up[i].load(Ordering::Relaxed)));
        frame.hrtf = u32::from(self.listener.hrtf.load(Ordering::Relaxed));
        frame.debug_mode = self.debug_mode.load(Ordering::Relaxed).min(9);
        for (slot, voice) in self.streams.iter_mut().zip(&mut frame.voices) {
            let Some(stream) = slot else {
                continue;
            };
            voice.id = stream.id;
            voice.name = stream.name;
            voice.position = std::array::from_fn(|i| {
                f32::from_bits(stream.gains.environment_source[i].load(Ordering::Relaxed))
            });
            if stream.ended.is_none() {
                let alive = stream.fill(&mut voice.pcm, self.rate);
                voice.active = u32::from(stream.synced);
                if stream.synced {
                    stream.first_submitted.get_or_insert(sequence);
                }
                if !alive {
                    stream.ended = Some(sequence);
                }
            }
        }
        let enabled = self.enabled.load(Ordering::Acquire);
        let ready = self
            .worker
            .process_scene(frame, enabled, &mut self.output, self.offline);
        let native = enabled && ready && self.worker.status.running.load(Ordering::Acquire);
        for stream in self.streams.iter().flatten() {
            if stream.gains.eal_owner.load(Ordering::Acquire) == stream.id {
                stream.gains.eal_native_mix.store(
                    (if native
                        && stream.first_submitted.is_some_and(|first| {
                            sequence >= first + LATENCY_BLOCKS + PREROLL_BLOCKS
                        })
                        && stream.synced
                    {
                        1.0f32
                    } else {
                        0.0
                    })
                    .to_bits(),
                    Ordering::Release,
                );
            }
        }
        let target = if native { 1.0 } else { 0.0 };
        let glide = 1.0 - (-1.0 / (GAIN_GLIDE_SECONDS * self.rate as f32)).exp();
        for stereo in self.output.chunks_exact_mut(2) {
            self.blend += (target - self.blend) * glide;
            for sample in stereo {
                *sample *= self.blend;
            }
        }
        for slot in &mut self.streams {
            if slot.as_ref().is_some_and(|s| {
                s.ended
                    .is_some_and(|last| sequence > last + LATENCY_BLOCKS + PREROLL_BLOCKS + 1)
            }) {
                *slot = None;
            }
        }
        self.cursor = 0;
    }
}
impl Iterator for EalSceneSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.cursor == self.output.len() {
            self.fill();
        }
        let value = self.output[self.cursor];
        self.cursor += 1;
        Some(value)
    }
}
impl Source for EalSceneSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::nz!(2)
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(self.rate).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn listener() -> Listener {
        Listener {
            entity: 0,
            origin: [0.; 3],
            ahead: [1., 0., 0.],
            up: [0., 0., 1.],
            left: [0., 1., 0.],
        }
    }
    fn sound(count: usize) -> Arc<RegisteredSound> {
        Arc::new(RegisteredSound {
            samples: vec![0.2; count].into(),
            sample_rate: rodio::nz!(48000),
        })
    }
    fn gains() -> Arc<Gains> {
        let g = Arc::new(Gains::new([1.; 2]));
        g.set_hrtf(1., [1., 0., 0.], true);
        g
    }
    fn drain(source: &mut EalSceneSource) {
        for _ in 0..BLOCK_FRAMES * 2 {
            source.next();
        }
    }
    #[test]
    fn late_source_waits_for_pcm_clock_and_ack_before_muting_fallback() {
        let enabled = Arc::new(AtomicBool::new(true));
        let (bus, mut source) = EalBus::mock(listener(), enabled);
        let g = gains();
        let _handle = bus
            .add("late.wav", sound(10000), g.clone(), false, 0, None)
            .unwrap();
        for _ in 0..5 {
            drain(&mut source);
            assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 0.);
        }
        assert_eq!(source.streams[0].as_ref().unwrap().cursor, 0.);
        g.eal_position.store(1234f64.to_bits(), Ordering::Release);
        drain(&mut source);
        assert_eq!(source.streams[0].as_ref().unwrap().cursor, 1490.);
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 0.);
        for _ in 0..LATENCY_BLOCKS+PREROLL_BLOCKS { drain(&mut source); }
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 1.);
    }
    #[test]
    fn final_partial_block_survives_transport_and_disabled_audio_falls_back() {
        let enabled = Arc::new(AtomicBool::new(true));
        let (bus, mut source) = EalBus::mock(listener(), enabled.clone());
        let g = gains();
        let _handle = bus
            .add("short.wav", sound(100), g.clone(), false, 0, Some(0))
            .unwrap();
        drain(&mut source);
        assert!(source.streams[0].is_some());
        drain(&mut source);
        assert!(source.streams[0].is_some());
        drain(&mut source);
        drain(&mut source);
        assert!(source.output.iter().any(|v| v.abs() > 0.0001));
        enabled.store(false, Ordering::Release);
        drain(&mut source);
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 0.);
        for _ in 0..4 {
            drain(&mut source);
        }
        assert!(source.streams[0].is_none());
    }
    #[test]
    fn capacity_waits_without_muting_and_old_generation_cannot_unmute_new() {
        let enabled = Arc::new(AtomicBool::new(true));
        let (bus, mut source) = EalBus::mock(listener(), enabled);
        let mut handles = Vec::new();
        for _ in 0..MAX_SOURCES {
            handles.push(
                bus.add("a", sound(10000), gains(), true, 0, Some(0))
                    .unwrap(),
            );
        }
        drain(&mut source);
        let g = gains();
        let _extra = bus
            .add("overflow", sound(10000), g.clone(), true, 0, Some(0))
            .unwrap();
        drain(&mut source);
        assert_eq!(bus.status.overflows.load(Ordering::Relaxed), 0);
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 0.);
        handles[0].stop();
        for _ in 0..10 {
            drain(&mut source);
        }
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 1.);
        let g = gains();
        let (bus, mut source) = EalBus::mock(listener(), Arc::new(AtomicBool::new(true)));
        let old = bus
            .add("old", sound(10000), g.clone(), true, 0, Some(0))
            .unwrap();
        drain(&mut source);
        let _new = bus
            .add("new", sound(10000), g.clone(), true, 0, Some(0))
            .unwrap();
        old.stop();
        for _ in 0..8 {
            drain(&mut source);
        }
        assert_eq!(f32::from_bits(g.eal_native_mix.load(Ordering::Acquire)), 1.);
    }
    #[test]
    fn scene_replaces_fallback_without_double_dry_and_toggle_recovers() {
        let enabled = Arc::new(AtomicBool::new(true));
        let (bus, mut scene) = EalBus::mock(listener(), enabled.clone());
        let g = gains();
        let pcm = sound(100000);
        let _handle = bus
            .add("loop", pcm.clone(), g.clone(), true, 0, Some(0))
            .unwrap();
        let mut dry = super::super::StereoSource::looping(pcm, g, 0);
        let mut peak = 0f32;
        let mut steady = 0f32;
        for block in 0..80 {
            for _ in 0..BLOCK_FRAMES * 2 {
                let value = scene.next().unwrap() + dry.next().unwrap();
                peak = peak.max(value.abs());
                if block == 79 {
                    steady = steady.max(value.abs());
                }
            }
        }
        assert!(
            (steady - 0.2).abs() < 0.001,
            "duplicate/missing steady direct output {steady}"
        );
        assert!(peak < 0.26, "transition doubled direct output {peak}");
        enabled.store(false, Ordering::Release);
        for _ in 0..80 * BLOCK_FRAMES * 2 {
            scene.next();
            dry.next();
        }
        assert!((dry.next().unwrap() - 0.2).abs() < 0.001);
    }
}
