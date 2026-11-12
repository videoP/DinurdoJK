//! Chat translation in the app: every received chat line gets a stable id,
//! shared by its chat-box record and its console row. Translations arrive
//! asynchronously and enrich that message in place; the received text, chat
//! log, sounds, order and fade timing are never changed.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use crate::{
    app::{App, ChatRecord, OverlayMode},
    chat_translation::{self as ct, lang, text, Display, Mode, ProviderConfig, Request, Submit, Translated},
};

/// Messages kept for console actions; older rows lose their menu.
const KEEP_MESSAGES: usize = 512;

/// Why a translation is requested; decides which skip rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Ask {
    /// Automatic mode: only confident detections of another language.
    Auto,
    /// "Always translate" player rule: any language but the target.
    Rule,
    /// Picked in the console: sent whatever the detector thinks.
    Manual,
}

pub(in crate::app) enum TranslationState {
    None,
    Pending { source: Option<&'static str>, target: &'static str, ask: Ask },
    Done { source: Option<String>, target: &'static str, text: String },
    Skipped(String),
    Failed(String),
}

/// What the chat filter (`cg_chatCensor`) made of a received line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::app) enum Censor {
    /// Not checked: the filter is off or had nothing to look at.
    #[default]
    None,
    /// Waiting for the model. `held` keeps the text off screen meanwhile.
    Pending { held: bool },
    Clean,
    /// `body` replaces the message text. `hidden` means the whole message
    /// was withheld, so a translation of it is not shown either.
    Censored { body: String, hidden: bool },
}

/// Chat-box and console stand-in for a message the filter is still checking.
pub(in crate::app) const CENSOR_PENDING: &str = "^9...";

pub(in crate::app) struct ChatMessage {
    id: u64,
    kind: crate::cgame::ChatKind,
    team: bool,
    demo: bool,
    own: bool,
    /// Sending client slot, when the server named it.
    sender: Option<i32>,
    /// Sender's name without colours at receive time; with the slot it
    /// identifies the player for rules ("" when unknown).
    sender_name: String,
    /// The same name as shown in menus (case kept).
    sender_label: String,
    /// The line exactly as received (one char per protocol byte).
    text: String,
    /// Char index where the message body starts, past the sender prefix and
    /// the body colour code. `None` when the two could not be separated.
    body_start: Option<usize>,
    state: TranslationState,
    show_original: bool,
    /// What the message part of its stored console rows currently reads.
    console_shown: String,
    pub(in crate::app) censor: Censor,
    /// The shown translation had words masked by the filter.
    translation_censored: bool,
}

/// A received chat line, as `notices` hands it over.
pub(in crate::app) struct ReceivedChat<'a> {
    pub(in crate::app) kind: crate::cgame::ChatKind,
    pub(in crate::app) team: bool,
    pub(in crate::app) demo: bool,
    pub(in crate::app) own: bool,
    pub(in crate::app) sender: Option<i32>,
    pub(in crate::app) sender_name: String,
    pub(in crate::app) text: &'a str,
    pub(in crate::app) body_start: Option<usize>,
}

/// What a chat-box record needs to show its translation.
#[derive(Debug, Clone)]
pub(in crate::app) struct ChatRecordTranslation {
    /// Displayed text up to the body, including the body colour code.
    prefix: String,
    /// Translation in HUD form; `None` if the HUD font cannot draw it.
    hud: Option<String>,
    marker: String,
    show_original: bool,
    /// The filter masked words in the translation.
    censored: bool,
}

/// What a chat-box record shows instead of the received text while the
/// filter holds or has changed it.
#[derive(Debug, Clone)]
pub(in crate::app) struct ChatRecordCensor {
    /// Displayed text up to the body, including the body colour code.
    prefix: String,
    body: String,
    /// The filter objected (as opposed to still checking).
    flagged: bool,
    hidden: bool,
}

/// Chat-box line marks, drawn as icons before the sender's name.
pub(in crate::app) const FLAG_CENSORED: u8 = crate::ui::CHAT_FLAG_CENSORED;
pub(in crate::app) const FLAG_TRANSLATED: u8 = crate::ui::CHAT_FLAG_TRANSLATED;

/// A chat-box record as it is drawn.
pub(in crate::app) struct HudChat {
    pub(in crate::app) text: String,
    /// Second line for `Display::Both`.
    pub(in crate::app) below: Option<String>,
    pub(in crate::app) flags: u8,
    /// The line as received, for the local-only hover; set when `flags` is.
    pub(in crate::app) original: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::app) enum RuleMode {
    /// Follow `cg_chatTranslate` like everyone else.
    #[default]
    Default,
    Always,
    Never,
}

