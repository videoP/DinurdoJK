//! Account.
use crate::app::egui_settings::{theme, App};

impl App {
    /// jaPRO account login plus secure, exact-endpoint automatic logins.
    ///
    /// A saved credential is keyed by the resolved UDP `IP:port`, never by a
    /// hostname or a friendly server name. Presence in the OS credential store
    /// means auto-login is enabled for that endpoint.
    pub(in crate::app::egui_settings) fn egui_japro_account(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "ACCOUNT",
            "Log in to your jaPRO account on the connected server. Register creates one if the server allows it.",
        );
        // jaPRO keeps both at 15 characters and takes them as single arguments.
        const LIMIT: usize = 15;
        let clean = |text: &mut String| {
            text.retain(|c| !c.is_whitespace() && !matches!(c, ';' | '"' | '\\'));
        };

        let mut username_changed = false;
        theme::row(
            ui,
            "Username",
            "ui_username. Up to 15 characters, no spaces. Saved to the config.",
            theme::Reset::None,
            |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.network.ui_username)
                        .char_limit(LIMIT)
                        .desired_width(220.0),
                );
                username_changed = response.changed();
            },
        );
        let mut submit = false;
        theme::row(
            ui,
            "Password",
            "ui_password. Up to 15 characters, no spaces. Kept for this session only and never written to the config.",
            theme::Reset::None,
            |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.network.ui_password)
                        .password(true)
                        .char_limit(LIMIT)
                        .desired_width(220.0),
                );
                submit = response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
            },
        );
        clean(&mut self.network.ui_username);
        clean(&mut self.network.ui_password);
        if username_changed {
            self.mark_config_dirty();
        }

        let connected = self.live_connected();
        let ready = !self.network.ui_username.is_empty() && !self.network.ui_password.is_empty();
        let mut command = None;
        theme::row(ui, "Account", "", theme::Reset::None, |ui| {
            ui.add_enabled_ui(connected, |ui| {
                ui.add_enabled_ui(ready, |ui| {
                    if theme::primary_button(ui, "LOG IN").clicked()
                        || (submit && ready && connected)
                    {
                        command = Some(format!(
                            "login {} {}",
                            self.network.ui_username, self.network.ui_password
                        ));
                    }
                    ui.add_space(8.0);
                    if theme::ghost_button(ui, "REGISTER").clicked() {
                        command = Some(format!(
                            "register {} {}",
                            self.network.ui_username, self.network.ui_password
                        ));
                    }
                });
                ui.add_space(8.0);
                if theme::ghost_button(ui, "LOG OUT").clicked() {
                    command = Some("logout".to_owned());
                }
            });
        });
        if !connected {
            theme::label(
                ui,
                theme::plain("Join a jaPRO server to log in.", 11.5, theme::TEXT_FAINT),
            );
        }
        if let Some(command) = command {
            self.forward_command_to_server(&command);
        }

        ui.add_space(16.0);
        theme::section(
            ui,
            "SAVED LOGINS",
            "Passwords are stored by the operating system, not in DinurdoJK.cfg. Auto-login requires an exact resolved IP:port match and a gamestate that identifies jaPRO.",
        );

        if !crate::credential_store::supported() {
            theme::label(
                ui,
                theme::plain(
                    "Secure saved logins are unavailable on this platform.",
                    11.5,
                    theme::TEXT_FAINT,
                ),
            );
            return;
        }

        let current = self.current_japro_server_endpoint();
        if let Some(server) = current {
            let saved = self
                .japro_saved_logins
                .iter()
                .find(|entry| entry.server == server)
                .cloned();
            theme::row(
                ui,
                "Current server",
                "The exact resolved UDP endpoint used as the credential identity. Hostnames and display names are never used for automatic login matching.",
                theme::Reset::None,
                |ui| {
                    theme::glow_label(ui, &server.to_string(), 12.5, theme::TEXT);
                },
            );

            let mut save_current = false;
            let mut forget_current = false;
            theme::row(
                ui,
                "Auto-login",
                "A saved login is sent at most once per connection, only after this exact endpoint's gamestate identifies the server as jaPRO.",
                theme::Reset::None,
                |ui| {
                    if let Some(saved) = &saved {
                        theme::glow_label(
                            ui,
                            &format!("ON  ·  {}", saved.username),
                            12.5,
                            theme::TEXT,
                        );
                        ui.add_space(10.0);
                        if theme::ghost_button(ui, "FORGET").clicked() {
                            forget_current = true;
                        }
                    } else {
                        ui.add_enabled_ui(ready, |ui| {
                            if theme::primary_button(ui, "SAVE & ENABLE").clicked() {
                                save_current = true;
                            }
                        });
                    }
                },
            );

            if save_current {
                match self.save_japro_login_for_current_server() {
                    Ok(()) => {
                        self.japro_credential_error = None;
                        self.console_status =
                            format!("JAPRO AUTO-LOGIN SAVED FOR {server} (EXACT IP:PORT)");
                    }
                    Err(error) => self.japro_credential_error = Some(error),
                }
            }
            if forget_current {
                match self.forget_japro_login(server) {
                    Ok(()) => {
                        self.japro_credential_error = None;
                        self.console_status = format!("JAPRO AUTO-LOGIN FORGOTTEN FOR {server}");
                    }
                    Err(error) => self.japro_credential_error = Some(error),
                }
            }
        } else {
            theme::label(
                ui,
                theme::plain(
                    "Connect to a jaPRO server to save an automatic login for its exact IP:port.",
                    11.5,
                    theme::TEXT_FAINT,
                ),
            );
        }

        if let Some(error) = &self.japro_credential_error {
            theme::label(
                ui,
                theme::plain(
                    &format!("Credential store: {error}"),
                    11.5,
                    theme::TEXT_FAINT,
                ),
            );
        }

        if !self.japro_saved_logins.is_empty() {
            ui.add_space(10.0);
            let saved_logins = self.japro_saved_logins.clone();
            let mut forget = None;
            for entry in saved_logins {
                theme::row(
                    ui,
                    &entry.server.to_string(),
                    "Saved in Windows Credential Manager. The password is never displayed here.",
                    theme::Reset::None,
                    |ui| {
                        theme::glow_label(ui, &entry.username, 12.5, theme::TEXT);
                        ui.add_space(10.0);
                        if theme::ghost_button(ui, "FORGET").clicked() {
                            forget = Some(entry.server);
                        }
                    },
                );
            }
            if let Some(server) = forget {
                match self.forget_japro_login(server) {
                    Ok(()) => {
                        self.japro_credential_error = None;
                        self.console_status = format!("JAPRO AUTO-LOGIN FORGOTTEN FOR {server}");
                    }
                    Err(error) => self.japro_credential_error = Some(error),
                }
            }
        }
    }
}
