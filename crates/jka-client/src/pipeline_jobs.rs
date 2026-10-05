//! Shared asynchronous GPU pipeline/shader compiler.
//!
//! Runtime-lazy GPU compilation must never call `Device::create_render_pipeline` on the
//! render thread.  Callers identify a logical pipeline slot (`family`, `slot`) and
//! a configuration fingerprint.  A request supersedes any older configuration for
//! the same slot, which makes scene rebuilds/vid_restart-style format changes safe:
//! stale worker completions are simply discarded.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    panic::{self, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineJobKey {
    pub family: &'static str,
    pub slot: u32,
    pub config: u64,
}

impl PipelineJobKey {
    pub const fn new(family: &'static str, slot: u32, config: u64) -> Self {
        Self { family, slot, config }
    }
}

pub(crate) fn hash_config<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

type PipelineJobValue = Box<dyn std::any::Any + Send>;
type BuildPipeline = Box<dyn FnOnce() -> PipelineJobValue + Send + 'static>;

enum WorkMessage {
    Compile {
        id: u64,
        key: PipelineJobKey,
        label: &'static str,
        build: BuildPipeline,
    },
    Shutdown,
}

struct CompletedJob {
    id: u64,
    key: PipelineJobKey,
    label: &'static str,
    elapsed: Duration,
    result: Result<PipelineJobValue, String>,
}

enum JobState {
    Compiling { id: u64 },
    Ready(PipelineJobValue),
    Failed,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PipelineJobStats {
    pub queued: u64,
    pub completed: u64,
    pub stale: u64,
    pub failed: u64,
    pub compiling: usize,
    pub ready: usize,
    pub slowest_ms: f64,
}

/// Bounded persistent worker pool for gameplay-lazy render pipelines.
///
/// The queue is intentionally non-blocking from the render thread.  Worker count
/// defaults to two: enough to overlap independent driver compiles without turning
/// a first-sighting event into a whole-machine CPU spike at very high frame rates.
pub(crate) struct PipelineJobManager {
    work_tx: mpsc::Sender<WorkMessage>,
    done_rx: mpsc::Receiver<CompletedJob>,
    workers: Vec<JoinHandle<()>>,
    states: HashMap<PipelineJobKey, JobState>,
    next_id: u64,
    queued: u64,
    completed: u64,
    stale: u64,
    failed: u64,
    slowest_ms: f64,
    stopping: Arc<AtomicBool>,
}

impl PipelineJobManager {
    pub fn new() -> Self {
        let (work_tx, work_rx) = mpsc::channel::<WorkMessage>();
        let (done_tx, done_rx) = mpsc::channel::<CompletedJob>();
        let work_rx = Arc::new(Mutex::new(work_rx));
        let stopping = Arc::new(AtomicBool::new(false));
        let available = thread::available_parallelism().map_or(2, |count| count.get());
        let worker_count = available.saturating_sub(1).clamp(1, 2);
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let work_rx = Arc::clone(&work_rx);
            let done_tx = done_tx.clone();
            let stopping_worker = Arc::clone(&stopping);
            let name = format!("pipeline-compile-{}", index + 1);
            if let Ok(handle) = thread::Builder::new().name(name).spawn(move || loop {
                let message = {
                    let receiver = match work_rx.lock() {
                        Ok(receiver) => receiver,
                        Err(_) => break,
                    };
                    receiver.recv()
                };
                let Ok(message) = message else { break };
                match message {
                    WorkMessage::Shutdown => break,
                    WorkMessage::Compile { id, key, label, build } => {
                        // Renderer teardown/vid_restart should not grind through jobs that
                        // were only queued speculatively. An in-flight driver compile cannot
                        // be cancelled safely, but queued work can be dropped immediately.
                        if stopping_worker.load(Ordering::Acquire) {
                            continue;
                        }
                        let started = Instant::now();
                        let result = panic::catch_unwind(AssertUnwindSafe(build))
                            .map_err(panic_message);
                        let _ = done_tx.send(CompletedJob {
                            id,
                            key,
                            label,
                            elapsed: started.elapsed(),
                            result,
                        });
                    }
                }
            }) {
                workers.push(handle);
            }
        }
        Self {
            work_tx,
            done_rx,
            workers,
            states: HashMap::new(),
            next_id: 1,
            queued: 0,
            completed: 0,
            stale: 0,
            failed: 0,
            slowest_ms: 0.0,
            stopping,
        }
    }

