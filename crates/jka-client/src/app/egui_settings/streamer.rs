//! Independent stream privacy controls; a stream presenter must explicitly
//! request the redacted snapshot. Never alter the local HUD by default.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app) fn egui_streamer_settings(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "STREAMER MODE", "Controls for a separate, privacy-filtered OBS picture. The game view and retained logs remain unchanged.");
        ui.add_space(12.0);
        let mut enabled = self.streamer.enabled;
        if ui.checkbox(&mut enabled, "Enable streamer filtering").changed() {
            let _ = self.set_console_cvar("cl_streamerMode", if enabled { "1" } else { "0" });
        }
        ui.add_enabled_ui(self.streamer.enabled, |ui| {
            let mut hide_pm = self.streamer.hide_private_messages;
            if ui.checkbox(&mut hide_pm, "Hide private messages").changed() {
                let _ = self.set_console_cvar("cl_streamerHidePrivate", if hide_pm { "1" } else { "0" });
            }
            ui.small("Masks identified jaPRO PMs in chat, console scrollback, and console notifications on the streamer snapshot.");
            ui.add_space(7.0);
            let mut hide_console = self.streamer.hide_console;
            if ui.checkbox(&mut hide_console, "Hide console text").changed() {
                let _ = self.set_console_cvar("cl_streamerHideConsole", if hide_console { "1" } else { "0" });
            }
            ui.small("Masks console output, commands, suggestions, search, and status, including the translucent notify lines.");
        });
        ui.separator();
        let mut preview = self.streamer_preview_local;
        if ui.checkbox(&mut preview, "Local redaction test (temporary preview)").changed() {
            self.streamer_preview_local = preview;
            self.publish_ui();
        }
        ui.small("Diagnostic only: previews the filtered UI on your own screen. Turn this OFF for normal local gameplay. It is not an OBS output.");
        ui.add_space(12.0);
        ui.colored_label(theme::WARNING, "OBS-only output is not connected yet.");
        ui.label("These settings prepare an independent, redacted UI snapshot. Do not capture the ordinary game window for privacy: the unfiltered local view is deliberately unchanged.");
    }
}
