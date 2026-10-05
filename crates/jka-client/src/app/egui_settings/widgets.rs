//! Widgets.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app) fn physics_menu_changed(&mut self) {
        self.mark_config_dirty();
        self.ensure_map_physics_mesh();
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    // --------------------------------------------------------------- rows --

    pub(in crate::app) fn set_world_lighting_quality(&mut self, target: usize) {
        match target.min(2) {
            0 => {
                // Preserve vertex_lighting while disabled so r_fullbright can
                // be used as a true A/B master without forgetting the chosen
                // enabled path.
                self.video.world_lighting = false;
            }
            1 => {
                self.video.world_lighting = true;
                self.video.vertex_lighting = true;
            }
            _ => {
                self.video.world_lighting = true;
                self.video.vertex_lighting = false;
            }
        }
        self.sync_classic_world_lighting();
        self.mark_config_dirty();
    }

    pub(in crate::app::egui_settings) fn egui_toggle_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        row: usize,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Video(row), |ui| {
            if theme::switch(ui, current).is_some() {
                self.video_selected = row;
                self.change_video_setting(1);
            }
        });
    }

    pub(in crate::app::egui_settings) fn egui_environment_toggle(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        row: usize,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Environment(row), |ui| {
            if theme::switch(ui, current).is_some() {
                self.environment_selected = row;
                self.change_environment_setting(1);
            }
        });
    }
}
