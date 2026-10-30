//! Scoreboard interaction: M2 or M3 while the board is up frees the mouse,
//! a click on a player opens a popup (mute, vote kick, vote spectate), and
//! the footer offers YES/NO while a vote is open. Hit testing uses the same
//! Taffy layout the renderer draws (`ui::scoreboard_hit_test`).
//!
//! Mutes are purely client-side: a muted client's chat and voice commands are
//! dropped before they reach the chat box, console or chat log, and the
//! scoreboard marks their row.
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
            WindowEvent::MouseInput { state, button: MouseButton::Right | MouseButton::Middle, .. } => {
                if *state == ElementState::Pressed {
                    if active { self.close_scoreboard_cursor() } else { self.open_scoreboard_cursor() }
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
            Some(ScoreboardHit::Action(index)) => {
                if let Some((menu, action)) = menu.and_then(|m| m.actions.get(index).copied().map(|a| (m, a))) {
                    self.apply_score_action(menu.client, &menu.name, action);
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
        self.scoreboard_cursor.hovered = self.scoreboard_hit(pointer);
        self.publish_transient_ui();
    }

    fn score_menu_for(&self, client: i32, pointer: [f32; 2]) -> Option<ScoreboardMenuUi> {
        let session = self.game_session.as_ref()?;
        let name = session.client_game.client_name(usize::try_from(client).ok()?)?;
        let own = session.client_game.current_snapshot().and_then(|s| s.player_state.field_i32("clientNum"));
        let mut actions = Vec::new();
        if Some(client) != own {
            actions.push(if self.muted_clients.contains_key(&client) { ScoreAction::Unmute } else { ScoreAction::Mute });
            if session.live {
                actions.push(ScoreAction::VoteKick);
                if self.active_server_mod() == crate::net::mod_support::ServerMod::Japro {
                    actions.push(ScoreAction::ForceSpectate);
                }
            }
        }
        (!actions.is_empty()).then(|| ScoreboardMenuUi {
            client,
            name,
            anchor: [pointer[0] as i32, pointer[1] as i32],
            actions,
        })
    }

    fn apply_score_action(&mut self, client: i32, name: &str, action: ScoreAction) {
        let plain = crate::logging::strip_jka_colors(name);
        match action {
            ScoreAction::Mute => {
                self.muted_clients.insert(client, name.to_owned());
                self.push_console_line(format!("Muted {plain}^7 on this client. Their chat is hidden until you unmute them."));
            }
            ScoreAction::Unmute => {
                self.muted_clients.remove(&client);
                self.push_console_line(format!("Unmuted {plain}^7."));
            }
            ScoreAction::VoteKick => {
                if let Ok(client) = usize::try_from(client) {
                    self.forward_command_to_server(&crate::vote::clientkick_vote(client));
                }
            }
            ScoreAction::ForceSpectate => {
                if let Ok(client) = usize::try_from(client) {
                    self.forward_command_to_server(&crate::vote::forcespec_vote(client));
                }
            }
        }
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
        for entry in &mut board.entries {
            entry.muted = self.muted_clients.contains_key(&entry.client);
            entry.crowned = leaders.contains(&entry.client);
            entry.h2h = self.match_stats.head_to_head(entry.client);
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
            head_to_head: self.scoreboard_settings.head_to_head,
            stats: Some({
                let mut stats = self.match_stats_ui();
                let me = self.game_session.as_ref().and_then(|session| session.client_game.presentation_client_num());
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
    fn playtime_clock_switches_to_hours() {
        assert_eq!(clock(Duration::from_secs(75)), "01:15");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }
}
