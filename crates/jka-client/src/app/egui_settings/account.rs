//! Saved per-server accounts.
use crate::app::egui_settings::{theme, App};
use crate::credential_store::AccountType;

impl App {
    /// Secure, exact-endpoint credentials. Server (join) passwords are applied
    /// automatically at connect; Login, Admin and RCON remain explicit actions.
    pub(in crate::app) fn egui_account(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "ACCOUNT",
            "Server passwords are used automatically on join for this exact IP:port. Login (/login), Admin (/amlogin), and RCON remain manual.",
        );

        if !crate::credential_store::supported() {
            theme::label(
                ui,
                theme::plain(
                    "Secure saved accounts are unavailable on this platform.",
                    11.5,
                    theme::TEXT_FAINT(),
                ),
            );
            return;
        }

        let current = self.current_account_server_endpoint();
        let current_accounts: Vec<_> = self
            .saved_accounts
            .iter()
            .filter(|entry| Some(entry.server) == current)
            .cloned()
            .collect();
        let other_accounts: Vec<_> = self
            .saved_accounts
            .iter()
            .filter(|entry| Some(entry.server) != current)
            .cloned()
            .collect();

        let mut login = None;
        let mut logout = None;
        let mut edit = None;
        let mut forget = None;
        let mut new_account = None;

        if let Some(server) = current {
            theme::row(
                ui,
                "Current server",
                "The resolved UDP IP:port is the credential identity. The friendly name is display-only and never redirects a credential.",
                theme::Reset::None,
                |ui| {
                    let name = self
                        .current_server_hostname()
                        .map(|name| crate::logging::strip_jka_colors(&name).trim().to_owned())
                        .filter(|name| !name.is_empty());
                    let display = name
                        .map(|name| format!("{name}  ·  {server}"))
                        .unwrap_or_else(|| server.to_string());
                    theme::glow_label(ui, &display, 12.5, theme::TEXT());
                },
            );

            if current_accounts.is_empty() {
                theme::label(
                    ui,
                    theme::plain(
                        "No saved accounts for this server yet.",
                        11.5,
                        theme::TEXT_FAINT(),
                    ),
                );
            } else {
                for entry in &current_accounts {
                    let account_type = entry.account_type;
                    let detail = match account_type {
                        AccountType::Login => format!("{}  ·  {}", entry.username, entry.server),
                        AccountType::Admin => format!("Admin password  ·  {}", entry.server),
                        AccountType::Rcon => format!("RCON password  ·  {}", entry.server),
                        AccountType::Server => format!("Server password  ·  {}", entry.server),
                    };
                    theme::row(
                        ui,
                        account_type.label(),
                        "Passwords stay in Windows Credential Manager and are never displayed here.",
                        theme::Reset::None,
                        |ui| {
                            theme::glow_label(ui, &detail, 12.5, theme::TEXT());
                            ui.add_space(10.0);
                            if account_type != AccountType::Server {
                                if theme::primary_button(ui, "LOG IN").clicked() {
                                    login = Some((entry.server, account_type));
                                }
                                ui.add_space(6.0);
                                if theme::ghost_button(ui, "LOG OUT").clicked() {
                                    logout = Some((entry.server, account_type));
                                }
                                ui.add_space(6.0);
                            }
                            if theme::ghost_button(ui, "EDIT").clicked() {
                                edit = Some((entry.server, account_type));
                            }
                            ui.add_space(6.0);
                            if theme::ghost_button(ui, "FORGET").clicked() {
                                forget = Some((entry.server, account_type));
                            }
                        },
                    );
                }
            }

            if current_accounts.len() < AccountType::ALL.len() {
                theme::row(ui, "", "", theme::Reset::None, |ui| {
                    if theme::primary_button(ui, "ADD ACCOUNT").clicked() {
                        new_account = Some(server);
                    }
                });
            }
        } else {
            theme::label(
                ui,
                theme::plain(
                    "Join a server to add or use accounts for its exact IP:port.",
                    11.5,
                    theme::TEXT_FAINT(),
                ),
            );
        }

