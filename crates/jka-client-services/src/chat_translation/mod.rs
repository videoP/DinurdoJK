//! Client-side chat translation: settings, language detection, decoding,
//! providers and the background request queue. The app keeps the messages;
//! this module only turns (text, source, target) into translations.
//!
//! Nothing here runs until the first request: no thread, no network.
//! Requests go to a small pool of persistent worker threads through a bounded
//! queue; results come back over a channel the app drains once per frame.

pub mod lang;
pub mod provider;
pub mod text;
mod http;

pub use provider::{ProviderConfig, ProviderKind, Request, Translated};
pub use text::Codepage;

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    /// Only messages the user asks for, from the console.
    Manual,
    /// Every message detected as another language, plus manual requests.
    Auto,
}

impl Mode {
    pub fn from_index(value: i32) -> Self {
        match value {
            1 => Self::Manual,
            2 => Self::Auto,
            _ => Self::Off,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Display {
    /// The translation replaces the message body; the original stays one
    /// click away in the console.
    #[default]
    Translated,
    /// Original line, then the translation under it.
    Both,
    /// Translations are only shown on request in the console.
    Original,
}

impl Display {
    pub fn from_index(value: i32) -> Self {
        match value {
            1 => Self::Both,
            2 => Self::Original,
            _ => Self::Translated,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    /// ISO 639-1 target; empty follows the Windows display language.
    pub target: String,
    pub display: Display,
    pub provider: ProviderKind,
    pub libre_url: String,
    /// Translate team chat automatically too.
    pub team: bool,
    /// Translate chat automatically during demo playback.
    pub demos: bool,
    pub codepage: Codepage,
    /// Longest message body sent to the service, in characters.
    pub max_chars: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            target: String::new(),
            display: Display::Translated,
            provider: ProviderKind::MyMemory,
            libre_url: String::new(),
            team: false,
            demos: false,
            codepage: Codepage::Auto,
            max_chars: 300,
        }
    }
}

/// What the cache and in-flight dedupe are keyed on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub provider: ProviderKind,
    pub request: Request,
}

struct Job {
    key: Key,
    generation: u64,
    config: Arc<ProviderConfig>,
}

struct JobResult {
    key: Key,
    generation: u64,
    result: Result<Translated, String>,
}

struct Workers {
    jobs: SyncSender<Job>,
    results: Receiver<JobResult>,
}

const WORKERS: usize = 2;
const QUEUE: usize = 24;
const CACHE: usize = 256;

#[derive(Debug, Default, Clone)]
pub struct Status {
    pub sent: u32,
    pub translated: u32,
    pub failed: u32,
    pub cached: u32,
    pub last_error: Option<String>,
}

pub enum Submit {
    Cached(Translated),
    /// Queued, or joined an identical request already in flight.
    Pending,
    Busy,
}

#[derive(Default)]
pub struct Translator {
    workers: Option<Workers>,
    generation: Arc<AtomicU64>,
    cache: HashMap<Key, (Translated, u64)>,
    cache_tick: u64,
    /// In-flight requests and the message ids waiting on each.
    pending: HashMap<Key, Vec<u64>>,
    pub status: Status,
}

fn spawn_workers(generation: Arc<AtomicU64>) -> Workers {
    let (job_tx, job_rx) = mpsc::sync_channel::<Job>(QUEUE);
    let (result_tx, result_rx) = mpsc::channel();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let next_slot = Arc::new(Mutex::new(Instant::now()));
    for index in 0..WORKERS {
        let job_rx = Arc::clone(&job_rx);
        let result_tx = result_tx.clone();
        let next_slot = Arc::clone(&next_slot);
        let generation = Arc::clone(&generation);
        // Detached on purpose: shutdown never waits on an HTTP timeout. The
        // threads exit once the app drops its end of the job queue.
        let _ = std::thread::Builder::new().name(format!("chat-translate-{index}")).spawn(move || loop {
            let job = match job_rx.lock() {
                Ok(rx) => match rx.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                },
                Err(_) => return,
            };
            let result = if job.generation != generation.load(Ordering::Relaxed) {
                Err(String::new())
            } else {
                // Space requests out across both workers (provider rate limits).
                let wait = {
                    let Ok(mut slot) = next_slot.lock() else { return };
                    let now = Instant::now();
                    let start = (*slot).max(now);
                    *slot = start + Duration::from_millis(job.key.provider.min_interval_ms());
                    start - now
                };
                std::thread::sleep(wait);
                provider::translate(&job.config, &job.key.request)
            };
            if result_tx.send(JobResult { key: job.key, generation: job.generation, result }).is_err() {
                return;
            }
        });
    }
    Workers { jobs: job_tx, results: result_rx }
}

impl Translator {
    pub fn submit(&mut self, key: Key, message: u64, config: &Arc<ProviderConfig>) -> Submit {
        if let Some((translated, used)) = self.cache.get_mut(&key) {
            self.cache_tick += 1;
            *used = self.cache_tick;
            self.status.cached += 1;
            return Submit::Cached(translated.clone());
        }
        if let Some(waiting) = self.pending.get_mut(&key) {
            waiting.push(message);
            return Submit::Pending;
        }
        let generation = self.generation.load(Ordering::Relaxed);
        let workers = self.workers.get_or_insert_with(|| spawn_workers(Arc::clone(&self.generation)));
        let job = Job { key: key.clone(), generation, config: Arc::clone(config) };
        match workers.jobs.try_send(job) {
            Ok(()) => {
                self.status.sent += 1;
                self.pending.insert(key, vec![message]);
                Submit::Pending
            }
            Err(TrySendError::Full(_)) => Submit::Busy,
            Err(TrySendError::Disconnected(_)) => {
                self.workers = None;
                Submit::Busy
            }
        }
    }

    /// Finished requests as (waiting message ids, result). Cheap when idle.
    pub fn poll(&mut self) -> Vec<(Vec<u64>, Result<Translated, String>)> {
        let Some(workers) = &self.workers else { return Vec::new() };
        let mut done = Vec::new();
        while let Ok(job) = workers.results.try_recv() {
            let current = job.generation == self.generation.load(Ordering::Relaxed);
            let Some(waiting) = self.pending.remove(&job.key).filter(|_| current) else { continue };
            match &job.result {
                Ok(translated) => {
                    self.status.translated += 1;
                    self.cache_tick += 1;
                    if self.cache.len() >= CACHE {
                        if let Some(oldest) = self.cache.iter().min_by_key(|(_, (_, used))| *used).map(|(key, _)| key.clone()) {
                            self.cache.remove(&oldest);
                        }
                    }
                    self.cache.insert(job.key, (translated.clone(), self.cache_tick));
                }
                Err(error) => {
                    self.status.failed += 1;
                    self.status.last_error = Some(error.clone());
                }
            }
            done.push((waiting, job.result));
        }
        done
    }

    /// Forget queued and in-flight work (disconnect, demo seek, settings
    /// change). Workers skip stale jobs without contacting the service.
    /// Returns the message ids that were waiting.
    pub fn cancel_all(&mut self) -> Vec<u64> {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending.drain().flat_map(|(_, ids)| ids).collect()
    }

    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    pub fn active(&self) -> bool {
        self.workers.is_some()
    }
}
