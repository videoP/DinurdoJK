//! The model host: one inference thread owning every loaded model, plus one
//! short-lived thread per download. Nothing starts until a model is asked for.
//!
//! Models are keyed by their address. Two features pointed at the same address
//! share one copy in memory. A loaded model stays loaded until `retain` drops
//! it or the process exits; its file stays in the cache directory.

use crate::download::{self, ModelUrl, Progress};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub enum ModelState {
    /// Not asked for yet.
    Idle,
    Downloading { done: u64, total: u64 },
    Loading,
    Ready,
    Failed(String),
}

impl ModelState {
    /// One line for a settings page or a console answer.
    pub fn describe(&self) -> String {
        match self {
            Self::Idle => "not loaded".to_owned(),
            Self::Downloading { done, total } if *total > 0 => format!(
                "downloading {:.0}% ({:.2} of {:.2} GB)",
                *done as f64 * 100.0 / *total as f64,
                *done as f64 / 1e9,
                *total as f64 / 1e9
            ),
            Self::Downloading { done, .. } => format!("downloading ({:.2} GB so far)", *done as f64 / 1e9),
            Self::Loading => "loading into memory".to_owned(),
            Self::Ready => "ready".to_owned(),
            Self::Failed(error) => format!("failed: {error}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    /// 0 picks the most likely token every time.
    pub temperature: f32,
    pub top_k: i32,
    pub top_p: f32,
    pub repeat_penalty: f32,
}

impl Sampling {
    /// Repeatable output for classification-style jobs.
    pub const EXACT: Self = Self { temperature: 0.0, top_k: 1, top_p: 1.0, repeat_penalty: 1.0 };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Short jobs a chat line is waiting on. They run between the tokens of a
    /// background job instead of queueing behind it.
    Interactive,
    Background,
}

pub struct Generate {
    /// Model address, as given to `ensure`.
    pub url: String,
    pub system: Option<String>,
    pub user: String,
    pub max_tokens: u32,
    /// Smallest context to allocate; grown to fit the prompt when needed.
    pub context: u32,
    pub sampling: Sampling,
    pub priority: Priority,
    /// Set to abandon the job; it then fails with "cancelled".
    pub cancel: Arc<AtomicBool>,
}

struct Entry {
    state: ModelState,
    progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
}

enum Command {
    Load { url: String, path: PathBuf },
    Unload { url: String },
    Run { request: Generate, reply: Sender<Result<String, String>> },
}

struct Shared {
    entries: Mutex<HashMap<String, Entry>>,
    cache_dir: Mutex<Option<PathBuf>>,
    threads: AtomicI32,
}

pub struct Engine {
    shared: Arc<Shared>,
    worker: Mutex<Option<Sender<Command>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The process-wide model host.
pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| Engine {
        shared: Arc::new(Shared {
            entries: Mutex::new(HashMap::new()),
            cache_dir: Mutex::new(None),
            threads: AtomicI32::new(0),
        }),
        worker: Mutex::new(None),
    })
}

/// Inference threads when the user has not chosen: a third of the logical
/// cores, so the game keeps its own. Chat jobs are short; more threads mostly
/// steal frames.
pub fn default_threads() -> i32 {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    (cores / 3).clamp(2, 6) as i32
}

impl Engine {
    /// Whether this build can run models at all.
    pub fn available(&self) -> bool {
        runtime::AVAILABLE
    }

    pub fn set_cache_dir(&self, directory: PathBuf) {
        *lock(&self.shared.cache_dir) = Some(directory);
    }

    pub fn cache_dir(&self) -> Option<PathBuf> {
        lock(&self.shared.cache_dir).clone()
    }

    /// 0 selects `default_threads`.
    pub fn set_threads(&self, threads: i32) {
        self.shared.threads.store(threads.clamp(0, 64), Ordering::Relaxed);
    }

    fn cached_file(&self, model: &ModelUrl) -> Result<PathBuf, String> {
        let directory = self.cache_dir().ok_or("no model cache directory")?;
        Ok(download::cache_path(&directory, model))
    }

    /// Whether the model file for `url` is already on disk.
    pub fn cached(&self, url: &str) -> bool {
        download::parse_url(url).ok().and_then(|model| self.cached_file(&model).ok()).is_some_and(|path| path.is_file())
    }

    /// Automatic feature restoration may load an existing file, but must not
    /// start a download without the user's confirmation.
    pub fn ensure_cached(&self, url: &str) -> bool {
        if !self.cached(url) {
            return false;
        }
        self.ensure(url);
        true
    }

    pub fn state(&self, url: &str) -> ModelState {
        let entries = lock(&self.shared.entries);
        match entries.get(url.trim()) {
            None => ModelState::Idle,
            Some(entry) => match &entry.state {
                ModelState::Downloading { .. } => ModelState::Downloading {
                    done: entry.progress.done.load(Ordering::Relaxed),
                    total: entry.progress.total.load(Ordering::Relaxed),
                },
                other => other.clone(),
            },
        }
    }

    fn send(&self, command: Command) {
        let mut worker = lock(&self.worker);
        let sender = worker.get_or_insert_with(|| spawn_worker(Arc::clone(&self.shared)));
        if let Err(mpsc::SendError(command)) = sender.send(command) {
            // The thread is gone (it only ends on a panic): start a new one.
            let sender = spawn_worker(Arc::clone(&self.shared));
            let _ = sender.send(command);
            *worker = Some(sender);
        }
    }

    /// Download (if needed) and load the model. Returns at once; follow
    /// `state`. A failed model is retried.
    pub fn ensure(&self, url: &str) {
        let url = url.trim().to_owned();
        let mut entries = lock(&self.shared.entries);
        if entries.get(&url).is_some_and(|entry| !matches!(entry.state, ModelState::Failed(_) | ModelState::Idle)) {
            return;
        }
        let mut entry = Entry {
            state: ModelState::Idle,
            progress: Arc::new(Progress::default()),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let fail = |mut entry: Entry, entries: &mut HashMap<String, Entry>, error: String| {
            entry.state = ModelState::Failed(error);
            entries.insert(url.clone(), entry);
        };
        if !runtime::AVAILABLE {
            return fail(entry, &mut entries, runtime::UNAVAILABLE.to_owned());
        }
        let model = match download::parse_url(&url) {
            Ok(model) => model,
            Err(error) => return fail(entry, &mut entries, error),
        };
        let path = match self.cached_file(&model) {
            Ok(path) => path,
            Err(error) => return fail(entry, &mut entries, error),
        };
        if path.is_file() {
            entry.state = ModelState::Loading;
            entries.insert(url.clone(), entry);
            drop(entries);
            self.send(Command::Load { url, path });
            return;
        }
        entry.state = ModelState::Downloading { done: 0, total: 0 };
        let progress = Arc::clone(&entry.progress);
        let cancel = Arc::clone(&entry.cancel);
        entries.insert(url.clone(), entry);
        drop(entries);
        let shared = Arc::clone(&self.shared);
        let thread_url = url.clone();
        let thread_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new().name("chat-llm-download".to_owned()).spawn(move || {
            let (url, cancel) = (thread_url, thread_cancel);
            let result = download::download(&model, &path, &progress, &cancel);
            let mut entries = lock(&shared.entries);
            // `retain` may have dropped or replaced this model meanwhile.
            let Some(entry) = entries.get_mut(&url).filter(|entry| Arc::ptr_eq(&entry.cancel, &cancel)) else { return };
            match result {
                Ok(true) => {
                    entry.state = ModelState::Loading;
                    drop(entries);
                    engine().send(Command::Load { url, path });
                }
                Ok(false) => {
                    entries.remove(&url);
                }
                Err(error) => entry.state = ModelState::Failed(error),
            }
        });
        if let Err(error) = spawned {
            let mut entries = lock(&self.shared.entries);
            if let Some(entry) = entries.get_mut(&url) {
                entry.state = ModelState::Failed(format!("could not start the download: {error}"));
            }
        }
    }

    /// Keep only these models: others are unloaded and their downloads stop.
    pub fn retain(&self, urls: &[&str]) {
        let keep: Vec<&str> = urls.iter().map(|url| url.trim()).collect();
        let dropped: Vec<String> = {
            let mut entries = lock(&self.shared.entries);
            let dropped: Vec<String> = entries.keys().filter(|url| !keep.contains(&url.as_str())).cloned().collect();
            for url in &dropped {
                if let Some(entry) = entries.remove(url) {
                    entry.cancel.store(true, Ordering::Relaxed);
                }
            }
            dropped
        };
        if lock(&self.worker).is_some() {
            for url in dropped {
                self.send(Command::Unload { url });
            }
        }
    }

    /// Unload the model and delete its file from the cache.
    pub fn delete_cached(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        let model = download::parse_url(url)?;
        let path = self.cached_file(&model)?;
        if let Some(entry) = lock(&self.shared.entries).remove(url) {
            entry.cancel.store(true, Ordering::Relaxed);
        }
        if lock(&self.worker).is_some() {
            self.send(Command::Unload { url: url.to_owned() });
        }
        download::remove(&path)
    }

    /// Run one job and wait for its answer. Call from a worker thread, never
    /// from the frame loop. Loading is explicit: stale queued jobs must not
    /// reload a model the user has unloaded.
    pub fn generate(&self, mut request: Generate) -> Result<String, String> {
        request.url = request.url.trim().to_owned();
        match self.state(&request.url) {
            ModelState::Ready => {}
            other => return Err(format!("model {}", other.describe())),
        }
        let (reply, answer) = mpsc::channel();
        self.send(Command::Run { request, reply });
        answer.recv().unwrap_or_else(|_| Err("the model thread stopped".to_owned()))
    }
}

fn spawn_worker(shared: Arc<Shared>) -> Sender<Command> {
    let (sender, receiver) = mpsc::channel();
    // Detached on purpose: shutdown never waits on a generation.
    let _ = std::thread::Builder::new().name("chat-llm".to_owned()).spawn(move || worker(&shared, &receiver));
    sender
}

fn set_state(shared: &Shared, url: &str, state: ModelState) -> bool {
    match lock(&shared.entries).get_mut(url) {
        Some(entry) => {
            entry.state = state;
            true
        }
        None => false,
    }
}

fn worker(shared: &Shared, receiver: &Receiver<Command>) {
    let mut host: Option<runtime::Host> = None;
    let mut models: HashMap<String, runtime::Model> = HashMap::new();
    // Commands that arrived while a job was running.
    let mut waiting: VecDeque<Command> = VecDeque::new();
    loop {
        let command = match waiting.pop_front() {
            Some(command) => command,
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => return,
            },
        };
        match command {
            Command::Load { url, path } => {
                if models.contains_key(&url) {
                    set_state(shared, &url, ModelState::Ready);
                    continue;
                }
                if host.is_none() {
                    match runtime::Host::start() {
                        Ok(started) => host = Some(started),
                        Err(error) => {
                            set_state(shared, &url, ModelState::Failed(error));
                            continue;
                        }
                    }
                }
                let Some(host) = host.as_ref() else { continue };
                match host.load(&path) {
                    // Dropped by `retain` while it loaded: let it go again.
                    Ok(model) => {
                        if set_state(shared, &url, ModelState::Ready) {
                            models.insert(url, model);
                        }
                    }
                    Err(error) => {
                        set_state(shared, &url, ModelState::Failed(error));
                    }
                }
            }
            Command::Unload { url } => {
                models.remove(&url);
            }
            Command::Run { request, reply } => {
                let (Some(host), Some(model)) = (host.as_ref(), models.get(&request.url)) else {
                    let _ = reply.send(Err("model is not loaded".to_owned()));
                    continue;
                };
                let threads = match shared.threads.load(Ordering::Relaxed) {
                    0 => default_threads(),
                    chosen => chosen,
                };
                let background = request.priority == Priority::Background;
                // Between tokens of a background job, serve interactive jobs
                // that came in; everything else waits its turn.
                let mut between = || {
                    if !background {
                        return;
                    }
                    while let Ok(command) = receiver.try_recv() {
                        match command {
                            Command::Run { request: urgent, reply } if urgent.priority == Priority::Interactive => {
                                let result = match models.get(&urgent.url) {
                                    Some(model) => host.generate(model, &urgent, threads, &mut || {}),
                                    None => Err("model is not loaded".to_owned()),
                                };
                                let _ = reply.send(result);
                            }
                            other => waiting.push_back(other),
                        }
                    }
                };
                let result = host.generate(model, &request, threads, &mut between);
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(feature = "llama")]
mod runtime {
    use super::Generate;
    use crate::template;
    use llama_cpp_2::{
        context::{params::LlamaContextParams, LlamaContext},
        llama_backend::LlamaBackend,
        llama_batch::LlamaBatch,
        model::{params::LlamaModelParams, LlamaChatMessage, LlamaModel},
        sampling::LlamaSampler,
        token::LlamaToken,
    };
    use std::{cell::RefCell, num::NonZeroU32, path::Path, sync::atomic::Ordering};

    pub(super) const AVAILABLE: bool = true;
    pub(super) const UNAVAILABLE: &str = "";
    /// Tokens fed to the model per decode call.
    const BATCH: usize = 512;
    /// Largest context a chat job may allocate, whatever the model allows.
    const MAX_CONTEXT: u32 = 16_384;
    /// Context kept alive per model for short jobs. Jobs that fit reuse it
    /// and skip the part of the prompt they share with the previous job
    /// (the fixed instruction), which is most of the work for a chat line.
    const SESSION_CONTEXT: u32 = 2048;

    pub(super) struct Host {
        backend: LlamaBackend,
    }

    pub(super) struct Model {
        // Declared before `model`: the session points into it and must be
        // dropped first.
        session: RefCell<Option<Session>>,
        template: Option<String>,
        model: Box<LlamaModel>,
    }

    /// A context that outlives one job, and the tokens its cache holds.
    struct Session {
        ctx: LlamaContext<'static>,
        tokens: Vec<LlamaToken>,
        size: u32,
        /// For models that cannot drop the tail of their cache (recurrent
        /// and hybrid ones): the saved state after a prompt prefix, and the
        /// prefix it belongs to.
        snapshot: Option<(Vec<LlamaToken>, Vec<u8>)>,
    }

    /// Shortest shared prompt prefix worth a snapshot.
    const SNAPSHOT_MIN_TOKENS: usize = 48;

    impl Host {
        pub(super) fn start() -> Result<Self, String> {
            let mut backend = LlamaBackend::init().map_err(|error| format!("llama.cpp did not start: {error}"))?;
            // llama.cpp logs every load and decode to stderr; the game owns it.
            backend.void_logs();
            Ok(Self { backend })
        }

        pub(super) fn load(&self, path: &Path) -> Result<Model, String> {
            if !crate::download::is_gguf(path) {
                return Err("the cached file is not a GGUF model".to_owned());
            }
            // CPU only. The renderer owns the GPU and its memory.
            let params = LlamaModelParams::default().with_n_gpu_layers(0);
            let model = LlamaModel::load_from_file(&self.backend, path, &params)
                .map_err(|error| format!("llama.cpp could not load the model: {error}"))?;
            let template = model.chat_template(None).ok().and_then(|template| template.to_string().ok());
            Ok(Model { session: RefCell::new(None), template, model: Box::new(model) })
        }

        fn prompt(&self, model: &Model, request: &Generate, user: &str) -> String {
            let system = request.system.as_deref().map(template::neutralize);
            let user = template::neutralize(user);
            let text = model.template.as_deref().unwrap_or("");
            if let Some(prompt) = template::render(template::family(text), system.as_deref(), &user, template::reasons(text)) {
                return prompt;
            }
            // A family this client does not lay out itself: let llama.cpp try.
            let built_in = (|| {
                let template = model.model.chat_template(None).ok()?;
                let mut chat = Vec::new();
                if let Some(system) = system.as_deref().filter(|system| !system.trim().is_empty()) {
                    chat.push(LlamaChatMessage::new("system".to_owned(), system.to_owned()).ok()?);
                }
                chat.push(LlamaChatMessage::new("user".to_owned(), user.clone()).ok()?);
                model.model.apply_chat_template(&template, &chat, true).ok()
            })();
            built_in.unwrap_or_else(|| template::plain(system.as_deref(), &user))
        }

        pub(super) fn generate(
            &self,
            model: &Model,
            request: &Generate,
            threads: i32,
            between: &mut dyn FnMut(),
        ) -> Result<String, String> {
            let cancelled = || request.cancel.load(Ordering::Relaxed);
            if cancelled() {
                return Err("cancelled".to_owned());
            }
            let vocab = model.model.vocab();
            let prompt = self.prompt(model, request, &request.user);
            // Templates that spell out the begin-of-text token must not get a second one.
            let bos = vocab.token_to_piece(vocab.bos(), true, None);
            let add_special = bos.is_empty() || !prompt.as_bytes().starts_with(&bos);
            let tokens = vocab.tokenize(prompt.as_bytes(), add_special, true);
            if tokens.is_empty() {
                return Err("nothing to send to the model".to_owned());
            }
            let max_tokens = request.max_tokens.max(1);
            let limit = model.model.n_ctx_train().clamp(512, MAX_CONTEXT);
            let needed = u32::try_from(tokens.len()).unwrap_or(u32::MAX).saturating_add(max_tokens).saturating_add(8);
            if needed > limit {
                return Err(format!("the text is too long for this model ({} of {limit} tokens)", tokens.len()));
            }
            let context = needed.max(request.context.min(limit));
            // A job that fits uses the model's standing context. It is busy
            // when this job interrupts another one on the same model; then,
            // and for long jobs, a context of its own is made and dropped.
            let mut standing = if context <= SESSION_CONTEXT { model.session.try_borrow_mut().ok() } else { None };
            let Some(slot) = standing.as_deref_mut() else {
                let mut ctx = self.context(model, context, threads)?;
                return Self::run(&mut ctx, model, request, &tokens, 0, between).map(|(answer, _)| answer);
            };
            if slot.as_ref().is_none_or(|session| session.size < context) {
                // Free the old one before allocating its replacement.
                *slot = None;
                let size = SESSION_CONTEXT.min(limit).max(context);
                let ctx = self.context(model, size, threads)?;
                // SAFETY: the context borrows `*model.model`, a heap allocation
                // that `Model` owns, never moves or replaces, and drops only
                // after `session` (field order). The 'static lifetime never
                // leaves this module.
                let ctx = unsafe { std::mem::transmute::<LlamaContext<'_>, LlamaContext<'static>>(ctx) };
                *slot = Some(Session { ctx, tokens: Vec::new(), size, snapshot: None });
            }
            let Some(session) = slot.as_mut() else { return Err("no model context".to_owned()) };
            let result = if Self::keeps_whole_cache(model) {
                // The part of the prompt that does not depend on the user
                // text: the instruction, identical from job to job.
                let fixed = vocab.tokenize(self.prompt(model, request, "").as_bytes(), add_special, true);
                let fixed = fixed.iter().zip(&tokens).take_while(|(a, b)| a == b).count().min(tokens.len() - 1);
                Self::run_from_snapshot(session, model, request, &tokens, fixed, between)
            } else {
                // Keep what the cache shares with this prompt; at least the
                // last prompt token is decoded again so there is something
                // to sample.
                let mut shared = session.tokens.iter().zip(&tokens).take_while(|(held, new)| held == new).count();
                shared = shared.min(tokens.len() - 1);
                if shared < session.tokens.len() {
                    let trimmed =
                        session.ctx.clear_kv_cache_seq(Some(0), u32::try_from(shared).ok(), None).unwrap_or(false);
                    if !trimmed {
                        session.ctx.clear_kv_cache();
                        shared = 0;
                    }
                }
                Self::run(&mut session.ctx, model, request, &tokens, shared, between)
            };
            match result {
                Ok((answer, generated)) => {
                    if session.snapshot.is_none() && !Self::keeps_whole_cache(model) {
                        session.tokens.clear();
                        session.tokens.extend_from_slice(&tokens);
                        session.tokens.extend(generated);
                    }
                    Ok(answer)
                }
                Err(error) => {
                    // The cache no longer matches any known token list.
                    *slot = None;
                    Err(error)
                }
            }
        }

        /// Recurrent and hybrid models cannot drop the tail of their cache.
        fn keeps_whole_cache(model: &Model) -> bool {
            model.model.is_recurrent() || model.model.is_hybrid()
        }

        /// Prefix reuse for models whose cache can only be kept whole: the
        /// state after the first `fixed` tokens (the instruction) is saved
        /// once and restored for each job that starts the same way.
        fn run_from_snapshot(
            session: &mut Session,
            model: &Model,
            request: &Generate,
            tokens: &[LlamaToken],
            fixed: usize,
            between: &mut dyn FnMut(),
        ) -> Result<(String, Vec<LlamaToken>), String> {
            session.ctx.clear_kv_cache();
            session.tokens.clear();
            let fits = |prefix: &[LlamaToken]| prefix.len() < tokens.len() && tokens.starts_with(prefix);
            let mut from = 0;
            match &session.snapshot {
                Some((prefix, state)) if fits(prefix) => {
                    // SAFETY: `state` was written by `copy_state_data` on this
                    // same context and is passed back whole.
                    let read = unsafe { session.ctx.set_state_data(state) };
                    if read == state.len() {
                        from = prefix.len();
                    } else {
                        session.ctx.clear_kv_cache();
                        session.snapshot = None;
                    }
                }
                _ => {
                    session.snapshot = None;
                    let shared = fixed;
                    if shared >= SNAPSHOT_MIN_TOKENS {
                        Self::feed(&mut session.ctx, request, tokens, 0, shared, between)?;
                        let mut state = vec![0u8; session.ctx.get_state_size()];
                        // SAFETY: the buffer has the size the context asked for.
                        let written = unsafe { session.ctx.copy_state_data(state.as_mut_ptr()) };
                        state.truncate(written);
                        session.snapshot = Some((tokens[..shared].to_vec(), state));
                        from = shared;
                    }
                }
            }
            Self::run(&mut session.ctx, model, request, tokens, from, between)
        }

        fn context<'m>(&self, model: &'m Model, size: u32, threads: i32) -> Result<LlamaContext<'m>, String> {
            let params = LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(size))
                .with_n_batch(BATCH as u32)
                .with_n_threads(threads)
                .with_n_threads_batch(threads);
            model
                .model
                .new_context(&self.backend, params)
                .map_err(|error| format!("llama.cpp could not create a context: {error}"))
        }

        /// Decode `tokens[from..to]`. The last token of the whole prompt gets
        /// logits; the returned batch is the last one decoded.
        fn feed(
            ctx: &mut LlamaContext<'_>,
            request: &Generate,
            tokens: &[LlamaToken],
            from: usize,
            to: usize,
            between: &mut dyn FnMut(),
        ) -> Result<LlamaBatch<'static>, String> {
            let mut batch = LlamaBatch::new(BATCH, 1);
            let last = tokens.len() - 1;
            let mut position = from as i32;
            for chunk in tokens[from..to].chunks(BATCH) {
                if request.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".to_owned());
                }
                batch.clear();
                for token in chunk {
                    let wants_logits = position as usize == last;
                    batch.add(*token, position, &[0], wants_logits).map_err(|error| error.to_string())?;
                    position += 1;
                }
                ctx.decode(&mut batch).map_err(|error| format!("llama.cpp decode failed: {error}"))?;
                between();
            }
            Ok(batch)
        }

        /// Decode `tokens[from..]` on top of a cache that already holds
        /// `tokens[..from]`, then sample the answer. Returns it with the
        /// tokens that were generated (they are in the cache too).
        fn run(
            ctx: &mut LlamaContext<'_>,
            model: &Model,
            request: &Generate,
            tokens: &[LlamaToken],
            from: usize,
            between: &mut dyn FnMut(),
        ) -> Result<(String, Vec<LlamaToken>), String> {
            let cancelled = || request.cancel.load(Ordering::Relaxed);
            let vocab = model.model.vocab();
            let mut batch = Self::feed(ctx, request, tokens, from, tokens.len(), between)?;
            let mut position = tokens.len() as i32;
            let sampling = request.sampling;
            let mut sampler = if sampling.temperature <= 0.0 {
                LlamaSampler::greedy()
            } else {
                // Fixed seed: the same line gets the same answer.
                LlamaSampler::chain_simple([
                    LlamaSampler::penalties(model.model.n_vocab(), 64, sampling.repeat_penalty.max(0.01), 0.0, 0.0),
                    LlamaSampler::top_k(sampling.top_k.max(1)),
                    LlamaSampler::top_p(sampling.top_p.clamp(0.01, 1.0), 1),
                    LlamaSampler::temp(sampling.temperature),
                    LlamaSampler::dist(0x444a_4b21),
                ])
            };
            let mut answer = Vec::<u8>::new();
            let mut generated = Vec::new();
            for _ in 0..request.max_tokens.max(1) {
                if cancelled() {
                    return Err("cancelled".to_owned());
                }
                let token = sampler.sample(ctx, batch.n_tokens() - 1);
                if vocab.is_eog(token) {
                    break;
                }
                answer.extend_from_slice(&vocab.token_to_piece(token, false, None));
                batch.clear();
                batch.add(token, position, &[0], true).map_err(|error| error.to_string())?;
                position += 1;
                ctx.decode(&mut batch).map_err(|error| format!("llama.cpp decode failed: {error}"))?;
                generated.push(token);
                between();
            }
            let answer = template::clean_answer(&String::from_utf8_lossy(&answer));
            if answer.is_empty() {
                return Err("the model gave no answer".to_owned());
            }
            Ok((answer, generated))
        }
    }
}