        if !other_accounts.is_empty() {
            ui.add_space(8.0);
            theme::row(
                ui,
                "Show other servers",
                "Off by default. Other endpoints are shown below for management only and cannot log in until you join that exact server.",
                theme::Reset::None,
                |ui| {
                    if let Some(show) = theme::switch(ui, self.show_other_accounts) {
                        self.show_other_accounts = show;
                    }
                },
            );

            if self.show_other_accounts {
                for entry in &other_accounts {
                    let account_type = entry.account_type;
                    let server_label = entry
                        .server_name
                        .as_deref()
                        .filter(|name| !name.is_empty())
                        .map(|name| format!("{name}  ·  {}", entry.server))
                        .unwrap_or_else(|| entry.server.to_string());
                    let detail = match account_type {
                        AccountType::Login => {
                            format!("{}  ·  {}", account_type.label(), entry.username)
                        }
                        AccountType::Admin | AccountType::Rcon | AccountType::Server => {
                            account_type.label().to_owned()
                        }
                    };
                    theme::row(
                        ui,
                        &server_label,
                        "Saved for another endpoint. Join that exact server before LOG IN / LOG OUT can be used.",
                        theme::Reset::None,
                        |ui| {
                            theme::glow_label(ui, &detail, 12.0, theme::TEXT_FAINT());
                            ui.add_space(10.0);
                            if account_type != AccountType::Server {
                                ui.add_enabled_ui(false, |ui| {
                                    let _ = theme::ghost_button(ui, "LOG IN");
                                    ui.add_space(6.0);
                                    let _ = theme::ghost_button(ui, "LOG OUT");
                                });
                                ui.add_space(6.0);
                            }
                            if theme::ghost_button(ui, "EDIT").clicked() {
                                edit = Some((entry.server, account_type));
                            }
                            ui.add_space(6.0);
                            if theme::ghost_button(ui, "FORGET").clicked() {
                                forget = Some((entry.server, account_type));
                            }
                        },
                    );
                }
            }
        }

        if let Some((server, account_type)) = login {
            if let Err(error) = self.login_with_saved_account(server, account_type) {
                self.account_credential_error = Some(error);
            } else {
                self.account_credential_error = None;
            }
        }
        if let Some((server, account_type)) = logout {
            if let Err(error) = self.logout_saved_account(server, account_type) {
                self.account_credential_error = Some(error);
            } else {
                self.account_credential_error = None;
            }
        }
        if let Some(server) = new_account {
            self.begin_new_account(server);
        } else if let Some((server, account_type)) = edit {
            self.begin_edit_account(server, account_type);
        }
        if let Some((server, account_type)) = forget {
            match self.forget_saved_account(server, account_type) {
                Ok(()) => {
                    self.account_credential_error = None;
                    self.console_status = format!(
                        "{} ACCOUNT FORGOTTEN FOR {server}",
                        account_type.label().to_ascii_uppercase()
                    );
                }
                Err(error) => self.account_credential_error = Some(error),
            }
        }

