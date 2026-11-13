//! Local-model chat features in the app: the chat filter (`cg_chatCensor`),
//! `chatQRD`, and the model housekeeping shared with local translation.
//!
//! The filter sees every received chat line after it has an id (see
//! `chat_translation`). With `cg_chatCensorHold` the line's text stays off
//! the chat box and console until the model has answered, so nothing is
//! shown first and replaced later. The chat log on disk and the protocol
//! are never touched; the received text survives only there and in the
//! local-only hover.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};

use crate::{
    app::{chat_translation::Censor, App},
    chat_llm::{
        self as cl,
        history::{self, Away, Entry, History},
        engine, CensorAction, ModelState, SummaryPass, Verdict,
    },
    chat_translation::{lang, text, Mode, ProviderKind, Translated},
};

/// Filter jobs that may wait for the model; more than this and new lines
/// are treated as unchecked rather than queued for a minute.
const QUEUE: usize = 12;
/// A held line is released (masked) if its verdict takes longer than this.
const VERDICT_TIMEOUT: Duration = Duration::from_secs(12);
const VERDICT_CACHE: usize = 256;
/// Longest a summary waits for its model to download and load.
const SUMMARY_MODEL_WAIT: Duration = Duration::from_secs(30 * 60);

enum Target {
    /// The line as received.
    Original,
    /// A finished translation, checked before it is shown.
    Translation(Translated),
}

struct Job {
    id: u64,
    generation: u64,
    target: Target,
    /// The text being judged.
    text: String,
    request: jka_llm::Generate,
}

struct Done {
    id: u64,
    generation: u64,
    target: Target,
    text: String,
    result: Result<String, String>,
}

struct Worker {
    jobs: SyncSender<Job>,
    done: Receiver<Done>,
}

struct SummaryRun {
    answer: Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    pass: SummaryPass,
    /// Stated after the model's text, whatever it wrote.
    disclaimers: Vec<String>,
    filter_was_on: bool,
}

#[derive(Clone, Copy)]
pub(in crate::app) enum ChatModelUse {
    Filter,
    Translation(Mode),
    Summary { minutes: Option<f32>, pass: SummaryPass },
}

pub(in crate::app) struct ChatModelDownload {
    pub(in crate::app) url: String,
    pub(in crate::app) usage: ChatModelUse,
}

/// Text fields and pending confirmations for the settings page.
#[derive(Default)]
pub(in crate::app) struct Edits {
    pub(in crate::app) translate_url: Option<String>,
    pub(in crate::app) assist_url: Option<String>,
    pub(in crate::app) translate_prompt: Option<String>,
    pub(in crate::app) censor_prompt: Option<String>,
    pub(in crate::app) summary_prompt: Option<String>,
    pub(in crate::app) note: Option<String>,
    pub(in crate::app) download: Option<ChatModelDownload>,
    pub(in crate::app) delete_model: Option<String>,
}

#[derive(Default)]
pub(in crate::app) struct Counters {
    pub(in crate::app) checked: u32,
    pub(in crate::app) censored: u32,
    pub(in crate::app) failed: u32,
    pub(in crate::app) last_error: Option<String>,
}

pub(in crate::app) struct ChatLlm {
    pub(in crate::app) settings: cl::Settings,
    pub(in crate::app) edits: Edits,
    pub(in crate::app) counters: Counters,
    history: History,
    away: Away,
    started: Instant,
    worker: Option<Worker>,
    generation: Arc<AtomicU64>,
    /// Lines waiting for a verdict on their received text.
    pending: HashMap<u64, Instant>,
    verdicts: HashMap<String, (Verdict, u64)>,
    verdict_tick: u64,
    summary: Option<SummaryRun>,
    /// A summary ran this session: keep its model loaded.
    summary_used: bool,
    /// Downloads explicitly confirmed this run, limited to selected model URLs.
    approved_downloads: Vec<String>,
    /// The saved settings have been applied to the model host.
    booted: bool,
    /// Model states last reported in the console.
    watched: Vec<(String, ModelState)>,
    last_watch: Instant,
    /// Free cursor over the game, in physical pixels: drives the hover.
    pub(in crate::app) pointer: Option<[f32; 2]>,
}

impl ChatLlm {
    pub(in crate::app) fn new(settings: cl::Settings) -> Self {
        let now = Instant::now();
        Self {
            settings,
            edits: Edits::default(),
            counters: Counters::default(),
            history: History::default(),
            away: Away::default(),
            started: now,
            worker: None,
            generation: Arc::new(AtomicU64::new(0)),
            pending: HashMap::new(),
            verdicts: HashMap::new(),
            verdict_tick: 0,
            summary: None,
            summary_used: false,
            approved_downloads: Vec::new(),
            booted: false,
            watched: Vec::new(),
            last_watch: now,
            pointer: None,
        }
    }

