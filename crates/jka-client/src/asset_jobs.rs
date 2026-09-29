//! Asynchronous asset registration for runtime cache misses.
//!
//! OpenJK's `R_RegisterModel`/`S_RegisterSound`-style calls read, decode and
//! parse synchronously and therefore hitch the frame that first needs an asset.
//! DinurdoJK keeps the same registration *semantics* (paths, fallbacks, skins)
//! but executes them off-thread:
//!
//! ```text
//! request(key) -> Ready      : cheap map lookup, no channel/lock traffic
//!              -> Pending    : same request already in flight, nothing queued
//!              -> (missing)  : state -> Pending, job queued, returns immediately
//! worker job   -> Completion : drained once per frame by the owner
//! ```
//!
//! Workers only do CPU-side preparation (VFS read, decompress, decode, parse).
//! The owner (presenter/renderer/audio) integrates the prepared result, so GPU
//! and cache ownership stay single-threaded. The pool is deliberately tiny and
//! separate from the latency-sensitive Ghoul2 skinning pool and from the
//! map-load pool, whose barrier semantics are different.

use crate::thread_activity::{self, Task, ThreadSlot};
use jka_assets::pk3::AssetSearchPath;
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
        mpsc, Arc, Condvar, Mutex, OnceLock, Weak,
    },
    thread,
    time::Instant,
};

/// Bound on queued (not yet running) jobs. A broken or hostile server cannot
/// make the client queue unbounded distinct assets; a rejected request simply
/// stays "missing" and is retried by the consumer's next lookup.
const MAX_QUEUED_JOBS: usize = 256;
const MAX_ASSET_WORKERS: usize = 2;

static ASYNC_ENABLED: AtomicBool = AtomicBool::new(true);

/// Developer A/B switch (`cg_asyncAssets`). When off, consumers keep the
/// original synchronous registration path.
pub fn async_enabled() -> bool {
    ASYNC_ENABLED.load(AtomicOrdering::Relaxed)
}

pub fn set_async_enabled(enabled: bool) {
    ASYNC_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)] // Low/Medium are reserved for prefetch and menu work.
pub enum AssetPriority {
    /// Menu thumbnails, prefetch, future-map material.
    Low = 0,
    /// Likely nearby dependencies.
    Medium = 1,
    /// Something visible or audible right now.
    High = 2,
}

struct QueuedJob {
    priority: AssetPriority,
    seq: u64,
    run: Box<dyn FnOnce() + Send + 'static>,
}

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl Eq for QueuedJob {}
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap pops the greatest: highest priority first, then oldest.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

struct PoolShared {
    queue: Mutex<BinaryHeap<QueuedJob>>,
    wake: Condvar,
    next_seq: AtomicU64,
}

pub struct AssetWorkerPool {
    shared: Arc<PoolShared>,
    workers: usize,
}

static POOL: OnceLock<Option<AssetWorkerPool>> = OnceLock::new();

/// Process-wide persistent asset pool (`jka-asset-N`). Sessions come and go;
/// the threads and their per-thread VFS views persist.
pub fn pool() -> Option<&'static AssetWorkerPool> {
    POOL.get_or_init(AssetWorkerPool::start).as_ref()
}

/// True when a request would actually be serviced asynchronously.
pub fn async_available() -> bool {
    async_enabled() && pool().is_some()
}

impl AssetWorkerPool {
    fn start() -> Option<Self> {
        let logical = thread::available_parallelism().map_or(1, |count| count.get());
        // Conservative: MAIN, RENDER, network/prediction and the Ghoul2
        // skinning pool keep priority over PK3 decompression.
        let wanted = if logical >= 6 { MAX_ASSET_WORKERS } else { 1 };
        let shared = Arc::new(PoolShared {
            queue: Mutex::new(BinaryHeap::new()),
            wake: Condvar::new(),
            next_seq: AtomicU64::new(0),
        });
        let mut started = 0;
        for index in 0..wanted {
            let worker_shared = Arc::clone(&shared);
            let slot = ThreadSlot::asset_worker(index);
            let spawned = thread::Builder::new()
                .name(format!("jka-asset-{index}"))
                .spawn(move || worker_loop(&worker_shared, slot));
            match spawned {
                Ok(_) => started += 1,
                Err(error) => eprintln!("[ASSET] could not start jka-asset-{index}: {error}"),
            }
        }
        if started == 0 {
            return None;
        }
        println!("[ASSET] async asset worker pool: {started} thread(s)");
        Some(Self {
            shared,
            workers: started,
        })
    }

