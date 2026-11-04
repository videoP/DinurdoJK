//! Dialogs.
use crate::app::egui_menu::{theme, App};

impl App {
    pub(in crate::app::egui_menu) fn egui_demo_missing_map_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.demo_missing_map_prompt.clone() else {
            return;
        };
        #[derive(Clone, Copy)]
        enum Choice {
            Continue,
            Exit,
        }
        let mut choice = None;
        egui::Area::new(egui::Id::new("demo_missing_map_prompt"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL())
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE()))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        theme::page_title(
                            ui,
                            "DEMO MAP NOT FOUND",
                            "This demo was recorded on a map that is not installed locally.",
                        );
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&prompt.map_name).size(22.0).strong().color(theme::TEXT()));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(&prompt.reason).color(theme::TEXT_DIM()));
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(
                            "You can still play the recorded snapshots, but world geometry and map-dependent effects may be missing."
                        ).color(theme::TEXT()));
                        ui.add_space(12.0);
                        if theme::primary_button(ui, "CONTINUE ANYWAY").clicked() {
                            choice = Some(Choice::Continue);
                        }
                        ui.add_space(8.0);
                        if theme::ghost_button(ui, "EXIT").clicked() {
                            choice = Some(Choice::Exit);
                        }
                    });
            });
        match choice {
            Some(Choice::Continue) => self.choose_demo_missing_map_continue(),
            Some(Choice::Exit) => self.choose_demo_missing_map_exit(),
            None => {}
        }
    }

}
