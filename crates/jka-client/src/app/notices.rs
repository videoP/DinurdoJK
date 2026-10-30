//! Notices.
use crate::app::{App, Instant, UiScoreEntry, UiScoreboard};
use winit::window::UserAttentionType;

/// jaPRO `cg.crosshairClientNum` / `cg.crosshairClientTime`: the last entity the
/// crosshair was on. Names read it, so it keeps fading after aiming away.
#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct CrosshairSeen {
    pub(in crate::app) entity: i32,
    pub(in crate::app) is_pilot: bool,
    pub(in crate::app) at: Instant,
    /// The most recent scan (this frame's, in jaPRO terms) found it.
    pub(in crate::app) on_target: bool,
}

pub(in crate::app) struct ChatRecord {
    pub(in crate::app) text: String,
    pub(in crate::app) team: bool,
    pub(in crate::app) created: Instant,
    /// Demo timeline position when this line was authored. Live sessions use wall time.
    pub(in crate::app) demo_elapsed_ms: Option<f64>,
}

/// One row in the classic transparent top-left console-notify timestamp ring.
/// The back entry is the current row (often blank after a completed print), just
/// like TaystJK's `con.current`; `notify == false` represents a zero timestamp.
pub(in crate::app) struct ConsoleNotifyRecord {
    pub(in crate::app) text: String,
    pub(in crate::app) team: bool,
    pub(in crate::app) created: Instant,
    pub(in crate::app) demo_elapsed_ms: Option<f64>,
    /// False is TaystJK's `con.times[row] = 0`: the console row still consumes
    /// a slot, but `[skipnotify]` / leading `*` prevents it from being drawn.
    pub(in crate::app) notify: bool,
}

/// Retained state for repeated chatbox Tab completion. The first Tab captures
/// the original word's ranked candidates; later Tabs cycle that same set even
/// though the visible input has already been replaced by a completed name.
pub(in crate::app) struct ChatPlayerCompletionCycle {
    pub(in crate::app) word_start: usize,
    pub(in crate::app) candidates: Vec<String>,
    pub(in crate::app) index: usize,
}

