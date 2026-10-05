//! Mod options.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app) fn egui_mod_settings(&mut self, ui: &mut egui::Ui) {
        use crate::net::mod_support::ServerMod;

        theme::page_title(ui, "MOD", "Settings for the active mod.");
        egui::ScrollArea::vertical()
            .id_salt("jka_mod_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let active = self.ui_mod();

                theme::section(
                    ui,
                    "ACTIVE MOD",
                    "Detected from the connected server's game state, or the game directory of a local game.",
                );
                theme::row(
                    ui,
                    "Active mod",
                    "The server mod detected from its advertised/configstring state.",
                    theme::Reset::None,
                    |ui| {
                        theme::glow_label(
                            ui,
                            active.map_or("Not connected", ServerMod::label),
                            12.5,
                            theme::TEXT_DIM,
                        );
                    },
                );

                match active {
                    Some(ServerMod::Japro) => {}
                    Some(_) => {
                        theme::section(
                            ui,
                            "SETTINGS",
                            "No client-side settings are available for this server mod yet.",
                        );
                        return;
                    }
                    None => {
                        theme::section(
                            ui,
                            "SETTINGS",
                            "Connect to a server or start a local game to view settings for its active mod.",
                        );
                        return;
                    }
                }

                self.egui_japro_account(ui);

                theme::section(
                    ui,
                    "JAPRO CONTROLS",
                    "jaPRO-specific bindings. Base JKA controls remain on the Controls page.",
                );

                if self.controls_waiting_for_key
                    && crate::keybinds::is_japro_selection(self.controls_selected)
                {
                    self.egui_binding_capture_overlay(ui.ctx());
                }

                let mut rebind = None;
                let mut clear = None;
                for (index, action) in crate::keybinds::JAPRO_CONTROL_ACTIONS.iter().enumerate() {
                    let selection = crate::keybinds::japro_selection(index);
                    let waiting = self.controls_waiting_for_key && self.controls_selected == selection;
                    let binding = self.bindings.display_for_command(action.command);
                    let tip = format!("Bound to the \"{}\" command.", action.command);
                    theme::row(
                        ui,
                        &super::egui_menu::title_case(action.label),
                        &tip,
                        theme::Reset::None,
                        |ui| {
                            let (text, color) = if waiting {
                                ("PRESS A KEY…".to_owned(), theme::WARNING)
                            } else if binding == "UNBOUND" {
                                ("Unbound".to_owned(), theme::TEXT_DISABLED)
                            } else {
                                (binding.clone(), theme::ACCENT)
                            };
                            let response =
                                super::egui_menu::binding_slot(ui, &text, color, waiting);
                            if response.clicked() {
                                rebind = Some(selection);
                            }
                            if response.secondary_clicked() {
                                clear = Some(selection);
                            }
                        },
                    );
                }

                if let Some(selection) = rebind {
                    self.controls_selected = selection;
                    self.controls_waiting_for_key = true;
                    self.publish_ui();
                } else if let Some(selection) = clear {
                    self.controls_selected = selection;
                    self.unbind_selected_control();
                }

                theme::section(
                    ui,
                    "JAPRO PLUGIN DISABLE",
                    "Complete TaystJK/jaPRO cp_pluginDisable preferences. Saved locally and sent in userinfo when you join a jaPRO server.",
                );
                let mut bits = self.network.plugin_disable;
                for option in crate::japro_cg::JAPRO_PLUGIN_DISABLE_OPTIONS {
                    let enabled = option.enabled(bits);
                    theme::row(
                        ui,
                        option.label,
                        option.tooltip,
                        theme::Reset::None,
                        |ui| {
                            if let Some(enabled) = theme::switch(ui, enabled) {
                                if enabled {
                                    bits |= option.mask();
                                } else {
                                    bits &= !option.mask();
                                }
                            }
                        },
                    );
                }
                if bits != self.network.plugin_disable {
                    if let Err(error) = self.set_console_cvar("cp_pluginDisable", &bits.to_string()) {
                        self.push_console_line(error);
                    }
                }

                theme::section(ui, "JAPRO HELPERS", "Visual aids for jaPRO movement styles.");
                theme::row(
                    ui,
                    "Jump height shade",
                    "r_jumpHeightShade. While airborne in jaPRO's SP movement style, tints flat surfaces by \
                     where you would land: green just below your jump height (speed kept), fading to red \
                     the lower it is, and dim blue to cyan above it up to your reachable height (speed \
                     halved). Draws nothing in other movement styles.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.jump_height_shade) {
                            if let Err(error) =
                                self.set_console_cvar("r_jumpHeightShade", if enabled { "1" } else { "0" })
                            {
                                self.push_console_line(error);
                            }
                        }
                    },
                );
            });
    }
}
