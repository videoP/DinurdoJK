//! Independent stream privacy controls; Spout captures a distinct redacted
//! composition, never the local HUD/egui window.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app) fn egui_streamer_settings(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "STREAMER MODE", "Controls for a separate, privacy-filtered OBS picture. The game view and retained logs remain unchanged.");
        ui.add_space(12.0);
        let mut enabled = self.streamer.enabled;
        if ui.checkbox(&mut enabled, "Enable streamer OBS output").changed() {
            let _ = self.set_console_cvar("cl_streamerMode", if enabled { "1" } else { "0" });
        }
        ui.add_enabled_ui(self.streamer.enabled, |ui| {
            let mut hide_pm = self.streamer.hide_private_messages;
            if ui.checkbox(&mut hide_pm, "Hide private messages").changed() {
                let _ = self.set_console_cvar("cl_streamerHidePrivate", if hide_pm { "1" } else { "0" });
            }
            ui.small("Masks identified jaPRO PMs in chat, console scrollback, and console notifications on the streamer snapshot.");
            ui.add_space(7.0);
            let mut hide_team = self.streamer.hide_team_chat;
            if ui.checkbox(&mut hide_team, "Hide team chat").changed() {
                let _ = self.set_console_cvar("cl_streamerHideTeamChat", if hide_team { "1" } else { "0" });
            }
            ui.small("Masks team-chat messages (including location/team chat), console scrollback and notifications, and your team-chat input in the streamer snapshot.");
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
        ui.label("OBS (Windows): install the OBS Spout2 Capture plugin, add a Spout2 Capture source, and select 'DinurdoJK Stream'.");
        ui.small("The Spout output contains a separate, filtered HUD and chat. Your ordinary game window remains unfiltered. Do not use Game/Window Capture for privacy.");
        ui.small("DX12: D3D11On12 GPU sharing. Vulkan: imported D3D11 KMT GPU texture (requires driver external-memory support). Both use the filtered 2D overlay; 60 FPS SDR stream. Unsupported devices fail closed.");
    }
}
