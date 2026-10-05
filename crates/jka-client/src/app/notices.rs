//! Notices.
use crate::app::{App, Instant, UiScoreEntry, UiScoreboard};

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
    pub(in crate::app) created: Instant,
    /// Demo timeline position when this line was authored. Live sessions use wall time.
    pub(in crate::app) demo_elapsed_ms: Option<f64>,
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
        if mode == 0 || kind == ChatKind::Voice {
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
                    let text = self.translate_server_text(&text);
                    for line in text.lines().filter(|line| !line.is_empty()) {
                        self.push_console_line(line.to_owned());
                    }
                }
                crate::cgame::CgameNotice::Chat { team, kind, text } => {
                    let text = crate::cgame::bytes_to_lossless_ascii(&text);
                    if log_live_chat {
                        self.chat_log
                            .message(kind, team, text.clone(), chat_server_time);
                    }
                    self.play_chat_sound(kind, &text);
                    self.push_console_line(text.clone());
                    if self.chat_lines.len() >= 16 {
                        self.chat_lines.pop_front();
                    }
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    self.chat_lines.push_back(ChatRecord {
                        text,
                        created: now,
                        demo_elapsed_ms,
                    });
                    self.last_chat_refresh = Instant::now();
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::CenterPrint(text) => {
                    // Every cp resets CG_CenterPrint time, including repeats.
                    // Keep console de-duplication separate from presentation.
                    let rendered = self.translate_server_text(&text);
                    let now = Instant::now();
                    let demo_elapsed_ms = self.current_demo_elapsed_ms(now);
                    self.center_print = Some(CenterPrintRecord {
                        text: rendered.clone(),
                        created: now,
                        demo_elapsed_ms,
                        y_fraction: 0.30,
                    });
                    self.last_center_refresh = Instant::now();
                    if text != self.last_center_print {
                        for line in rendered.lines().filter(|line| !line.trim().is_empty()) {
                            self.push_console_line(line.to_owned());
                        }
                        self.last_center_print = text;
                    }
                    self.publish_transient_ui();
                }
                crate::cgame::CgameNotice::Obituary {
                    target,
                    attacker,
                    key,
                } => {
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
                    let line = if let Some(attacker) = attacker {
                        format!("{target}^7 {localized} {attacker}^7")
                    } else {
                        format!("{target}^7 {localized}")
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
                    let target = format!("{target}^7");
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
                    if let Some(player) = player {
                        text = format!("{player}^7 {text}");
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
                    let mapped_entries: Vec<UiScoreEntry> = entries
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
                                deaths,
                                ping: entry.ping,
                                time: entry.time,
                                team: entry.team,
                            }
                        })
                        .collect();
                    // TaystJK suppresses score/deaths in Duel and CTF. Power
                    // Duel has the win/loss score path, so keep it there too.
                    let show_deaths = score_deaths_mode != 0
                        && !matches!(gametype, 3 | 4 | 8)
                        && mapped_entries.iter().any(|entry| entry.deaths.is_some());
                    self.scoreboard = Some(UiScoreboard {
                        team_scores,
                        // JKA team modes start at GT_TEAM (6): Team FFA, Siege,
                        // CTF and CTY. Duel/FFA scoreboards do not need team UI.
                        team_game: gametype >= 6,
                        // GT_DUEL / GT_POWERDUEL.
                        spectator_scores: matches!(gametype, 3 | 4),
                        show_deaths,
                        entries: mapped_entries,
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
                    if let Some(session) = self.game_session.as_mut() {
                        session.reward_active = None;
                        session.reward_queue.clear();
                        session.client_deaths = [0; 32];
                    }
                    self.push_console_line("^3map_restart");
                }
            }
        }
    }
}
