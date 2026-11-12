//! Scoreboard interaction: +altattack while the board is up toggles the mouse,
//! a click on a player opens a popup (follow, mute, vote kick, vote spectate), and
//! the footer offers YES/NO while a vote is open. Hit testing uses the same
//! Taffy layout the renderer draws (`ui::scoreboard_hit_test`).
//!
//! Mutes are purely client-side: a muted client's chat and voice commands are
//! dropped before they reach the chat box, console or chat log, and the
//! scoreboard marks their row.
use crate::app::chat_translation::{RuleChange, RuleMode};
use crate::app::{App, Instant, OverlayMode};
use crate::ui::{
    ScoreAction, ScoreboardChrome, ScoreboardCursorUi, ScoreboardHit, ScoreboardMenuUi,
    UiScoreboard,
};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

#[derive(Default)]
pub(in crate::app) struct ScoreboardCursor {
    pub(in crate::app) active: bool,
    pointer: Option<[f32; 2]>,
    hovered: Option<ScoreboardHit>,
    menu: Option<ScoreboardMenuUi>,
    /// Playtime second last published, so the footer clock ticks.
    clock_second: Option<u64>,
    /// First visible scoreboard line; the mouse wheel moves it.
    scroll: u16,
    /// Footer toggle: personal stats instead of the player list.
    stats_open: bool,
}

/// `HH:MM` local wall-clock time.
fn local_hhmm() -> String {
    let [hour, minute, _] = crate::logging::local_hms();
    format!("{hour:02}:{minute:02}")
}