impl RuleMode {
    fn label(self) -> &'static str {
        match self {
            Self::Default => "Chat setting",
            Self::Always => "Always",
            Self::Never => "Never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum RuleChange {
    Mode(RuleMode),
    /// Language hint; `None` is automatic detection.
    Source(Option<&'static str>),
}

/// Session-only per-player rule. A client slot is reused by other players,
/// so the rule also remembers the name and stops matching when it changes.
struct PlayerRule {
    name: String,
    mode: RuleMode,
    /// What this player usually writes. A hint, not a fact: a confident
    /// detection of something else still wins.
    source: Option<&'static str>,
}

#[derive(Default)]
enum CjkFont {
    #[default]
    NotNeeded,
    Loading(std::sync::mpsc::Receiver<Option<(String, Vec<u8>)>>),
    Done,
}

#[derive(Default)]
pub(in crate::app) struct ChatTranslation {
    pub(in crate::app) settings: ct::Settings,
    translator: ct::Translator,
    messages: VecDeque<ChatMessage>,
    next_id: u64,
    config: Option<Arc<ProviderConfig>>,
    in_session: bool,
    cjk_font: CjkFont,
    /// Model address and instruction for the local provider, copied from
    /// the `cg_chatLlm*` settings.
    pub(in crate::app) local_model: (String, String),
    rules: HashMap<i32, PlayerRule>,
    /// Classic console right-click menu: message id and cursor position in
    /// physical pixels.
    console_menu: Option<(u64, (f64, f64))>,
    /// Setup page fields.
    pub(in crate::app) key_input: String,
    pub(in crate::app) url_input: Option<String>,
    pub(in crate::app) key_note: Option<String>,
}

pub(in crate::app) enum ChatMenuAction {
    Translate(u64),
    ToggleOriginal(u64),
    Copy(String),
    /// Change the rule for the message's sender.
    Rule(u64, RuleChange),
}

impl ChatTranslation {
    pub(in crate::app) fn new(settings: ct::Settings) -> Self {
        Self { settings, ..Self::default() }
    }

    pub(in crate::app) fn has_message(&self, id: u64) -> bool {
        self.message(id).is_some()
    }

    fn message(&self, id: u64) -> Option<&ChatMessage> {
        let index = self.messages.binary_search_by_key(&id, |message| message.id).ok()?;
        self.messages.get(index)
    }

    fn message_mut(&mut self, id: u64) -> Option<&mut ChatMessage> {
        let index = self.messages.binary_search_by_key(&id, |message| message.id).ok()?;
        self.messages.get_mut(index)
    }

    pub(in crate::app) fn message_ref(&self, id: u64) -> Option<&ChatMessage> {
        self.message(id)
    }

    pub(in crate::app) fn message_mut_ref(&mut self, id: u64) -> Option<&mut ChatMessage> {
        self.message_mut(id)
    }

    /// Forget the cached provider settings (they changed).
    pub(in crate::app) fn drop_config(&mut self) {
        self.config = None;
    }

    fn rule_for(&self, message: &ChatMessage) -> Option<&PlayerRule> {
        let rule = self.rules.get(&message.sender?)?;
        (!message.sender_name.is_empty() && rule.name == message.sender_name).then_some(rule)
    }

    /// Mode and language hint for a player, by slot and current name.
    pub(in crate::app) fn rule(&self, slot: i32, name: &str) -> (RuleMode, Option<&'static str>) {
        let name = crate::logging::strip_jka_colors(name).trim().to_ascii_lowercase();
        self.rules
            .get(&slot)
            .filter(|rule| rule.name == name)
            .map_or((RuleMode::Default, None), |rule| (rule.mode, rule.source))
    }

    /// Provider settings plus its key from Windows Credential Manager, read
    /// once and reused until the settings or the key change.
    fn config(&mut self) -> Arc<ProviderConfig> {
        let kind = self.settings.provider;
        let libre_url = self.settings.libre_url.clone();
        let (local_url, local_prompt) = self.local_model.clone();
        Arc::clone(self.config.get_or_insert_with(|| {
            let key = kind.secret_name().and_then(|name| crate::credential_store::read_secret(name).ok().flatten());
            Arc::new(ProviderConfig { kind, libre_url, key, local_url, local_prompt })
        }))
    }

    pub(in crate::app) fn status_line(&self) -> String {
        let status = &self.translator.status;
        let mut line = format!(
            "{} sent, {} translated, {} from cache, {} failed",
            status.sent, status.translated, status.cached, status.failed
        );
        if self.translator.in_flight() > 0 {
            line.push_str(&format!(", {} waiting", self.translator.in_flight()));
        }
        if !self.rules.is_empty() {
            line.push_str(&format!(", {} player rule(s)", self.rules.len()));
        }
        if let Some(error) = &status.last_error {
            line.push_str(&format!(". Last error: {error}"));
        }
        line
    }
}

fn marker(source: Option<&str>, target: &str) -> String {
    match source {
        Some(source) => format!("^9[{source}>{target}]"),
        None => format!("^9[{target}]"),
    }
}

/// The message part of a console row: `None` keeps the original. The
/// classic console's byte font only has Latin-1 glyphs (`unicode == false`);
/// the HL2 console can show any translation.
fn console_text(display: Display, message: &ChatMessage, unicode: bool) -> Option<String> {
    let TranslationState::Done { source, target, text } = &message.state else { return None };
    if message.show_original || message.hidden() {
        return None;
    }
    let marker = marker(source.as_deref(), target);
    let shown = if unicode { Some(text.clone()) } else { text::hud_text(text) };
    let line = message.shown_line();
    let Some(shown) = shown else {
        return Some(format!("{line} {marker}^9 (HL2 console)"));
    };
    Some(match (display, message.body_start) {
        (Display::Both, _) | (_, None) => format!("{line} {marker}^7 {shown}"),
        (_, Some(start)) => {
            let prefix: String = message.text.chars().take(start).collect();
            format!("{prefix}{shown} {marker}")
        }
    })
}

impl ChatMessage {
    pub(in crate::app) fn sender(&self) -> (Option<i32>, &str) {
        (self.sender, &self.sender_label)
    }

    pub(in crate::app) fn team(&self) -> bool {
        self.team
    }

    pub(in crate::app) fn is_voice(&self) -> bool {
        self.kind == crate::cgame::ChatKind::Voice
    }

    /// The line exactly as received.
    pub(in crate::app) fn received_line(&self) -> &str {
        &self.text
    }

    fn prefix(&self) -> String {
        self.body_start.map_or_else(String::new, |start| self.text.chars().take(start).collect())
    }

    /// The message body as received (the whole line when the sender could
    /// not be separated), still in wire form.
    pub(in crate::app) fn body(&self) -> String {
        match self.body_start {
            Some(start) => self.text.chars().skip(start).collect(),
            None => self.text.clone(),
        }
    }

    fn hidden(&self) -> bool {
        matches!(self.censor, Censor::Censored { hidden: true, .. })
    }

    /// The received line as it may be shown: unchanged, held back, or with
    /// the filter's text in place of the body. Never a translation.
    pub(in crate::app) fn shown_line(&self) -> String {
        match &self.censor {
            Censor::Pending { held: true } => format!("{}{CENSOR_PENDING}", self.prefix()),
            // Console rows are byte text: a body kept in Cyrillic for the
            // chat box falls back to the bare mask here.
            Censor::Censored { body, .. } => {
                let latin = text::hud_text(body);
                format!("{}{}", self.prefix(), latin.as_deref().unwrap_or(crate::chat_llm::MASK))
            }
            _ => self.text.clone(),
        }
    }

    /// Tell the message what its console row was first printed as.
    pub(in crate::app) fn set_console_shown(&mut self, shown: String) {
        self.console_shown = shown;
    }

    /// What the chat-box record needs to mirror `shown_line`.
    pub(in crate::app) fn record_censor(&self, record_text: &str) -> Option<ChatRecordCensor> {
        let prefix = || self.body_start.map_or_else(String::new, |start| record_text.chars().take(start).collect());
        match &self.censor {
            Censor::Pending { held: true } => {
                Some(ChatRecordCensor { prefix: prefix(), body: CENSOR_PENDING.to_owned(), flagged: false, hidden: false })
            }
            Censor::Censored { body, hidden } => {
                Some(ChatRecordCensor { prefix: prefix(), body: body.clone(), flagged: true, hidden: *hidden })
            }
            _ => None,
        }
    }
}

/// Decide whether and how to translate; `Err` is the reason not to.
fn plan(settings: &ct::Settings, message: &ChatMessage, ask: Ask, hint: Option<&'static str>) -> Result<Request, String> {
    let line = match message.body_start {
        Some(start) => message.text.chars().skip(start).collect::<String>(),
        // Without a known body only an explicit request may send the line,
        // name included.
        None if ask == Ask::Manual => message.text.clone(),
        None => return Err("sender and message could not be separated".to_owned()),
    };
    let body = crate::cgame::text_to_jka_bytes(&line);
    let hint_script = hint.and_then(lang::find).map(|language| language.script);
    // A Cyrillic speaker's non-UTF-8 chat is Windows-1251, whatever it looks like.
    let codepage = if settings.codepage == ct::Codepage::Auto
        && hint_script == Some(lang::Script::Cyrillic)
        && std::str::from_utf8(&body).is_err()
    {
        ct::Codepage::Windows1251
    } else {
        settings.codepage
    };
    let text = text::provider_text(&body, codepage, usize::from(settings.max_chars));
    if text.is_empty() {
        return Err("nothing to translate".to_owned());
    }
    if ask != Ask::Manual {
        if let Some(reason) = text::auto_skip_reason(&text) {
            return Err(reason.to_owned());
        }
    }
    // A web service is only asked when the detector is confident, to spare
    // its quota and keep chat off the network. The local model costs neither:
    // it gets every line that is not clearly the target language already, and
    // a translation that comes back unchanged is dropped (see
    // `finish_chat_translation`).
    let local = settings.provider == ct::ProviderKind::Local;
    let target = lang::resolve_target(&settings.target);
    let target_script = lang::find(target).map_or(lang::Script::Latin, |language| language.script);
    let already = || Err(format!("already {}", lang::name(target)));
    let source = match lang::detect(&text) {
        None => match hint {
            Some(hint) if hint == target && ask != Ask::Manual => return already(),
            Some(hint) if hint != target => Some(hint),
            _ if ask == Ask::Auto && !local => return Err("no language detected".to_owned()),
            _ => None,
        },
        Some(detection) => {
            let sure = detection.confidence >= 0.9;
            // An unsure detector defers to the player's usual language, if
            // the text is in that language's writing system.
            let hint = hint.filter(|_| !sure && hint_script == Some(detection.script));
            if let Some(hint) = hint {
                if hint == target {
                    if ask != Ask::Manual {
                        return already();
                    }
                    None
                } else {
                    Some(hint)
                }
            } else if detection.code == Some(target) && (sure || (!local && detection.script == target_script)) {
                if ask != Ask::Manual {
                    return already();
                }
                // Asked for anyway: let the service decide what it is.
                None
            } else if detection.script != target_script {
                // A different writing system is reason enough on its own.
                detection.code.filter(|_| detection.confidence >= 0.5)
            } else if sure && detection.code.is_some() {
                detection.code
            } else if ask == Ask::Auto && !local {
                return Err("language uncertain".to_owned());
            } else {
                None
            }
        }
    };
    Ok(Request { source, target, text })
}

/// A chat-box record as drawn: the line itself, the filter's version of it,
/// or its translated form, plus an optional second line for `Both`.
///
/// `wide` says the chat box can draw Cyrillic (the docked styles): the hover
/// then shows the received line decoded instead of byte for byte.
pub(in crate::app) fn hud_chat(display: Display, record: &ChatRecord, wide: bool) -> HudChat {
    let censor = record.censor.as_ref();
    let line = censor.map_or_else(|| record.text.clone(), |censor| format!("{}{}", censor.prefix, censor.body));
    let mut flags = if censor.is_some_and(|censor| censor.flagged) { FLAG_CENSORED } else { 0 };
    let translation = record
        .translation
        .as_ref()
        .filter(|t| !t.show_original && display != Display::Original)
        // A withheld message stays withheld in every language.
        .filter(|_| !censor.is_some_and(|censor| censor.hidden));
    let (text, below) = match translation {
        None => (line, None),
        Some(translation) => {
            flags |= FLAG_TRANSLATED;
            if translation.censored {
                flags |= FLAG_CENSORED;
            }
            match (&translation.hud, display) {
                (Some(hud), Display::Both) => (line, Some(format!("  {}^7 {hud}", translation.marker))),
                (Some(hud), _) => (format!("{}{hud} {}", translation.prefix, translation.marker), None),
                (None, _) => (format!("{line} {}^9 (HL2 console)", translation.marker), None),
            }
        }
    };
    let original = (flags != 0).then(|| {
        let decoded = text::decode(&crate::cgame::text_to_jka_bytes(&record.text), ct::Codepage::Auto);
        text::hud_text_wide(&decoded, true).filter(|_| wide).unwrap_or_else(|| record.text.clone())
    });
    HudChat { text, below, flags, original }
}

impl App {
    /// Called for every chat line that reaches the chat box, before its
    /// console print so the console row can carry the id.
    pub(in crate::app) fn record_chat_message(&mut self, chat: ReceivedChat<'_>) -> u64 {
        let state = &mut self.chat_translation;
        let sender_label = crate::logging::strip_jka_colors(&chat.sender_name).trim().to_owned();
        let sender_name = sender_label.to_ascii_lowercase();
        // Someone else now holds the slot: its rule no longer applies.
        if let Some(slot) = chat.sender {
            if state.rules.get(&slot).is_some_and(|rule| rule.name != sender_name) {
                state.rules.remove(&slot);
            }
        }
        state.next_id += 1;
        if state.messages.len() >= KEEP_MESSAGES {
            state.messages.pop_front();
        }
        state.messages.push_back(ChatMessage {
            id: state.next_id,
            kind: chat.kind,
            team: chat.team,
            demo: chat.demo,
            own: chat.own,
            sender: chat.sender,
            sender_name,
            sender_label,
            text: chat.text.to_owned(),
            body_start: chat.body_start,
            state: TranslationState::None,
            show_original: false,
            console_shown: chat.text.to_owned(),
            censor: Censor::None,
            translation_censored: false,
        });
        state.next_id
    }

    /// Automatic mode and player rules: after the message is visible, never before.
    pub(in crate::app) fn auto_translate_chat(&mut self, id: u64) {
        let state = &self.chat_translation;
        let settings = &state.settings;
        if settings.mode == Mode::Off {
            return;
        }
        let Some(message) = state.message(id) else { return };
        if message.kind == crate::cgame::ChatKind::Voice {
            return;
        }
        // cg_chatTranslateIgnoreSelf: you know what you wrote. A message of
        // your own can still be translated by hand from the console.
        if message.own && self.chat_llm.settings.translate_ignore_self {
            return;
        }
        let ask = match state.rule_for(message).map_or(RuleMode::Default, |rule| rule.mode) {
            RuleMode::Never => return,
            // An explicit rule covers team and demo chat, and Manual mode.
            RuleMode::Always => Ask::Rule,
            RuleMode::Default
                if settings.mode == Mode::Auto
                    && (!message.team || settings.team)
                    && (!message.demo || settings.demos) => Ask::Auto,
            RuleMode::Default => return,
        };
        self.request_chat_translation(id, ask);
    }

    pub(in crate::app) fn request_chat_translation(&mut self, id: u64, ask: Ask) {
        let state = &mut self.chat_translation;
        if state.settings.mode == Mode::Off {
            return;
        }
        let Some(message) = state.message(id) else { return };
        if matches!(message.state, TranslationState::Pending { .. } | TranslationState::Done { .. }) {
            return;
        }
        let hint = state.rule_for(message).and_then(|rule| rule.source);
        let request = match plan(&state.settings, message, ask, hint) {
            Ok(request) => request,
            Err(reason) => {
                if let Some(message) = state.message_mut(id) {
                    message.state = TranslationState::Skipped(reason);
                }
                return;
            }
        };
        let config = state.config();
        let (source, target) = (request.source, request.target);
        if let Some(message) = state.message_mut(id) {
            message.state = TranslationState::Pending { source, target, ask };
        }
        let key = ct::Key { provider: config.kind, request };
        match state.translator.submit(key, id, &config) {
            Submit::Cached(translated) => {
                self.filter_then_finish_chat_translation(id, Ok(translated));
                self.publish_transient_ui();
            }
            Submit::Pending => {}
            Submit::Busy => {
                if let Some(message) = state.message_mut(id) {
                    message.state = TranslationState::Failed("too many translations waiting".to_owned());
                }
            }
        }
    }

    /// `censored` says the filter masked words in this translation.
    pub(in crate::app) fn finish_chat_translation(&mut self, id: u64, result: Result<Translated, String>, censored: bool) {
        let display = self.chat_translation.settings.display;
        let (codepage, max_chars) =
            (self.chat_translation.settings.codepage, usize::from(self.chat_translation.settings.max_chars));
        let Some(message) = self.chat_translation.message_mut(id) else { return };
        let TranslationState::Pending { source, target, ask } = message.state else { return };
        let translated = match result {
            Ok(translated) => translated,
            Err(error) => {
                message.state = TranslationState::Failed(error);
                return;
            }
        };
        let source = translated.detected.filter(|code| !code.is_empty()).or(source.map(str::to_owned));
        if ask != Ask::Manual && source.as_deref() == Some(target) {
            message.state = TranslationState::Skipped(format!("already {}", lang::name(target)));
            return;
        }
        // The message came back as it went in: it was the target language.
        let same = |a: &str, b: &str| {
            let fold = |value: &str| -> String { value.chars().filter(|ch| ch.is_alphanumeric()).flat_map(char::to_lowercase).collect() };
            fold(a) == fold(b)
        };
        let sent = text::provider_text(&crate::cgame::text_to_jka_bytes(&message.body()), codepage, max_chars);
        if ask != Ask::Manual && same(&sent, &translated.text) {
            message.state = TranslationState::Skipped(format!("already {}", lang::name(target)));
            return;
        }
        let marker = marker(source.as_deref(), target);
        let hud = text::hud_text_wide(&translated.text, self.chat_settings.docked());
        message.show_original = display == Display::Original && ask == Ask::Auto;
        let show_original = message.show_original;
        let body_start = message.body_start;
        message.translation_censored = censored;
        message.state = TranslationState::Done { source, target, text: translated.text };
        if matches!(target, "zh" | "ja" | "ko") && matches!(self.chat_translation.cjk_font, CjkFont::NotNeeded) {
            self.chat_translation.cjk_font = load_cjk_font(target);
        }
        // Update the chat-box line in place if it is still kept; an expired
        // line is not revived, the console keeps the translation.
        if let Some(record) = self.chat_lines.iter_mut().find(|record| record.chat_id == Some(id)) {
            let prefix = body_start.map_or_else(String::new, |start| record.text.chars().take(start).collect());
            record.translation = Some(ChatRecordTranslation { prefix, hud, marker, show_original, censored });
        }
        self.sync_console_chat_rows(Some(id));
    }

    /// Once per frame. Free when translation has never been used.
    pub(in crate::app) fn poll_chat_translations(&mut self) {
        let in_session = self.game_session.is_some();
        if in_session != self.chat_translation.in_session {
            self.chat_translation.in_session = in_session;
            if !in_session {
                // Player rules are per connection: slots mean nothing elsewhere.
                self.chat_translation.rules.clear();
                self.cancel_chat_translations();
                self.cancel_chat_filter();
                self.cancel_quick_rundown();
            }
        }
        if !self.chat_translation.translator.active() {
            return;
        }
        let finished = self.chat_translation.translator.poll();
        if finished.is_empty() {
            return;
        }
        for (ids, result) in finished {
            for id in ids {
                self.filter_then_finish_chat_translation(id, result.clone());
            }
        }
        self.publish_transient_ui();
    }

    /// Disconnect, demo seek or a settings change: drop waiting requests.
    /// Their messages can be translated again from the console.
    pub(in crate::app) fn cancel_chat_translations(&mut self) {
        for id in self.chat_translation.translator.cancel_all() {
            if let Some(message) = self.chat_translation.message_mut(id) {
                message.state = TranslationState::None;
            }
        }
    }

    pub(in crate::app) fn chat_translation_settings_changed(&mut self) {
        self.chat_translation.config = None;
        self.cancel_chat_translations();
        // The local provider's model is loaded only while it is selected.
        self.chat_llm_configure();
        self.mark_config_dirty();
        self.publish_ui();
        self.publish_transient_ui();
    }

    pub(in crate::app) fn chat_translation_display_changed(&mut self) {
        self.sync_console_chat_rows(None);
        self.mark_config_dirty();
        self.publish_ui();
        self.publish_transient_ui();
    }

    /// Rewrite stored console rows of translated (or toggled) messages, so the
    /// classic console, its selection and copy all see the shown text. The
    /// chat log on disk keeps the original. `only` limits it to one message.
    pub(in crate::app) fn sync_console_chat_rows(&mut self, only: Option<u64>) {
        let display = self.chat_translation.settings.display;
        let mut changes = HashMap::new();
        for message in self.chat_translation.messages.iter_mut().filter(|message| only.is_none_or(|id| message.id == id)) {
            let wanted = console_text(display, message, false).unwrap_or_else(|| message.shown_line());
            if wanted != message.console_shown {
                let shown = std::mem::replace(&mut message.console_shown, wanted.clone());
                changes.insert(message.id, (shown, wanted));
            }
        }
        let Some(&oldest) = changes.keys().min() else { return };
        let mut changed = false;
        // Ids grow with the console, so scan from the newest row back.
        for index in (0..self.console_chat_ids.len()).rev() {
            let Some(id) = self.console_chat_ids[index] else { continue };
            if id < oldest {
                break;
            }
            let Some((shown, wanted)) = changes.get(&id) else { continue };
            let Some(row) = self.console_lines.get(index).and_then(|row| row.strip_suffix(shown.as_str())).map(|stamp| format!("{stamp}{wanted}")) else {
                continue;
            };
            self.console_lines[index] = row;
            changed = true;
            let touches = |point: Option<crate::app::ConsolePoint>| point.is_some_and(|point| point.line == index);
            if touches(self.console_selection_anchor) || touches(self.console_selection_focus) {
                self.console_selection_anchor = None;
                self.console_selection_focus = None;
                self.console_selecting = false;
            }
        }
        if changed {
            self.console_reflow.borrow_mut().valid = false;
            if self.console_search_open {
                self.rebuild_console_search(false);
            }
            self.mark_companion_console_dirty();
            if self.overlay == OverlayMode::Console {
                self.publish_ui();
            }
        }
    }

    fn toggle_chat_translation_original(&mut self, id: u64) {
        let Some(message) = self.chat_translation.message_mut(id) else { return };
        message.show_original = !message.show_original;
        let show_original = message.show_original;
        if let Some(translation) = self
            .chat_lines
            .iter_mut()
            .find(|record| record.chat_id == Some(id))
            .and_then(|record| record.translation.as_mut())
        {
            translation.show_original = show_original;
        }
        self.sync_console_chat_rows(Some(id));
        self.publish_transient_ui();
    }

    /// HL2 console row for a translated message, with the full Unicode
    /// translation; `None` to show the stored row as is.
    pub(in crate::app) fn console_chat_translation_line(&self, line: &str, id: u64) -> Option<String> {
        let message = self.chat_translation.message(id)?;
        let shown = console_text(self.chat_translation.settings.display, message, true)?;
        let stamp = line.strip_suffix(message.console_shown.as_str())?;
        Some(format!("{stamp}{shown}"))
    }

    /// The message as received, decoded, when what is shown differs from it:
    /// the HL2 console's hover text. egui never reaches the stream output.
    pub(in crate::app) fn console_chat_original(&self, id: u64) -> Option<String> {
        let state = &self.chat_translation;
        let message = state.message(id)?;
        let changed = matches!(message.censor, Censor::Censored { .. })
            || (matches!(message.state, TranslationState::Done { .. }) && !message.show_original);
        changed.then(|| {
            let bytes = text::strip_color_bytes(&crate::cgame::text_to_jka_bytes(&message.text));
            format!("Original (never on the stream output):\n{}", text::decode(&bytes, state.settings.codepage))
        })
    }

    /// Right-click menu entries for a console chat row.
    pub(in crate::app) fn chat_translation_menu(&self, ui: &mut egui::Ui, id: u64) -> Option<ChatMenuAction> {
        let state = &self.chat_translation;
        let message = state.message(id)?;
        let mut action = None;
        let off = state.settings.mode == Mode::Off;
        match &message.state {
            TranslationState::Pending { .. } => {
                ui.label(egui::RichText::new("Translating...").weak());
            }
            TranslationState::Done { text, .. } => {
                let label = if message.show_original { "Show translation" } else { "Show original" };
                if ui.button(label).clicked() {
                    action = Some(ChatMenuAction::ToggleOriginal(id));
                }
                if ui.button("Copy translation").clicked() {
                    action = Some(ChatMenuAction::Copy(text.clone()));
                }
            }
            other => {
                let label = if matches!(other, TranslationState::Failed(_)) { "Retry translation" } else { "Translate message" };
                let response = ui.add_enabled(!off, egui::Button::new(label));
                let response = if off {
                    response.on_disabled_hover_text("Turn on chat translation in Setup > Interface > Translation.")
                } else {
                    response
                };
                if response.clicked() {
                    action = Some(ChatMenuAction::Translate(id));
                }
                match other {
                    TranslationState::Failed(error) => {
                        ui.label(egui::RichText::new(error).weak().small());
                    }
                    TranslationState::Skipped(reason) => {
                        ui.label(egui::RichText::new(format!("Not translated: {reason}")).weak().small());
                    }
                    _ => {}
                }
            }
        }
        if ui.button("Copy original").clicked() {
            let decoded = text::decode(&text::strip_color_bytes(&crate::cgame::text_to_jka_bytes(&message.text)), state.settings.codepage);
            action = Some(ChatMenuAction::Copy(decoded));
        }
        let rule_target = message.sender.is_some()
            && !message.sender_name.is_empty()
            && !message.own
            && message.kind != crate::cgame::ChatKind::Voice;
        if rule_target && !off {
            ui.separator();
            let (mode, source) = state.rule_for(message).map_or((RuleMode::Default, None), |rule| (rule.mode, rule.source));
            // Use the same per-player choices as the scoreboard's
            // TRANSLATION... menu, without maintaining another rule path.
            // The action is applied after the console UI frame, so all
            // menus (including the classic console popup) share the handler.
            ui.menu_button(format!("Translation: {}", message.sender_label), |ui| {
                let mode_text = match mode {
                    RuleMode::Default => "Chat setting",
                    RuleMode::Always => "Always",
                    RuleMode::Never => "Never",
                };
                ui.label(egui::RichText::new(format!(
                    "{mode_text}, {}", source.map_or("Auto-detect", lang::name)
                )).weak());
                ui.separator();
                if mode != RuleMode::Always && ui.button("Always translate").clicked() {
                    action = Some(ChatMenuAction::Rule(id, RuleChange::Mode(RuleMode::Always)));
                }
                if mode != RuleMode::Never && ui.button("Never translate").clicked() {
                    action = Some(ChatMenuAction::Rule(id, RuleChange::Mode(RuleMode::Never)));
                }
                if mode != RuleMode::Default && ui.button("Use chat setting").clicked() {
                    action = Some(ChatMenuAction::Rule(id, RuleChange::Mode(RuleMode::Default)));
                }
                ui.menu_button("Language hint", |ui| {
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        if ui.selectable_label(source.is_none(), "Auto-detect").clicked() {
                            action = Some(ChatMenuAction::Rule(id, RuleChange::Source(None)));
                        }
                        for language in lang::LANGUAGES {
                            if ui.selectable_label(source == Some(language.code), language.name).clicked() {
                                action = Some(ChatMenuAction::Rule(id, RuleChange::Source(Some(language.code))));
                            }
                        }
                    });
                }).response.on_hover_text(
                    "A hint used when detection is unsure; a clear detection of another language still wins."
                );
            });
        }
        action
    }

