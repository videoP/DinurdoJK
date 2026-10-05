use super::App;
use crate::{strafe_trail, ui::OverlayMode};

impl App {
    pub(super) fn open_strafe_trails(&mut self) {
        if self.strafe_trail_name_input.is_empty() {
            self.strafe_trail_name_input = self
                .strafe_trails
                .available()
                .first()
                .cloned()
                .unwrap_or_default();
        }
        self.strafe_trail_log_input = self.strafe_trails.settings.log_name.clone();
        if let Err(error) = self.strafe_trails.request_scan(&self.base, self.game.as_deref()) {
            self.push_console_line(format!("^1Strafe trail scan:^7 {error}"));
        }
        self.set_overlay(OverlayMode::StrafeTrails);
    }

    pub(super) fn egui_strafe_trails(&mut self, root: &mut egui::Ui) {
        let mut close = false;
        let mut refresh = false;
        let mut load: Option<(String, u8)> = None;
        let mut clear: Option<i32> = None;
        let mut toggle_client: Option<i32> = None;
        let mut stop_recording = false;
        let mut start_recording: Option<String> = None;
        let mut settings_changed = false;

        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("STRAFE TRAILS");
                ui.add_space(8.0);
                ui.label(egui::RichText::new("jaPRO-compatible trail files, loaded asynchronously").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() { close = true; }
                    if ui.button("Refresh files").clicked() { refresh = true; }
                });
            });
            ui.separator();

            ui.columns(2, |columns| {
                let left = &mut columns[0];
                left.heading("Trail files");
                left.horizontal(|ui| {
                    ui.label("Filter");
                    ui.text_edit_singleline(&mut self.strafe_trail_filter);
                });
                left.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.strafe_trail_name_input);
                    ui.label("Slot");
                    ui.add(egui::DragValue::new(&mut self.strafe_trail_slot).range(0..=31));
                    if ui.button("Load").clicked() && !self.strafe_trail_name_input.trim().is_empty() {
                        load = Some((self.strafe_trail_name_input.trim().to_owned(), self.strafe_trail_slot));
                    }
                });
                if self.strafe_trails.catalog_pending() {
                    left.label(egui::RichText::new("Scanning VFS…").weak());
                }
                if let Some(error) = self.strafe_trails.catalog_error() {
                    left.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let filter = self.strafe_trail_filter.trim().to_ascii_lowercase();
                let names = self.strafe_trails.available().to_vec();
                egui::ScrollArea::vertical().max_height(260.0).show(left, |ui| {
                    for name in names {
                        if !filter.is_empty() && !name.to_ascii_lowercase().contains(&filter) { continue; }
                        ui.horizontal(|ui| {
                            if ui.small_button("Load").clicked() {
                                load = Some((name.clone(), self.strafe_trail_slot));
                            }
                            if ui.selectable_label(self.strafe_trail_name_input == name, &name).clicked() {
                                self.strafe_trail_name_input = name;
                            }
                        });
                    }
                });

                left.add_space(10.0);
                left.heading("Loaded");
                if left.button("Clear all trail geometry").clicked() { clear = Some(-1); }
                let loaded = self.strafe_trails.loaded()
                    .map(|trail| (trail.slot, trail.name.clone(), trail.point_count, trail.segment_count))
                    .collect::<Vec<_>>();
                if loaded.is_empty() { left.label(egui::RichText::new("No loaded trails.").weak()); }
                for (slot, name, points, segments) in loaded {
                    left.horizontal(|ui| {
                        if ui.small_button("Clear").clicked() { clear = Some(i32::from(slot)); }
                        ui.label(format!("[{slot:02}] {name}"));
                        ui.label(egui::RichText::new(format!("{points} pts / {segments} seg")).weak().small());
                    });
                }

                let right = &mut columns[1];
                right.heading("Appearance");
                settings_changed |= right.add(egui::Slider::new(&mut self.strafe_trails.settings.radius, 0.1..=20.0).text("Radius")).changed();
                settings_changed |= right.add(egui::Slider::new(&mut self.strafe_trails.settings.draw_distance, 512.0..=32768.0).logarithmic(true).text("Draw distance")).changed();
                settings_changed |= right.checkbox(&mut self.strafe_trails.settings.ghost, "Ghost / translucent").changed();
                settings_changed |= right.add(egui::Slider::new(&mut self.strafe_trails.settings.life_seconds, 0.5..=60.0).text("Live trail life (s)")).changed();
                settings_changed |= right.add(egui::Slider::new(&mut self.strafe_trails.settings.fps, 1.0..=125.0).text("Recorded SV_FPS")).changed();
                settings_changed |= right.checkbox(&mut self.strafe_trails.settings.plums, "Second markers (plums)").changed();
                right.label(egui::RichText::new("Plums use Recorded SV_FPS to label loaded CFG trails at 1-second intervals, matching jaPRO/TaystJK.").weak().small());
                right.label(egui::RichText::new("Loaded CFGs use a 2D spatial index; only nearby segments are handed to the existing threaded FX line tessellator.").weak().small());

                right.add_space(10.0);
                right.heading("Live players");
                let players = (0..strafe_trail::MAX_CLIENTS)
                    .filter_map(|client| self.game_session.as_ref()?.client_game.client_name(client).map(|name| (client, name)))
                    .collect::<Vec<_>>();
                if players.is_empty() {
                    right.label(egui::RichText::new("Connect to a server or play a demo to trace players.").weak());
                }
                egui::ScrollArea::vertical().max_height(210.0).show(right, |ui| {
                    for (client, name) in players {
                        let mut enabled = self.strafe_trails.is_tracing(client);
                        if ui.checkbox(&mut enabled, format!("#{client:02}  {name}")).changed() {
                            toggle_client = Some(client as i32);
                        }
                    }
                });

                right.add_space(10.0);
                right.heading("Record your race");
                right.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.strafe_trail_log_input);
                    if ui.button("Record").clicked() && !self.strafe_trail_log_input.trim().is_empty() {
                        start_recording = Some(self.strafe_trail_log_input.trim().to_owned());
                    }
                    if ui.button("Stop").clicked() { stop_recording = true; }
                });
                right.label(egui::RichText::new(format!("cg_logStrafeTrail = {}", self.strafe_trails.settings.log_name)).weak().small());
                right.label(egui::RichText::new("Points are queued to the worker only while your jaPRO race timer is active; file writes never block the render/CGame frame.").weak().small());
            });
        });

        if refresh {
            if let Err(error) = self.strafe_trails.request_scan(&self.base, self.game.as_deref()) {
                self.push_console_line(format!("^1Strafe trail scan:^7 {error}"));
            }
        }
        if let Some((name, slot)) = load {
            match self.strafe_trails.request_load(&self.base, self.game.as_deref(), &name, slot) {
                Ok(()) => self.push_console_line(format!("^3loadTrail:^7 queued {name} (slot {slot})")),
                Err(error) => self.push_console_line(format!("^1loadTrail:^7 {error}")),
            }
        }
        if let Some(slot) = clear {
            let count = self.strafe_trails.clear(slot);
            self.push_console_line(format!("^3clearTrail:^7 cleared matching trail geometry ({count} loaded file(s))."));
        }
        if let Some(client) = toggle_client {
            if let Err(error) = self.strafe_trails.toggle_player(client) {
                self.push_console_line(format!("^1strafeTrail:^7 {error}"));
            } else {
                settings_changed = true;
            }
        }
        if stop_recording {
            let active_dir = jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref());
            if let Err(error) = self.strafe_trails.set_log_name(active_dir, "0") {
                self.push_console_line(format!("^1cg_logStrafeTrail:^7 {error}"));
            } else { settings_changed = true; }
        } else if let Some(name) = start_recording {
            let active_dir = jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref());
            if let Err(error) = self.strafe_trails.set_log_name(active_dir, &name) {
                self.push_console_line(format!("^1cg_logStrafeTrail:^7 {error}"));
            } else { settings_changed = true; }
        }
        if settings_changed { self.mark_config_dirty(); }
        if close { self.set_overlay(OverlayMode::None); }
    }
}
