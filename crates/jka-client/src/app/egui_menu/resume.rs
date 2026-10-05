//! Resume.
use crate::app::egui_menu::{
    jka_colored_text, menu_action, theme, App, GameSession, JoinMode, OverlayMode,
};

impl App {
    pub(in crate::app::egui_menu) fn egui_resume_page(&mut self, ui: &mut egui::Ui) {
        if self.map_editor.is_some() {
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "EDIT SOURCE MAP").clicked() {
                    self.set_overlay(OverlayMode::MapEdit);
                }
                theme::label(
                    ui,
                    theme::plain(
                        "Loose .map brush editing is available for this world.",
                        10.5,
                        theme::TEXT_FAINT,
                    ),
                );
            });
            ui.add_space(12.0);
        }
        let demo_rate = self
            .game_session
            .as_ref()
            .and_then(GameSession::playback_rate);
        let mode = match self.local_server.as_ref().map(|server| server.mode()) {
            Some(JoinMode::Player) => "Playing",
            _ => "Spectating",
        };
        if let Some(rate) = demo_rate {
            theme::page_title(
                ui,
                "DEMO",
                if rate == 0.0 {
                    "Playback paused."
                } else {
                    "Demo playback controls."
                },
            );
            theme::section(ui, "PLAYBACK", "Pause keeps the current demo time fixed; Play resumes at the previous non-zero demo speed.");
            let label = if rate == 0.0 { "PLAY" } else { "PAUSE" };
            if theme::primary_button(ui, label).clicked() {
                self.console_status = match self.game_session.as_mut() {
                    Some(playback) => match playback.toggle_pause(std::time::Instant::now()) {
                        Ok(0.0) => "DEMO PAUSED".into(),
                        Ok(rate) => format!("DEMO PLAYING AT {rate}X"),
                        Err(error) => error,
                    },
                    None => "NO DEMO IS PLAYING".into(),
                };
                self.egui_repaint_requested = true;
            }
            ui.add_space(8.0);
        } else if self.live_connected() {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
            if let Some(hostname) = self.current_server_hostname() {
                ui.horizontal(|ui| {
                    theme::glow_label(ui, "On", 12.0, theme::TEXT_FAINT);
                    ui.add(egui::Label::new(jka_colored_text(
                        &hostname,
                        13.0,
                        theme::TEXT,
                    )));
                });
                ui.add_space(8.0);
            }
        } else {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
        }

        let can_join = self.live_connected()
            || self
                .local_server
                .as_ref()
                .is_some_and(|server| server.can_join());

        theme::section(ui, "SESSION", "");
        let team_gametype = self.live_connected()
            && self
                .game_session
                .as_ref()
                .is_some_and(|session| session.client_game.gametype() >= crate::cgame::GT_TEAM);
        if team_gametype {
            if menu_action(ui, "Join red", "Ask the server to join the red team.", true) {
                self.join_live_team("red", "JOIN RED REQUEST SENT");
                return;
            }
            if menu_action(
                ui,
                "Join blue",
                "Ask the server to join the blue team.",
                true,
            ) {
                self.join_live_team("blue", "JOIN BLUE REQUEST SENT");
                return;
            }
        } else if menu_action(
            ui,
            "Join game",
            if self.live_connected() {
                "Ask the server to leave spectator and join play."
            } else if can_join {
                "Spawn in at the next available spawn point."
            } else {
                "No spawn point is available on this map."
            },
            can_join,
        ) {
            self.join_as(JoinMode::Player);
            return;
        }
        if menu_action(
            ui,
            "Join spectate",
            if self.live_connected() {
                "Ask the server to join spectators."
            } else {
                "Watch the map with a free-flying camera."
            },
            true,
        ) {
            self.join_as(JoinMode::Spectator);
            return;
        }
        self.egui_spectator_actions(ui);

        ui.add_space(12.0);
        theme::section(ui, "TOOLS", "Client-side training and analysis tools.");
        if menu_action(
            ui,
            "Strafe trails",
            "Load recorded jaPRO trails, trace live players, and record race lines.",
            true,
        ) {
            self.open_strafe_trails();
            return;
        }
        if menu_action(
            ui,
            "Race ghosts",
            "Browse race demos for the current map/style and race multiple synchronized ghosts.",
            self.game_session
                .as_ref()
                .is_some_and(|session| session.live),
        ) {
            self.open_race_ghosts();
            return;
        }

        ui.add_space(12.0);
        theme::section(ui, "LEAVE", "Leave the current session or exit DinurdoJK.");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::ghost_button(ui, "QUIT TO MAIN MENU").clicked() {
                self.disconnect_to_main_menu();
                return;
            }
            ui.add_space(10.0);
            if theme::ghost_button(ui, "QUIT TO DESKTOP").clicked() {
                self.request_quit();
            }
        });
    }

    pub(in crate::app::egui_menu) fn join_live_team(&mut self, team: &str, status: &str) {
        debug_assert!(matches!(team, "red" | "blue"));
        self.forward_command_to_server(&format!("team {team}"));
        self.console_status = status.into();
        self.set_overlay(OverlayMode::None);
    }

    pub(in crate::app) fn join_as(&mut self, mode: JoinMode) {
        if self.live_connected() {
            let command = match mode {
                JoinMode::Player => "team free",
                JoinMode::Spectator => "team spectator",
            };
            self.forward_command_to_server(command);
            self.console_status = match mode {
                JoinMode::Player => "JOIN GAME REQUEST SENT".into(),
                JoinMode::Spectator => "JOIN SPECTATE REQUEST SENT".into(),
            };
            self.set_overlay(OverlayMode::None);
            return;
        }

        if mode == JoinMode::Player && !self.solo_initial_spawn_pending {
            // ClientSpawn -> SelectSpawnPoint(ps.origin): random among the
            // furthest half of the spots, so a rejoin does not reuse this one.
            if let Some(index) = crate::scene::select_spawn_index(
                &self.spawns,
                self.camera.position.to_array(),
                false,
                super::random_unit(),
            ) {
                self.spawn_index = index;
            }
        }
        let Some(spawn) = self.spawns.get(self.spawn_index).copied() else {
            return;
        };
        let result = match self.local_server.as_mut() {
            Some(server) => server.join(mode, spawn),
            None => return,
        };
        match result {
            Ok(()) => {
                if mode == JoinMode::Player {
                    self.solo_initial_spawn_pending = false;
                }
                self.push_local_snapshot(true);
                self.third_person_camera.reset();
                self.update_solo_player_view_and_presentation();
                self.set_overlay(OverlayMode::None);
            }
            Err(error) => {
                self.console_status = error;
                self.publish_ui();
            }
        }
    }
}