#[cfg(not(feature = "llama"))]
mod runtime {
    use super::Generate;
    use std::path::Path;

    pub(super) const AVAILABLE: bool = false;
    pub(super) const UNAVAILABLE: &str = "this build has no local model runtime (built without the local-llm feature)";

    pub(super) struct Host;
    pub(super) struct Model;

    impl Host {
        pub(super) fn start() -> Result<Self, String> {
            Err(UNAVAILABLE.to_owned())
        }

        pub(super) fn load(&self, _path: &Path) -> Result<Model, String> {
            Err(UNAVAILABLE.to_owned())
        }

        pub(super) fn generate(
            &self,
            _model: &Model,
            _request: &Generate,
            _threads: i32,
            _between: &mut dyn FnMut(),
        ) -> Result<String, String> {
            Err(UNAVAILABLE.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_generation_does_not_reload_a_released_model() {
        let host = Engine {
            shared: Arc::new(Shared {
                entries: Mutex::new(HashMap::new()),
                cache_dir: Mutex::new(None),
                threads: AtomicI32::new(0),
            }),
            worker: Mutex::new(None),
        };
        let url = "https://example.com/model.gguf";
        let download_cancel = Arc::new(AtomicBool::new(false));
        lock(&host.shared.entries).insert(url.to_owned(), Entry {
            state: ModelState::Ready,
            progress: Arc::new(Progress::default()),
            cancel: Arc::clone(&download_cancel),
        });
        host.retain(&[]);
        assert!(download_cancel.load(Ordering::Relaxed));

        let result = host.generate(Generate {
            url: url.to_owned(),
            system: None,
            user: "queued chat".to_owned(),
            max_tokens: 8,
            context: 512,
            sampling: Sampling::EXACT,
            priority: Priority::Interactive,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(result.unwrap_err(), "model not loaded");
        assert_eq!(host.state(url), ModelState::Idle);
        assert!(lock(&host.shared.entries).is_empty());
        assert!(lock(&host.worker).is_none());
        // Restoring a feature without a cached file also cannot restart it.
        assert!(!host.ensure_cached(url));
        assert_eq!(host.state(url), ModelState::Idle);
        assert!(lock(&host.worker).is_none());
    }

    #[test]
    fn states_describe_themselves_for_the_settings_page() {
        assert_eq!(ModelState::Idle.describe(), "not loaded");
        let half = ModelState::Downloading { done: 500_000_000, total: 1_000_000_000 };
        assert_eq!(half.describe(), "downloading 50% (0.50 of 1.00 GB)");
        assert!(ModelState::Failed("x".to_owned()).describe().ends_with("x"));
    }

    #[test]
    fn default_thread_count_leaves_room_for_the_game() {
        let threads = default_threads();
        assert!((2..=6).contains(&threads));
    }
}