    pub(in crate::app) fn apply_chat_menu_action(&mut self, action: ChatMenuAction) {
        match action {
            ChatMenuAction::Translate(id) => {
                if let Some(message) = self.chat_translation.message_mut(id) {
                    if !matches!(message.state, TranslationState::Done { .. }) {
                        message.state = TranslationState::None;
                    }
                }
                self.request_chat_translation(id, Ask::Manual);
            }
            ChatMenuAction::ToggleOriginal(id) => self.toggle_chat_translation_original(id),
            ChatMenuAction::Copy(text) => {
                if let Err(error) = crate::clipboard::set_text(&text) {
                    self.push_console_error(format!("Copy failed: {error}"));
                }
            }
            ChatMenuAction::Rule(id, change) => {
                let Some(message) = self.chat_translation.message(id) else { return };
                let Some(slot) = message.sender else { return };
                let name = message.sender_label.clone();
                self.change_chat_translation_rule(slot, &name, change);
                // Picking "Always" on a message translates it now as well.
                if change == RuleChange::Mode(RuleMode::Always) {
                    if let Some(message) = self.chat_translation.message_mut(id) {
                        if !matches!(message.state, TranslationState::Done { .. }) {
                            message.state = TranslationState::None;
                        }
                    }
                    self.request_chat_translation(id, Ask::Manual);
                }
            }
        }
    }

