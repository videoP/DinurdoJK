//! Sun.
use crate::app::egui_settings::{
    color_row, segmented_row, theme, ui, App, EntityAmbientLightingMode,
};

impl App {
    // ---------------------------------------------------------- environment --

    pub(in crate::app::egui_settings) fn egui_sun(&mut self, ui: &mut egui::Ui) {
        // A held slider that isn't moving produces no change events, so keep the
        // sun-to-head beam alive for as long as the yaw/pitch drag continues.
        if self.sun_ray_until.is_some()
            && ui.ctx().dragged_id().is_some()
            && matches!(
                self.environment_selected,
                ui::ENV_ROW_SUN_YAW | ui::ENV_ROW_SUN_PITCH
            )
        {
            self.touch_sun_ray();
        }
        const SOURCES: [(bool, &str); 2] = [(false, "Map shader"), (true, "Custom")];
        if let Some(target) = segmented_row(
            ui,
            "Sun source",
            "Map shader uses the strongest sun/q3map_sun/q3map_sunExt referenced by the current sky. Custom replaces that runtime sun everywhere the renderer consumes it; baked lightmaps and the map-load voxel GI bake are unchanged.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_SOURCE),
            self.video.sun_override,
            &SOURCES,
        ) {
            self.environment_selected = ui::ENV_ROW_SUN_SOURCE;
            self.set_sun_override(target, target);
        }

        let authored = self.map_sun_editor_values();
        const VISIBILITY: [(ui::SunVisibilityMode, &str); 3] = [
            (ui::SunVisibilityMode::Legacy, "Legacy"),
            (ui::SunVisibilityMode::SkyPortals, "Sky portals"),
            (ui::SunVisibilityMode::Filtered, "Filtered / High"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Sun visibility",
            "Legacy treats q3 shader sun as an unconditional directional light. Sky portals requires an authored sky surface along the sun ray. Filtered / High also evaluates translucent world surfaces with stable alpha/opacity coverage; alpha-tested cutouts keep their exact texture mask.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_VISIBILITY),
            self.video.sun_visibility,
            &VISIBILITY,
        ) {
            self.environment_selected = ui::ENV_ROW_SUN_VISIBILITY;
            self.video.sun_visibility = target;
            self.sync_post_effects();
            self.mark_config_dirty();
        }

        const ENTITY_SUN: [(bool, &str); 2] = [(false, "Baked"), (true, "Runtime sun")];
        ui.add_enabled_ui(self.video.entity_ambient_lighting == EntityAmbientLightingMode::BspLightgridClassic, |ui| {
        if let Some(target) = segmented_row(
            ui,
            "Entity sun",
            "Baked keeps the stock lightgrid on players and models. Runtime sun estimates how much of each lightgrid probe is the map's baked sun, removes it, and lights entities with the runtime sun instead (color, intensity and direction, including the Custom sun above). Indoor and torch-lit probes are left alone. Requires Entity ambient lighting = BSP lightgrid.",
            theme::Reset::Environment(ui::ENV_ROW_ENTITY_SUN_LIGHTING),
            self.video.entity_sun_lighting,
            &ENTITY_SUN,
        ) {
            self.environment_selected = ui::ENV_ROW_ENTITY_SUN_LIGHTING;
            self.video.entity_sun_lighting = target;
            self.sync_post_effects();
            self.mark_config_dirty();
        }
        });

        if !self.video.sun_override {
            if let Some((yaw, pitch, intensity, color)) = authored {
                theme::banner(
                    ui,
                    &format!(
                        "MAP SUN  yaw {:.1}°   pitch {:.1}°   intensity {:.0}   rgb {:.2} {:.2} {:.2}",
                        yaw, pitch, intensity, color[0], color[1], color[2]
                    ),
                    theme::TEXT_FAINT,
                );
            } else {
                theme::banner(
                    ui,
                    "No referenced q3 shader sun on this map; renderer fallback sun is active.",
                    theme::TEXT_FAINT,
                );
            }
        }

        ui.add_enabled_ui(self.video.sun_override, |ui| {
            theme::row(
                ui,
                "Yaw / azimuth",
                "q3map sun azimuth in map space. 0° is east and 90° is north.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_YAW),
                |ui| {
                    let mut value = self.video.sun_yaw;
                    let readout = format!("{value:.1}°");
                    if theme::slider(ui, &mut value, 0.0..=360.0, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_YAW;
                        self.set_sun_yaw(value);
                    }
                },
            );
            theme::row(
                ui,
                "Pitch / elevation",
                "q3map sun elevation above the horizon. 0° is on the horizon; 90° is directly overhead.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_PITCH),
                |ui| {
                    let mut value = self.video.sun_pitch;
                    let readout = format!("{value:.1}°");
                    if theme::slider(ui, &mut value, -90.0..=90.0, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_PITCH;
                        self.set_sun_pitch(value);
                    }
                },
            );
            theme::row(
                ui,
                "Intensity",
                "Runtime q3 shader-sun intensity. This drives sun-aware effects but does not relight baked lightmaps.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_INTENSITY),
                |ui| {
                    let mut value = self.video.sun_intensity;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=ui::SUN_INTENSITY_MAX, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_INTENSITY;
                        self.set_sun_intensity(value);
                    }
                },
            );
            let mut color = self.video.sun_color;
            let mut changed = false;
            color_row(
                ui,
                "Color",
                "Sun chromaticity. RGB is normalized exactly like q3map_sun, so intensity remains the separate brightness control.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_COLOR),
                &mut color,
                &mut changed,
            );
            if changed {
                self.environment_selected = ui::ENV_ROW_SUN_COLOR;
                self.set_sun_color(color);
            }
        });
    }
}