    /// Queue a compile if this exact pipeline configuration is neither compiling
    /// nor ready.  Any older configuration for the same logical family+slot is
    /// invalidated immediately; a late completion from it cannot be installed.
    pub fn request<T: Send + 'static>(
        &mut self,
        key: PipelineJobKey,
        label: &'static str,
        build: impl FnOnce() -> T + Send + 'static,
    ) -> bool {
        self.poll();
        self.states.retain(|other, _| {
            other.family != key.family || other.slot != key.slot || *other == key
        });
        if self.states.contains_key(&key) {
            return false;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.states.insert(key, JobState::Compiling { id });
        if self
            .work_tx
            .send(WorkMessage::Compile {
                id,
                key,
                label,
                build: Box::new(move || Box::new(build()) as PipelineJobValue),
            })
            .is_ok()
        {
            self.queued = self.queued.saturating_add(1);
            true
        } else {
            self.states.insert(key, JobState::Failed);
            self.failed = self.failed.saturating_add(1);
            false
        }
    }

    /// Returns a finished pipeline without ever waiting for a worker.
    pub fn take_ready<T: Send + 'static>(&mut self, key: PipelineJobKey) -> Option<T> {
        self.poll();
        if !matches!(self.states.get(&key), Some(JobState::Ready(_))) {
            return None;
        }
        match self.states.remove(&key) {
            Some(JobState::Ready(value)) => match value.downcast::<T>() {
                Ok(value) => Some(*value),
                Err(_) => {
                    self.failed = self.failed.saturating_add(1);
                    eprintln!("Renderer pipeline job type mismatch for {}:{}", key.family, key.slot);
                    None
                }
            },
            _ => None,
        }
    }

    pub fn stats(&mut self) -> PipelineJobStats {
        self.poll();
        let mut compiling = 0;
        let mut ready = 0;
        for state in self.states.values() {
            match state {
                JobState::Compiling { .. } => compiling += 1,
                JobState::Ready(_) => ready += 1,
                JobState::Failed => {}
            }
        }
        PipelineJobStats {
            queued: self.queued,
            completed: self.completed,
            stale: self.stale,
            failed: self.failed,
            compiling,
            ready,
            slowest_ms: self.slowest_ms,
        }
    }

    fn poll(&mut self) {
        while let Ok(done) = self.done_rx.try_recv() {
            let current = matches!(
                self.states.get(&done.key),
                Some(JobState::Compiling { id }) if *id == done.id
            );
            if !current {
                self.stale = self.stale.saturating_add(1);
                continue;
            }
            let elapsed_ms = done.elapsed.as_secs_f64() * 1000.0;
            self.slowest_ms = self.slowest_ms.max(elapsed_ms);
            self.completed = self.completed.saturating_add(1);
            match done.result {
                Ok(pipeline) => {
                    self.states.insert(done.key, JobState::Ready(pipeline));
                }
                Err(error) => {
                    self.states.insert(done.key, JobState::Failed);
                    self.failed = self.failed.saturating_add(1);
                    eprintln!(
                        "Renderer pipeline job failed: {} ({:.1} ms): {}",
                        done.label, elapsed_ms, error
                    );
                }
            }
        }
    }
}

impl Drop for PipelineJobManager {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        for _ in 0..self.workers.len() {
            let _ = self.work_tx.send(WorkMessage::Shutdown);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "pipeline compiler panicked".to_owned()
    }
}
