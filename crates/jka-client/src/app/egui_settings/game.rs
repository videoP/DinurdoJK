//! Game.
use crate::app::egui_settings::{segmented_row, theme, App};

#[cfg(windows)]
#[derive(Clone)]
struct JkaLinkSettings {
    status: Result<crate::protocol_link::RegistrationStatus, String>,
    confirm_replace: bool,
    message: Option<Result<String, String>>,
}

#[cfg(windows)]
fn jka_link_settings(ui: &mut egui::Ui) {
    use crate::protocol_link::{self, RegistrationStatus};

    // Cache the registry read across egui frames. Refresh handles external changes.
    let id = ui.make_persistent_id("jka_protocol_registration_state");
    let mut state = ui.ctx().data(|data| data.get_temp::<JkaLinkSettings>(id))
        .unwrap_or_else(|| JkaLinkSettings {
            status: protocol_link::registration_status(),
            confirm_replace: false,
            message: None,
        });

    theme::section(ui, "SERVER LINKS", "Windows jka:// server links. No registry changes occur on game startup.");
    theme::row(ui, "Open jka:// links", "Explicitly register this executable to handle jka://host[:port] links for your Windows user. Other clients' associations are not replaced without confirmation.", theme::Reset::None, |ui| {
        match state.status.clone() {
            Ok(RegistrationStatus::OwnedByUs) => {
                ui.label("Registered to DinurdoJK");
                if ui.button("Unregister").clicked() {
                    state.message = Some(match protocol_link::unregister_explicit() {
                        Ok(true) => Ok("DinurdoJK's jka:// handler removed.".to_owned()),
                        Ok(false) => Err("Association is no longer owned by this executable; nothing was removed.".to_owned()),
                        Err(error) => Err(error),
                    });
                    state.status = protocol_link::registration_status();
                    state.confirm_replace = false;
                }
            }
            Ok(RegistrationStatus::Unregistered) => {
                ui.label("Not registered");
                if ui.button("Register").clicked() {
                    state.message = Some(protocol_link::register_if_unclaimed_explicit()
                        .map(|()| "DinurdoJK registered for jka:// links.".to_owned()));
                    state.status = protocol_link::registration_status();
                }
            }
            Ok(RegistrationStatus::OtherHandler) => {
                ui.label("Another application is registered");
                if ui.button("Change handler...").clicked() {
                    state.confirm_replace = true;
                    state.message = None;
                }
            }
            Err(ref error) => { ui.colored_label(theme::WARNING(), error); }
        }
        if ui.button("Refresh").clicked() {
            state.status = protocol_link::registration_status();
            state.confirm_replace = false;
            state.message = None;
        }
    });

    if state.confirm_replace {
        ui.label("Replace the current jka:// registry handler with DinurdoJK for this Windows user?");
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                state.confirm_replace = false;
            }
            if ui.button("Confirm replacement").clicked() {
                // Another process might have changed this association after
                // the UI was opened; this is the user's explicit opt-in.
                state.message = Some(protocol_link::register_explicit()
                    .map(|()| "DinurdoJK registered for jka:// links.".to_owned()));
                state.status = protocol_link::registration_status();
                state.confirm_replace = false;
            }
        });
    }
    if let Some(message) = &state.message {
        match message {
            Ok(text) => { ui.label(text); }
            Err(error) => { ui.colored_label(theme::WARNING(), error); }
        }
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
}

