use crate::thread_activity::{self, Task, ThreadSlot};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};

const TASK_COUNT: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct MapJobProgress {
    pub request_id: u64,
    pub task: Task,
    pub completed: u32,
    pub total: u32,
}

type ProgressReporter = Arc<dyn Fn(MapJobProgress) + Send + Sync + 'static>;

struct ProgressState {
    request_id: AtomicU64,
    completed: [AtomicU32; TASK_COUNT],
    total: [AtomicU32; TASK_COUNT],
    reporter: Option<ProgressReporter>,
}

impl ProgressState {
    fn new(reporter: Option<ProgressReporter>) -> Self {
        Self {
            request_id: AtomicU64::new(0),
            completed: std::array::from_fn(|_| AtomicU32::new(0)),
            total: std::array::from_fn(|_| AtomicU32::new(0)),
            reporter,
        }
    }

    fn begin_request(&self, request_id: u64) {
        self.request_id.store(request_id, Ordering::Release);
        for value in &self.completed {
            value.store(0, Ordering::Relaxed);
        }
        for value in &self.total {
            value.store(0, Ordering::Relaxed);
        }
    }

    fn queued(&self, task: Task) -> u64 {
        let request_id = self.request_id.load(Ordering::Acquire);
        let index = task as usize;
        let total = self.total[index].fetch_add(1, Ordering::Relaxed) + 1;
        let completed = self.completed[index].load(Ordering::Relaxed);
        self.report(request_id, task, completed, total);
        request_id
    }

    fn begin_progressive(&self, task: Task, total_units: u32) -> u64 {
        let request_id = self.request_id.load(Ordering::Acquire);
        let index = task as usize;
        let total_units = total_units.max(1);
        self.completed[index].store(0, Ordering::Relaxed);
        self.total[index].store(total_units, Ordering::Relaxed);
        self.report(request_id, task, 0, total_units);
        request_id
    }

    fn set_progressive(&self, request_id: u64, task: Task, completed_units: u32) {
        if self.request_id.load(Ordering::Acquire) != request_id {
            return;
        }
        let index = task as usize;
        let total = self.total[index].load(Ordering::Relaxed).max(1);
        let completed = completed_units.min(total);
        self.completed[index].store(completed, Ordering::Relaxed);
        self.report(request_id, task, completed, total);
    }

    fn finish_progressive(&self, request_id: u64, task: Task) {
        if self.request_id.load(Ordering::Acquire) != request_id {
            return;
        }
        let index = task as usize;
        let total = self.total[index].load(Ordering::Relaxed).max(1);
        self.completed[index].store(total, Ordering::Relaxed);
        self.report(request_id, task, total, total);
    }

    fn finished(&self, request_id: u64, task: Task) {
        if self.request_id.load(Ordering::Acquire) != request_id {
            return;
        }
        let index = task as usize;
        let completed = self.completed[index].fetch_add(1, Ordering::Relaxed) + 1;
        let total = self.total[index].load(Ordering::Relaxed);
        self.report(request_id, task, completed, total);
    }

    fn report(&self, request_id: u64, task: Task, completed: u32, total: u32) {
        let Some(reporter) = &self.reporter else {
            return;
        };
        reporter(MapJobProgress {
            request_id,
            task,
            completed,
            total,
        });
    }
}

struct Job {
    task: Task,
    run: Box<dyn FnOnce() + Send + 'static>,
}

pub struct MapJobPool {
    sender: Option<mpsc::Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
    worker_count: usize,
    progress: Arc<ProgressState>,
}

pub struct JobHandle<T> {
    receiver: mpsc::Receiver<Result<T, String>>,
}

#[derive(Clone)]
pub struct JobProgress {
    request_id: u64,
    task: Task,
    total_units: u32,
    state: Arc<ProgressState>,
}

impl JobProgress {
    pub fn set_units(&self, completed_units: u32) {
        self.state
            .set_progressive(self.request_id, self.task, completed_units.min(self.total_units));
    }