    pub fn worker_count(&self) -> usize {
        self.workers
    }

    fn submit(&self, priority: AssetPriority, run: Box<dyn FnOnce() + Send + 'static>) -> bool {
        let seq = self.shared.next_seq.fetch_add(1, AtomicOrdering::Relaxed);
        {
            let Ok(mut queue) = self.shared.queue.lock() else {
                return false;
            };
            if queue.len() >= MAX_QUEUED_JOBS {
                return false;
            }
            queue.push(QueuedJob { priority, seq, run });
        }
        self.shared.wake.notify_one();
        true
    }
}

fn worker_loop(shared: &PoolShared, slot: Option<ThreadSlot>) {
    loop {
        let job = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            loop {
                if let Some(job) = queue.pop() {
                    break job;
                }
                queue = match shared.wake.wait(queue) {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
            }
        };
        let _activity = slot.map(|slot| thread_activity::activity(slot, Task::AssetLoad));
        (job.run)();
    }
}

// ---------------------------------------------------------------------------
// Worker-side VFS

static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

/// Description of a VFS a worker can open independently. `AssetSearchPath`
/// needs `&mut` for reads, so each worker thread lazily opens its own view of
/// the same search directories instead of contending with the owner thread.
#[derive(Debug)]
pub struct AssetSource {
    id: u64,
    dirs: Vec<PathBuf>,
    allow_overrides: bool,
}

impl AssetSource {
    pub fn from_search_path(assets: &AssetSearchPath) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_SOURCE_ID.fetch_add(1, AtomicOrdering::Relaxed),
            dirs: assets.search_dirs().to_vec(),
            allow_overrides: assets.asset_overrides_allowed(),
        })
    }

    /// Same directories under a new identity: worker views re-mount PK3s the
    /// next time they are used (e.g. after a download changed the disk).
    pub fn refreshed(&self) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_SOURCE_ID.fetch_add(1, AtomicOrdering::Relaxed),
            dirs: self.dirs.clone(),
            allow_overrides: self.allow_overrides,
        })
    }
}

thread_local! {
    static WORKER_VFS: RefCell<Option<(u64, AssetSearchPath)>> = const { RefCell::new(None) };
}

/// Run `f` against this worker thread's VFS view for `source`, opening it on
/// first use (or when the source identity changed).
pub fn with_worker_vfs<R>(
    source: &AssetSource,
    f: impl FnOnce(&mut AssetSearchPath) -> R,
) -> Result<R, String> {
    WORKER_VFS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|(id, _)| *id) != Some(source.id) {
            let mut assets = AssetSearchPath::open_search_dirs(&source.dirs)
                .map_err(|error| format!("could not open asset search path: {error}"))?;
            assets.set_allow_asset_overrides(source.allow_overrides);
            *slot = Some((source.id, assets));
        }
        let (_, assets) = slot.as_mut().expect("worker vfs just installed");
        Ok(f(assets))
    })
}

// ---------------------------------------------------------------------------
// Registry

pub enum AssetState<T> {
    Pending,
    Ready(Arc<T>),
    Failed(String),
}

