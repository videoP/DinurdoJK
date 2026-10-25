//! Bounded EAL effects transport. Native OpenAL work and teardown stay on a
//! dedicated thread. The playback callback only exchanges fixed-size blocks.
use crate::{
    eal::EalMap,
    eal_native::{NativeScene, SceneFrame},
};
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) const BLOCK_FRAMES: usize = 256;
pub(crate) const LATENCY_BLOCKS: u64 = 2;
const QUEUE_BLOCKS: usize = 8;

// Exactly one producer and one consumer, neither clonable. A release of write
// publishes a slot; a release of read permits reuse. Operations never wait,
// allocate, or acquire a mutex. The handles require &mut access to exchange data.
struct Ring<T: Copy + Send> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    read: AtomicUsize,
    write: AtomicUsize,
}
// SAFETY: slots are only accessed by their exclusive producer/consumer, and
// acquire/release counters prevent simultaneous access or reuse before reading.
unsafe impl<T: Copy + Send> Sync for Ring<T> {}
struct Producer<T: Copy + Send>(Arc<Ring<T>>);
struct Consumer<T: Copy + Send>(Arc<Ring<T>>);
fn ring<T: Copy + Send>() -> (Producer<T>, Consumer<T>) {
    let state = Arc::new(Ring {
        slots: (0..QUEUE_BLOCKS)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        read: AtomicUsize::new(0),
        write: AtomicUsize::new(0),
    });
    (Producer(Arc::clone(&state)), Consumer(state))
}
impl<T: Copy + Send> Producer<T> {
    fn push(&mut self, value: T) -> bool {
        let write = self.0.write.load(Ordering::Relaxed);
        if write.wrapping_sub(self.0.read.load(Ordering::Acquire)) >= QUEUE_BLOCKS {
            return false;
        }
        // SAFETY: the consumer released this slot and cannot read until write is published.
        unsafe {
            (*self.0.slots[write % QUEUE_BLOCKS].get()).write(value);
        }
        self.0.write.store(write.wrapping_add(1), Ordering::Release);
        true
    }
}
impl<T: Copy + Send> Consumer<T> {
    fn pop(&mut self) -> Option<T> {
        let read = self.0.read.load(Ordering::Relaxed);
        if read == self.0.write.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the producer published this slot and cannot reuse it before read is released.
        let value = unsafe { (*self.0.slots[read % QUEUE_BLOCKS].get()).assume_init_read() };
        self.0.read.store(read.wrapping_add(1), Ordering::Release);
        Some(value)
    }
}

#[derive(Clone, Copy)]
struct InputBlock {
    sequence: u64,
    enabled: bool,
    #[cfg(test)]
    samples: [f32; BLOCK_FRAMES],
    frame: SceneFrame,
}
#[derive(Clone, Copy)]
struct OutputBlock {
    sequence: u64,
    samples: [f32; BLOCK_FRAMES * 2],
}

#[derive(Default)]
pub(crate) struct WorkerStatus {
    pub running: AtomicBool,
    pub underruns: AtomicU64,
    pub overflows: AtomicU64,
    pub max_render_us: AtomicU64,
    pub sources: AtomicU64,
    pub rooms: AtomicU64,
    pub filtered_sources: AtomicU64,
    pub diffracted_sources: AtomicU64,
    pub directional_rooms: AtomicU64,
    pub hrtf: AtomicBool,
    pub queue_starves: AtomicU64,
    pub restarts: AtomicU64,
    pub multi_processed: AtomicU64,
    pub buffer_reuse: AtomicU64,
    pub input_edge_bits: AtomicUsize,
    pub output_edge_bits: AtomicUsize,
    pub callback_underfeed_samples: AtomicU64,
    pub callback_dropped_samples: AtomicU64,
    pub transport: AtomicU64,
    pub hrtf_status: AtomicU64,
    error: Mutex<Option<String>>,
}
impl WorkerStatus {
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn fail(&self, error: String) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        self.running.store(false, Ordering::Release);
    }
}

