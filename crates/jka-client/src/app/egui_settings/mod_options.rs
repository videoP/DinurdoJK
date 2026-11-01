//! Settings that belong to the currently active server mod.
use crate::app::egui_settings::{theme, App};
use crate::net::mod_support::ServerMod;

impl App {
    fn egui_japro_plugin_disable_options(
        &mut self,
        ui: &mut egui::Ui,
        include: impl Fn(u8) -> bool,
    ) {
        let mut bits = self.network.plugin_disable;
        for option in crate::japro_cg::JAPRO_PLUGIN_DISABLE_OPTIONS
            .iter()
            .copied()
            .filter(|option| include(option.bit))
        {
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
    }

    /// Mod-specific movement preferences shown on Setup -> Movement.
    pub(in crate::app) fn egui_mod_movement_settings(&mut self, ui: &mut egui::Ui) {
        let Some(active) = self.ui_mod() else {
            return;
        };
        let heading = active.label().to_uppercase();

        match active {
            ServerMod::Japro => {
                theme::section(
                    ui,
                    &heading,
                    "Movement-specific preferences advertised to the active jaPRO server.",
                );
                // TaystJK's jaPRO cp_pluginDisable movement/gameplay cluster:
                // BHOP, no-roll, no-cartwheel, alternate run animation and duel teleport.
                self.egui_japro_plugin_disable_options(ui, |bit| (19..=23).contains(&bit));
            }
            _ => {
                theme::section(
                    ui,
                    &heading,
                    "Movement-specific settings for the active mod.",
                );
                theme::label(
                    ui,
                    theme::plain(
                        "No client-side movement preferences are available for this mod yet.",
                        11.5,
                        theme::TEXT_FAINT(),
                    ),
                );
            }
        }
    }

    /// Dynamic Setup -> [CURRENT MOD] page. General movement and bindings live
    /// on their own pages; account/login lives on Join, while this page owns
    /// presentation/setup preferences for the active mod.
    pub(in crate::app) fn egui_mod_setup_settings(&mut self, ui: &mut egui::Ui) {
        let active = self.ui_mod();
        let title = active.map_or_else(|| "MOD".to_owned(), |mod_| mod_.label().to_uppercase());
        theme::page_title(
            ui,
            &title,
            "Client-side presentation and setup preferences for the active mod.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_current_mod_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| match active {
                Some(ServerMod::Japro) => {
                    self.egui_japro_game_settings(ui);

                    theme::section(
                        ui,
                        "JAPRO PREFERENCES",
                        "jaPRO client preferences for presentation, feedback, weapon setup and movement helpers. Core movement preferences live on Setup -> Movement.",
                    );
                    self.egui_japro_plugin_disable_options(ui, |bit| !(19..=23).contains(&bit));
                    theme::row(
                        ui,
                        "Jump height shade",
                        "r_jumpHeightShade. While airborne in jaPRO's SP movement style, tints flat surfaces by where you would land: green just below your jump height (speed kept), fading to red the lower it is, and dim blue to cyan above it up to your reachable height (speed halved). Draws nothing in other movement styles.",
                        theme::Reset::None,
                        |ui| {
                            if let Some(enabled) = theme::switch(ui, self.jump_height_shade) {
                                if let Err(error) = self.set_console_cvar(
                                    "r_jumpHeightShade",
                                    if enabled { "1" } else { "0" },
                                ) {
                                    self.push_console_line(error);
                                }
                            }
                        },
                    );
                }
                Some(mod_) => {
                    let heading = mod_.label().to_uppercase();
                    theme::section(
                        ui,
                        &heading,
                        "Settings exposed specifically for the active mod.",
                    );
                    theme::label(
                        ui,
                        theme::plain(
                            "No client-side settings are available for this mod yet.",
                            11.5,
                            theme::TEXT_FAINT(),
                        ),
                    );
                }
                None => {
                    theme::section(ui, "MOD", "No active mod detected.");
                    theme::label(
                        ui,
                        theme::plain(
                            "Connect to a server or start a local game to expose its mod-specific settings.",
                            11.5,
                            theme::TEXT_FAINT(),
                        ),
                    );
                }
            });
    }
}