impl App {
    pub(in crate::app) fn egui_game_settings(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "GAME / VIEW",
            "Launch, weapon and view-effect options. Camera controls live on the Camera tab.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_game_view_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(ui, "STARTUP", "Choose where the game opens on your next launch.");
                theme::row(
                    ui,
                    "Game Launch screen",
                    "Menu to open when starting the game. Applies next launch; launching directly into a map takes priority.",
                    theme::Reset::None,
                    |ui| {
                        let mut selected = self.game_launch_screen;
                        egui::ComboBox::from_id_salt("game_launch_screen")
                            .selected_text(selected.label())
                            .show_ui(ui, |ui| {
                                for &(screen, _, label) in crate::config::GameLaunchScreen::OPTIONS {
                                    ui.selectable_value(&mut selected, screen, label);
                                }
                            });
                        if selected != self.game_launch_screen {
                            let _ = self.set_console_cvar("ui_gameLaunchScreen", selected.page_name());
                        }
                    },
                );

                #[cfg(windows)]
                jka_link_settings(ui);

                theme::section(ui, "UPDATES", "Check GitHub Releases on launch. Downloads are always manual.");
                theme::row(ui, "Check for updates at launch",
                    "Checks videop/DinurdoJK once at launch. Only announces a newer stable release; never downloads automatically.",
                    theme::Reset::None, |ui| {
                        if let Some(on) = theme::switch(ui, self.updater.auto_check) {
                            if let Err(e) = self.updater.set_auto_check(on) {
                                self.updater.error = Some(e);
                            }
                        }
                    });
                if ui.button("Check for Updates").clicked() { self.check_for_updates(true); }
                if self.updater.checking { ui.label("Checking GitHub..."); }
                if let Some(result) = &self.updater.manual_result { ui.label(result); }
                if let Some(error) = &self.updater.error { ui.colored_label(theme::WARNING(), error); }
                if let Some(release) = self.updater.release.clone() {
                    ui.label(format!("Available: {} (installed: {})", release.tag, env!("CARGO_PKG_VERSION")));
                    if self.updater.downloading {
                        let ratio = if self.updater.total == 0 { 0.0 } else { self.updater.downloaded as f32 / self.updater.total as f32 };
                        ui.add(egui::ProgressBar::new(ratio.clamp(0.0, 1.0)).show_percentage());
                    } else if self.updater.staged.is_some() {
                        ui.label("Verified. It will apply when you next start the game.");
                        if self.updater.applying {
                            ui.label("Starting the installer...");
                        } else if ui.button("Restart & Apply Update").clicked() {
                            self.apply_update_now();
                        }
                    } else if ui.button("Download & Install").clicked() {
                        self.download_update();
                    }
                }

                theme::section(ui, "WEAPONS", "Weapon selection and projectile presentation.");
                theme::row(
                    ui,
                    "Simulate projectiles",
                    "cg_simulatedProjectiles. Show your own projectile immediately after firing on jaPRO servers advertising unlagged projectiles. Pseudorandom spread uses the local firing command time when advertised by jcinfo; unsupported spread overrides remain server authoritative. Not used on vanilla JKA/JK2 or JA+.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.simulated_projectiles) {
                            let _ = self.set_console_cvar("cg_simulatedProjectiles", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Predict rocket knockback",
                    "cg_predictKnockback. In jaPRO race mode, anticipate your own primary rocket splash impulse using the shared simulated projectile trajectory and native Pmove. Requires Simulate projectiles and a server advertising UNLAGGEDPROJ. Default off; diagnostics: console values 2..5.",
                    theme::Reset::None,
                    |ui| {
                        let enabled = self.network.predict_knockback != 0;
                        ui.add_enabled_ui(self.simulated_projectiles, |ui| {
                            if let Some(value) = theme::switch(ui, enabled) {
                                let _ = self.set_console_cvar("cg_predictKnockback", if value { "1" } else { "0" });
                            }
                        });
                    },
                );
                if let Some(value) = segmented_row(
                    ui,
                    "Auto switch",
                    "cg_autoSwitch. Switch to a weapon you pick up if it is better than the current one (never away from the saber), and to your best weapon when one runs dry. Safe skips rockets, thermal detonators and mines.",
                    theme::Reset::None,
                    self.audio.game.auto_switch,
                    &[(0, "Never"), (1, "Safe weapons"), (2, "Any weapon")],
                ) {
                    let _ = self.set_console_cvar("cg_autoSwitch", &value.to_string());
                }

                theme::section(
                    ui,
                    "PLAYER ANIMATIONS",
                    "Local rendering preferences; server gameplay and target selection are unchanged.",
                );
                if let Some(mode) = segmented_row(
                    ui,
                    "Automatic head tracking",
                    "cg_headLook. Off: no automatic look-target head turns. On: unchanged TaystJK/OpenJK behavior. Smoothed: gradually rotate toward new targets, without changing their selection or gameplay aim.",
                    theme::Reset::None,
                    self.head_look,
                    &[(0, "Off"), (1, "On"), (2, "Smoothed")],
                ) {
                    let _ = self.set_console_cvar("cg_headLook", &mode.to_string());
                }

                theme::row(
                    ui,
                    "Directional swim tilt",
                    "cg_swimTilt. Tilt the full player model toward the swim velocity, with a gradual head-up lift near the water surface. Keeps the original swim animations. 0 = original JKA pose, 1 = near-horizontal forward swim. Camera, collisions and gameplay remain unchanged.",
                    theme::Reset::None,
                    |ui| {
                        let mut strength = self.swim_tilt;
                        if ui.add(egui::Slider::new(&mut strength, 0.0..=1.0).step_by(0.05)).changed() {
                            let _ = self.set_console_cvar("cg_swimTilt", &format!("{strength:.2}"));
                        }
                    },
                );

                theme::row(
                    ui,
                    "Swim turn banking",
                    "cg_swimTurnBank. Up to 60° of bank based on how quickly horizontal swim velocity changes direction. Active only while swimming and moving; 0 disables extra turning roll. cg_swimTilt 0 disables all swim tilting.",
                    theme::Reset::None,
                    |ui| {
                        let mut strength = self.swim_turn_bank;
                        if ui.add(egui::Slider::new(&mut strength, 0.0..=1.0).step_by(0.05)).changed() {
                            let _ = self.set_console_cvar("cg_swimTurnBank", &format!("{strength:.2}"));
                        }
                    },
                );

                theme::row(
                    ui,
                    "Airborne acceleration lean",
                    "cg_moveLean. Client-only jump/strafejump/bunnyhop pitch and roll primarily from AIR acceleration/braking, with a small speed baseline. PM_NORMAL air movement only: zero through 500 UPS, full by 800. Grounded and noclip movement are unchanged; swimming uses its separate controls.",
                    theme::Reset::None,
                    |ui| {
                        let mut strength = self.move_lean;
                        if ui.add(egui::Slider::new(&mut strength, 0.0..=1.0).step_by(0.05)).changed() {
                            let _ = self.set_console_cvar("cg_moveLean", &format!("{strength:.2}"));
                        }
                    },
                );

                theme::section(
                    ui,
                    "SPECTATING & DEMOS",
                    "Presentation options shared by every server/mod and demo playback.",
                );
                theme::row(
                    ui,
                    "Player outlines",
                    "cg_specHud. Outline other living players through walls while spectating or watching demos. Team players are red/blue; neutral/FFA players are yellow.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(on) = theme::switch(ui, self.japro_cg.spec_hud) {
                            let _ = self.set_console_cvar("cg_specHud", if on { "1" } else { "0" });
                        }
                    },
                );