pub(crate) struct EalWorker {
    input: Producer<InputBlock>,
    output: Consumer<OutputBlock>,
    pending: [Option<OutputBlock>; QUEUE_BLOCKS],
    sequence: u64,
    stop: Arc<AtomicBool>,
    thread: thread::Thread,
    pub status: Arc<WorkerStatus>,
}
impl EalWorker {
    pub fn new(rate: u32, map: Arc<EalMap>) -> Result<Self, String> {
        Self::spawn_with_status(move |status| {
            let mut scene = NativeScene::new(rate, &map)?;
            let mut previous_sequence: Option<u64> = None;
            Ok(
                move |job: InputBlock| -> Result<[f32; BLOCK_FRAMES * 2], String> {
                    let mut output = [0.0; BLOCK_FRAMES * 2];
                    let mut frame = job.frame;
                    if !job.enabled {
                        for voice in &mut frame.voices {
                            voice.active = 0;
                        }
                    }
                    // Advance native effect time after missed blocks instead of
                    // replaying old source PCM. Recovery is bounded.
                    if let Some(previous) = previous_sequence {
                        let missing = job
                            .sequence
                            .saturating_sub(previous + 1)
                            .min(QUEUE_BLOCKS as u64);
                        let mut silent = frame;
                        for voice in &mut silent.voices {
                            voice.pcm.fill(0.0);
                        }
                        for _ in 0..missing {
                            scene.render(&silent, &mut output)?;
                        }
                    }
                    previous_sequence = Some(job.sequence);
                    // PCM reference bypasses OpenAL, EAX and EalMan, but keeps
                    // the EAL per-source sample clocks, worker queue and
                    // Rodio playback/crossfade entirely unchanged. Test this
                    // before changing EAX or audio-device buffering.
                    let stats = if frame.debug_mode == 1 {
                        let mut stats = crate::eal_native::SceneStats::default();
                        for voice in &frame.voices {
                            if voice.active == 0 {
                                continue;
                            }
                            stats.sources += 1;
                            for (i, &sample) in voice.pcm.iter().enumerate() {
                                output[2 * i] += sample * 0.5;
                                output[2 * i + 1] += sample * 0.5;
                            }
                        }
                        stats
                    } else {
                        scene.render(&frame, &mut output)?
                    };
                    status
                        .sources
                        .store(stats.sources as u64, Ordering::Relaxed);
                    status.rooms.store(stats.rooms as u64, Ordering::Relaxed);
                    status
                        .filtered_sources
                        .store(stats.filtered_sources as u64, Ordering::Relaxed);
                    status
                        .diffracted_sources
                        .store(stats.diffracted_sources as u64, Ordering::Relaxed);
                    status
                        .directional_rooms
                        .store(stats.directional_rooms as u64, Ordering::Relaxed);
                    status.hrtf.store(stats.hrtf != 0, Ordering::Relaxed);
                    status
                        .callback_underfeed_samples
                        .store(stats.callback_underfeed_samples, Ordering::Relaxed);
                    status
                        .callback_dropped_samples
                        .store(stats.callback_dropped_samples, Ordering::Relaxed);
                    status
                        .transport
                        .store(u64::from(stats.transport), Ordering::Relaxed);
                    status
                        .hrtf_status
                        .store(u64::from(stats.hrtf_status), Ordering::Relaxed);
                    status
                        .queue_starves
                        .store(u64::from(stats.queue_starves), Ordering::Relaxed);
                    status
                        .restarts
                        .store(u64::from(stats.restarts), Ordering::Relaxed);
                    status
                        .multi_processed
                        .store(u64::from(stats.multi_processed), Ordering::Relaxed);
                    status
                        .buffer_reuse
                        .store(u64::from(stats.buffer_reuse), Ordering::Relaxed);
                    status
                        .input_edge_bits
                        .store(stats.input_edge_max.to_bits() as usize, Ordering::Relaxed);
                    status
                        .output_edge_bits
                        .store(stats.output_edge_max.to_bits() as usize, Ordering::Relaxed);
                    Ok(output)
                },
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn test_silent() -> Self {
        Self::spawn(|| {
            let mut previous = [0.0; BLOCK_FRAMES * 2];
            Ok(move |job: InputBlock| {
                let mut output = [0.0; BLOCK_FRAMES * 2];
                for voice in &job.frame.voices {
                    if voice.active != 0 {
                        for i in 0..BLOCK_FRAMES {
                            output[i * 2] += voice.pcm[i];
                            output[i * 2 + 1] += voice.pcm[i];
                        }
                    }
                }
                Ok(std::mem::replace(&mut previous, output))
            })
        })
        .unwrap()
    }

    #[cfg(test)]
    fn spawn<F, P>(setup: F) -> Result<Self, String>
    where
        F: FnOnce() -> Result<P, String> + Send + 'static,
        P: FnMut(InputBlock) -> Result<[f32; BLOCK_FRAMES * 2], String> + 'static,
    {
        Self::spawn_with_status(move |_| setup())
    }

    fn spawn_with_status<F, P>(setup: F) -> Result<Self, String>
    where
        F: FnOnce(Arc<WorkerStatus>) -> Result<P, String> + Send + 'static,
        P: FnMut(InputBlock) -> Result<[f32; BLOCK_FRAMES * 2], String> + 'static,
    {
        let (input, mut requests) = ring::<InputBlock>();
        let (mut replies, output) = ring::<OutputBlock>();
        let status = Arc::new(WorkerStatus::default());
        let worker_status = Arc::clone(&status);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (ready, initialized) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("eal-effects".into())
            .stack_size(4 * 1024 * 1024)
            .spawn(move || {
                // P may contain !Send OpenAL handles; it is constructed here and
                // never crosses a thread boundary, including during cleanup.
                let mut process = match setup(Arc::clone(&worker_status)) {
                    Ok(process) => process,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                worker_status.running.store(true, Ordering::Release);
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while !worker_stop.load(Ordering::Acquire) {
                    let Some(job) = requests.pop() else {
                        thread::park();
                        continue;
                    };
                    let started = Instant::now();
                    let samples = match process(job) {
                        Ok(samples) => samples,
                        Err(error) => {
                            worker_status.fail(error);
                            break;
                        }
                    };
                    worker_status
                        .max_render_us
                        .fetch_max(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                    if !replies.push(OutputBlock {
                        sequence: job.sequence,
                        samples,
                    }) {
                        worker_status.overflows.fetch_add(1, Ordering::Relaxed);
                    }
                }
                worker_status.running.store(false, Ordering::Release);
            })
            .map_err(|e| format!("cannot start EAL effects worker: {e}"))?;
        let thread = handle.thread().clone();
        match initialized.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                input,
                output,
                pending: [None; QUEUE_BLOCKS],
                sequence: 0,
                stop,
                thread,
                status,
            }),
            Ok(Err(error)) => {
                stop.store(true, Ordering::Release);
                thread.unpark();
                Err(error)
            }
            Err(error) => {
                stop.store(true, Ordering::Release);
                thread.unpark();
                Err(format!("EAL worker initialization failed: {error}"))
            }
        }
    }

    pub fn process_scene(
        &mut self,
        frame: SceneFrame,
        enabled: bool,
        output: &mut [f32; BLOCK_FRAMES * 2],
        offline: bool,
    ) -> bool {
        self.exchange(
            InputBlock {
                sequence: 0,
                #[cfg(test)]
                samples: [0.0; BLOCK_FRAMES],
                enabled,
                frame,
            },
            output,
            offline,
        )
    }

    #[cfg(test)]
    pub fn process(
        &mut self,
        samples: [f32; BLOCK_FRAMES],
        _environment: i32,
        enabled: bool,
        output: &mut [f32; BLOCK_FRAMES * 2],
        offline: bool,
    ) -> bool {
        let mut frame = SceneFrame::silent();
        frame.voices[0].active = 1;
        frame.voices[0].id = 1;
        frame.voices[0].position = [100.0, 0.0, 0.0];
        frame.voices[0].pcm = samples;
        self.exchange(
            InputBlock {
                sequence: 0,
                samples,
                enabled,
                frame,
            },
            output,
            offline,
        )
    }

    fn exchange(
        &mut self,
        mut input: InputBlock,
        output: &mut [f32; BLOCK_FRAMES * 2],
        offline: bool,
    ) -> bool {
        let enabled = input.enabled;
        output.fill(0.0);
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.receive(sequence);
        if self.status.running.load(Ordering::Acquire) {
            input.sequence = sequence;
            if !self.input.push(input) {
                self.status.overflows.fetch_add(1, Ordering::Relaxed);
            } else {
                // Wake without waiting for the worker; no timer-resolution
                // assumptions are needed for Windows playback deadlines.
                self.thread.unpark();
            }
        }
        // Offline export advances its software mixer faster than wall time.
        // Wait only in that mode; physical playback always takes the path below.
        if offline {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.status.running.load(Ordering::Acquire)
                && !self.pending[sequence as usize % QUEUE_BLOCKS]
                    .as_ref()
                    .is_some_and(|block| block.sequence == sequence)
            {
                self.receive(sequence);
                if Instant::now() >= deadline {
                    self.status.fail("EAL offline renderer timed out".into());
                    break;
                }
                thread::park_timeout(Duration::from_millis(1));
            }
        }
        if sequence >= LATENCY_BLOCKS && self.status.running.load(Ordering::Acquire) {
            let wanted = sequence - LATENCY_BLOCKS;
            let slot = &mut self.pending[wanted as usize % QUEUE_BLOCKS];
            if slot.as_ref().is_some_and(|block| block.sequence == wanted) {
                *output = slot.take().unwrap().samples;
                return true;
            } else if enabled {
                self.status.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        sequence < LATENCY_BLOCKS || !enabled
    }
    fn receive(&mut self, sequence: u64) {
        for _ in 0..QUEUE_BLOCKS {
            let Some(reply) = self.output.pop() else {
                break;
            };
            if reply.sequence + LATENCY_BLOCKS >= sequence {
                self.pending[reply.sequence as usize % QUEUE_BLOCKS] = Some(reply);
            }
        }
    }
}
impl Drop for EalWorker {
    fn drop(&mut self) {
        // No join or native destruction in the audio callback. The worker owns
        // all native handles and releases them after observing this flag.
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_bounds_and_wraparound_preserve_order() {
        let (mut producer, mut consumer) = ring::<usize>();
        for cycle in 0..100 {
            for i in 0..QUEUE_BLOCKS {
                assert!(producer.push(cycle * QUEUE_BLOCKS + i));
            }
            assert!(!producer.push(999));
            for i in 0..QUEUE_BLOCKS {
                assert_eq!(consumer.pop(), Some(cycle * QUEUE_BLOCKS + i));
            }
            assert_eq!(consumer.pop(), None);
        }
    }
    #[test]
    fn ring_exchanges_blocks_across_threads() {
        let (mut producer, mut consumer) = ring::<usize>();
        let worker = thread::spawn(move || {
            for value in 0..100_000 {
                while !producer.push(value) {
                    thread::yield_now();
                }
            }
        });
        for value in 0..100_000 {
            loop {
                if let Some(got) = consumer.pop() {
                    assert_eq!(got, value);
                    break;
                }
                thread::yield_now();
            }
        }
        worker.join().unwrap();
    }
    #[test]
    fn slow_renderer_never_blocks_callback_and_does_not_replay_late_audio() {
        let mut worker = EalWorker::spawn(|| {
            Ok(|job: InputBlock| {
                thread::sleep(Duration::from_millis(100));
                Ok([job.sequence as f32 + 1.0; BLOCK_FRAMES * 2])
            })
        })
        .unwrap();
        let mut output = [0.0; BLOCK_FRAMES * 2];
        let started = Instant::now();
        for _ in 0..20 {
            worker.process([0.0; BLOCK_FRAMES], -1, true, &mut output, false);
        }
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(worker.status.underruns.load(Ordering::Relaxed) > 0);
        thread::sleep(Duration::from_millis(120));
        worker.process([0.0; BLOCK_FRAMES], -1, true, &mut output, false);
        assert!(output.iter().all(|&v| v == 0.0));
    }
    #[test]
    fn offline_render_preserves_fixed_latency_without_scheduler_underruns() {
        let mut worker =
            EalWorker::spawn(|| Ok(|job: InputBlock| Ok([job.samples[0]; BLOCK_FRAMES * 2])))
                .unwrap();
        let mut output = [0.0; BLOCK_FRAMES * 2];
        for sequence in 0..20 {
            worker.process(
                [sequence as f32 + 1.0; BLOCK_FRAMES],
                -1,
                true,
                &mut output,
                true,
            );
            let expected = if sequence < LATENCY_BLOCKS {
                0.0
            } else {
                (sequence - LATENCY_BLOCKS) as f32 + 1.0
            };
            assert!(output.iter().all(|&v| v == expected));
        }
        assert_eq!(worker.status.underruns.load(Ordering::Relaxed), 0);
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires EAL_TEST_OPENAL_DLL and EAL_TEST_MAP"]
    fn native_worker_handles_room_changes_and_live_deadlines() {
        let path = std::env::var_os("EAL_TEST_MAP").expect("EAL_TEST_MAP");
        let map = Arc::new(EalMap::decode(&std::fs::read(path).unwrap()).unwrap());
        let mut worker = EalWorker::new(48000, map).unwrap();
        let mut output = [0.0; BLOCK_FRAMES * 2];
        let mut energy = 0.0f64;
        for sequence in 0..160 {
            let samples = std::array::from_fn(|i| {
                ((sequence * BLOCK_FRAMES + i) as f32 * 440.0 * std::f32::consts::TAU / 48000.0)
                    .sin()
                    * 0.05
            });
            worker.process(
                samples,
                if sequence / 32 % 2 == 0 { -1 } else { 0 },
                !(100..120).contains(&sequence),
                &mut output,
                true,
            );
            assert!(output.iter().all(|value| value.is_finite()));
            energy += output.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
        }
        assert!(energy > 0.000001);
        assert_eq!(worker.status.underruns.load(Ordering::Relaxed), 0);
        assert!(worker.status.error().is_none());
        let mut max_callback_us = 0;
        for _ in 0..80 {
            let started = Instant::now();
            worker.process([0.01; BLOCK_FRAMES], 0, true, &mut output, false);
            max_callback_us = max_callback_us.max(started.elapsed().as_micros());
            thread::sleep(Duration::from_micros(5333));
        }
        println!("Native EAL worker: max callback {max_callback_us} us, max render {} us, {} underruns, {} dropped blocks",
            worker.status.max_render_us.load(Ordering::Relaxed), worker.status.underruns.load(Ordering::Relaxed),
            worker.status.overflows.load(Ordering::Relaxed));
        assert!(worker.status.error().is_none());
        assert_eq!(worker.status.overflows.load(Ordering::Relaxed), 0);
    }
}
