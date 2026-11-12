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
    /// The checkbox reflects actual automatic behavior. A default player
    /// follows the global mode, while an explicit rule overrides it.
    pub(in crate::app) fn checked(self, global: Mode) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Default => global == Mode::Auto,
        }
    }

    pub(in crate::app) fn toggled(self, global: Mode) -> Self {
        if self.checked(global) { Self::Never } else { Self::Always }
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
        Arc::clone(self.config.get_or_insert_with(|| {
            let key = kind.secret_name().and_then(|name| crate::credential_store::read_secret(name).ok().flatten());
            Arc::new(ProviderConfig { kind, libre_url, key })
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
    if message.show_original {
        return None;
    }
    let marker = marker(source.as_deref(), target);
    let shown = if unicode { Some(text.clone()) } else { text::hud_text(text) };
    let Some(shown) = shown else {
        return Some(format!("{} {marker}^9 (HL2 console)", message.text));
    };
    Some(match (display, message.body_start) {
        (Display::Both, _) | (_, None) => format!("{} {marker}^7 {shown}", message.text),
        (_, Some(start)) => {
            let prefix: String = message.text.chars().take(start).collect();
            format!("{prefix}{shown} {marker}")
        }
    })
}

/// Decide whether and how to translate; `Err` is the reason not to.
fn plan(settings: &ct::Settings, message: &ChatMessage, ask: Ask, hint: Option<&'static str>) -> Result<Request, String> {
    let line = match message.body_start {
        Some(start) => message.text.chars().skip(start).collect::<String>(),
        // An unsplittable named chat line must never send the player's name
        // to the provider, even when translated manually from the console.
        // Anonymous lines retain the legacy explicit-translate fallback.
        None if ask == Ask::Manual && message.sender_name.is_empty() => message.text.clone(),
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
    let target = lang::resolve_target(&settings.target);
    let target_script = lang::find(target).map_or(lang::Script::Latin, |language| language.script);
    let already = || Err(format!("already {}", lang::name(target)));
    let source = match lang::detect(&text) {
        None => match hint {
            Some(hint) if hint == target && ask != Ask::Manual => return already(),
            Some(hint) if hint != target => Some(hint),
            _ if ask == Ask::Auto => return Err("no language detected".to_owned()),
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
            } else if detection.code == Some(target) && (sure || detection.script == target_script) {
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
            } else if ask == Ask::Auto {
                return Err("language uncertain".to_owned());
            } else {
                None
            }
        }
    };
    Ok(Request { source, target, text })
}

/// Chat-box text for a record: the line itself, or its translated form, plus
/// an optional second line for `Both`.
pub(in crate::app) fn hud_chat_text(display: Display, record: &ChatRecord) -> (String, Option<String>) {
    let Some(translation) = record.translation.as_ref().filter(|t| !t.show_original && display != Display::Original) else {
        return (record.text.clone(), None);
    };
    match (&translation.hud, display) {
        (Some(hud), Display::Both) => (record.text.clone(), Some(format!("  {}^7 {hud}", translation.marker))),
        (Some(hud), _) => (format!("{}{hud} {}", translation.prefix, translation.marker), None),
        (None, _) => (format!("{} {}^9 (HL2 console)", record.text, translation.marker), None),
    }
}

/// Only identify a sender if their *actual displayed name* is the unambiguous
/// prefix of a chat command. Names can include colons, so match against the
/// complete current client name, not the first `:` in an arbitrary line.
/// The explicit delimiters also prevent matching Bob to Bobby.
fn chat_prefix_matches_sender(text: &str, body_start: Option<usize>, name: &str) -> bool {
    let name = crate::logging::strip_jka_colors(name).trim().to_ascii_lowercase();
    if name.is_empty() { return false; }
    let prefix: String = match body_start {
        Some(start) => text.chars().take(start).collect(),
        None => text.to_owned(),
    };
    let prefix = crate::logging::strip_jka_colors(&prefix).trim_start().to_ascii_lowercase();
    let ordinary = format!("{name}: ");
    let team = format!("({name}): ");
    // For messages with a known body boundary also support the server's
    // located-chat format, where `<location>` follows the player name.
    prefix.starts_with(&ordinary) || prefix.starts_with(&team)
        || body_start.is_some() && (prefix.starts_with(&format!("{name}<"))
            || prefix.starts_with(&format!("({name})<")))
}

/// Derive the missing body offset for legacy `chat`/`tchat` commands that
/// omitted the `\x19` separator. This is only used *after* an unambiguous
/// live client name matched the displayed prefix. Count raw characters, not
/// visible glyphs, so JKA ^N colour codes remain in the prefix.
fn chat_body_start_for_named_sender(text: &str, name: &str) -> Option<usize> {
    let name = crate::logging::strip_jka_colors(name).trim().to_ascii_lowercase();
    if name.is_empty() { return None; }
    let mut visible = String::new();
    let mut raw_indices = Vec::new();
    let mut chars = text.chars().enumerate().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '^' && chars.peek().is_some_and(|(_, next)| next.is_ascii_digit()) {
            chars.next();
        } else {
            visible.push(ch);
            raw_indices.push(i);
        }
    }
    let visible_lower = visible.to_ascii_lowercase();
    let prefix_len = [format!("{name}: "), format!("({name}): ")].into_iter()
        .find(|pattern| visible_lower.starts_with(pattern))?
        .chars().count();
    // A message with no body is not a recognized speaker line.
    raw_indices.get(prefix_len).copied()
}

impl App {
    /// Called for every chat line that reaches the chat box, before its
    /// console print so the console row can carry the id.
    pub(in crate::app) fn record_chat_message(&mut self, chat: ReceivedChat<'_>) -> u64 {
        // Legacy/mod servers sometimes omit the numeric client argument.
        // Resolve the *displayed chat prefix* against current CS_PLAYERS, not
        // the potentially stale scoreboard. Ambiguous names get no rule UI.
        let fallback = if chat.sender.is_none()
            && chat.kind != crate::cgame::ChatKind::Voice
        {
            self.game_session.as_ref().and_then(|session| {
                let mut matching = session.client_game.player_id_list().into_iter()
                    .filter(|(_, _, name, _)| chat_prefix_matches_sender(chat.text, chat.body_start, name));
                let first = matching.next()?;
                matching.next().is_none().then_some((first.0 as i32, first.2))
            })
        } else { None };
        let sender = chat.sender.or_else(|| fallback.as_ref().map(|(slot, _)| *slot));
        let sender_display = fallback.as_ref().map_or(chat.sender_name.as_str(), |(_, name)| name.as_str());
        // A server can provide the sender's slot but omit the 0x19 chat
        // separator. Recover the body boundary for *both* known senders and
        // name-matched fallback senders; otherwise manual translation sends
        // `Name: message` instead of only `message`.
        let body_start = chat.body_start.or_else(|| chat_body_start_for_named_sender(chat.text, sender_display));
        let sender_label = crate::logging::strip_jka_colors(sender_display).trim().to_owned();
        let sender_name = sender_label.to_ascii_lowercase();
        let own = chat.own || sender.is_some_and(|slot| self.net.as_ref()
            .is_some_and(|net| net.session().client_num() == slot));
        let state = &mut self.chat_translation;
        // Someone else now holds the slot: its rule no longer applies.
        if let Some(slot) = sender {
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
            own,
            sender,
            sender_name,
            sender_label,
            text: chat.text.to_owned(),
            body_start,
            state: TranslationState::None,
            show_original: false,
            console_shown: chat.text.to_owned(),
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
        if message.kind == crate::cgame::ChatKind::Voice || message.own {
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
                self.finish_chat_translation(id, Ok(translated));
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

    fn finish_chat_translation(&mut self, id: u64, result: Result<Translated, String>) {
        let display = self.chat_translation.settings.display;
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
        let marker = marker(source.as_deref(), target);
        let hud = text::hud_text(&translated.text);
        message.show_original = display == Display::Original && ask == Ask::Auto;
        let show_original = message.show_original;
        let body_start = message.body_start;
        message.state = TranslationState::Done { source, target, text: translated.text };
        if matches!(target, "zh" | "ja" | "ko") && matches!(self.chat_translation.cjk_font, CjkFont::NotNeeded) {
            self.chat_translation.cjk_font = load_cjk_font(target);
        }
        // Update the chat-box line in place if it is still kept; an expired
        // line is not revived, the console keeps the translation.
        if let Some(record) = self.chat_lines.iter_mut().find(|record| record.chat_id == Some(id)) {
            let prefix = body_start.map_or_else(String::new, |start| record.text.chars().take(start).collect());
            record.translation = Some(ChatRecordTranslation { prefix, hud, marker, show_original });
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
                self.finish_chat_translation(id, result.clone());
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
    fn sync_console_chat_rows(&mut self, only: Option<u64>) {
        let display = self.chat_translation.settings.display;
        let mut changes = HashMap::new();
        for message in self.chat_translation.messages.iter_mut().filter(|message| only.is_none_or(|id| message.id == id)) {
            let wanted = console_text(display, message, false).unwrap_or_else(|| message.text.clone());
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
        let rule_target = self.console_chat_translate_checked(id).is_some();
        if rule_target && !off {
            ui.separator();
            let (mode, source) = state.rule_for(message).map_or((RuleMode::Default, None), |rule| (rule.mode, rule.source));
            ui.label(egui::RichText::new(&message.sender_label).strong());
            let checked = mode.checked(state.settings.mode);
            ui.horizontal(|ui| {
                let text = egui::RichText::new("A/a").monospace().strong().color(
                    if checked { egui::Color32::from_rgb(109, 220, 230) }
                    else { egui::Color32::from_rgb(159, 164, 166) }
                );
                let response = ui.add(egui::Button::new(text).selected(checked))
                    .on_hover_text("Toggle automatic translation for this player");
                ui.label("Translate this player");
                if response.clicked() {
                    action = Some(ChatMenuAction::Rule(id,
                        RuleChange::Mode(mode.toggled(state.settings.mode))));
                }
            });
            if mode != RuleMode::Default && ui.small_button("Use global chat setting").clicked() {
                action = Some(ChatMenuAction::Rule(id, RuleChange::Mode(RuleMode::Default)));
            }
            ui.menu_button(source.map_or("Auto-detect", lang::name), |ui| {
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
                "Source language when detection is uncertain. Clear detection still takes priority."
            );
            if mode == RuleMode::Default {
                ui.label(egui::RichText::new("Following global chat setting").weak().small());
            }
        } else if !off && !rule_target {
            // A translated line is not necessarily attributable to a current
            // player (e.g. old scrollback or servers that omit client IDs).
            // Don't silently hide the controls without explaining why.
            ui.separator();
            ui.label(egui::RichText::new("Player translation settings unavailable")
                .weak().small()).on_hover_text("No verified current player for this message");
        }
        action
    }

    /// A sender checkbox is shown only on eligible, still-recognized chat rows.
    /// The returned value includes the effective global automatic setting.
    pub(in crate::app) fn console_chat_translate_checked(&self, id: u64) -> Option<bool> {
        let state = &self.chat_translation;
        if state.settings.mode == Mode::Off { return None; }
        let message = state.message(id)?;
        let slot = message.sender?;
        if message.sender_name.is_empty() || message.own
            || message.kind == crate::cgame::ChatKind::Voice { return None; }
        // Console history can outlive a client in the same slot. Never let a
        // checkbox on an old message change a different player's rule.
        let name = self.game_session.as_ref()?
            .client_game.client_name(usize::try_from(slot).ok()?)?;
        if crate::logging::strip_jka_colors(&name).trim().to_ascii_lowercase() != message.sender_name {
            return None;
        }
        Some(state.rule_for(message).map_or(RuleMode::Default, |rule| rule.mode)
            .checked(state.settings.mode))
    }

    /// Reused by inline icons and the context menus, without duplicating rules.
    pub(in crate::app) fn toggle_console_chat_translate(&mut self, id: u64) {
        let Some(message) = self.chat_translation.message(id) else { return };
        let Some(slot) = message.sender else { return };
        if self.console_chat_translate_checked(id).is_none() { return; }
        let name = message.sender_label.clone();
        let mode = self.chat_translation.rule(slot, &name).0;
        let next = mode.toggled(self.chat_translation.settings.mode);
        self.change_chat_translation_rule(slot, &name, RuleChange::Mode(next));
        self.publish_ui();
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
                if self.console_chat_translate_checked(id).is_none() { return; }
                let Some(message) = self.chat_translation.message(id) else { return };
                let Some(slot) = message.sender else { return };
                let name = message.sender_label.clone();
                self.change_chat_translation_rule(slot, &name, change);
                self.publish_ui();
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

    /// Click the compact translation checkbox in the classic console's
    /// left margin. Derive the logical row from the same wrap layout as text
    /// selection; do not mistake a continuation row for a new message.
    pub(in crate::app) fn click_classic_console_translate_checkbox(
        &mut self, w: u32, h: u32, x: f64, y: f64,
    ) -> bool {
        if self.console_hl2 || self.overlay != OverlayMode::Console || !(3.0..18.0).contains(&x) {
            return false;
        }
        let Some((row, _)) = crate::ui::console_text_hit(
            w, h, self.console_size, self.console_scale, 20.0, y,
        ) else { return false; };
        let id = {
            let layout = self.console_wrap_layout();
            let capacity = crate::ui::console_visible_line_capacity(w, h, self.console_size, self.console_scale);
            let max_scroll = layout.rows.len().saturating_sub(capacity.max(1));
            let end = layout.rows.len().saturating_sub(self.console_scroll.min(max_scroll));
            let start = end.saturating_sub(capacity);
            layout.rows.get(start + row).filter(|segment| segment.start_col == 0)
                .and_then(|segment| self.console_chat_ids.get(segment.line).copied().flatten())
        };
        let Some(id) = id.filter(|&id| self.console_chat_translate_checked(id).is_some()) else { return false; };
        self.toggle_console_chat_translate(id);
        true
    }

    /// Classic console right-click: open the chat menu for that row.
    pub(in crate::app) fn open_console_chat_menu(&mut self, line: usize) {
        let id = self.console_chat_ids.get(line).copied().flatten().filter(|&id| self.chat_translation.has_message(id));
        self.chat_translation.console_menu = id.map(|id| (id, self.cursor_position));
        self.publish_ui();
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
                self.publish_ui();
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
            self.publish_ui();
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
            self.publish_ui();
        } else if clicked_outside {
            self.chat_translation.console_menu = None;
            self.publish_ui();
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
        }
    }

    fn english() -> ct::Settings {
        ct::Settings { mode: Mode::Auto, target: "en".to_owned(), ..ct::Settings::default() }
    }

    #[test]
    fn resolves_server_chat_without_sender_id_using_unambiguous_prefix() {
        assert!(chat_prefix_matches_sender("Padawan^7: ^2hola", None, "Padawan"));
        assert!(chat_prefix_matches_sender("(Padawan)^7: ^5hola", None, "^5Padawan"));
        assert!(chat_prefix_matches_sender("P:ad^7: ^2hello", None, "P:ad"));
        assert!(!chat_prefix_matches_sender("Padawan2: hello", None, "Padawan"));
        assert!(!chat_prefix_matches_sender("server says Padawan: hello", None, "Padawan"));
        assert!(!chat_prefix_matches_sender("Padawan", None, "Padawan"));
        assert_eq!(chat_body_start_for_named_sender("Padawan^7: ^2hola", "Padawan"), Some("Padawan^7: ^2".chars().count()));
        assert_eq!(chat_body_start_for_named_sender("(Padawan)^7: ^5hola", "^5Padawan"), Some("(Padawan)^7: ^5".chars().count()));
        assert_eq!(chat_body_start_for_named_sender("P:ad^7: ^2hello", "P:ad"), Some("P:ad^7: ^2".chars().count()));
        assert_eq!(chat_body_start_for_named_sender("Padawan2: hello", "Padawan"), None);
    }

    #[test]
    fn named_sender_without_server_separator_translates_only_chat_body() {
        // Some mod servers supply a numeric sender ID but omit JKA's 0x19
        // boundary. The sender name is already known from that client slot.
        for (line, name, body) in [
            ("Padawan^7: ^2hola amigos", "Padawan", "hola amigos"),
            ("(Padawan)^7: ^5hola equipo", "^5Padawan", "hola equipo"),
            ("P:ad^7: ^2buenos días", "P:ad", "buenos días"),
        ] {
            let start = chat_body_start_for_named_sender(line, name);
            let chat = message(line, start);
            let request = plan(&english(), &chat, Ask::Manual, Some("es")).unwrap();
            assert_eq!(request.text, body);
        }
        // A named sender with an unrecognized prefix must not fall back to
        // translating their name as part of the message.
        assert!(plan(&english(), &message("Padawan: hola", None), Ask::Manual, None).is_err());
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
        // ...and even a manual request must not send an unseparated named
        // chat line (potentially including that player's name).
        assert!(plan(&english(), &message(&russian, None), Ask::Manual, None).is_err());
        // Explicit translation of a genuinely anonymous line remains possible.
        let mut anonymous = message(&russian, None);
        anonymous.sender = None;
        anonymous.sender_name.clear();
        anonymous.sender_label.clear();
        assert_eq!(plan(&english(), &anonymous, Ask::Manual, None).unwrap().text, "Привет всем, как дела?");
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
    fn translation_checkbox_follows_global_mode_until_overridden() {
        assert!(!RuleMode::Default.checked(Mode::Manual));
        assert!(RuleMode::Default.checked(Mode::Auto));
        assert_eq!(RuleMode::Default.toggled(Mode::Manual), RuleMode::Always);
        assert_eq!(RuleMode::Default.toggled(Mode::Auto), RuleMode::Never);
        assert_eq!(RuleMode::Always.toggled(Mode::Manual), RuleMode::Never);
        assert_eq!(RuleMode::Never.toggled(Mode::Auto), RuleMode::Always);
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