fn clock(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

/// `n` of a player's userinfo with colour codes removed, lower-cased.
fn plain_name(name: &str) -> String {
    crate::logging::strip_jka_colors(name).trim().to_ascii_lowercase()
}

/// Context menu actions: only a live spectator can follow a playing target.
/// Mute and voting remain available to ordinary players.
fn score_actions(own: bool, spectating: bool, playable: bool, muted: bool, live: bool, japro: bool,
    translate: Option<(bool, Option<&'static str>, bool)>) -> Vec<ScoreAction> {
    if own { return Vec::new(); }
    let mut actions = Vec::new();
    if spectating && playable {
        actions.push(ScoreAction::Follow);
    }
    actions.push(if muted { ScoreAction::Unmute } else { ScoreAction::Mute });
    // Show the checkbox and source selector immediately in the player popup;
    // neither requires opening a second "Translation..." submenu.
    if live {
        if let Some((checked, source, overridden)) = translate {
            actions.push(ScoreAction::Translate(checked));
            actions.push(ScoreAction::TranslateLanguages(source));
            if overridden { actions.push(ScoreAction::TranslateDefault); }
        }
    }
    if live {
        actions.push(ScoreAction::VoteKick);
        if japro { actions.push(ScoreAction::ForceSpectate); }
    }
    actions
}

/// This is the single-step picker shown by clicking the inline Aa icon.
/// Keep its action list declarative so the model and renderer share one order.
fn translation_picker_actions(checked: bool, source: Option<&'static str>, overridden: bool) -> Vec<ScoreAction> {
    let mut actions = Vec::with_capacity(14);
    actions.push(ScoreAction::Translate(checked));
    for (index, &(code, _)) in crate::ui::SCOREBOARD_TRANSLATE_LANGUAGES.iter().enumerate() {
        actions.push(ScoreAction::TranslateHint(index as u8, (if code.is_empty() { None } else { Some(code) }) == source));
    }
    if overridden {
        actions.push(ScoreAction::TranslateDefault);
    }
    actions
}

fn is_translation_picker(menu: &ScoreboardMenuUi) -> bool {
    menu.actions.iter().any(|action| matches!(action, ScoreAction::TranslateHint(_, _)))
}

impl App {
    fn ui_framebuffer_size(&self) -> (u32, u32) {
        self.window.as_ref().map_or((0, 0), |window| {
            let size = window.inner_size();
            (size.width, size.height)
        })
    }

    pub(in crate::app) fn scoreboard_cursor_active(&self) -> bool {
        self.scoreboard_cursor.active
    }

    fn open_scoreboard_cursor(&mut self) {
        let scroll = self.scoreboard_cursor.scroll;
        self.scoreboard_cursor = ScoreboardCursor { active: true, scroll, ..ScoreboardCursor::default() };
        self.set_capture(false);
        self.publish_transient_ui();
    }

    /// The scoreboard borrows alternate attack as a *toggle*, not a held
    /// gameplay attack. Only a fresh +altattack press while the board is up
    /// changes mouse mode; the matching -altattack has no scoreboard action.
    pub(in crate::app) fn scoreboard_altattack_press(&mut self) -> bool {
        if self.overlay != OverlayMode::None || self.front_end
            || self.game_session.is_none() || !self.scoreboard_should_show()
        {
            return false;
        }
        if self.scoreboard_cursor.active {
            self.close_scoreboard_cursor();
        } else {
            self.open_scoreboard_cursor();
        }
        true
    }

    /// While the mouse is free, normal captured-mouse binding dispatch is
    /// disabled. Recognize only whichever mouse button the player actually
    /// bound to +altattack, so it can also close scoreboard mouse mode.
    fn scoreboard_altattack_mouse_button(&self, button: MouseButton) -> bool {
        let number = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::Back => 4,
            MouseButton::Forward => 5,
            _ => return false,
        };
        self.bindings
            .get_resolved(crate::app::BindKey::Mouse(number), self.spectator_binding_context())
            .is_some_and(|binding| crate::app::keybinds::binding_contains_command(binding, "+altattack"))
    }

    pub(in crate::app) fn close_scoreboard_cursor(&mut self) {
        if !self.scoreboard_cursor.active {
            return;
        }
        let scroll = self.scoreboard_cursor.scroll;
        self.scoreboard_cursor = ScoreboardCursor { scroll, ..ScoreboardCursor::default() };
        if self.overlay == OverlayMode::None && self.game_session.is_some() && !self.front_end {
            self.set_capture(true);
        }
        self.publish_transient_ui();
    }

    /// Mouse and Escape handling while the scoreboard is up. Returns true when
    /// the event was consumed and must not reach egui or gameplay bindings.
    pub(in crate::app) fn scoreboard_cursor_window_event(&mut self, event: &WindowEvent) -> bool {
        if self.overlay != OverlayMode::None || self.front_end || self.game_session.is_none() {
            if self.scoreboard_cursor.active {
                self.scoreboard_cursor = ScoreboardCursor::default();
            }
            return false;
        }
        let active = self.scoreboard_cursor.active;
        if !active && !self.scoreboard_should_show() {
            return false;
        }
        match event {
            WindowEvent::MouseInput { state, button, .. }
                if active && self.scoreboard_altattack_mouse_button(*button) =>
            {
                // The released cursor bypasses normal gameplay mouse binds.
                // Dispatch this *one* scoreboard command explicitly instead.
                if *state == ElementState::Pressed {
                    self.scoreboard_altattack_press();
                }
                true
            }
            WindowEvent::CursorMoved { position, .. } if active => {
                let pointer = [position.x as f32, position.y as f32];
                self.scoreboard_cursor.pointer = Some(pointer);
                let hovered = self.scoreboard_hit(pointer);
                if hovered != self.scoreboard_cursor.hovered {
                    self.scoreboard_cursor.hovered = hovered;
                    self.publish_transient_ui();
                }
                true
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } if active => {
                if *state == ElementState::Pressed {
                    if let Some(pointer) = self.scoreboard_cursor.pointer {
                        self.scoreboard_click(pointer);
                    }
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // The wheel scrolls a long board, with or without the cursor;
                // on a board that fits it keeps its usual binds.
                let Some(board) = self.visible_scoreboard() else { return active };
                let (w, h) = self.ui_framebuffer_size();
                let max = crate::ui::scoreboard_max_scroll(&board, w, h);
                if max == 0 {
                    return active;
                }
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => -y.signum() as i32 * 2,
                    winit::event::MouseScrollDelta::PixelDelta(p) => -(p.y / 24.0).round() as i32,
                };
                let next = (i32::from(self.scoreboard_cursor.scroll) + lines).clamp(0, max as i32) as u16;
                if next != self.scoreboard_cursor.scroll {
                    self.scoreboard_cursor.scroll = next;
                    if let Some(pointer) = self.scoreboard_cursor.pointer {
                        self.scoreboard_cursor.hovered = self.scoreboard_hit(pointer);
                    }
                    self.publish_transient_ui();
                }
                true
            }
            WindowEvent::MouseInput { .. } if active => true,
            WindowEvent::KeyboardInput { event: key, .. }
                if active
                    && key.state == ElementState::Pressed
                    && key.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
            {
                if self.scoreboard_cursor.menu.take().is_some() {
                    self.publish_transient_ui();
                } else {
                    self.close_scoreboard_cursor();
                }
                true
            }
            _ => false,
        }
    }

    fn scoreboard_hit(&self, pointer: [f32; 2]) -> Option<ScoreboardHit> {
        let board = self.visible_scoreboard()?;
        let (w, h) = self.ui_framebuffer_size();
        crate::ui::scoreboard_hit_test(&board, w, h, pointer)
    }

    fn scoreboard_click(&mut self, pointer: [f32; 2]) {
        let hit = self.scoreboard_hit(pointer);
        let menu = self.scoreboard_cursor.menu.take();
        match hit {
            Some(ScoreboardHit::TranslateMenu(client)) => {
                // The icon opens the language picker directly, rather than
                // silently toggling a rule and requiring a second menu.
                if menu.as_ref().is_none_or(|open| open.client != client || !is_translation_picker(open)) {
                    self.scoreboard_cursor.menu = self.score_translation_picker_for(client, pointer);
                }
            }
            Some(ScoreboardHit::Action(index)) => {
                if let Some((menu, action)) = menu.and_then(|m| m.actions.get(index).copied().map(|a| (m, a))) {
                    self.apply_score_action(menu, action);
                }
            }
            Some(ScoreboardHit::Row(client)) => {
                // Clicking the row whose popup is open closes it again.
                if menu.as_ref().is_none_or(|open| open.client != client) {
                    self.scoreboard_cursor.menu = self.score_menu_for(client, pointer);
                }
            }
            Some(ScoreboardHit::VoteYes) => self.forward_command_to_server("vote yes"),
            Some(ScoreboardHit::VoteNo) => self.forward_command_to_server("vote no"),
            Some(ScoreboardHit::StatsToggle) => {
                self.scoreboard_cursor.stats_open = !self.scoreboard_cursor.stats_open;
                self.scoreboard_cursor.scroll = 0;
            }
            None => {}
        }
        // Following closes mouse mode and republishes the board itself.
        if self.scoreboard_cursor.active {
            self.scoreboard_cursor.hovered = self.scoreboard_hit(pointer);
            self.publish_transient_ui();
        }
    }

    fn score_menu_for(&self, client: i32, pointer: [f32; 2]) -> Option<ScoreboardMenuUi> {
        let session = self.game_session.as_ref()?;
        let name = session.client_game.client_name(usize::try_from(client).ok()?)?;
        // In spectator follow mode the snapshot's clientNum belongs to the
        // *watched player*. Always use the connection slot for our identity.
        let own = self.net.as_ref().map(|net| net.session().client_num()).or_else(|| {
            session.client_game.current_snapshot().and_then(|s| s.player_state.field_i32("clientNum"))
        });
        let playable = self.scoreboard.as_ref().is_some_and(|board| {
            board.entries.iter().any(|entry| entry.client == client && entry.team != 3)
        });
        let global = self.chat_translation.settings.mode;
        let (rule, source) = self.chat_translation.rule(client, &name);
        let translate = (global != crate::chat_translation::Mode::Off).then_some((
            rule.checked(global), source, rule != RuleMode::Default,
        ));
        let actions = score_actions(
            Some(client) == own,
            self.is_spectating_live(),
            playable,
            self.muted_clients.contains_key(&client),
            session.live,
            self.active_server_mod() == crate::net::mod_support::ServerMod::Japro,
            translate,
        );
        (!actions.is_empty()).then(|| ScoreboardMenuUi {
            client,
            name,
            anchor: [pointer[0] as i32, pointer[1] as i32],
            actions,
            confirm_kick: false,
            title: None,
            detail: None,
        })
    }

    /// A click on the Aa icon goes straight to a single popup with the ON/OFF
    /// action and every language choice (including Auto-detect). No submenu.
    fn score_translation_picker_for(&self, client: i32, pointer: [f32; 2]) -> Option<ScoreboardMenuUi> {
        let menu = self.score_menu_for(client, pointer)?;
        if !self.score_target_is_current(&menu, false)
            || !menu.actions.iter().any(|action| matches!(action, ScoreAction::Translate(_)))
        {
            return None;
        }
        let (mode, source) = self.chat_translation.rule(client, &menu.name);
        let checked = mode.checked(self.chat_translation.settings.mode);
        let actions = translation_picker_actions(checked, source, mode != RuleMode::Default);
        let title = format!("TRANSLATE - {}", crate::logging::strip_jka_colors(&menu.name));
        let source_label = source.map_or("Auto-detect", crate::chat_translation::lang::name);
        let detail = format!("{}  |  FROM: {}", if checked { "ON" } else { "OFF" }, source_label);
        Some(ScoreboardMenuUi { actions, title: Some(title), detail: Some(detail), ..menu })
    }

    /// Rule changes apply to the player still in the slot; otherwise nothing.
    fn change_score_translation_rule(&mut self, menu: ScoreboardMenuUi, change: RuleChange) {
        if self.score_target_is_current(&menu, false) {
            self.change_chat_translation_rule(menu.client, &menu.name, change);
            // Keep the user in the picker after changing an option so its
            // source selection and ON/OFF state update without another click.
            let anchor = [menu.anchor[0] as f32, menu.anchor[1] as f32];
            self.scoreboard_cursor.menu = if is_translation_picker(&menu) {
                self.score_translation_picker_for(menu.client, anchor)
            } else {
                self.score_menu_for(menu.client, anchor)
            };
        } else {
            self.push_console_line("^3Translation:^7 player left or changed slots");
        }
    }

    fn apply_score_action(&mut self, menu: ScoreboardMenuUi, action: ScoreAction) {
        let client = menu.client;
        let plain = crate::logging::strip_jka_colors(&menu.name);
        match action {
            ScoreAction::Follow => {
                if self.is_spectating_live() && self.score_target_is_current(&menu, true) {
                    self.forward_command_to_server(&format!("follow {client}"));
                    self.close_scoreboard_cursor();
                } else {
                    self.push_console_line("^3Follow:^7 player is no longer available to spectate");
                }
            }
            ScoreAction::Mute => {
                self.muted_clients.insert(client, menu.name);
                self.push_console_line(format!("Muted {plain}^7 on this client. Their chat is hidden until you unmute them."));
            }
            ScoreAction::Unmute => {
                self.muted_clients.remove(&client);
                self.push_console_line(format!("Unmuted {plain}^7."));
            }
            ScoreAction::VoteKick => {
                // First click only opens confirmation. Do not send a vote yet.
                self.scoreboard_cursor.menu = Some(ScoreboardMenuUi {
                    actions: vec![ScoreAction::ConfirmVoteKick, ScoreAction::CancelVoteKick],
                    confirm_kick: true,
                    ..menu
                });
            }
            ScoreAction::ConfirmVoteKick => {
                // The player might have left or their slot been reassigned
                // while confirmation was open. Never kick-vote a new occupant.
                if menu.confirm_kick && self.score_target_is_current(&menu, false) {
                    if let Ok(client) = usize::try_from(client) {
                        self.forward_command_to_server(&crate::vote::clientkick_vote(client));
                    }
                } else {
                    self.push_console_line("^3Kick vote:^7 player left or changed slots; vote cancelled");
                }
            }
            ScoreAction::CancelVoteKick => {}
            ScoreAction::ForceSpectate => {
                if let Ok(client) = usize::try_from(client) {
                    self.forward_command_to_server(&crate::vote::forcespec_vote(client));
                }
            }
            ScoreAction::Translate(_) => {
                let mode = self.chat_translation.rule(menu.client, &menu.name).0;
                self.change_score_translation_rule(menu,
                    RuleChange::Mode(mode.toggled(self.chat_translation.settings.mode)));
            }
            ScoreAction::TranslateDefault => self.change_score_translation_rule(menu, RuleChange::Mode(RuleMode::Default)),
            ScoreAction::TranslateLanguages(_) => {
                self.scoreboard_cursor.menu = self.score_translation_picker_for(client,
                    [menu.anchor[0] as f32, menu.anchor[1] as f32]);
            }
            ScoreAction::TranslateHint(index, _) => {
                let code = crate::ui::SCOREBOARD_TRANSLATE_LANGUAGES
                    .get(usize::from(index))
                    .map(|&(code, _)| code)
                    .filter(|code| !code.is_empty());
                self.change_score_translation_rule(menu, RuleChange::Source(code));
            }
        }
    }

    fn score_target_is_current(&self, menu: &ScoreboardMenuUi, require_playing: bool) -> bool {
        let Some(session) = self.game_session.as_ref().filter(|session| session.live) else {
            return false;
        };
        let client = menu.client;
        if self.net.as_ref().is_some_and(|net| net.session().client_num() == client) {
            return false;
        }
        if !usize::try_from(client).ok().and_then(|slot| session.client_game.client_name(slot))
            .is_some_and(|name| plain_name(&name) == plain_name(&menu.name))
        {
            return false;
        }
        !require_playing || self.scoreboard.as_ref().is_some_and(|board| {
            board.entries.iter().any(|entry| entry.client == client && entry.team != 3)
        })
    }

    /// Chat filter for client-side mutes. Servers that name the sender are
    /// matched by client number; otherwise the line's `name^7: ` prefix is
    /// compared with the muted players' names.
    pub(in crate::app) fn chat_sender_muted(&self, sender: Option<i32>, text: &str) -> bool {
        if self.muted_clients.is_empty() {
            return false;
        }
        if let Some(sender) = sender {
            return self.muted_clients.contains_key(&sender);
        }
        let line = plain_name(text);
        self.muted_clients.values().any(|name| {
            let name = plain_name(name);
            !name.is_empty() && line.strip_prefix(&name).is_some_and(|rest| rest.starts_with(':') || rest.starts_with(" ("))
        })
    }

    /// A mute follows the person, not the slot: forget it once the slot is
    /// empty or holds someone with a different name.
    pub(in crate::app) fn prune_muted_clients(&mut self) {
        let Some(session) = self.game_session.as_ref() else {
            self.muted_clients.clear();
            return;
        };
        self.muted_clients.retain(|&client, name| {
            usize::try_from(client)
                .ok()
                .and_then(|slot| session.client_game.client_name(slot))
                .is_some_and(|current| plain_name(&current) == plain_name(name))
        });
    }

    /// `scoreboardstats`: players <-> personal match breakdown.
    pub(in crate::app) fn toggle_scoreboard_stats(&mut self) {
        self.scoreboard_cursor.stats_open = !self.scoreboard_cursor.stats_open;
        self.scoreboard_cursor.scroll = 0;
        self.publish_transient_ui();
    }

    /// The match is over and the server is showing scores.
    pub(in crate::app) fn scoreboard_intermission(&self) -> bool {
        self.current_player_state()
            .and_then(|ps| ps.field_i32("pm_type"))
            .is_some_and(|pm| pm == crate::cgame::PM_INTERMISSION)
    }

    /// Clients holding the top score (crowned). Nobody is crowned at 0.
    pub(in crate::app) fn score_leaders(&self) -> Vec<i32> {
        let Some(board) = self.scoreboard.as_ref() else { return Vec::new() };
        let players = || board.entries.iter().filter(|entry| entry.team != 3 && entry.ping >= 0);
        let Some(top) = players().map(|entry| entry.score).max().filter(|&top| top > 0) else {
            return Vec::new();
        };
        players().filter(|entry| entry.score == top).map(|entry| entry.client).collect()
    }

    /// Per-tick upkeep: join clock, mute pruning, and dropping the cursor when
    /// a menu or console takes over. True when the visible board must be
    /// republished (its playtime clock moved).
    pub(in crate::app) fn tick_scoreboard_ui(&mut self) -> bool {
        let connected = self.net.is_some() || self.local_server.is_some();
        if !connected {
            self.session_joined = None;
            self.muted_clients.clear();
        } else if self.session_joined.is_none() && self.game_session.is_some() {
            self.session_joined = Some((local_hhmm(), Instant::now()));
        }
        if self.scoreboard_cursor.active && (self.overlay != OverlayMode::None || self.game_session.is_none()) {
            self.scoreboard_cursor = ScoreboardCursor::default();
        } else if self.scoreboard_cursor.active && !self.scoreboard_should_show() {
            // The board went away on its own (intermission ended): take the
            // mouse back for the game.
            self.close_scoreboard_cursor();
        }
        let second = self.session_joined.as_ref().map(|(_, since)| since.elapsed().as_secs());
        if second == self.scoreboard_cursor.clock_second {
            return false;
        }
        self.scoreboard_cursor.clock_second = second;
        if self.game_session.is_some() {
            self.prune_muted_clients();
        }
        self.scoreboard_should_show()
    }

    /// Fill in the DinurdoJK parts of a board about to be published: header
    /// and footer text, mute marks, crowns and the cursor state.
    pub(in crate::app) fn decorate_scoreboard(&self, board: &mut UiScoreboard) {
        let leaders = self.score_leaders();
        let spectating = self.stats_spectating();
        for entry in &mut board.entries {
            entry.muted = self.muted_clients.contains_key(&entry.client);
            let (rule, source) = self.chat_translation.rule(entry.client, &entry.name);
            // The source-language badge reflects the user's explicit hint,
            // independent of the global automatic-translation switch.
            entry.translate_source = source;
            entry.translate_checked = if self.chat_translation.settings.mode != crate::chat_translation::Mode::Off
                && self.game_session.as_ref().is_some_and(|session| session.live)
                && self.net.as_ref().is_some_and(|net| net.session().client_num() != entry.client)
            {
                Some(rule.checked(self.chat_translation.settings.mode))
            } else { None };
            entry.crowned = leaders.contains(&entry.client);
            entry.h2h = if spectating { None } else { self.match_stats.head_to_head(entry.client) };
            let info = self.game_session.as_ref()
                .and_then(|session| session.client_game.client_info(usize::try_from(entry.client).ok()?, &session.siege_classes));
            (entry.model, entry.model_fallback) = info.map(|info| {
                let fallback = info.missing_model_fallback();
                (format!("{}/{}", info.model_name, info.skin_name), format!("{}/{}", fallback.model_name, fallback.skin_name))
            }).unwrap_or_default();
        }
        let (gametype, map) = self.game_session.as_ref().map_or((0, String::new()), |session| {
            (session.client_game.gametype(), session.map_name.clone().unwrap_or_default())
        });
        let mode = crate::vote::GAME_TYPES
            .iter()
            .find(|(number, _)| i32::from(*number) == gametype)
            .map_or("GAME", |(_, label)| label)
            .to_ascii_uppercase();
        board.chrome = ScoreboardChrome {
            server_name: self.current_server_hostname().unwrap_or_else(|| "DinurdoJK".to_owned()),
            subtitle: if map.is_empty() { mode } else { format!("{mode}  ·  {map}") },
            joined: self.session_joined.as_ref().map(|(at, _)| at.clone()),
            playtime: self.session_joined.as_ref().map(|(_, since)| clock(since.elapsed())),
            vote: self.vote_hud_line().map(|line| crate::logging::strip_jka_colors(&line)),
            cursor: self.scoreboard_cursor.active.then(|| ScoreboardCursorUi {
                pointer: self.scoreboard_cursor.pointer.map(|p| [p[0] as i32, p[1] as i32]),
                hovered: self.scoreboard_cursor.hovered,
                menu: self.scoreboard_cursor.menu.clone(),
            }),
            japro: self.active_server_mod() == crate::net::mod_support::ServerMod::Japro,
            scroll: self.scoreboard_cursor.scroll,
            avatars: self.scoreboard_settings.avatars,
            head_to_head: self.scoreboard_settings.head_to_head && !spectating,
            stats: Some({
                let mut stats = self.match_stats_ui();
                let me = self.stats_viewer();
                if let Some(row) = board.entries.iter().find(|entry| Some(entry.client) == me) {
                    stats.accuracy = (row.accuracy > 0).then_some(row.accuracy);
                    stats.excellent = row.excellent.max(0);
                    stats.gauntlet = row.gauntlet.max(0);
                }
                stats
            }),
            // The match breakdown takes over the board at intermission.
            stats_open: self.scoreboard_cursor.stats_open || self.scoreboard_intermission(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::clock;
    use std::time::Duration;

    #[test]
    fn spectator_popup_follows_active_players_but_not_spectators_or_self() {
        use super::{score_actions, ScoreAction};
        assert_eq!(score_actions(false, true, true, false, true, false, None),
            vec![ScoreAction::Follow, ScoreAction::Mute, ScoreAction::VoteKick]);
        assert_eq!(score_actions(false, true, false, false, true, false, None),
            vec![ScoreAction::Mute, ScoreAction::VoteKick]);
        assert_eq!(score_actions(true, true, true, false, true, true, Some((false, None, false))), Vec::new());
        assert_eq!(score_actions(false, false, true, false, true, false, None),
            vec![ScoreAction::Mute, ScoreAction::VoteKick]);
        assert_eq!(score_actions(false, true, true, true, true, true, None),
            vec![ScoreAction::Follow, ScoreAction::Unmute, ScoreAction::VoteKick, ScoreAction::ForceSpectate]);
        assert_eq!(score_actions(false, false, true, false, true, false, Some((true, None, false))),
            vec![ScoreAction::Mute, ScoreAction::Translate(true), ScoreAction::TranslateLanguages(None), ScoreAction::VoteKick]);
        assert_eq!(score_actions(false, false, true, false, false, false, Some((true, None, false))), vec![ScoreAction::Mute]);
    }

    #[test]
    fn clicking_the_icon_can_select_a_language_in_one_popup() {
        use super::{translation_picker_actions, ScoreAction};
        let actions = translation_picker_actions(false, Some("es"), true);
        assert_eq!(actions[0], ScoreAction::Translate(false));
        assert_eq!(actions[1], ScoreAction::TranslateHint(0, false));
        assert_eq!(actions[4], ScoreAction::TranslateHint(3, true));
        assert_eq!(actions.last(), Some(&ScoreAction::TranslateDefault));
        assert_eq!(actions.iter().filter(|a| matches!(a, ScoreAction::TranslateHint(_, true))).count(), 1);
        let auto = translation_picker_actions(true, None, false);
        assert_eq!(auto[1], ScoreAction::TranslateHint(0, true));
        assert!(!auto.contains(&ScoreAction::TranslateDefault));
    }

    #[test]
    fn playtime_clock_switches_to_hours() {
        assert_eq!(clock(Duration::from_secs(75)), "01:15");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }
}