    /// Any key, button, wheel or mouse movement the game window received.
    pub(in crate::app) fn note_input(&mut self, now: Instant) {
        self.away.input(now);
    }

    fn cached_verdict(&mut self, text: &str) -> Option<Verdict> {
        // The same words can be banter in one moment and bait in the next.
        if cl::verdict_reads_context(self.settings.censor_severity, self.settings.censor_ragebait) {
            return None;
        }
        self.verdict_tick += 1;
        let tick = self.verdict_tick;
        self.verdicts.get_mut(text).map(|(verdict, used)| {
            *used = tick;
            verdict.clone()
        })
    }

    fn remember_verdict(&mut self, text: String, verdict: Verdict) {
        if cl::verdict_reads_context(self.settings.censor_severity, self.settings.censor_ragebait) {
            return;
        }
        if self.verdicts.len() >= VERDICT_CACHE {
            if let Some(oldest) = self.verdicts.iter().min_by_key(|(_, (_, used))| *used).map(|(key, _)| key.clone()) {
                self.verdicts.remove(&oldest);
            }
        }
        self.verdict_tick += 1;
        self.verdicts.insert(text, (verdict, self.verdict_tick));
    }

    fn submit(&mut self, job: Job) -> bool {
        let worker = self.worker.get_or_insert_with(|| spawn_worker(Arc::clone(&self.generation)));
        match worker.jobs.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => {
                self.worker = None;
                false
            }
        }
    }

    pub(in crate::app) fn status_line(&self) -> String {
        let counters = &self.counters;
        let mut line = format!("{} checked, {} censored, {} failed", counters.checked, counters.censored, counters.failed);
        if !self.pending.is_empty() {
            line.push_str(&format!(", {} waiting", self.pending.len()));
        }
        if let Some(error) = &counters.last_error {
            line.push_str(&format!(". Last error: {error}"));
        }
        line
    }
}