/// Result of [`AssetRegistry::request`].
pub enum Requested<T> {
    Ready(Arc<T>),
    /// Already in flight (deduplicated) or newly queued by this call.
    Pending,
    Failed(String),
    /// Pool unavailable or its queue is full; nothing was recorded.
    Rejected,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AssetStats {
    pub queued: u64,
    pub duplicates_avoided: u64,
    pub completed: u64,
    pub failed: u64,
    pub rejected: u64,
    pub worker_ms: f64,
    pub integrate_ms: f64,
}

struct Completion<T> {
    key: String,
    result: Result<T, String>,
    queue_ms: f64,
    worker_ms: f64,
}

/// Keyed asset states with request deduplication and a terminal failure state.
/// The consumer's *desired* key is separate from this cache: a completion only
/// fills its own key and never overwrites what a consumer currently wants.
pub struct AssetRegistry<T: Send + Sync + 'static> {
    label: &'static str,
    states: HashMap<String, AssetState<T>>,
    tx: mpsc::Sender<Completion<T>>,
    rx: mpsc::Receiver<Completion<T>>,
    pending: usize,
    /// Dropped with the registry; queued jobs check it and skip stale work.
    alive: Arc<()>,
    stats: AssetStats,
}

impl<T: Send + Sync + 'static> AssetRegistry<T> {
    pub fn new(label: &'static str) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            label,
            states: HashMap::new(),
            tx,
            rx,
            pending: 0,
            alive: Arc::new(()),
            stats: AssetStats::default(),
        }
    }

    pub fn state(&self, key: &str) -> Option<&AssetState<T>> {
        self.states.get(key)
    }

    #[cfg(test)]
    pub fn ready(&self, key: &str) -> Option<Arc<T>> {
        match self.states.get(key) {
            Some(AssetState::Ready(asset)) => Some(Arc::clone(asset)),
            _ => None,
        }
    }

    /// Count a request answered from an in-flight job by a caller's own fast
    /// path (which looks the state up first to stay allocation-free).
    pub fn note_duplicate(&mut self) {
        self.stats.duplicates_avoided += 1;
    }

    pub fn pending_count(&self) -> usize {
        self.pending
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stats(&self) -> AssetStats {
        self.stats
    }

    /// Ensure `key` is loaded or loading. `job` runs on an asset worker and
    /// must be side-effect free apart from producing the prepared asset.
    pub fn request<F>(
        &mut self,
        key: &str,
        priority: AssetPriority,
        job: F,
    ) -> Requested<T>
    where
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        match self.states.get(key) {
            Some(AssetState::Ready(asset)) => return Requested::Ready(Arc::clone(asset)),
            Some(AssetState::Failed(error)) => return Requested::Failed(error.clone()),
            Some(AssetState::Pending) => {
                self.stats.duplicates_avoided += 1;
                return Requested::Pending;
            }
            None => {}
        }
        let Some(pool) = pool() else {
            self.stats.rejected += 1;
            return Requested::Rejected;
        };
        let tx = self.tx.clone();
        let alive = Arc::downgrade(&self.alive);
        let queued_at = Instant::now();
        let job_key = key.to_owned();
        let run = Box::new(move || {
            if Weak::strong_count(&alive) == 0 {
                return; // owner (session) is gone; skip the work
            }
            let started = Instant::now();
            let result = panic::catch_unwind(AssertUnwindSafe(job))
                .unwrap_or_else(|_| Err(format!("asset worker panicked while loading {job_key}")));
            let _ = tx.send(Completion {
                key: job_key,
                result,
                queue_ms: started.duration_since(queued_at).as_secs_f64() * 1000.0,
                worker_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
        });
        if !pool.submit(priority, run) {
            self.stats.rejected += 1;
            return Requested::Rejected;
        }
        self.states.insert(key.to_owned(), AssetState::Pending);
        self.pending += 1;
        self.stats.queued += 1;
        println!("[ASSET] {} request key={key} state=missing -> queued ({priority:?})", self.label);
        Requested::Pending
    }

    /// Integrate up to `budget` finished jobs. Call once per frame; returns
    /// immediately (no channel touch) when nothing is in flight.
    pub fn drain(&mut self, budget: usize) -> usize {
        let mut completed = 0;
        if self.pending == 0 {
            return completed;
        }
        while completed < budget {
            let Ok(done) = self.rx.try_recv() else { break };
            let integrate_started = Instant::now();
            self.pending = self.pending.saturating_sub(1);
            self.stats.completed += 1;
            self.stats.worker_ms += done.worker_ms;
            match done.result {
                Ok(asset) => {
                    self.states
                        .insert(done.key.clone(), AssetState::Ready(Arc::new(asset)));
                    let integrate_ms = integrate_started.elapsed().as_secs_f64() * 1000.0;
                    self.stats.integrate_ms += integrate_ms;
                    println!(
                        "[ASSET] {} ready key={} queue_ms={:.2} worker_ms={:.2} integrate_ms={:.2}",
                        self.label, done.key, done.queue_ms, done.worker_ms, integrate_ms,
                    );
                }
                Err(error) => {
                    self.stats.failed += 1;
                    println!(
                        "[ASSET] {} FAILED key={} worker_ms={:.2}: {error}",
                        self.label, done.key, done.worker_ms,
                    );
                    self.states.insert(done.key.clone(), AssetState::Failed(error));
                }
            }
            completed += 1;
        }
        if completed > 0 && self.pending == 0 {
            let stats = self.stats;
            println!(
                "[ASSET] {} idle: queued={} duplicates_avoided={} completed={} failed={} rejected={} worker_ms={:.1} integrate_ms={:.2}",
                self.label,
                stats.queued,
                stats.duplicates_avoided,
                stats.completed,
                stats.failed,
                stats.rejected,
                stats.worker_ms,
                stats.integrate_ms,
            );
        }
        completed
    }

    /// Forget terminal failures so they may be requested again. Use after an
    /// event that can change what exists (download finished, content mounted),
    /// never per frame.
    pub fn retry_failed(&mut self) -> usize {
        let before = self.states.len();
        self.states
            .retain(|_, state| !matches!(state, AssetState::Failed(_)));
        before - self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait_for<T: Send + Sync + 'static>(registry: &mut AssetRegistry<T>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while registry.pending_count() > 0 {
            registry.drain(64);
            assert!(Instant::now() < deadline, "asset jobs did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn duplicate_requests_share_one_job() {
        let runs = Arc::new(AtomicU64::new(0));
        let mut registry = AssetRegistry::<u32>::new("test");
        for _ in 0..10 {
            let runs = Arc::clone(&runs);
            registry.request("a", AssetPriority::High, move || {
                runs.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(7)
            });
        }
        assert_eq!(registry.stats().queued, 1);
        assert_eq!(registry.stats().duplicates_avoided, 9);
        wait_for(&mut registry);
        assert_eq!(runs.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(registry.ready("a").as_deref(), Some(&7));
    }

    #[test]
    fn failures_are_terminal_until_retry() {
        let mut registry = AssetRegistry::<u32>::new("test");
        registry.request("bad", AssetPriority::High, || Err("missing".to_owned()));
        wait_for(&mut registry);
        let runs = Arc::new(AtomicU64::new(0));
        for _ in 0..5 {
            let runs = Arc::clone(&runs);
            assert!(matches!(
                registry.request("bad", AssetPriority::High, move || {
                    runs.fetch_add(1, AtomicOrdering::SeqCst);
                    Ok(1)
                }),
                Requested::Failed(_)
            ));
        }
        assert_eq!(runs.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(registry.retry_failed(), 1);
        registry.request("bad", AssetPriority::High, || Ok(3));
        wait_for(&mut registry);
        assert_eq!(registry.ready("bad").as_deref(), Some(&3));
    }

    #[test]
    fn worker_panic_becomes_failed_state() {
        let mut registry = AssetRegistry::<u32>::new("test");
        registry.request("boom", AssetPriority::Low, || panic!("boom"));
        wait_for(&mut registry);
        assert!(matches!(registry.state("boom"), Some(AssetState::Failed(_))));
    }

    #[test]
    fn higher_priority_runs_first() {
        let mut heap = BinaryHeap::new();
        for (priority, seq) in [
            (AssetPriority::Low, 0),
            (AssetPriority::High, 1),
            (AssetPriority::High, 2),
            (AssetPriority::Medium, 3),
        ] {
            heap.push(QueuedJob {
                priority,
                seq,
                run: Box::new(|| {}),
            });
        }
        let order = std::iter::from_fn(|| heap.pop().map(|job| job.seq)).collect::<Vec<_>>();
        assert_eq!(order, vec![1, 2, 3, 0]);
    }
}
