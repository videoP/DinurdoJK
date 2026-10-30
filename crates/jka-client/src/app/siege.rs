//! Native presentation for the server-driven Siege cgame surfaces.
//! The data/commands follow TaystJK; only the widget implementation is native egui.

use crate::{
    app::{App, OverlayMode},
    ui::UiScoreEntry,
};
use crate::cgame::CgameServerAction;
use jka_assets::siege::{SiegeClass, SiegeMapDefinition, SiegeMapTeam, SIEGE_TEAM1, SIEGE_TEAM2};

const PW_REDFLAG: i32 = 4;
const PW_BLUEFLAG: i32 = 5;
const PW_NEUTRALFLAG: i32 = 6;

/// TaystJK CG_DrawClientScore icon ownership for GT_SIEGE. The score command's
/// powerup mask is authoritative for carried flags; otherwise CS_PLAYERS'
/// siegeclass resolves to the parsed .scl class_shader. Indices are local to
/// the returned path list and are rebased into the shared UI atlas at draw time.
pub(in crate::app) fn siege_scoreboard_icons(
    entries: &mut [UiScoreEntry],
    catalog: &jka_assets::siege::SiegeCatalog,
) -> Vec<String> {
    let mut paths = Vec::new();
    for entry in entries {
        entry.siege_icon = None;
        let flag = if entry.powerups & (1 << PW_NEUTRALFLAG) != 0 {
            Some("icons/iconf_neutral1")
        } else if entry.powerups & (1 << PW_REDFLAG) != 0 {
            Some("gfx/hud/mpi_rflag")
        } else if entry.powerups & (1 << PW_BLUEFLAG) != 0 {
            Some("gfx/hud/mpi_bflag")
        } else {
            None
        };
        let path = flag.map(str::to_owned).or_else(|| {
            catalog
                .class(&entry.siege_class)
                .map(|class| class.class_shader.trim())
                .filter(|shader| !shader.is_empty())
                .map(str::to_owned)
        });
        let Some(path) = path else { continue };
        let Ok(index) = u16::try_from(paths.len()) else { break };
        paths.push(path);
        entry.siege_icon = Some(index);
    }
    paths
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum SiegeDialog {
    Briefing { team: i32 },
    ClassSelect { team: i32 },
    Profile { team: i32 },
    /// `CG_DrawSiegeMessage(..., 0)` / `UIMENU_SIEGEMESSAGE`.
    RoundMessage,
}

impl App {
    pub(in crate::app) fn drain_siege_server_actions(&mut self) {
        let actions = self
            .game_session
            .as_mut()
            .map(|session| session.client_game.drain_siege_server_actions())
            .unwrap_or_default();

        for action in actions {
            match action {
                CgameServerAction::SiegeExtendedData(entries) => {
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_siege_extended_data(&entries);
                    }
                }
                CgameServerAction::SiegeBriefing { team } => {
                    let team = self.siege_action_team(team);
                    self.open_siege_dialog(SiegeDialog::Briefing { team });
                }
                CgameServerAction::SiegeClassSelect => {
                    let team = self.siege_action_team(0);
                    self.open_siege_dialog(SiegeDialog::ClassSelect { team });
                }
                CgameServerAction::SiegeProfile => {
                    // TaystJK CG_SiegeProfileMenu_f explicitly suppresses the
                    // player-config/profile UI during demo playback. Class
                    // select is intentionally different and remains allowed.
                    if !self.demo_playback_active() {
                        let team = self.siege_action_team(0);
                        self.open_siege_dialog(SiegeDialog::Profile { team });
                    }
                }
                _ => {}
            }
        }
    }

    fn siege_action_team(&self, requested: i32) -> i32 {
        if matches!(requested, SIEGE_TEAM1 | SIEGE_TEAM2) {
            return requested;
        }
        self.game_session
            .as_ref()
            .map(|session| session.siege_viewer_team())
            .filter(|team| matches!(*team, SIEGE_TEAM1 | SIEGE_TEAM2))
            .unwrap_or(SIEGE_TEAM1)
    }

    /// Native equivalent of the stock `ui_opensiegemenu ingame_objectives`
    /// surface. TaystJK gates UI_OpenSiegeMenu_f on GT_SIEGE; keep that
    /// authority here and resolve an unspecified team from the current viewer.
    pub(in crate::app) fn open_siege_objectives_menu(&mut self) {
        let team = self.siege_action_team(0);
        self.open_siege_dialog(SiegeDialog::Briefing { team });
    }

    fn open_siege_dialog(&mut self, dialog: SiegeDialog) {
        // These cgame commands are only meaningful in Siege. Ignore stale/demo
        // command text from another gametype rather than manufacturing a menu.
        if !self
            .game_session
            .as_ref()
            .is_some_and(|session| session.client_game.gametype() == 7)
        {
            return;
        }
        // Briefing/class menus may be the first localized surface in a session.
        let _ = self.translate_server_text(b"x");
        if self.siege_dialog.is_none() {
            self.siege_dialog_return_overlay = self.overlay;
            self.siege_dialog_opened_overlay = self.overlay == OverlayMode::None;
            if self.siege_dialog_opened_overlay {
                self.set_overlay(OverlayMode::Game);
            }
        }
        if dialog != SiegeDialog::RoundMessage {
            self.siege_round_message = None;
        }
        self.siege_dialog = Some(dialog);
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn show_siege_round_message(&mut self, text: String) {
        self.siege_round_message = Some(text);
        self.open_siege_dialog(SiegeDialog::RoundMessage);
    }

    fn close_siege_dialog(&mut self) {
        self.siege_dialog = None;
        self.siege_round_message = None;
        let return_overlay = self.siege_dialog_return_overlay;
        let restore = self.siege_dialog_opened_overlay;
        self.siege_dialog_opened_overlay = false;
        self.siege_dialog_return_overlay = OverlayMode::None;
        if restore {
            self.set_overlay(return_overlay);
        } else {
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app) fn resolve_siege_text(&self, text: &str) -> String {
        let trimmed = text.trim();
        let Some(key) = trimmed.strip_prefix('@') else {
            return trimmed.to_owned();
        };
        let Some(table) = self.stringed.as_ref() else {
            return trimmed.to_owned();
        };
        let resolved = table.get(key);
        if resolved.is_empty() {
            trimmed.to_owned()
        } else {
            String::from_utf8_lossy(resolved).into_owned()
        }
    }

    fn siege_dialog_data(
        &self,
        dialog: SiegeDialog,
    ) -> Option<(SiegeMapDefinition, SiegeMapTeam, Vec<SiegeClass>, Option<SiegeClass>)> {
        let session = self.game_session.as_ref()?;
        let map = session.active_siege_map()?;
        let team_num = match dialog {
            SiegeDialog::Briefing { team }
            | SiegeDialog::ClassSelect { team }
            | SiegeDialog::Profile { team } => team,
            SiegeDialog::RoundMessage => return None,
        };
        let map_team = match team_num {
            SIEGE_TEAM2 => map.team2.clone(),
            _ => map.team1.clone(),
        };
        let classes = session
            .siege_catalog
            .team_theme(&map_team.use_team)
            .map(|theme| {
                theme
                    .class_names
                    .iter()
                    .filter_map(|name| session.siege_catalog.class(name).cloned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let current_class = session
            .siege_viewer_client()
            .and_then(|client| usize::try_from(client).ok())
            .and_then(|client| session.client_game.client_info(client, &session.siege_classes))
            .and_then(|info| session.siege_catalog.class(&info.siege_class).cloned());
        Some((map, map_team, classes, current_class))
    }

    pub(in crate::app) fn siege_hud_ui(&self) -> Option<crate::app::UiSiegeHud> {
        let session = self.game_session.as_ref()?;
        if session.client_game.gametype() != 7 {
            return None;
        }
        let now = session.cgame_time;
        let round = session.client_game.siege_round_state(now);
        let banner = match round.state {
            1 => Some("WAITING FOR PLAYERS".to_owned()),
            2 => {
                let remaining = (jka_assets::siege::SIEGE_ROUND_BEGIN_TIME
                    - now.saturating_sub(round.server_time))
                    .clamp(0, jka_assets::siege::SIEGE_ROUND_BEGIN_TIME);
                Some(format!("ROUND BEGINS IN {}", remaining / 1000 + 1))
            }
            _ => None,
        };

        let map = session.active_siege_map();
        let (timed_team, authored_time) = match map.as_ref() {
            Some(map) if map.team1.timed_ms > 0 => (SIEGE_TEAM1, map.team1.timed_ms),
            Some(map) if map.team2.timed_ms > 0 => (SIEGE_TEAM2, map.team2.timed_ms),
            _ => (0, 0),
        };
        let beat_time = session.client_game.siege_time_override();
        let team_switch = session
            .client_game
            .serverinfo_value(b"g_siegeTeamSwitch")
            .and_then(|value| value.trim().parse::<i32>().ok())
            .unwrap_or(0)
            != 0;
        let round_began = round.round_began_time.unwrap_or(round.server_time);
        let timer_ms = if timed_team != 0 && round.state == 0 {
            if team_switch && beat_time <= 0 {
                Some(now.saturating_sub(round_began).max(0))
            } else {
                let target = if beat_time > 0 { beat_time } else { authored_time };
                Some(round_began.saturating_add(target).saturating_sub(now).max(0))
            }
        } else {
            None
        };
        let timer = timer_ms.map(|ms| format!("{:02}:{:02}", ms / 60_000, (ms / 1000) % 60));
        let viewer_team = session.siege_viewer_team();
        let timer_color = if timed_team == 0 {
            [1.0, 1.0, 1.0, 1.0]
        } else if viewer_team == timed_team {
            [1.0, 0.25, 0.22, 1.0]
        } else {
            [0.30, 1.0, 0.42, 1.0]
        };

        let respawn = (session.siege_respawn_time > now).then(|| {
            let ms = session.siege_respawn_time.saturating_sub(now);
            format!("RESPAWN IN {}", (ms + 999) / 1000)
        });

        let stat_viewer = self.siege_stat_viewer_ui();
        Some(crate::app::UiSiegeHud { banner, timer, timer_color, respawn, stat_viewer })
    }

    fn siege_stat_viewer_ui(&self) -> Option<crate::app::UiSiegeStatViewer> {
        const EF_DEAD: i32 = 1 << 1;
        let session = self.game_session.as_ref()?;
        let viewer = session.siege_viewer_client()?;
        let viewer_info = usize::try_from(viewer).ok()
            .and_then(|client| session.client_game.client_info(client, &session.siege_classes))?;
        let viewer_class = session.siege_catalog.class(&viewer_info.siege_class)?;
        if !viewer_class.has_flag(jka_assets::siege::CFL_STATVIEWER) {
            return None;
        }
        let seen = self.crosshair_seen.filter(|seen| seen.on_target)?;
        let target = usize::try_from(seen.entity).ok().filter(|target| *target < 32)?;
        if target == usize::try_from(viewer).ok()? {
            return None;
        }
        let target_info = session.client_game.client_info(target, &session.siege_classes)?;
        if target_info.team != viewer_info.team {
            return None;
        }
        let extended = session.siege_extended.get(target)?;
        if extended.last_updated <= 0
            || extended.last_updated > session.cgame_time
            || session.cgame_time.saturating_sub(extended.last_updated) > 10_000
        {
            return None;
        }
        let state = session.presented_entities.iter().find(|entity| usize::from(entity.number) == target)?;
        if state.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0 {
            return None;
        }
        let weapon = state.state.field_i32("weapon").unwrap_or(-1);
        if extended.weapon < 0 || weapon != extended.weapon {
            return None;
        }
        let ammo_max = jka_movement::weapon_info(weapon)
            .and_then(|(ammo, _, _)| match ammo {
                0 => Some(0),
                1 => Some(100),
                2 | 3 | 4 => Some(300),
                5 => Some(25),
                6 => Some(800),
                7 | 8 | 9 => Some(10),
                _ => None,
            })
            .unwrap_or(0);
        Some(crate::app::UiSiegeStatViewer {
            name: target_info.name,
            health: extended.health,
            max_health: extended.max_health,
            ammo: extended.ammo,
            ammo_max,
        })
    }

    pub(in crate::app) fn egui_siege_dialog(&mut self, root: &mut egui::Ui) {
        let Some(dialog) = self.siege_dialog else { return; };
        // The dialog selector is App-owned so it can outlive the CGame object
        // during a disconnect/map transition. Never let an old Siege surface
        // bleed into the next session or the front end. Transition code owns
        // the overlay at that point, so discard state without restoring the
        // overlay captured by the previous session.
        if !self
            .game_session
            .as_ref()
            .is_some_and(|session| session.client_game.gametype() == 7)
        {
            self.siege_dialog = None;
            self.siege_round_message = None;
            self.siege_dialog_opened_overlay = false;
            self.siege_dialog_return_overlay = OverlayMode::None;
            return;
        }
        if dialog == SiegeDialog::RoundMessage {
            let message = self.siege_round_message.clone().unwrap_or_default();
            let mut close = false;
            egui::Window::new("Siege")
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(root.ctx(), |ui| {
                    if !message.is_empty() {
                        ui.heading(message);
                    }
                    ui.separator();
                    if ui.button("Continue").clicked() { close = true; }
                });
            if close {
                self.close_siege_dialog();
            }
            return;
        }
        let data = self.siege_dialog_data(dialog);
        let objective_status = self
            .game_session
            .as_ref()
            .map(|session| session.client_game.siege_objective_status())
            .unwrap_or_default();
        let mut close = false;
        let mut choose_class: Option<String> = None;

        egui::Window::new(match dialog {
            SiegeDialog::Briefing { .. } => "Siege Briefing",
            SiegeDialog::ClassSelect { .. } => "Choose Siege Class",
            SiegeDialog::Profile { .. } => "Siege Class Profile",
            SiegeDialog::RoundMessage => "Siege",
        })
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(root.ctx(), |ui| {
            let Some((map, team, classes, current_class)) = data.as_ref() else {
                ui.label("This server's Siege definition could not be resolved from the active game assets.");
                ui.separator();
                if ui.button("Close").clicked() { close = true; }
                return;
            };

            let mission = self.resolve_siege_text(&map.mission_name);
            if !mission.trim().is_empty() {
                ui.heading(mission);
            }
            ui.label(format!("Team: {}", self.resolve_siege_text(&team.group_name)));
            ui.separator();

            match dialog {
                SiegeDialog::Briefing { team: team_num } => {
                    let briefing = self.resolve_siege_text(&team.briefing);
                    if !briefing.is_empty() {
                        ui.label(briefing);
                        ui.add_space(8.0);
                    }
                    ui.strong("Objectives");
                    let completed = if team_num == SIEGE_TEAM2 {
                        &objective_status.team2
                    } else {
                        &objective_status.team1
                    };
                    for objective in &team.objectives {
                        let done = completed.get(objective.number.saturating_sub(1)).copied().unwrap_or(false);
                        let marker = if done { "[done]" } else if objective.final_objective { "[final]" } else { "[ ]" };
                        let desc = self.resolve_siege_text(&objective.description);
                        ui.label(format!("{marker} {}", if desc.is_empty() { &objective.goal_name } else { &desc }));
                    }
                }
                SiegeDialog::ClassSelect { .. } => {
                    if classes.is_empty() {
                        ui.label(format!("No usable classes were found for theme '{}'.", team.use_team));
                    } else {
                        for class in classes {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(&class.name);
                                    ui.label(format!("HP {}  Armor {}", class.start_health, class.start_armor));
                                    if ui.button("Select").clicked() {
                                        choose_class = Some(class.name.clone());
                                    }
                                });
                                let desc = self.resolve_siege_text(&class.description);
                                if !desc.is_empty() { ui.label(desc); }
                            });
                        }
                    }
                }
                SiegeDialog::Profile { .. } => {
                    if let Some(class) = current_class {
                        ui.heading(&class.name);
                        let desc = self.resolve_siege_text(&class.description);
                        if !desc.is_empty() { ui.label(desc); }
                        ui.separator();
                        ui.label(format!("Health: {}/{}", class.start_health, class.max_health));
                        ui.label(format!("Armor: {}/{}", class.start_armor, class.max_armor));
                        ui.label(format!("Speed: {:.2}x", class.speed));
                        if !class.weapons.is_empty() { ui.label(format!("Weapons: {}", class.weapons)); }
                        if !class.saber1.is_empty() { ui.label(format!("Saber: {}", class.saber1)); }
                    } else {
                        ui.label("No active Siege class is reported for the viewed player.");
                    }
                }
                SiegeDialog::RoundMessage => {}
            }

            ui.separator();
            if ui.button("Close").clicked() { close = true; }
        });

        if let Some(class_name) = choose_class {
            // TaystJK UI emits this exact server command. The class name is
            // asset-authored; remove quotes to keep the command token bounded.
            let class_name = class_name.replace('"', "");
            if let Err(error) = self.queue_server_command(&format!("siegeclass \"{class_name}\"")) {
                self.push_console_line(format!("^1Siege class selection failed: {error}"));
            } else {
                close = true;
            }
        }
        if close {
            self.close_siege_dialog();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siege_scoreboard_prefers_carried_flags_to_class_badges() {
        let class = jka_assets::siege::parse_siege_class(br#"
            ClassInfo {
                name "Rebel Tech"
                class_shader gfx/siege/tech
            }
        "#).unwrap().unwrap();
        let catalog = jka_assets::siege::SiegeCatalog {
            classes: vec![class],
            ..Default::default()
        };
        let row = UiScoreEntry {
            client: 1, name: "Player".into(), score: 0, powerups: 0,
            captures: 0, assists: 0, defends: 0, deaths: None,
            ping: 20, time: 1, team: 1, siege_class: "rebel tech".into(),
            siege_icon: None, spectating_you: false, muted: false, crowned: false, model: String::new(), model_fallback: String::new(), h2h: None, accuracy: 0, excellent: 0, gauntlet: 0,
        };
        let mut rows = vec![row.clone(), row.clone(), row.clone(), row];
        rows[1].powerups = 1 << PW_REDFLAG;
        rows[2].powerups = (1 << PW_NEUTRALFLAG) | (1 << PW_BLUEFLAG);
        rows[3].siege_class = "Unknown class".into();
        rows[3].siege_icon = Some(99);
        assert_eq!(siege_scoreboard_icons(&mut rows, &catalog),
            ["gfx/siege/tech", "gfx/hud/mpi_rflag", "icons/iconf_neutral1"]);
        assert_eq!(rows.iter().map(|row| row.siege_icon).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2), None]);
    }
}
