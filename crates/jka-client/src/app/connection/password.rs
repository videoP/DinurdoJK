//! Connection password.
use crate::app::{App, ServerPasswordPrompt};

impl App {
    pub(in crate::app) fn show_server_password_prompt(
        &mut self,
        target: String,
        message: Option<String>,
    ) {
        // Stop a paused/rejected handshake while the dialog is open. Otherwise
        // CL_CheckForResend keeps sending the same bad connect packet and the
        // server keeps repeating the rejection in the console.
        if self.net.is_some() || self.game_session.is_some() {
            self.disconnect_to_main_menu();
        }
        self.server_password_prompt = Some(ServerPasswordPrompt { target, message });
        self.console_status = "SERVER PASSWORD REQUIRED".into();
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app) fn submit_server_password_prompt(&mut self) {
        let Some(prompt) = self.server_password_prompt.clone() else {
            return;
        };
        if self.network.password.is_empty() {
            if let Some(prompt) = self.server_password_prompt.as_mut() {
                prompt.message = Some("Enter the server password to continue.".to_owned());
            }
            self.egui_repaint_requested = true;
            return;
        }
        self.server_password_prompt = None;
        self.connect_to_server(&prompt.target);
    }

    pub(in crate::app) fn cancel_server_password_prompt(&mut self) {
        self.server_password_prompt = None;
        self.console_status = "CONNECTION CANCELLED".into();
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app) fn is_server_password_rejection(text: &str) -> bool {
        let lower = crate::logging::strip_jka_colors(text).to_ascii_lowercase();
        lower.contains("password")
            && [
                "invalid",
                "incorrect",
                "wrong",
                "bad",
                "required",
                "require",
                "need",
            ]
            .iter()
            .any(|word| lower.contains(word))
    }
}