        // Work on a clone so rendering the editor never holds a mutable borrow
        // of an App field while a save/cancel action needs &mut self.
        let mut editor = self.account_editor.clone();
        let mut save_editor = None;
        let mut cancel_editor = false;
        if let Some(state) = editor.as_mut() {
            const USERNAME_LIMIT: usize = 64;
            const PASSWORD_LIMIT: usize = 128;
            let new_entry = state.original_type.is_none();

            ui.add_space(14.0);
            theme::section(
                ui,
                if new_entry {
                    "ADD ACCOUNT"
                } else {
                    "EDIT ACCOUNT"
                },
                if new_entry {
                    "Choose how this credential is used, then save it for this exact endpoint."
                } else {
                    "Change the display name, type or username. Leave Password blank to keep the existing saved password."
                },
            );
            theme::row(
                ui,
                "Endpoint",
                "Security identity. This cannot be edited; add an account while connected to another endpoint to save one there.",
                theme::Reset::None,
                |ui| {
                    theme::glow_label(ui, &state.server.to_string(), 12.5, theme::TEXT());
                },
            );
            theme::row(
                ui,
                "Server name",
                "Optional local display name. It is never used to choose where credentials are sent.",
                theme::Reset::None,
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.server_name)
                            .char_limit(96)
                            .desired_width(260.0)
                            .hint_text("optional friendly name"),
                    );
                },
            );
            theme::row(
                ui,
                "Type",
                "Login sends /login <user> <password>. Admin sends /amlogin <password>. RCON sets rconPassword locally for rcon commands. Server is the join password (g_password) for private servers and is filled in automatically when connecting.",
                theme::Reset::None,
                |ui| {
                    egui::ComboBox::from_id_salt(("saved_account_type", state.server))
                        .selected_text(state.account_type.label())
                        .show_ui(ui, |ui| {
                            for account_type in AccountType::ALL.iter().copied() {
                                let occupied = self.saved_accounts.iter().any(|entry| {
                                    entry.server == state.server
                                        && entry.account_type == account_type
                                        && state.original_type != Some(account_type)
                                });
                                ui.add_enabled_ui(!occupied, |ui| {
                                    ui.selectable_value(
                                        &mut state.account_type,
                                        account_type,
                                        account_type.label(),
                                    );
                                });
                            }
                        });
                },
            );

            if state.account_type == AccountType::Login {
                theme::row(
                    ui,
                    "Username",
                    "Username passed to the server's /login command.",
                    theme::Reset::None,
                    |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.username)
                                .char_limit(USERNAME_LIMIT)
                                .desired_width(220.0),
                        );
                    },
                );
                state
                    .username
                    .retain(|c| !c.is_whitespace() && !matches!(c, ';' | '"' | '\\'));
            }

            let password_tip = match state.account_type {
                AccountType::Login => {
                    "Password passed to /login. Stored in Windows Credential Manager, never DinurdoJK.cfg."
                }
                AccountType::Admin => {
                    "Admin password passed to /amlogin. Stored in Windows Credential Manager."
                }
                AccountType::Rcon => {
                    "RCON password loaded into the temporary rconPassword cvar when LOG IN is pressed."
                }
                AccountType::Server => {
                    "Server join password (g_password). Used automatically when this server asks for one."
                }
            };
            theme::row(
                ui,
                "Password",
                if new_entry {
                    password_tip
                } else {
                    "Enter a new password, or leave blank to keep the existing saved password."
                },
                theme::Reset::None,
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.password)
                            .password(true)
                            .char_limit(PASSWORD_LIMIT)
                            .desired_width(220.0)
                            .hint_text(if new_entry {
                                "password"
                            } else {
                                "keep existing"
                            }),
                    );
                },
            );
            match state.account_type {
                AccountType::Login | AccountType::Admin => state
                    .password
                    .retain(|c| !c.is_whitespace() && !matches!(c, ';' | '"' | '\\')),
                AccountType::Rcon | AccountType::Server => {
                    state.password.retain(|c| !c.is_control())
                }
            }

            theme::row(ui, "", "", theme::Reset::None, |ui| {
                let username_ready = state.account_type != AccountType::Login
                    || !state.username.is_empty();
                let password_ready = !new_entry || !state.password.is_empty();
                ui.add_enabled_ui(username_ready && password_ready, |ui| {
                    if theme::primary_button(ui, "SAVE").clicked() {
                        save_editor = Some(state.clone());
                    }
                });
                ui.add_space(8.0);
                if theme::ghost_button(ui, "CANCEL").clicked() {
                    cancel_editor = true;
                }
            });
        }
        self.account_editor = editor;

        if let Some(state) = save_editor {
            match self.save_account_edit(
                state.server,
                &state.server_name,
                state.account_type,
                state.original_type,
                &state.username,
                &state.password,
            ) {
                Ok(()) => {
                    self.account_editor = None;
                    self.account_credential_error = None;
                    self.console_status = if state.original_type.is_none() {
                        format!(
                            "{} ACCOUNT SAVED FOR {}",
                            state.account_type.label().to_ascii_uppercase(),
                            state.server
                        )
                    } else {
                        format!(
                            "{} ACCOUNT UPDATED FOR {}",
                            state.account_type.label().to_ascii_uppercase(),
                            state.server
                        )
                    };
                }
                Err(error) => self.account_credential_error = Some(error),
            }
        } else if cancel_editor {
            self.account_editor = None;
            self.account_credential_error = None;
        }

        if let Some(error) = &self.account_credential_error {
            theme::label(
                ui,
                theme::plain(
                    &format!("Credential store: {error}"),
                    11.5,
                    theme::TEXT_FAINT(),
                ),
            );
        }
    }
}