    /// Shared by the console menu and the scoreboard popup. `name` may carry
    /// colour codes. A rule back at defaults is dropped.
    pub(in crate::app) fn change_chat_translation_rule(&mut self, slot: i32, name: &str, change: RuleChange) {
        let name = crate::logging::strip_jka_colors(name).trim().to_ascii_lowercase();
        if name.is_empty() {
            return;
        }
        let rules = &mut self.chat_translation.rules;
        let rule = rules.entry(slot).or_insert_with(|| PlayerRule { name: name.clone(), mode: RuleMode::Default, source: None });
        if rule.name != name {
            *rule = PlayerRule { name, mode: RuleMode::Default, source: None };
        }
        match change {
            RuleChange::Mode(mode) => rule.mode = mode,
            RuleChange::Source(source) => rule.source = source,
        }
        let mode = rule.mode;
        if mode == RuleMode::Default && rule.source.is_none() {
            rules.remove(&slot);
        }
        if mode == RuleMode::Always {
            self.translate_recent_chat_from(slot);
        }
    }

    /// Messages from `slot` still in the chat box get translated when the
    /// player is switched to "Always".
    fn translate_recent_chat_from(&mut self, slot: i32) {
        let ids: Vec<u64> = self.chat_lines.iter().filter_map(|record| record.chat_id).collect();
        for id in ids {
            let Some(message) = self.chat_translation.message(id) else { continue };
            if message.sender != Some(slot) || self.chat_translation.rule_for(message).is_none() {
                continue;
            }
            if let Some(message) = self.chat_translation.message_mut(id) {
                if matches!(message.state, TranslationState::Skipped(_) | TranslationState::Failed(_)) {
                    message.state = TranslationState::None;
                }
            }
            self.auto_translate_chat(id);
        }
    }

