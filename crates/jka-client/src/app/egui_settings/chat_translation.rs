//! Setup -> Interface -> TRANSLATION: `cg_chatTranslate*`.
use crate::app::egui_settings::{theme, App};
use crate::chat_translation::{lang, Codepage, Display, Mode, ProviderKind};

impl App {
    pub(in crate::app) fn egui_chat_translation_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "TRANSLATION",
            "Translate incoming chat. Right-click a chat line in the console to translate it, copy it, show the original, or always translate that player.",
        );
        let settings = self.chat_translation.settings.clone();
        theme::row(
            ui,
            "Translate chat",
            "cg_chatTranslate. Off sends nothing anywhere. Manual translates only messages you pick in the console. Automatic translates every message detected as another language.",
            theme::Reset::None,
            |ui| {
                for (mode, label) in [(Mode::Off, "Off"), (Mode::Manual, "Manual"), (Mode::Auto, "Automatic")] {
                    if theme::chip(ui, label, settings.mode == mode).clicked() && settings.mode != mode {
                        let _ = self.set_console_cvar("cg_chatTranslate", &mode.index().to_string());
                    }
                    ui.add_space(3.0);
                }
            },
        );
        if settings.mode == Mode::Off {
            return;
        }
        theme::banner(
            ui,
            &format!(
                "Message text (never names, server or account details) is sent to {}.",
                if settings.provider == ProviderKind::Libre { "your LibreTranslate server" } else { settings.provider.label() }
            ),
            theme::TEXT_DIM(),
        );
        let resolved = lang::resolve_target(&settings.target);
        theme::row(
            ui,
            "Translate into",
            "cg_chatTranslateTarget. System follows the Windows display language (English if it is not supported). The chat box shows Latin-alphabet translations; others (Russian, Greek, Chinese, Japanese, Korean) appear in the console, marked in the chat box.",
            theme::Reset::None,
            |ui| {
                let selected = if settings.target.is_empty() {
                    format!("System ({})", lang::name(resolved))
                } else {
                    lang::name(&settings.target).to_owned()
                };
                egui::ComboBox::from_id_salt("chat_translate_target")
                    .selected_text(selected)
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(settings.target.is_empty(), format!("System ({})", lang::name(resolved))).clicked() {
                            let _ = self.set_console_cvar("cg_chatTranslateTarget", "");
                        }
                        for language in lang::targets() {
                            if ui.selectable_label(settings.target == language.code, language.name).clicked() {
                                let _ = self.set_console_cvar("cg_chatTranslateTarget", language.code);
                            }
                        }
                    });
            },
        );
        theme::row(
            ui,
            "Show",
            "cg_chatTranslateDisplay. Translated replaces the message body and marks it [ru>en]; Both keeps the original with the translation under it; Original shows translations only when you ask in the console.",
            theme::Reset::None,
            |ui| {
                for (display, label) in [(Display::Translated, "Translated"), (Display::Both, "Both"), (Display::Original, "Original")] {
                    if theme::chip(ui, label, settings.display == display).clicked() && settings.display != display {
                        let _ = self.set_console_cvar("cg_chatTranslateDisplay", &display.index().to_string());
                    }
                    ui.add_space(3.0);
                }
            },
        );
        theme::row(
            ui,
            "Service",
            "cg_chatTranslateProvider. MyMemory is free without a key (limited characters per day). DeepL needs a free or paid API key. LibreTranslate is a server you run or choose.",
            theme::Reset::None,
            |ui| {
                for provider in ProviderKind::ALL {
                    if theme::chip(ui, provider.label(), settings.provider == provider).clicked() && settings.provider != provider {
                        let _ = self.set_console_cvar("cg_chatTranslateProvider", provider.name());
                    }
                    ui.add_space(3.0);
                }
            },
        );
        if settings.provider == ProviderKind::Libre {
            theme::row(
                ui,
                "Server address",
                "cg_chatTranslateUrl, e.g. http://localhost:5000. Press Enter to apply.",
                theme::Reset::None,
                |ui| {
                    let input = self.chat_translation.url_input.get_or_insert_with(|| settings.libre_url.clone());
                    let response = ui.add(
                        egui::TextEdit::singleline(input).hint_text("http://localhost:5000").desired_width(260.0),
                    );
                    if response.lost_focus() && input.trim() != settings.libre_url {
                        let value = input.trim().to_owned();
                        if let Err(error) = self.set_console_cvar("cg_chatTranslateUrl", &value) {
                            self.chat_translation.key_note = Some(error);
                        }
                    }
                },
            );
        }
        if settings.provider.secret_name().is_some() {
            let has_key = self.chat_translation_has_key();
            let tip = if settings.provider == ProviderKind::DeepL {
                "Your DeepL API key (free keys end in :fx). Stored in Windows Credential Manager, never in the cfg or console. Save with an empty field to remove it."
            } else {
                "Optional key for LibreTranslate servers that require one. Stored in Windows Credential Manager. Save with an empty field to remove it."
            };
            theme::row(ui, "API key", tip, theme::Reset::None, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.chat_translation.key_input)
                        .password(true)
                        .hint_text(if has_key { "saved - type to replace" } else { "paste key" })
                        .desired_width(200.0),
                );
                let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                if theme::ghost_button(ui, "SAVE").clicked() || enter {
                    self.save_chat_translation_key();
                }
            });
        }
        if let Some(note) = &self.chat_translation.key_note {
            theme::hint(ui, note, theme::TEXT_DIM());
        }
        theme::row(
            ui,
            "Team chat",
            "cg_chatTranslateTeam. Also translate team chat automatically. Off sends team chat only when you pick a message.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, settings.team) {
                    let _ = self.set_console_cvar("cg_chatTranslateTeam", if value { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Demo chat",
            "cg_chatTranslateDemos. Translate chat automatically during demo playback.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, settings.demos) {
                    let _ = self.set_console_cvar("cg_chatTranslateDemos", if value { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Old chat encoding",
            "cg_chatTranslateCodepage. How to read chat sent by clients that do not use UTF-8. Automatic recognises Russian (Windows-1251) and Western European (Windows-1252) text.",
            theme::Reset::None,
            |ui| {
                egui::ComboBox::from_id_salt("chat_translate_codepage")
                    .selected_text(settings.codepage.label())
                    .show_ui(ui, |ui| {
                        for codepage in Codepage::ALL {
                            if ui.selectable_label(settings.codepage == codepage, codepage.label()).clicked() {
                                let _ = self.set_console_cvar("cg_chatTranslateCodepage", codepage.name());
                            }
                        }
                    });
            },
        );
        theme::hint(ui, &self.chat_translation.status_line(), theme::TEXT_DIM());
    }
}