fn spawn_worker(generation: Arc<AtomicU64>) -> Worker {
    let (job_tx, job_rx) = mpsc::sync_channel::<Job>(QUEUE);
    let (done_tx, done_rx) = mpsc::channel();
    // Detached: it blocks on the model thread and ends when the app drops
    // its end of the queue.
    let _ = std::thread::Builder::new().name("chat-filter".to_owned()).spawn(move || {
        while let Ok(job) = job_rx.recv() {
            let result = if job.generation == generation.load(Ordering::Relaxed) {
                engine().generate(job.request)
            } else {
                Err(String::new())
            };
            let done = Done { id: job.id, generation: job.generation, target: job.target, text: job.text, result };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    Worker { jobs: job_tx, done: done_rx }
}

fn team_name(team: i32) -> Option<&'static str> {
    match team {
        1 => Some("red team"),
        2 => Some("blue team"),
        3 => Some("spectator"),
        _ => None,
    }
}

fn game_type_name(game_type: i32) -> &'static str {
    match game_type {
        0 => "free for all",
        1 => "holocron",
        2 => "jedi master",
        3 => "duel",
        4 => "power duel",
        6 => "team free for all",
        7 => "siege",
        8 => "capture the flag",
        9 => "capture the ysalamiri",
        _ => "unknown",
    }
}

fn plain(name: &str) -> String {
    crate::logging::strip_jka_colors(name).trim().to_owned()
}

impl App {
    /// Point the model host at the cache, hand local translation its model,
    /// and load exactly the models the enabled features need.
    pub(in crate::app) fn chat_llm_configure(&mut self) {
        let settings = &self.chat_llm.settings;
        let translation = &self.chat_translation.settings;
        let translate = translation.mode != Mode::Off && translation.provider == ProviderKind::Local;
        let assist = settings.censor || self.chat_llm.summary_used;
        let translate_url = settings.translate_model().to_owned();
        let assist_url = settings.assist_model().to_owned();
        self.chat_llm.approved_downloads.retain(|url| url == &translate_url || url == &assist_url);
        let local_model = (translate_url.clone(), cl::unescape_prompt(&settings.translate_prompt));
        if self.chat_translation.local_model != local_model {
            self.chat_translation.local_model = local_model;
            self.chat_translation.drop_config();
        }
        let host = engine();
        if host.cache_dir().is_none() {
            host.set_cache_dir(jka_assets::pk3::dinurdojk_cache_directory(&self.base).join("models"));
        }
        host.set_threads(i32::from(settings.threads));
        let mut needed: Vec<&str> = Vec::new();
        if translate {
            needed.push(&translate_url);
        }
        if assist && !needed.contains(&assist_url.as_str()) {
            needed.push(&assist_url);
        }
        host.retain(&needed);
        for url in &needed {
            if self.chat_llm.approved_downloads.iter().any(|approved| approved == url) {
                host.ensure(url);
            } else {
                host.ensure_cached(url);
            }
        }
        self.chat_llm.watched = needed.iter().map(|url| ((*url).to_owned(), host.state(url))).collect();
    }

    pub(in crate::app) fn chat_model_can_start(&self, url: &str) -> bool {
        engine().cached(url) || self.chat_llm.approved_downloads.iter().any(|approved| approved == url)
    }

    pub(in crate::app) fn request_chat_model_download(&mut self, usage: ChatModelUse) {
        let url = match usage {
            ChatModelUse::Translation(_) => self.chat_llm.settings.translate_model(),
            _ => self.chat_llm.settings.assist_model(),
        }.to_owned();
        self.unload_chat_model(Some(&url));
        self.chat_llm.edits.download = Some(ChatModelDownload { url, usage });
        self.push_console_line("^5CHAT MODEL:^7 download needs confirmation in Setup > Interface, or /chatLlmDownload confirm. /chatLlmDownload cancel dismisses it.");
    }

    pub(in crate::app) fn confirm_chat_model_download(&mut self) {
        let Some(download) = self.chat_llm.edits.download.take() else { return };
        let selected = match download.usage {
            ChatModelUse::Translation(_) => self.chat_llm.settings.translate_model(),
            _ => self.chat_llm.settings.assist_model(),
        };
        if download.url != selected {
            return;
        }
        if !self.chat_llm.approved_downloads.contains(&download.url) {
            self.chat_llm.approved_downloads.push(download.url);
        }
        match download.usage {
            ChatModelUse::Filter => {
                self.chat_llm.edits.note = self.set_console_cvar("cg_chatCensor", "1").err();
            }
            ChatModelUse::Translation(mode) => {
                self.chat_llm.edits.note = self.set_console_cvar("cg_chatTranslate", &mode.index().to_string()).err();
            }
            ChatModelUse::Summary { minutes, pass } => self.start_quick_rundown(minutes, pass),
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn chat_model_in_use(&self, url: &str) -> bool {
        (url == self.chat_llm.settings.assist_model() && (self.chat_llm.settings.censor || self.chat_llm.summary_used))
            || (url == self.chat_llm.settings.translate_model()
                && self.chat_translation.settings.provider == ProviderKind::Local
                && self.chat_translation.settings.mode != Mode::Off)
    }

    /// Stop the selected model's consumers before releasing it. Shared model
    /// URLs must stop both consumers or configuration would load it again.
    pub(in crate::app) fn unload_chat_model(&mut self, url: Option<&str>) {
        let translate = url.is_none_or(|url| url == self.chat_llm.settings.translate_model());
        let assist = url.is_none_or(|url| url == self.chat_llm.settings.assist_model());
        self.chat_llm.approved_downloads.retain(|approved| url.is_some_and(|url| approved != url));
        if self.chat_llm.edits.download.as_ref().is_some_and(|pending| url.is_none_or(|url| pending.url == url)) {
            self.chat_llm.edits.download = None;
        }
        self.chat_llm.edits.delete_model = None;
        if translate && self.chat_translation.settings.provider == ProviderKind::Local {
            self.chat_translation.settings.mode = Mode::Off;
            self.chat_translation.drop_config();
            self.cancel_chat_translations();
        }
        if assist {
            self.chat_llm.settings.censor = false;
            self.cancel_chat_filter();
            self.cancel_quick_rundown();
            self.chat_llm.summary_used = false;
        }
        self.chat_llm_configure();
        self.mark_config_dirty();
        self.publish_ui();
        self.publish_transient_ui();
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn chat_llm_settings_changed(&mut self) {
        self.cancel_chat_filter();
        self.chat_llm.verdicts.clear();
        self.chat_llm_configure();
        self.mark_config_dirty();
        self.publish_ui();
        self.publish_transient_ui();
    }

    /// What the filter is told about the match around message `id`.
    fn chat_match_context(&self, id: u64) -> cl::MatchContext {
        let mut context = cl::MatchContext::default();
        let own_client = self.net.as_ref().map(|net| net.session().client_num());
        let team_of = |client: i32| {
            let session = self.game_session.as_ref()?;
            let info = session.client_game.client_info(usize::try_from(client).ok()?, &session.siege_classes)?;
            team_name(info.team)
        };
        context.main_player = plain(&self.current_follow_name().unwrap_or_else(|| self.network.name.clone()));
        context.main_team = own_client.and_then(team_of);
        if let Some(message) = self.chat_translation.message_ref(id) {
            let (slot, name) = message.sender();
            context.sender = name.to_owned();
            context.sender_team = slot.and_then(team_of);
            context.team_chat = message.team();
        }
        if let Some(session) = &self.game_session {
            context.game_type = game_type_name(session.client_game.gametype()).to_owned();
        }
        context.kills = self.recent_kills_for_chat(8, Duration::from_secs(120));
        // Context-reading modes get a longer look back.
        let settings = &self.chat_llm.settings;
        let lines = if cl::verdict_reads_context(settings.censor_severity, settings.censor_ragebait) { 10 } else { 6 };
        context.recent_chat = self.chat_llm.history.before(id, lines);
        context
    }

    /// Called for every received chat line once it has an id: records it for
    /// summaries and starts the filter. Returns what the console row should
    /// read when that is not the received line (held back, or already known
    /// to be filtered).
    pub(in crate::app) fn chat_llm_received(&mut self, id: u64) -> Option<String> {
        let codepage = self.chat_translation.settings.codepage;
        let (body, sender, team, voice) = {
            let message = self.chat_translation.message_ref(id)?;
            let body = text::provider_text(&crate::cgame::text_to_jka_bytes(&message.body()), codepage, 400);
            (body, message.sender().1.to_owned(), message.team(), message.is_voice())
        };
        if voice || body.is_empty() {
            return None;
        }
        let now = Instant::now();
        let [hour, minute, _] = crate::logging::local_hms();
        self.chat_llm.history.push(Entry {
            id,
            at: now,
            clock: [hour as u8, minute as u8],
            sender: if sender.is_empty() { "unknown".to_owned() } else { sender },
            team,
            text: body.clone(),
            filtered: None,
        });
        if !self.chat_llm.settings.censor || cl::censor_skips(&body) {
            return None;
        }
        if let Some(verdict) = self.chat_llm.cached_verdict(&body) {
            self.apply_chat_verdict(id, verdict, false);
        } else {
            let url = self.chat_llm.settings.assist_model().to_owned();
            if engine().state(&url) != ModelState::Ready {
                // Still downloading or loading: chat is shown unfiltered
                // until the model is there (the console says so once).
                return None;
            }
            let held = self.chat_llm.settings.censor_hold;
            let context = self.chat_match_context(id);
            let settings = &self.chat_llm.settings;
            let request = cl::censor_job(
                &url,
                &cl::unescape_prompt(&settings.censor_prompt),
                settings.censor_severity,
                settings.censor_ragebait,
                &context,
                &body,
                Arc::new(AtomicBool::new(false)),
            );
            let generation = self.chat_llm.generation.load(Ordering::Relaxed);
            let queued = self.chat_llm.submit(Job { id, generation, target: Target::Original, text: body, request });
            let message = self.chat_translation.message_mut_ref(id)?;
            if queued {
                message.censor = Censor::Pending { held };
                self.chat_llm.pending.insert(id, now);
            } else if held {
                // Too many lines waiting: unchecked text is not shown.
                self.chat_llm.counters.failed += 1;
                self.apply_chat_verdict(id, Verdict::Censored(cl::MASK.to_owned()), false);
            }
        }
        let message = self.chat_translation.message_mut_ref(id)?;
        let shown = message.shown_line();
        if shown == message.received_line() {
            return None;
        }
        message.set_console_shown(shown.clone());
        Some(shown)
    }

    /// Put a verdict on message `id`: update the message, its chat-box
    /// record and (with `sync`) its console rows.
    fn apply_chat_verdict(&mut self, id: u64, verdict: Verdict, sync: bool) {
        let action = self.chat_llm.settings.censor_action;
        let censor = match verdict {
            Verdict::Clean => Censor::Clean,
            Verdict::Censored(body) => {
                self.chat_llm.counters.censored += 1;
                let hidden = action == CensorAction::Hide;
                let shown = if hidden {
                    format!("^9{}", cl::HIDDEN_NOTE)
                } else {
                    // The docked chat box draws Cyrillic as well as Latin-1;
                    // a masked line in any other script is the bare mask.
                    text::hud_text_wide(&body, self.chat_settings.docked()).unwrap_or_else(|| cl::MASK.to_owned())
                };
                self.chat_llm.history.set_filtered(id, if hidden { cl::HIDDEN_NOTE.to_owned() } else { body });
                Censor::Censored { body: shown, hidden }
            }
        };
        let Some(message) = self.chat_translation.message_mut_ref(id) else { return };
        message.censor = censor;
        if let Some(record) = self.chat_lines.iter_mut().find(|record| record.chat_id == Some(id)) {
            record.censor = self.chat_translation.message_ref(id).and_then(|message| message.record_censor(&record.text));
        }
        if sync {
            self.sync_console_chat_rows(Some(id));
        }
    }

    /// A verdict could not be had. Text that was being held is not released.
    fn chat_verdict_failed(&mut self, id: u64, target: Target, error: &str) {
        if !error.is_empty() {
            self.chat_llm.counters.failed += 1;
            self.chat_llm.counters.last_error = Some(error.to_owned());
        }
        let held = self.chat_llm.settings.censor_hold;
        match target {
            Target::Original => {
                let was_held = self
                    .chat_translation
                    .message_ref(id)
                    .is_some_and(|message| message.censor == Censor::Pending { held: true });
                if was_held {
                    self.apply_chat_verdict(id, Verdict::Censored(cl::MASK.to_owned()), true);
                } else if let Some(message) = self.chat_translation.message_mut_ref(id) {
                    message.censor = Censor::None;
                }
            }
            Target::Translation(translated) if held => {
                let masked = Translated { text: cl::MASK.to_owned(), detected: translated.detected };
                self.finish_chat_translation(id, Ok(masked), true);
            }
            Target::Translation(translated) => self.finish_chat_translation(id, Ok(translated), false),
        }
    }

    /// A translation is ready: with the filter on it is checked before it is
    /// shown, like the received text was.
    pub(in crate::app) fn filter_then_finish_chat_translation(&mut self, id: u64, result: Result<Translated, String>) {
        let translated = match result {
            Ok(translated) if self.chat_llm.settings.censor && !cl::censor_skips(&translated.text) => translated,
            other => return self.finish_chat_translation(id, other, false),
        };
        if let Some(verdict) = self.chat_llm.cached_verdict(&translated.text) {
            return self.finish_filtered_chat_translation(id, translated, verdict);
        }
        let url = self.chat_llm.settings.assist_model().to_owned();
        if engine().state(&url) != ModelState::Ready {
            return self.finish_chat_translation(id, Ok(translated), false);
        }
        let context = self.chat_match_context(id);
        let settings = &self.chat_llm.settings;
        let request = cl::censor_job(
            &url,
            &cl::unescape_prompt(&settings.censor_prompt),
            settings.censor_severity,
            settings.censor_ragebait,
            &context,
            &translated.text,
            Arc::new(AtomicBool::new(false)),
        );
        let generation = self.chat_llm.generation.load(Ordering::Relaxed);
        let text = translated.text.clone();
        let job = Job { id, generation, target: Target::Translation(translated.clone()), text, request };
        if !self.chat_llm.submit(job) {
            self.chat_verdict_failed(id, Target::Translation(translated), "too many chat lines waiting for the filter");
        }
    }

    fn finish_filtered_chat_translation(&mut self, id: u64, translated: Translated, verdict: Verdict) {
        match verdict {
            Verdict::Clean => self.finish_chat_translation(id, Ok(translated), false),
            Verdict::Censored(body) => {
                if self.chat_llm.settings.censor_action == CensorAction::Hide {
                    // The translation exposed what the original hid: the
                    // whole message is withheld.
                    self.apply_chat_verdict(id, Verdict::Censored(body), true);
                    let masked = Translated { text: cl::MASK.to_owned(), detected: translated.detected };
                    self.finish_chat_translation(id, Ok(masked), true);
                } else {
                    self.chat_llm.counters.censored += 1;
                    self.finish_chat_translation(id, Ok(Translated { text: body, detected: translated.detected }), true);
                }
            }
        }
    }

    /// Drop queued filter work (disconnect, settings change). Held lines
    /// stay masked; nothing unchecked is released.
    pub(in crate::app) fn cancel_chat_filter(&mut self) {
        self.chat_llm.generation.fetch_add(1, Ordering::Relaxed);
        let waiting: Vec<u64> = self.chat_llm.pending.drain().map(|(id, _)| id).collect();
        for id in waiting {
            self.chat_verdict_failed(id, Target::Original, "");
        }
    }

    /// Once per frame. Free while no local-model feature has been used.
    pub(in crate::app) fn poll_chat_llm(&mut self) {
        let now = Instant::now();
        let mut changed = false;
        if !self.chat_llm.booted {
            self.chat_llm.booted = true;
            self.chat_llm_configure();
        }
        if self.chat_llm.worker.is_some() {
            let generation = self.chat_llm.generation.load(Ordering::Relaxed);
            let mut finished = Vec::new();
            if let Some(worker) = &self.chat_llm.worker {
                while let Ok(done) = worker.done.try_recv() {
                    finished.push(done);
                }
            }
            for done in finished {
                if done.generation != generation {
                    // Cancelled. A translation still has to leave "pending".
                    if let Target::Translation(translated) = done.target {
                        self.chat_verdict_failed(done.id, Target::Translation(translated), "");
                        changed = true;
                    }
                    continue;
                }
                changed = true;
                if matches!(done.target, Target::Original) && self.chat_llm.pending.remove(&done.id).is_none() {
                    // Timed out earlier and already resolved.
                    continue;
                }
                match done.result {
                    Ok(answer) => {
                        self.chat_llm.counters.checked += 1;
                        let verdict = cl::read_verdict(&answer, &done.text);
                        self.chat_llm.remember_verdict(done.text, verdict.clone());
                        match done.target {
                            Target::Original => self.apply_chat_verdict(done.id, verdict, true),
                            Target::Translation(translated) => {
                                self.finish_filtered_chat_translation(done.id, translated, verdict);
                            }
                        }
                    }
                    Err(error) => self.chat_verdict_failed(done.id, done.target, &error),
                }
            }
            let late: Vec<u64> = self
                .chat_llm
                .pending
                .iter()
                .filter(|(_, since)| now.saturating_duration_since(**since) >= VERDICT_TIMEOUT)
                .map(|(id, _)| *id)
                .collect();
            for id in late {
                self.chat_llm.pending.remove(&id);
                self.chat_verdict_failed(id, Target::Original, "the chat filter took too long");
                changed = true;
            }
        }
        changed |= self.poll_quick_rundown();
        changed |= self.watch_chat_models(now);
        if changed {
            self.publish_transient_ui();
        }
    }

    /// Say in the console when a model becomes ready or fails, and keep the
    /// settings page's download progress moving.
    fn watch_chat_models(&mut self, now: Instant) -> bool {
        if self.chat_llm.watched.is_empty()
            || now.saturating_duration_since(self.chat_llm.last_watch) < Duration::from_millis(400)
        {
            return false;
        }
        self.chat_llm.last_watch = now;
        let mut lines = Vec::new();
        let mut busy = false;
        for (url, seen) in &mut self.chat_llm.watched {
            let state = engine().state(url);
            busy |= matches!(state, ModelState::Downloading { .. } | ModelState::Loading);
            if std::mem::discriminant(&state) == std::mem::discriminant(seen) {
                *seen = state;
                continue;
            }
            let name = url.rsplit('/').next().unwrap_or(url).to_owned();
            match &state {
                ModelState::Downloading { .. } => lines.push(format!(
                    "^5CHAT MODEL:^7 downloading {name} (once; chat is not filtered or translated locally until it is ready)"
                )),
                ModelState::Ready => lines.push(format!("^5CHAT MODEL:^7 {name} is ready")),
                ModelState::Failed(error) => lines.push(format!("^1CHAT MODEL:^7 {name}: {error}")),
                ModelState::Loading | ModelState::Idle => {}
            }
            *seen = state;
        }
        let announced = !lines.is_empty();
        for line in lines {
            self.push_console_line(line);
        }
        if busy || announced {
            // The settings page shows progress; egui only repaints on demand.
            self.egui_repaint_requested = true;
        }
        announced
    }

    /// `chatQRD [minutes] [truth|cleanse]`.
    pub(in crate::app) fn start_quick_rundown(&mut self, minutes: Option<f32>, pass: SummaryPass) {
        if self.chat_llm.summary.is_some() {
            self.push_console_line("^5QUICK RUNDOWN:^7 one is already being written");
            return;
        }
        if !engine().available() {
            self.push_console_line("^1QUICK RUNDOWN:^7 this build has no local model runtime");
            return;
        }
        let now = Instant::now();
        let back = |minutes: f32| {
            now.checked_sub(Duration::from_secs_f32(minutes * 60.0)).unwrap_or(self.chat_llm.started).max(self.chat_llm.started)
        };
        let (since, span) = match minutes {
            Some(minutes) => (back(minutes), format!("the last {minutes:.0} min")),
            None => match self.chat_llm.away.since(now) {
                Some(left) => {
                    let away = now.saturating_duration_since(left).as_secs_f32() / 60.0;
                    (left, format!("the {away:.0} min since your last input before going idle"))
                }
                None => (
                    back(history::FALLBACK_MINUTES),
                    format!("the last {:.0} min (you were not away)", history::FALLBACK_MINUTES),
                ),
            },
        };
        let transcript = self.chat_llm.history.transcript(since, pass, history::TRANSCRIPT_CHARS);
        if transcript.messages == 0 {
            self.push_console_line(format!("^5QUICK RUNDOWN:^7 nobody wrote anything in {span}"));
            return;
        }
        let url = self.chat_llm.settings.assist_model();
        if !self.chat_model_can_start(url) {
            self.request_chat_model_download(ChatModelUse::Summary { minutes, pass });
            return;
        }
        let disclaimers = if pass == SummaryPass::Cleanse { self.chat_llm.history.disclaimers(since) } else { Vec::new() };
        self.chat_llm.summary_used = true;
        self.chat_llm_configure();
        let settings = &self.chat_llm.settings;
        let url = settings.assist_model().to_owned();
        let cancel = Arc::new(AtomicBool::new(false));
        let player = plain(&self.network.name);
        let language = lang::resolve_target(&self.chat_translation.settings.target);
        let request = cl::summary_job(
            &url,
            &cl::unescape_prompt(&settings.summary_prompt),
            pass,
            &player,
            language,
            &transcript.text,
            Arc::clone(&cancel),
        );
        let (tx, rx) = mpsc::channel();
        let wait_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new().name("quick-rundown".to_owned()).spawn(move || {
            // The model may still be downloading: wait for it rather than
            // making the player ask again.
            let deadline = Instant::now() + SUMMARY_MODEL_WAIT;
            let result = loop {
                if wait_cancel.load(Ordering::Relaxed) {
                    break Err("cancelled".to_owned());
                }
                match engine().state(&url) {
                    ModelState::Ready => break engine().generate(request),
                    ModelState::Failed(error) => break Err(error),
                    ModelState::Idle => break Err("model was unloaded".to_owned()),
                    ModelState::Downloading { .. } | ModelState::Loading => {}
                }
                if Instant::now() >= deadline {
                    break Err("the model did not become ready".to_owned());
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            let _ = tx.send(result);
        });
        if spawned.is_err() {
            self.push_console_line("^1QUICK RUNDOWN:^7 could not start");
            return;
        }
        let omitted = if transcript.omitted > 0 {
            format!(", {} older left out", transcript.omitted)
        } else {
            String::new()
        };
        let waiting = match engine().state(settings.assist_model()) {
            ModelState::Ready => String::new(),
            other => format!(" Model {}.", other.describe()),
        };
        self.push_console_line(format!(
            "^5QUICK RUNDOWN:^7 {} pass over {span}: {} message{}{omitted}. Writing...{waiting}",
            pass.name(),
            transcript.messages,
            if transcript.messages == 1 { "" } else { "s" },
        ));
        self.chat_llm.summary =
            Some(SummaryRun { answer: rx, cancel, pass, disclaimers, filter_was_on: self.chat_llm.settings.censor });
    }

    fn poll_quick_rundown(&mut self) -> bool {
        let Some(run) = &self.chat_llm.summary else { return false };
        let result = match run.answer.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("the summary thread stopped".to_owned()),
        };
        let Some(run) = self.chat_llm.summary.take() else { return false };
        match result {
            Err(error) => self.push_console_line(format!("^1QUICK RUNDOWN:^7 failed: {error}")),
            Ok(answer) => {
                for line in cl::read_summary(&answer) {
                    self.push_console_line(format!("^5SUMMARY:^7 {line}"));
                }
                match run.pass {
                    SummaryPass::Cleanse => {
                        for line in &run.disclaimers {
                            self.push_console_line(format!("^3{line}"));
                        }
                        let basis = if run.filter_was_on {
                            "Periods listed above had messages changed by the chat filter."
                        } else {
                            "The chat filter was off, so hostile periods are only as the model reported them."
                        };
                        self.push_console_line(format!(
                            "^3DISCLAIMER:^7 cleansed summary. Wording was softened and hostile content is described, not quoted. {basis} chatQRD truth gives the unfiltered account."
                        ));
                    }
                    SummaryPass::Truth => self.push_console_line(
                        "^3NOTE:^7 truth pass: written from the chat as it was sent, and may repeat offensive language. chatQRD cleanse gives a clean version.",
                    ),
                }
            }
        }
        true
    }

    /// Disconnect: stop a summary that is still being written.
    pub(in crate::app) fn cancel_quick_rundown(&mut self) {
        if let Some(run) = self.chat_llm.summary.take() {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// `chatLlmStatus`.
    pub(in crate::app) fn chat_llm_status_lines(&self) -> Vec<String> {
        let settings = &self.chat_llm.settings;
        let host = engine();
        let mut lines = Vec::new();
        if !host.available() {
            lines.push("^3CHAT MODELS:^7 this build has no local model runtime (built without local-llm)".to_owned());
        }
        for (role, url) in [("translation", settings.translate_model()), ("filter and summary", settings.assist_model())] {
            let cached = if host.cached(url) { "on disk" } else { "not downloaded" };
            lines.push(format!("^3CHAT MODELS:^7 {role}: {} ({cached}) {url}", host.state(url).describe()));
        }
        lines.push(format!(
            "^3CHAT FILTER:^7 {}, strictness {}, ragebait check {}, {}, {}",
            if settings.censor { "on" } else { "off" },
            settings.censor_severity,
            if settings.censor_ragebait { "on" } else { "off" },
            if settings.censor_action == CensorAction::Hide { "hides messages" } else { "masks words" },
            self.chat_llm.status_line(),
        ));
        lines.push(format!("^3QUICK RUNDOWN:^7 {} message(s) kept for chatQRD", self.chat_llm.history.len()));
        lines
    }

    /// The free cursor moved over the game: lines with marks show their
    /// received text while hovered.
    pub(in crate::app) fn chat_hover_moved(&mut self, position: (f64, f64)) {
        let wanted = (!self.captured && self.chat_lines.iter().any(|record| record.censor.is_some() || record.translation.is_some()))
            .then_some([position.0 as f32, position.1 as f32]);
        if wanted.is_none() && self.chat_llm.pointer.is_none() {
            return;
        }
        self.chat_llm.pointer = wanted;
        self.publish_transient_ui();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use winit::platform::windows::EventLoopBuilderExtWindows;

    #[test]
    fn chat_model_download_consent_and_visible_stop_control() {
        let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
            .with_any_thread(true).build().unwrap();
        let fixture = crate::cvar_compatibility::Fixture::new();
        std::fs::write(fixture.0.join("DinurdoJK.cfg"), "").unwrap();
        let mut app = App::new(fixture.0.clone(), None,
            crate::scene::MapSource::Bsp("mp/ffa3".into()), false, event_loop.create_proxy()).unwrap();
        engine().set_cache_dir(fixture.0.join("models"));
        let first = "https://example.invalid/first.gguf";
        let selected = "https://example.invalid/selected.gguf";
        app.set_console_cvar("cg_chatLlmModel", first).unwrap();
        app.set_console_cvar("cg_chatCensor", "1").unwrap();
        assert!(!app.chat_llm.settings.censor);
        assert_eq!(app.chat_llm.edits.download.as_ref().unwrap().url, first);
        assert_eq!(engine().state(first), ModelState::Idle);

        // Switching selections cancels the pending request without downloading.
        app.set_console_cvar("cg_chatLlmModel", selected).unwrap();
        assert!(app.chat_llm.edits.download.is_none());
        assert_eq!(engine().state(selected), ModelState::Idle);
        assert!(!app.chat_llm.settings.censor);
        app.set_console_cvar("cg_chatTranslateModel", selected).unwrap();
        app.set_console_cvar("cg_chatTranslateProvider", "local").unwrap();
        app.set_console_cvar("cg_chatTranslate", "2").unwrap();
        assert_eq!(app.chat_translation.settings.mode, Mode::Off);
        assert!(matches!(app.chat_llm.edits.download.as_ref().unwrap().usage, ChatModelUse::Translation(Mode::Auto)));
        assert_eq!(engine().state(selected), ModelState::Idle);

        // Simulate shared consumers and an in-flight summary without starting
        // inference or downloading a model. Click the real settings stop button.
        app.chat_llm.edits.download = None;
        app.chat_llm.approved_downloads.push(selected.to_owned());
        app.chat_llm.settings.censor = true;
        app.chat_llm.summary_used = true;
        app.chat_translation.settings.mode = Mode::Auto;
        let cancel = Arc::new(AtomicBool::new(false));
        let (_reply, answer) = mpsc::channel();
        app.chat_llm.summary = Some(SummaryRun {
            answer, cancel: Arc::clone(&cancel), pass: SummaryPass::Truth,
            disclaimers: Vec::new(), filter_was_on: true,
        });
        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 1600.0));
        let output = context.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            app.egui_chat_filter_settings(ui);
        });
        let stop = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "STOP & UNLOAD" => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        }).expect("stop button must remain visible beside model status");
        assert!(screen.contains_rect(stop));
        let position = stop.center();
        let events = vec![egui::Event::PointerMoved(position),
            egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }];
        let _ = context.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| {
            app.egui_chat_filter_settings(ui);
        });
        assert!(!app.chat_llm.settings.censor);
        assert_eq!(app.chat_translation.settings.mode, Mode::Off);
        assert!(!app.chat_llm.summary_used);
        assert!(app.chat_llm.summary.is_none());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.chat_llm.approved_downloads.is_empty());
        assert_eq!(engine().state(selected), ModelState::Idle);
    }
}