    /// Classic console right-click: open the chat menu for that row.
    pub(in crate::app) fn open_console_chat_menu(&mut self, line: usize) {
        let id = self.console_chat_ids.get(line).copied().flatten().filter(|&id| self.chat_translation.has_message(id));
        self.chat_translation.console_menu = id.map(|id| (id, self.cursor_position));
        self.egui_repaint_requested = true;
    }

    /// The classic console is not egui; its chat menu is a lone egui popup
    /// painted over it while open.
    pub(in crate::app) fn classic_console_chat_menu_open(&self) -> bool {
        self.chat_translation.console_menu.is_some() && self.overlay == OverlayMode::Console && !self.console_hl2
    }

    /// Pointer input belongs to the popup while it is open; Escape closes it.
    pub(in crate::app) fn route_console_chat_menu_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{ElementState, KeyEvent, WindowEvent};
        use winit::keyboard::{KeyCode, PhysicalKey};
        if !self.classic_console_chat_menu_open() {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(KeyCode::Escape), state: ElementState::Pressed, .. },
                ..
            } => {
                self.chat_translation.console_menu = None;
                self.egui_repaint_requested = true;
                true
            }
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {
                if let WindowEvent::CursorMoved { position, .. } = event {
                    self.cursor_position = (position.x, position.y);
                }
                if let (Some(window), Some(state)) = (self.window.as_ref(), self.egui_state.as_mut()) {
                    let response = state.on_window_event(window, event);
                    self.egui_repaint_requested |= response.repaint;
                }
                true
            }
            _ => false,
        }
    }

    pub(in crate::app) fn egui_console_chat_menu(&mut self, ctx: &egui::Context) {
        let Some((id, (x, y))) = self.chat_translation.console_menu else { return };
        if !self.classic_console_chat_menu_open() || !self.chat_translation.has_message(id) {
            self.chat_translation.console_menu = None;
            return;
        }
        let points = ctx.pixels_per_point().max(0.1);
        let pos = egui::pos2(x as f32 / points, y as f32 / points);
        let mut action = None;
        let area = egui::Area::new(egui::Id::new("classic_console_chat_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(180.0);
                    action = self.chat_translation_menu(ui, id);
                });
            });
        let clicked_outside = ctx.input(|input| input.pointer.any_pressed())
            && !ctx.pointer_interact_pos().is_some_and(|pointer| area.response.rect.contains(pointer));
        if let Some(action) = action {
            self.chat_translation.console_menu = None;
            self.apply_chat_menu_action(action);
        } else if clicked_outside {
            self.chat_translation.console_menu = None;
        }
    }

    /// Chinese, Japanese and Korean translations need a system font in the
    /// console; it is read off-thread the first time one arrives.
    pub(in crate::app) fn ensure_chat_translation_fonts(&mut self, ctx: &egui::Context) {
        let CjkFont::Loading(rx) = &self.chat_translation.cjk_font else { return };
        let Ok(loaded) = rx.try_recv() else { return };
        self.chat_translation.cjk_font = CjkFont::Done;
        let Some((name, bytes)) = loaded else { return };
        use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
        let families = [egui::FontFamily::Monospace, egui::FontFamily::Proportional]
            .into_iter()
            .map(|family| InsertFontFamily { family, priority: FontPriority::Lowest })
            .collect();
        ctx.add_font(FontInsert::new(&name, egui::FontData::from_owned(bytes), families));
    }

    pub(in crate::app) fn save_chat_translation_key(&mut self) {
        let state = &mut self.chat_translation;
        let Some(name) = state.settings.provider.secret_name() else { return };
        let key = state.key_input.trim().to_owned();
        state.key_input.clear();
        let result = if key.is_empty() {
            crate::credential_store::delete_secret(name).map(|()| "API key removed.")
        } else {
            crate::credential_store::save_secret(name, &key).map(|()| "API key saved in Windows Credential Manager.")
        };
        state.key_note = Some(result.map_or_else(|error| format!("Could not save the key: {error}"), str::to_owned));
        state.config = None;
        self.cancel_chat_translations();
    }

    pub(in crate::app) fn chat_translation_has_key(&mut self) -> bool {
        self.chat_translation.config().key.as_deref().is_some_and(|key| !key.is_empty())
    }
}

