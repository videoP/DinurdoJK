//! Fog.
use crate::app::egui_settings::{segmented_row, theme, ui, App, FogMode};

impl App {
    pub(in crate::app::egui_settings) fn egui_fog(&mut self, ui: &mut egui::Ui) {
        const FOG: [(FogMode, &str); 4] = [
            (FogMode::Off, "Off"),
            (FogMode::LegacyDrawFog1, "Legacy 1"),
            (FogMode::LegacyDrawFog2, "Legacy 2"),
            (FogMode::Volumetric, "Volumetric"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Fog mode",
            "Legacy 1 mirrors OpenJK r_drawfog 1: redraw fog after all material stages. \
             Legacy 2 mirrors the JKA/OpenJK default r_drawfog 2: global fog is applied \
             during material stages while local brush fog uses a redraw. Volumetric \
             marches a froxel grid instead, so fog receives light and shows god rays.",
            theme::Reset::Environment(ui::ENV_ROW_FOG_MODE),
            self.video.fog_mode,
            &FOG,
        ) {
            self.environment_selected = ui::ENV_ROW_FOG_MODE;
            self.video.fog_mode = target;
            self.sync_post_effects();
            self.mark_config_dirty();
        }
        theme::row(
            ui,
            "Fog strength",
            "Multiplies the density the map authored. Leave at the map default \
             unless a map is unplayably thick.",
            theme::Reset::Environment(ui::ENV_ROW_FOG_STRENGTH),
            |ui| {
                let mut value = self.video.fog_strength;
                let readout = if self.video.fog_strength <= 0.001 {
                    "Map default".to_owned()
                } else {
                    format!("{:.2}×", self.video.fog_strength)
                };
                if theme::slider(ui, &mut value, 0.0..=ui::MAX_FOG_STRENGTH, &readout) {
                    self.set_fog_strength(value);
                }
            },
        );
    }
}
