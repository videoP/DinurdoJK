//! Movement settings.
use crate::app::egui_menu::{theme, App};

impl App {
    pub(in crate::app) fn egui_movement_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "MOVEMENT",
            "Client movement/input cadence and movement-specific preferences for the active mod.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_movement_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(
                    ui,
                    "CLIENT MOVEMENT",
                    "Authoritative server movement stays unchanged; this controls how often the client authors user commands.",
                );
                theme::row(
                    ui,
                    "Player movement rate",
                    "cl_commandRate. How often this client authors movement/input commands. With normal pmove_fixed 0 play, those command timestamps drive Pmove. If a remote server enables pmove_fixed, prediction automatically obeys that server's pmove_msec instead. This never caps render FPS or the event-rate subframe/late-latched mouse-look path.",
                    theme::Reset::MovementCommandRate,
                    |ui| {
                        let mut rate = self.network.command_rate;
                        if ui
                            .add(
                                egui::DragValue::new(&mut rate)
                                    .range(15..=1000)
                                    .speed(1.0)
                                    .suffix(" Hz")
                                    .update_while_editing(false),
                            )
                            .changed()
                        {
                            self.set_player_movement_rate(rate);
                        }
                    },
                );
                theme::row(
                    ui,
                    "Command window",
                    "cl_commandsize. TaystJK-compatible command history for prediction, packet selection and CONNECTION INTERRUPTED. AUTO considers movement rate and ping (at least 64). Manual values: 64, 128, 256 or 512. Does not change packet cadence.",
                    theme::Reset::MovementCommandSize,
                    |ui| {
                        let configured = self.japro_cg.lagometer.command_size;
                        let ping = self.net.as_ref().map(|net| net.session().ping());
                        let effective = self.japro_cg.lagometer.effective_command_backup(
                            self.network.command_rate, ping,
                        );
                        let mut automatic = configured == crate::lagometer::AUTO_COMMAND_SIZE;
                        ui.horizontal(|ui| {
                            if ui.selectable_label(automatic, "AUTO").clicked() && !automatic {
                                let _ = self.set_console_cvar("cl_commandsize", "auto");
                                automatic = true;
                            }
                            if ui.selectable_label(!automatic, "Manual").clicked() && automatic {
                                // Switching to manual starts at the current effective value.
                                let _ = self.set_console_cvar("cl_commandsize", &effective.to_string());
                                automatic = false;
                            }
                            if automatic {
                                let window_ms = effective as f32 * 1000.0 / self.network.command_rate.max(15) as f32;
                                ui.label(format!("{effective} cmds (~{window_ms:.0} ms)"));
                            } else {
                                let size = if configured == crate::lagometer::AUTO_COMMAND_SIZE {
                                    effective
                                } else {
                                    configured
                                };
                                let mut notch = (size as u32).ilog2().clamp(6, 9);
                                if ui.add(egui::Slider::new(&mut notch, 6..=9).show_value(false)).changed() {
                                    let value = 1_u32 << notch;
                                    let _ = self.set_console_cvar("cl_commandsize", &value.to_string());
                                }
                                ui.label(format!("{} cmds", 1_u32 << notch));
                            }
                        });
                    },
                );

                self.egui_mod_movement_settings(ui);
            });
    }
}