fn load_cjk_font(target: &str) -> CjkFont {
    let files: &[&str] = match target {
        "ja" => &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"],
        "ko" => &["malgun.ttf", "gulim.ttc"],
        _ => &["msyh.ttc", "simsun.ttc", "msjh.ttc"],
    };
    let paths: Vec<_> = crate::ui::system_font_dirs()
        .into_iter()
        .flat_map(|dir| files.iter().map(move |file| dir.join(file)))
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("chat-translate-font".to_owned()).spawn(move || {
        let found = paths.iter().find_map(|path| {
            let bytes = std::fs::read(path).ok()?;
            Some((path.file_stem()?.to_string_lossy().into_owned(), bytes))
        });
        let _ = tx.send(found);
    });
    CjkFont::Loading(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str, body_start: Option<usize>) -> ChatMessage {
        ChatMessage {
            id: 1,
            kind: crate::cgame::ChatKind::Say,
            team: false,
            demo: false,
            own: false,
            sender: Some(3),
            sender_name: "ivan".to_owned(),
            sender_label: "Ivan".to_owned(),
            text: text.to_owned(),
            body_start,
            state: TranslationState::None,
            show_original: false,
            console_shown: text.to_owned(),
            censor: Censor::None,
            translation_censored: false,
        }
    }

    fn english() -> ct::Settings {
        ct::Settings { mode: Mode::Auto, target: "en".to_owned(), ..ct::Settings::default() }
    }

    #[test]
    fn only_the_body_is_sent() {
        let line = crate::cgame::bytes_to_lossless_ascii("Ivan^7: ^2Привет всем, как дела?".as_bytes());
        let start = "Ivan^7: ^2".len();
        let request = plan(&english(), &message(&line, Some(start)), Ask::Auto, None).unwrap();
        assert_eq!(request.text, "Привет всем, как дела?");
        assert_eq!((request.source, request.target), (Some("ru"), "en"));
    }

    #[test]
    fn automatic_mode_skips_target_language_and_shorthand() {
        let start = "Bob^7: ^2".len();
        let english_line = "Bob^7: ^2where is the enemy flag carrier going";
        assert!(plan(&english(), &message(english_line, Some(start)), Ask::Auto, None).is_err());
        assert!(plan(&english(), &message(english_line, Some(start)), Ask::Rule, None).is_err());
        assert!(plan(&english(), &message("Bob^7: ^2gg wp", Some(start)), Ask::Rule, None).is_err());
        let russian = crate::cgame::bytes_to_lossless_ascii("Привет всем, как дела?".as_bytes());
        // Without a separable body nothing is sent automatically...
        assert!(plan(&english(), &message(&russian, None), Ask::Auto, None).is_err());
        assert!(plan(&english(), &message(&russian, None), Ask::Rule, None).is_err());
        // ...but an explicit request may send the whole line.
        assert_eq!(plan(&english(), &message(&russian, None), Ask::Manual, None).unwrap().text, "Привет всем, как дела?");
    }

    #[test]
    fn local_model_gets_lines_the_detector_is_unsure_about() {
        let start = "Bob^7: ^2".len();
        let local = ct::Settings { provider: ct::ProviderKind::Local, ..english() };
        // Too short for the detector: a web service is not asked, the local
        // model is, without a source language.
        let short = message("Bob^7: ^2vamos todos", Some(start));
        assert!(plan(&english(), &short, Ask::Auto, None).is_err());
        let request = plan(&local, &short, Ask::Auto, None).unwrap();
        assert_eq!((request.text.as_str(), request.target), ("vamos todos", "en"));
        // Clearly the target language already, or chat shorthand: still skipped.
        let english_line = message("Bob^7: ^2where is the enemy flag carrier going right now", Some(start));
        assert!(plan(&local, &english_line, Ask::Auto, None).is_err());
        assert!(plan(&local, &message("Bob^7: ^2gg wp", Some(start)), Ask::Auto, None).is_err());
    }

    #[test]
    fn classic_console_rows_fold_or_point_to_the_hl2_console() {
        let start = "Bob^7: ^2".len();
        let mut done = message("Bob^7: ^2hola amigos", Some(start));
        done.state = TranslationState::Done { source: Some("es".to_owned()), target: "en", text: "hello “friends”".to_owned() };
        assert_eq!(console_text(Display::Translated, &done, false).unwrap(), "Bob^7: ^2hello \"friends\" ^9[es>en]");
        done.state = TranslationState::Done { source: Some("en".to_owned()), target: "ru", text: "привет".to_owned() };
        assert!(console_text(Display::Translated, &done, false).unwrap().ends_with("(HL2 console)"));
        assert_eq!(console_text(Display::Translated, &done, true).unwrap(), "Bob^7: ^2привет ^9[en>ru]");
        done.show_original = true;
        assert!(console_text(Display::Translated, &done, true).is_none());
    }

    #[test]
    fn player_rules_stop_matching_when_the_slot_changes_hands() {
        let mut state = ChatTranslation::default();
        state.rules.insert(3, PlayerRule { name: "ivan".to_owned(), mode: RuleMode::Always, source: Some("ru") });
        let mut chat = message("x", None);
        assert_eq!(state.rule_for(&chat).map(|rule| rule.mode), Some(RuleMode::Always));
        assert_eq!(state.rule(3, "^1Ivan"), (RuleMode::Always, Some("ru")));
        chat.sender_name = "someone else".to_owned();
        assert!(state.rule_for(&chat).is_none());
        chat.sender = None;
        assert!(state.rule_for(&chat).is_none());
    }

    fn record(text: &str) -> ChatRecord {
        ChatRecord {
            text: text.to_owned(),
            team: false,
            created: std::time::Instant::now(),
            demo_elapsed_ms: None,
            chat_id: Some(1),
            translation: None,
            censor: None,
        }
    }

    #[test]
    fn filtered_lines_keep_the_name_and_never_show_the_received_text() {
        let line = "Bob^7: ^2you utter muppet";
        let start = "Bob^7: ^2".len();
        let mut chat = message(line, Some(start));
        assert_eq!(chat.shown_line(), line);
        chat.censor = Censor::Pending { held: true };
        assert_eq!(chat.shown_line(), "Bob^7: ^2^9...");
        chat.censor = Censor::Pending { held: false };
        assert_eq!(chat.shown_line(), line);
        chat.censor = Censor::Censored { body: "you utter ***".to_owned(), hidden: false };
        assert_eq!(chat.shown_line(), "Bob^7: ^2you utter ***");

        let mut shown = record(line);
        shown.censor = chat.record_censor(line);
        let hud = hud_chat(Display::Translated, &shown, false);
        assert_eq!(hud.text, "Bob^7: ^2you utter ***");
        assert_eq!(hud.flags, FLAG_CENSORED);
        // The received line travels only as the local hover text.
        assert_eq!(hud.original.as_deref(), Some(line));

        // A translation of a masked message shows both marks, and the
        // console's "both" form builds on the masked line.
        chat.state = TranslationState::Done { source: Some("en".to_owned()), target: "de", text: "du ***".to_owned() };
        assert_eq!(console_text(Display::Both, &chat, false).unwrap(), "Bob^7: ^2you utter *** ^9[en>de]^7 du ***");
        shown.translation = Some(ChatRecordTranslation {
            prefix: "Bob^7: ^2".to_owned(),
            hud: Some("du ***".to_owned()),
            marker: "^9[en>de]".to_owned(),
            show_original: false,
            censored: true,
        });
        let hud = hud_chat(Display::Translated, &shown, false);
        assert_eq!((hud.text.as_str(), hud.flags), ("Bob^7: ^2du *** ^9[en>de]", FLAG_CENSORED | FLAG_TRANSLATED));

        // A withheld message is withheld in every language.
        chat.censor = Censor::Censored { body: "^9[hidden]".to_owned(), hidden: true };
        assert!(console_text(Display::Translated, &chat, false).is_none());
        shown.censor = chat.record_censor(line);
        let hud = hud_chat(Display::Translated, &shown, false);
        assert_eq!((hud.text.as_str(), hud.flags), ("Bob^7: ^2^9[hidden]", FLAG_CENSORED));
    }

    #[test]
    fn docked_chat_keeps_cyrillic_and_the_console_falls_back() {
        let line = crate::cgame::bytes_to_lossless_ascii("Ivan^7: ^2ты полный дурак".as_bytes());
        let start = "Ivan^7: ^2".len();
        let mut chat = message(&line, Some(start));
        chat.censor = Censor::Censored { body: "ты полный ***".to_owned(), hidden: false };
        // The byte-font console cannot draw it.
        assert_eq!(chat.shown_line(), "Ivan^7: ^2***");
        let mut shown = record(&line);
        shown.censor = chat.record_censor(&line);
        let hud = hud_chat(Display::Translated, &shown, true);
        assert_eq!(hud.text, "Ivan^7: ^2ты полный ***");
        // The hover shows what was written, readable.
        assert_eq!(hud.original.as_deref(), Some("Ivan^7: ^2ты полный дурак"));
        // Without a font for it the hover stays byte for byte.
        assert_eq!(hud_chat(Display::Translated, &shown, false).original.as_deref(), Some(line.as_str()));
    }

    #[test]
    fn untouched_lines_have_no_marks_and_no_hover() {
        let hud = hud_chat(Display::Translated, &record("Bob^7: ^2hello"), false);
        assert_eq!((hud.text.as_str(), hud.flags, hud.original), ("Bob^7: ^2hello", 0, None));
    }

    #[test]
    fn language_hint_helps_only_unsure_same_script_detection() {
        let start = "Bob^7: ^2".len();
        // Short Latin text: automatic detection gives up, the hint decides.
        let short = message("Bob^7: ^2vamos todos", Some(start));
        assert_eq!(plan(&english(), &short, Ask::Rule, Some("es")).unwrap().source, Some("es"));
        // A hint for another writing system is ignored.
        let russian = crate::cgame::bytes_to_lossless_ascii("Bob^7: ^2Привет всем, как дела?".as_bytes());
        assert_eq!(plan(&english(), &message(&russian, Some(start)), Ask::Rule, Some("es")).unwrap().source, Some("ru"));
        // Old Windows-1251 bytes decode as Cyrillic for a Russian speaker.
        let (cp1251, _, _) = encoding_rs::WINDOWS_1251.encode("Привет");
        let line = crate::cgame::bytes_to_lossless_ascii(&[b"Bob^7: ^2".as_slice(), &cp1251].concat());
        assert_eq!(plan(&english(), &message(&line, Some(start)), Ask::Rule, Some("ru")).unwrap().text, "Привет");
    }
}