                theme::section(ui, "VIEW EFFECTS", "Camera effects driven by the game.");
                theme::row(
                    ui,
                    "Score numbers",
                    "cg_scorePlums. Floating score numbers where you score.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.audio.game.score_plums) {
                            let _ = self.set_console_cvar("cg_scorePlums", if value { "1" } else { "0" });
                        }
                    },
                );
                if let Some(value) = segmented_row(
                    ui,
                    "Gibs",
                    "cg_blood. Gibbed players: off (a death voice plays instead), skull or brain only, or full gibs. Needs the jaPRO models/gibs assets; jaPRO itself defaults to off.",
                    theme::Reset::None,
                    self.audio.game.blood,
                    &[(0, "Off"), (1, "Skull / brain"), (2, "Full")],
                ) {
                    let _ = self.set_console_cvar("cg_blood", &value.to_string());
                }
                if let Some(value) = segmented_row(
                    ui,
                    "Screen shake",
                    "cg_screenShake. Effects: shake from explosions and creature stomps. Effects + weapons: also the kick of firing rockets, alt repeater, flechette and charged shots, plus shake while charging bryar/bowcaster/demp2. Server-triggered shake events (rancors, scripted quakes) are unaffected, as in OpenJK.",
                    theme::Reset::None,
                    self.screen_shake,
                    &[(0, "Off"), (1, "Effects"), (2, "Effects + weapons")],
                ) {
                    let _ = self.set_console_cvar("cg_screenShake", &value.to_string());
                }
            });
    }
}