pub(in crate::app) struct CenterPrintRecord {
    pub(in crate::app) text: String,
    pub(in crate::app) created: Instant,
    pub(in crate::app) demo_elapsed_ms: Option<f64>,
    /// CG_CenterPrint y in the legacy 480-high virtual coordinate space, as a
    /// screen fraction. Standard center prints use 0.30; TaystJK mode 3 uses 0.10.
    pub(in crate::app) y_fraction: f32,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct RewardSpec {
    pub(in crate::app) shader: &'static str,
    /// TaystJK's Q3 reward assets are optional in some game installs. Keep the
    /// stock JKA medal as a visual fallback without changing mode semantics.
    pub(in crate::app) fallback_shader: Option<&'static str>,
    pub(in crate::app) sound: &'static [&'static str],
    pub(in crate::app) count: i32,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct ActiveReward {
    pub(in crate::app) spec: RewardSpec,
    pub(in crate::app) started: Instant,
}

pub(in crate::app) const REWARD_TIME_MS: f32 = 3000.0;

pub(in crate::app) const REWARD_BLOB_MS: f32 = 200.0;

pub(in crate::app) const REWARD_ICON_SIZE: f32 = 48.0;

pub(in crate::app) const REWARD_MAX_STACK: usize = 10;

pub(in crate::app) const REWARD_CAPTURE_SOUND: &[&str] = &["sound/chars/protocol/misc/capture.wav"];

pub(in crate::app) const REWARD_IMPRESSIVE_SOUND: &[&str] = &["sound/chars/protocol/misc/40MOM025"];

pub(in crate::app) const REWARD_EXCELLENT_SOUND: &[&str] = &["sound/chars/protocol/misc/40MOM053"];

pub(in crate::app) const REWARD_HUMILIATION_SOUND: &[&str] =
    &["sound/chars/protocol/misc/40MOM019"];

pub(in crate::app) const REWARD_DEFEND_SOUND: &[&str] = &["sound/chars/protocol/misc/40MOM024"];

pub(in crate::app) const REWARD_ASSIST_SOUND: &[&str] = &["sound/chars/protocol/misc/40MOM026"];

pub(in crate::app) const REWARD_DENIED_SOUND: &[&str] = &["sound/chars/protocol/misc/40MOM017"];

pub(in crate::app) const REWARD_IMPRESSIVE_Q3_SOUND: &[&str] = &[
    "sound/feedback/impressive.wav",
    "sound/chars/protocol/misc/40MOM025",
];

pub(in crate::app) const REWARD_EXCELLENT_Q3_SOUND: &[&str] = &[
    "sound/feedback/excellent.wav",
    "sound/chars/protocol/misc/40MOM053",
];

pub(in crate::app) const REWARD_HUMILIATION_Q3_SOUND: &[&str] = &[
    "sound/feedback/humiliation.wav",
    "sound/chars/protocol/misc/40MOM019",
];

pub(in crate::app) const REWARD_DENIED_Q3_SOUND: &[&str] = &[
    "sound/feedback/denied.wav",
    "sound/chars/protocol/misc/40MOM017",
];

pub(in crate::app) fn tayst_reward_spec(
    kind: crate::cgame::RewardKind,
    mode: u8,
    count: i32,
) -> Option<RewardSpec> {
    use crate::cgame::RewardKind;
    let q3 = mode == 2;
    let (shader, fallback_shader, sound) = match kind {
        RewardKind::Capture => ("medal_capture", None, REWARD_CAPTURE_SOUND),
        RewardKind::Impressive if q3 => (
            "medal_impressiveQ3",
            Some("medal_impressive"),
            REWARD_IMPRESSIVE_Q3_SOUND,
        ),
        RewardKind::Impressive => ("medal_impressive", None, REWARD_IMPRESSIVE_SOUND),
        RewardKind::Excellent if q3 => (
            "medal_excellentQ3",
            Some("medal_excellent"),
            REWARD_EXCELLENT_Q3_SOUND,
        ),
        RewardKind::Excellent => ("medal_excellent", None, REWARD_EXCELLENT_SOUND),
        RewardKind::Humiliation if q3 => (
            "medal_gauntletQ3",
            Some("medal_gauntlet"),
            REWARD_HUMILIATION_Q3_SOUND,
        ),
        RewardKind::Humiliation => ("medal_gauntlet", None, REWARD_HUMILIATION_SOUND),
        // TaystJK only swaps the three frag awards above in mode 2.
        RewardKind::Defend => ("medal_defend", None, REWARD_DEFEND_SOUND),
        RewardKind::Assist => ("medal_assist", None, REWARD_ASSIST_SOUND),
        RewardKind::Denied | RewardKind::GauntletEvent => return None,
    };
    Some(RewardSpec {
        shader,
        fallback_shader,
        sound,
        count,
    })
}

pub(in crate::app) fn tayst_place_string(raw_rank: i32) -> String {
    const RANK_TIED_FLAG: i32 = 0x4000;
    let tied = raw_rank & RANK_TIED_FLAG != 0;
    let place = (raw_rank & !RANK_TIED_FLAG).max(0) + 1;
    let suffix = if (11..=13).contains(&(place % 100)) {
        "th"
    } else {
        match place % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    let color = match place {
        1 => "^4",
        2 => "^1",
        3 => "^3",
        _ => "^7",
    };
    if tied {
        format!("Tied for {color}{place}{suffix}^7")
    } else {
        format!("{color}{place}{suffix}^7")
    }
}

pub(in crate::app) fn charset_glyph_uv(glyph: u8) -> [f32; 4] {
    // OpenJK SCR_DrawSmallChar / this client's fixed charset renderer: the
    // logical atlas is 16x16, while each glyph occupies half a cell in U.
    let stride = 1.0 / 16.0;
    let u0 = f32::from(glyph & 15) * stride;
    let v0 = f32::from(glyph >> 4) * stride;
    [u0, v0, u0 + 1.0 / 32.0, v0 + stride]
}

impl App {
    /// CG_CheckSVStringEdRef against the lazily loaded StringEd table.
    pub(in crate::app) fn translate_server_text(&mut self, text: &[u8]) -> String {
        if self.stringed.is_none() {
            let table =
                jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                    .map(|mut assets| crate::cgame::stringed::StringEd::load(&mut assets))
                    .unwrap_or_default();
            devprintln!(1, "STRINGED: {} strings loaded", table.len());
            self.stringed = Some(table);
        }
        let translated = self
            .stringed
            .as_ref()
            .expect("loaded above")
            .translate_server_text(text);
        crate::cgame::bytes_to_lossless_ascii(&translated)
    }

    /// jaPRO CG_Chat_f `cg_chatSounds`: 1 plays the legacy talk beep for every
    /// chat line; 2 gives private messages and team chat their own beeps.
    pub(in crate::app) fn play_chat_sound(&mut self, kind: crate::cgame::ChatKind, text: &str) {
        use crate::cgame::ChatKind;
        const TALK: &str = "sound/player/talk.wav";
        const TEAM_CHAT: &str = "sound/movers/switches/button_11.mp3";
        const PRIVATE_CHAT: &[&str] = &[
            "sound/interface/commlink_off.mp3",
            "sound/movers/switches/button_15.mp3",
        ];
        let mode = self.audio.game.chat_sounds;
        if mode == 0 || kind == ChatKind::Voice || self.chat_sounds_muted_by_combat() {
            return;
        }
        let candidates: &[&str] = match (mode, kind) {
            // jaPRO tags a private message by the "^7]: ^6" between name and text.
            (2, ChatKind::Say) if text.contains("^7]: ^6") => PRIVATE_CHAT,
            (2, ChatKind::Team) => &[TEAM_CHAT],
            _ => &[TALK],
        };
        if let Some(sound) = self
            .game_session
            .as_mut()
            .and_then(|session| session.sound_presenter.as_mut())
        {
            sound.play_local_sound(candidates);
        }
    }

    /// Result of TaystJK-style chat notification matching. A normal word/name
    /// match is both highlighted and alerted; `con_notifywords -1` alerts every
    /// chat without pretending each line was a word match.
    fn console_chat_notification(&self, kind: crate::cgame::ChatKind, text: &str) -> (bool, bool) {
        if kind == crate::cgame::ChatKind::Voice {
            return (false, false);
        }

        let configured = self.console_notify_words.trim();
        let every_chat = self
            .console_notify_terms
            .first()
            .is_some_and(|word| word == "-1");
        let plain = crate::logging::strip_jka_colors(text).replace('\x19', "");
        let Some((_, body)) = plain.rsplit_once(':') else {
            return (false, every_chat);
        };
        let body = body.to_ascii_lowercase();

        // DinurdoJK extension requested by the client: the current player name
        // is always a notification term. Strip colour escapes before matching,
        // exactly as we do for the incoming chat body.
        let own_name = crate::logging::strip_jka_colors(&self.network.name)
            .replace('\x19', "")
            .trim()
            .to_ascii_lowercase();
        let own_match = !own_name.is_empty() && body.contains(&own_name);

        let word_match = configured != "0"
            && !every_chat
            && self
                .console_notify_terms
                .iter()
                .any(|word| body.contains(word.as_str()));

        let highlighted = own_match || word_match;
        let alert = highlighted || every_chat;
        (highlighted, alert)
    }

    /// Cross-platform equivalent of TaystJK's Windows `con_alert`: request OS
    /// attention only while unfocused. The optional beep is independent of
    /// focus so a highlighted mention still has audible feedback while playing.
    fn fire_console_notification_alert(&mut self) -> bool {
        if !self.window_focused {
            if let Some(window) = self.window.as_ref() {
                window.request_user_attention(Some(UserAttentionType::Informational));
            }
        }
        if self.console_notify_beep {
            if let Some(sound) = self
                .game_session
                .as_mut()
                .and_then(|session| session.sound_presenter.as_mut())
            {
                return sound.play_local_sound(&["sound/player/talk.wav"]);
            }
        }
        false
    }

    fn classify_tayst_print_notification(
        &self,
        raw: &str,
    ) -> (crate::logging::ConsoleLineKind, bool) {
        use crate::logging::ConsoleLineKind;
        let lower = raw.to_ascii_lowercase();
        let mut kind = ConsoleLineKind::Normal;
        let mut alert = false;

        // TaystJK performs both checks in order. A pathological line matching
        // both keeps the alert raised by either condition while the later vote
        // classification wins the stamp colour.
        if lower.contains("@@@plconnect")
            || lower.contains("@@@disconnect")
            || lower.contains("@@@was_kicked")
            || lower.contains("timed out")
        {
            kind = ConsoleLineKind::NotifyConnect;
            alert |= self.console_notify_connect;
        }
        if lower.contains("@@@plcalledvote") {
            kind = ConsoleLineKind::NotifyVote;
            alert |= self.console_notify_vote;
        }
        (kind, alert)
    }

    pub(in crate::app) fn queue_tayst_reward(
        &mut self,
        kind: crate::cgame::RewardKind,
        count: i32,
    ) {
        use crate::cgame::RewardKind;
        let mode = self.audio.game.draw_rewards;
        if mode == 0 {
            return;
        }
        // TaystJK leaves JA+ to its own award-event presentation path.
        if matches!(kind, RewardKind::Denied | RewardKind::GauntletEvent)
            && self.connected_server_mod() == crate::net::mod_support::ServerMod::Japlus
        {
            return;
        }
        let Some(session) = self.game_session.as_mut() else {
            return;
        };

        // TaystJK handles these player-event bits as announcer-only feedback,
        // not entries in the medal FIFO.
        if matches!(kind, RewardKind::Denied | RewardKind::GauntletEvent) {
            let sound = match (kind, mode == 2) {
                (RewardKind::Denied, true) => REWARD_DENIED_Q3_SOUND,
                (RewardKind::Denied, false) => REWARD_DENIED_SOUND,
                (RewardKind::GauntletEvent, true) => REWARD_HUMILIATION_Q3_SOUND,
                (RewardKind::GauntletEvent, false) => REWARD_HUMILIATION_SOUND,
                _ => unreachable!(),
            };
            if !session.suppress_audio {
                if let Some(presenter) = session.sound_presenter.as_mut() {
                    presenter.play_announcer_sound(sound);
                }
            }
            return;
        }

        let Some(spec) = tayst_reward_spec(kind, mode, count) else {
            return;
        };
        if session.reward_active.is_none() {
            if !session.suppress_audio {
                if let Some(presenter) = session.sound_presenter.as_mut() {
                    presenter.play_announcer_sound(spec.sound);
                }
            }
            session.reward_active = Some(ActiveReward {
                spec,
                started: Instant::now(),
            });
        } else if session.reward_queue.len() + 1 < REWARD_MAX_STACK {
            session.reward_queue.push_back(spec);
        }
    }

    /// Best-effort follower hint for the scoreboard. JKA does not send the
    /// followed client number for other spectators, but a following spectator's
    /// scoreboard ping can mirror the target's ping. Treat that only as a weak
    /// signal: the spectator must match three consecutive *different* local ping
    /// values. Duplicate score replies do not advance the streak, and any
    /// mismatch/absence resets it immediately.
    fn update_spectator_ping_guesses(&mut self, entries: &mut [UiScoreEntry]) {
        for entry in entries.iter_mut() {
            entry.spectating_you = false;
        }

        if !self.score_spectator_guess {
            for guess in &mut self.spectator_ping_guesses {
                guess.reset();
            }
            return;
        }

        // The displayed playerState can belong to the followed player while we
        // spectate. Use the netchan gamestate's client number instead: this is
        // the network-owned local slot, so "you" never silently means the POV.
        let local_client = self
            .net
            .as_ref()
            .map(|net| net.session().decoder().client_number);
        let Some(local_client) = local_client else {
            for guess in &mut self.spectator_ping_guesses {
                guess.reset();
            }
            return;
        };
        let Some(local_entry) = entries
            .iter()
            .find(|entry| entry.client == local_client && entry.team != 3 && entry.ping > 0)
        else {
            // Do not try to infer "spectating you" while we are a spectator or
            // when a listen/LAN server reports an uninformative zero ping.
            for guess in &mut self.spectator_ping_guesses {
                guess.reset();
            }
            return;
        };
        let local_ping = local_entry.ping;

        for client in 0..self.spectator_ping_guesses.len() {
            let entry = entries
                .iter_mut()
                .find(|entry| entry.client == client as i32 && entry.team == 3 && entry.ping >= 0);
            let guess = &mut self.spectator_ping_guesses[client];
            let Some(entry) = entry else {
                guess.reset();
                continue;
            };

            if entry.ping != local_ping {
                guess.reset();
                continue;
            }

            if guess.last_matching_ping != Some(local_ping) {
                guess.last_matching_ping = Some(local_ping);
                guess.distinct_match_streak = guess.distinct_match_streak.saturating_add(1);
                if guess.distinct_match_streak >= 3 {
                    guess.confirmed = true;
                }
            }
            entry.spectating_you = guess.confirmed;
        }
    }

    /// CG_ServerCommand text output for the console and chat box.
    pub(in crate::app) fn drain_cgame_notices(&mut self) {
        let Some(session) = self.game_session.as_mut() else {
            return;
        };
        let gametype = session.client_game.gametype();
        let log_live_chat = self.chat_log_enabled && session.live && !session.local;
        let chat_server_time = session
            .client_game
            .current_snapshot()
            .map(|snapshot| snapshot.server_time);
        let rewards_match_viewer = session.client_game.presentation_player_state().is_some();
        let notices = session.client_game.drain_notices();
        let misses = std::mem::take(&mut self.predictor.misses);
        for miss in misses {
            self.push_console_line(miss);
        }
        for notice in notices {
            match notice {
                crate::cgame::CgameNotice::Print(text) => {
                    // TaystJK classifies these before StringEd translation so its
                    // @@@ markers are still available. The stamp colour is
                    // semantic even when the desktop-alert cvar is disabled.
                    let raw = crate::cgame::bytes_to_lossless_ascii(&text);
                    let (line_kind, alert) = self.classify_tayst_print_notification(&raw);
                    if alert {
                        let _ = self.fire_console_notification_alert();
                    }
                    let text = self.translate_server_text(&text);
                    for line in text.lines().filter(|line| !line.is_empty()) {
                        if line_kind == crate::logging::ConsoleLineKind::Normal {
                            self.push_console_line(line.to_owned());
                        } else {
                            self.push_console_notification(line.to_owned(), line_kind);
                        }
                    }
                }
                crate::cgame::CgameNotice::Chat { team, kind, text, sender } => {
                    let text = crate::cgame::bytes_to_lossless_ascii(&text);
                    // Scoreboard mutes are client-side: the line never reaches
                    // the chat box, console or chat log.
                    if self.chat_sender_muted(sender, &text) {
                        continue;
                    }
                    if log_live_chat {
                        self.chat_log
                            .message(kind, team, text.clone(), chat_server_time);
                    }
                    // TaystJK CG_Chat_f: only ordinary chat/team chat is filtered.
                    // Keep the server's original text in the persistent chat log.
                    if self.clean_chatbox != 0 && matches!(kind, crate::cgame::ChatKind::Say | crate::cgame::ChatKind::Team) {
                        let uncolored = clean_chatbox_strip_codes(&text);
                        if kind == crate::cgame::ChatKind::Say
                            && (uncolored.to_ascii_lowercase().contains("media - currently playing: ")
                                || uncolored.to_ascii_lowercase().contains("hi everybody!")) {
                            continue;
                        }
                        // Compare the full original text, including the sender and color codes.
                        // TaystJK uses a prefix comparison; exact equality avoids incorrectly
                        // suppressing a longer follow-up message with a shared prefix.
                        if self.last_clean_chat_message.as_deref() == Some(text.as_str()) {
                            continue;
                        }
                    }
                    if matches!(kind, crate::cgame::ChatKind::Say | crate::cgame::ChatKind::Team) {
                        self.last_clean_chat_message = Some(text.clone());
                    }
                    let (highlighted, alert) = self.console_chat_notification(kind, &text);
                    let notify_beep_played = if alert {
                        self.fire_console_notification_alert()
                    } else {
                        false
                    };
                    // Do not stack the ordinary cg_chatSounds beep on top of the
                    // notification beep. If the notify sound could not play,
                    // preserve the normal chat sound path.
                    if !notify_beep_played {
                        self.play_chat_sound(kind, &text);
                    }
                    // TaystJK's enabled chatbox path calls
                    // `trap->Print("*%s\n", chatStr)`. CL_ConsolePrint strips
                    // the leading `*` and records a zero notify timestamp, so
                    // chat remains in console scrollback but never appears in
                    // Con_DrawNotify. A con_notifywords hit only changes the
                    // console stamp colour / desktop alert; it does not promote
                    // the chat line into the top-left notify overlay.
                    let console_kind = match (team, highlighted) {
                        (true, true) => crate::logging::ConsoleLineKind::TeamChatMention,
                        (true, false) => crate::logging::ConsoleLineKind::TeamChat,
                        (false, true) => crate::logging::ConsoleLineKind::NotifyMention,
                        (false, false) => crate::logging::ConsoleLineKind::Normal,
                    };
                    self.push_console_line_no_notify(text.clone(), console_kind);
                    if self.chat_lines.len() >= crate::app::chat_anim::KEEP_LINES {
                        self.chat_lines.pop_front();
                    }
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    // Keep raw server text in console/history/logs; clean only the
                    // dedicated chatbox presentation, as controlled by the cvar.
                    let display_text = clean_chatbox_colors(&text, self.clean_chatbox);
                    self.chat_lines.push_back(ChatRecord {
                        text: display_text,
                        team,
                        created: now,
                        demo_elapsed_ms,
                    });
                    self.last_chat_refresh = Instant::now();
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::CenterPrint(text) => {
                    // JKA/Tayst `cp` is presentation-only: CG_ServerCommand calls
                    // CG_CenterPrint and returns. It must not also become CG_Printf
                    // / console output. Every cp still resets the center-print time,
                    // including repeated text.
                    let rendered = self.translate_server_text(&text);
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    self.center_print = Some(CenterPrintRecord {
                        text: rendered,
                        created: now,
                        demo_elapsed_ms,
                        y_fraction: 0.30,
                    });
                    self.last_center_refresh = now;
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::SiegeCenterPrint(text) => {
                    // CG_DrawSiegeMessageNonMenu does *not* use the generic
                    // server @@@ grammar. A single leading @ names a complete
                    // StringEd key, then the result is fed to CG_CenterPrint at
                    // 30% screen height.
                    let _ = self.translate_server_text(b"x"); // lazily load StringEd
                    let raw = crate::cgame::bytes_to_lossless_ascii(&text);
                    let rendered = self.resolve_siege_text(&raw);
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    self.center_print = Some(CenterPrintRecord {
                        text: rendered,
                        created: now,
                        demo_elapsed_ms,
                        y_fraction: 0.30,
                    });
                    self.last_center_refresh = now;
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::SiegeRoundMessage(text) => {
                    // CG_SiegeRoundOver routes wonround/lostround through
                    // CG_DrawSiegeMessage(..., 0), i.e. UIMENU_SIEGEMESSAGE.
                    let _ = self.translate_server_text(b"x"); // lazily load StringEd
                    let raw = crate::cgame::bytes_to_lossless_ascii(&text);
                    let rendered = self.resolve_siege_text(&raw);
                    self.show_siege_round_message(rendered);
                }
                crate::cgame::CgameNotice::Obituary {
                    target,
                    attacker,
                    key,
                    target_client,
                    attacker_client,
                    mod_,
                } => {
                    self.push_killfeed(
                        (target_client, target.clone()),
                        attacker_client.zip(attacker.clone()),
                        mod_,
                    );
                    self.note_match_obituary(target_client, attacker_client, mod_);
                    // OpenJK asks StringEd for MP_INGAME_<key> and prints the
                    // result with names reset to white. Loading the table lazily
                    // keeps live play and demos on the same localization path.
                    let _ = self.translate_server_text(b"x");
                    let full_key = format!("MP_INGAME_{key}");
                    let localized = self
                        .stringed
                        .as_ref()
                        .map(|table| crate::cgame::bytes_to_lossless_ascii(table.get(&full_key)))
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| {
                            if attacker.is_some() {
                                "was killed by".to_owned()
                            } else {
                                "died".to_owned()
                            }
                        });
                    let target = crate::logging::jka_color_scope(&target, '7');
                    let line = if let Some(attacker) = attacker.as_deref() {
                        let attacker = crate::logging::jka_color_scope(attacker, '7');
                        format!("{target} {localized} {attacker}")
                    } else {
                        format!("{target} {localized}")
                    };
                    self.push_console_line(line);
                }
                crate::cgame::CgameNotice::KillMessage {
                    target,
                    rank,
                    score,
                    gametype,
                } => {
                    let mode = self.audio.game.kill_message;
                    if mode == 0 {
                        continue;
                    }
                    // Keep the same STRINGED keys as JAPro/TaystJK rather than
                    // embedding English in the event layer. Missing tables get
                    // small English fallbacks so the notification still works.
                    let _ = self.translate_server_text(b"x");
                    let killed = self
                        .stringed
                        .as_ref()
                        .map(|table| {
                            crate::cgame::bytes_to_lossless_ascii(
                                table.get("MP_INGAME_KILLED_MESSAGE"),
                            )
                        })
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "Killed".to_owned());
                    let place_with = self
                        .stringed
                        .as_ref()
                        .map(|table| {
                            crate::cgame::bytes_to_lossless_ascii(table.get("MP_INGAME_PLACE_WITH"))
                        })
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "place with".to_owned());
                    let target = crate::logging::jka_color_scope(&target, '7');
                    // EternalJK/TaystJK's score/position suffix is an FFA
                    // convenience. Mode 2 explicitly suppresses it.
                    let text = if mode != 2 && gametype == 0 {
                        match (rank, score) {
                            (Some(rank), Some(score)) => format!(
                                "{killed} {target}.\n{} {place_with} {score}.",
                                tayst_place_string(rank),
                            ),
                            // A reconstructed demo POV has no authoritative
                            // PERS_RANK/PERS_SCORE; do not append recorder data.
                            _ => format!("{killed} {target}"),
                        }
                    } else {
                        format!("{killed} {target}")
                    };
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    self.center_print = Some(CenterPrintRecord {
                        text,
                        created: now,
                        demo_elapsed_ms,
                        y_fraction: if mode >= 3 { 0.10 } else { 0.30 },
                    });
                    self.last_center_refresh = now;
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::Reward { kind, count } => {
                    // Reward counters live only in the recorded playerState. A
                    // reconstructed demo POV must not inherit the recorder's
                    // Excellent/medal/spree feedback.
                    if rewards_match_viewer {
                        self.queue_tayst_reward(kind, count);
                    }
                }
                crate::cgame::CgameNotice::ItemPickupLine { classname } => {
                    let _ = self.translate_server_text(b"x");
                    let table = self.stringed.as_ref();
                    let pickup = table.map_or(String::new(), |table| {
                        crate::cgame::bytes_to_lossless_ascii(table.get("MP_INGAME_PICKUPLINE"))
                    });
                    let name = table
                        .map(|table| {
                            crate::cgame::bytes_to_lossless_ascii(
                                table.get(&format!("SP_INGAME_{}", classname.to_ascii_uppercase())),
                            )
                        })
                        .filter(|text| !text.is_empty())
                        .unwrap_or(classname);
                    self.push_console_line(format!("{pickup} {name}"));
                }
                crate::cgame::CgameNotice::StringEd {
                    key,
                    player,
                    team,
                    center,
                } => {
                    // Loads the table on first use.
                    let _ = self.translate_server_text(b"x");
                    let template = crate::cgame::bytes_to_lossless_ascii(
                        self.stringed
                            .as_ref()
                            .map_or(&[][..], |table| table.get(key)),
                    );
                    if template.is_empty() {
                        continue;
                    }
                    // CG_PrintCTFMessage: `%s` becomes the team name; the player
                    // name leads the line either way.
                    let mut text = match (team, template.contains("%s")) {
                        (Some(team), true) => template.replace("%s", team),
                        _ => template,
                    };
                    if let Some(player) = player.as_deref() {
                        let player = crate::logging::jka_color_scope(player, '7');
                        text = format!("{player} {text}");
                    }
                    if center {
                        let now = Instant::now();
                        let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                        self.center_print = Some(CenterPrintRecord {
                            text: text.clone(),
                            created: now,
                            demo_elapsed_ms,
                            y_fraction: 0.30,
                        });
                        self.last_center_refresh = Instant::now();
                        self.publish_transient_ui();
                    } else {
                        self.push_console_line(text);
                    }
                }
                crate::cgame::CgameNotice::Scores {
                    team_scores,
                    entries,
                } => {
                    let local_deaths = self
                        .game_session
                        .as_ref()
                        .map(|session| session.client_deaths)
                        .unwrap_or([0; 32]);
                    let score_deaths_mode = self.score_deaths;
                    let mut mapped_entries: Vec<UiScoreEntry> = entries
                        .into_iter()
                        .map(|entry| {
                            let counted = usize::try_from(entry.client)
                                .ok()
                                .filter(|&client| client < local_deaths.len())
                                .map(|client| local_deaths[client]);
                            let deaths = match score_deaths_mode {
                                0 => None,
                                1 => entry.deaths,
                                // TaystJK mode 2 prefers the server's JA+/jaPRO
                                // death field and falls back to CG_Obituary count.
                                2 => entry.deaths.or(counted),
                                // Mode 3 is the explicit local/debug counter.
                                3 => counted,
                                _ => entry.deaths,
                            };
                            UiScoreEntry {
                                client: entry.client,
                                name: entry.name,
                                score: entry.score,
                                powerups: entry.powerups,
                                captures: entry.captures,
                                assists: entry.assist_count,
                                defends: entry.defend_count,
                                deaths,
                                ping: entry.ping,
                                time: entry.time,
                                team: entry.team,
                                siege_class: entry.siege_class,
                                siege_icon: None,
                                spectating_you: false,
                                muted: false,
                                crowned: false,
                                model: String::new(),
                                model_fallback: String::new(),
                                h2h: None,
                                accuracy: entry.accuracy,
                                excellent: entry.excellent_count,
                                gauntlet: entry.gauntlet_count,
                            }
                        })
                        .collect();
                    self.update_spectator_ping_guesses(&mut mapped_entries);
                    let scoreboard_icon_paths = if gametype == 7 {
                        self.game_session
                            .as_ref()
                            .map(|session| {
                                crate::app::siege::siege_scoreboard_icons(
                                    &mut mapped_entries,
                                    &session.siege_catalog,
                                )
                            })
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    // TaystJK suppresses score/deaths in Duel and CTF. Power
                    // Duel has the win/loss score path, so keep it there too.
                    let show_deaths = score_deaths_mode != 0
                        && !matches!(gametype, 3 | 4 | 8)
                        && mapped_entries.iter().any(|entry| entry.deaths.is_some());
                    self.scoreboard = Some(UiScoreboard {
                        team_scores,
                        // The live ready mask comes from the current snapshot at draw time,
                        // matching TaystJK CG_DrawClientScore rather than the `scores` payload.
                        ready_clients: 0,
                        // JKA team modes start at GT_TEAM (6): Team FFA, Siege,
                        // CTF and CTY. Duel/FFA scoreboards do not need team UI.
                        team_game: gametype >= 6,
                        ctf: gametype == 8,
                        siege: gametype == 7,
                        siege_win_team: self.game_session.as_ref()
                            .map(|session| session.client_game.siege_win_team())
                            .unwrap_or(0),
                        // GT_DUEL / GT_POWERDUEL.
                        spectator_scores: matches!(gametype, 3 | 4),
                        show_deaths,
                        icon_paths: scoreboard_icon_paths,
                        entries: mapped_entries,
                        chrome: Default::default(),
                    });
                    if self.scoreboard_should_show() {
                        self.publish_ui();
                    }
                    if self.companion_scoreboard_visible() {
                        self.mark_companion_dirty();
                    }
                }
                crate::cgame::CgameNotice::MapRestart => {
                    self.predictor.reset();
                    self.reset_match_stats();
                    for guess in &mut self.spectator_ping_guesses {
                        guess.reset();
                    }
                    if let Some(session) = self.game_session.as_mut() {
                        session.reward_active = None;
                        session.reward_queue.clear();
                        session.client_deaths = [0; 32];
                        session.siege_extended.fill(crate::app::session::types::SiegeExtendedClient::default());
                        session.siege_respawn_time = 0;
                        session.siege_last_countdown = 0;
                    }
                    self.push_console_line("^3map_restart");
                }
            }
        }
    }
}

/// Remove JKA color escapes for moderation filters. This does not affect the name
/// or the original console/chat log message.
fn clean_chatbox_strip_codes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'^' && i + 1 < bytes.len() && bytes[i + 1] != b'^' {
            i += 2;
        } else {
            let ch = text[i..].chars().next().expect("valid utf8 character");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Match TaystJK CG_ChatBox_AddString: the sender/punctuation retains its
/// original color codes, while mode 1 strips color codes from the message body
/// and mode 2 replaces an initial disallowed body color with ^2 (green).
fn clean_chatbox_colors(text: &str, mode: u8) -> String {
    if mode == 0 { return text.to_owned(); }
    let separator = ["^7: ^2", "^7): ^5", "^7]: ^6"]
        .iter()
        .find_map(|sep| text.find(sep).map(|i| (i + sep.len(), *sep)));
    let Some((body_start, _)) = separator else { return text.to_owned(); };
    let (prefix, body) = text.split_at(body_start);
    if mode == 1 { return format!("{prefix}{}", clean_chatbox_strip_codes(body)); }
    let bytes = body.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'^' && matches!(bytes[1], b'0' | b'1' | b'3'..=b'9') {
        return format!("{prefix}^2{}", &body[2..]);
    }
    text.to_owned()
}

#[cfg(test)]
mod clean_chatbox_tests {
    use super::*;
    #[test]
    fn mode_one_cleans_body_only() {
        assert_eq!(clean_chatbox_colors("^1Name^7: ^2^4hello ^6world", 1), "^1Name^7: ^2hello world");
    }
    #[test]
    fn mode_two_recolors_first_body_code() {
        assert_eq!(clean_chatbox_colors("^1Name^7: ^2^4hello ^6world", 2), "^1Name^7: ^2^2hello ^6world");
        assert_eq!(clean_chatbox_colors("^1Name^7: ^2plain", 2), "^1Name^7: ^2plain");
    }
    #[test]
    fn mode_zero_unchanged() {
        assert_eq!(clean_chatbox_colors("^1Name^7: ^2^4hi", 0), "^1Name^7: ^2^4hi");
    }
}