    pub fn set_fraction(&self, fraction: f32) {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.set_units((fraction * self.total_units as f32).round() as u32);
    }
}

impl<T> JobHandle<T> {
    pub fn join(self) -> Result<T, String> {
        self.receiver
            .recv()
            .map_err(|_| "map worker job channel disconnected".to_string())?
    }
}

impl MapJobPool {
    pub fn new_with_progress<F>(reporter: F) -> Result<Self, String>
    where
        F: Fn(MapJobProgress) + Send + Sync + 'static,
    {
        Self::create(Some(Arc::new(reporter)))
    }

    fn create(reporter: Option<ProgressReporter>) -> Result<Self, String> {
        let logical = thread::available_parallelism().map_or(1, |count| count.get());
        // Keep MAIN and RENDER responsive, then use the remaining hardware
        // parallelism for CPU-heavy map preparation. The map-loader coordinator
        // spends most of its time waiting on workers, so it does not need a
        // dedicated reserved hardware thread. Cap at eight so a large machine
        // cannot stampede PK3 IO or create excessive transient work.
        let worker_count = logical.saturating_sub(2).clamp(1, 8);
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let progress = Arc::new(ProgressState::new(reporter));
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let slot = ThreadSlot::worker(index).ok_or("too many map workers")?;
            let handle = thread::Builder::new()
                .name(format!("jka-worker-{index}"))
                .spawn(move || loop {
                    let job = {
                        let Ok(receiver) = receiver.lock() else {
                            return;
                        };
                        match receiver.recv() {
                            Ok(job) => job,
                            Err(_) => return,
                        }
                    };
                    let _activity = thread_activity::activity(slot, job.task);
                    (job.run)();
                })
                .map_err(|error| format!("Could not start map worker {index}: {error}"))?;
            workers.push(handle);
        }
        Ok(Self {
            sender: Some(sender),
            workers,
            worker_count,
            progress,
        })
    }

    pub fn begin_request(&self, request_id: u64) {
        self.progress.begin_request(request_id);
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn submit<T, F>(&self, task: Task, job: F) -> Result<JobHandle<T>, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let request_id = self.progress.queued(task);
        let (tx, rx) = mpsc::channel();
        let progress = Arc::clone(&self.progress);
        let run = Box::new(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(job))
                .map_err(|_| format!("map worker panicked while running {}", task.label()));
            progress.finished(request_id, task);
            let _ = tx.send(result);
        });
        self.sender
            .as_ref()
            .ok_or("map worker pool is shutting down")?
            .send(Job { task, run })
            .map_err(|_| "map worker pool disconnected".to_string())?;
        Ok(JobHandle { receiver: rx })
    }

    /// Submit a single long-running task that can expose fractional progress in
    /// the normal map loading UI. This is intentionally separate from `submit`:
    /// ordinary tasks report job counts, while heavyweight bakes report units.
    pub fn submit_progressive<T, F>(
        &self,
        task: Task,
        total_units: u32,
        job: F,
    ) -> Result<JobHandle<T>, String>
    where
        T: Send + 'static,
        F: FnOnce(JobProgress) -> T + Send + 'static,
    {
        let total_units = total_units.max(1);
        let request_id = self.progress.begin_progressive(task, total_units);
        let (tx, rx) = mpsc::channel();
        let state = Arc::clone(&self.progress);
        let job_progress = JobProgress {
            request_id,
            task,
            total_units,
            state: Arc::clone(&state),
        };
        let run = Box::new(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| job(job_progress)))
                .map_err(|_| format!("map worker panicked while running {}", task.label()));
            state.finish_progressive(request_id, task);
            let _ = tx.send(result);
        });
        self.sender
            .as_ref()
            .ok_or("map worker pool is shutting down")?
            .send(Job { task, run })
            .map_err(|_| "map worker pool disconnected".to_string())?;
        Ok(JobHandle { receiver: rx })
    }
}

impl Drop for MapJobPool {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
