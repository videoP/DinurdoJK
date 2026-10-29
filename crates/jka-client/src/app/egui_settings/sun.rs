//! Sky appearance and celestial positions. Map illumination lives in Relighting.
use crate::app::egui_settings::{
    color_row, segmented_row, theme, ui, App, VideoSection, VideoSettings,
};

/// Float settings count as changed once they differ by more than a slider's
/// display precision.
fn differs(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-4
}

impl App {
    /// How many settings on the Sky page differ from their defaults. The custom
    /// sun's yaw/pitch/intensity/colour only count while Relight map is enabled,
    /// since otherwise the map's own sun is in charge.
    pub(in crate::app) fn sun_page_changes(&self) -> usize {
        let v = &self.video;
        let d = VideoSettings::default();
        let s = &v.blue_water_environment;
        let e = crate::blue_water_environment::Settings::default();
        let mut flags = vec![
            v.sky_backend != d.sky_backend,
            v.sky_fractal_override != d.sky_fractal_override,
            v.sun_visibility != d.sun_visibility,
            s.moon_custom != e.moon_custom,
            differs(s.moon_phase, e.moon_phase),
            differs(s.moon_elevation, e.moon_elevation),
            differs(s.moon_azimuth, e.moon_azimuth),
            differs(s.rayleigh, e.rayleigh),
            differs(s.turbidity, e.turbidity),
            differs(s.mie, e.mie),
            differs(s.mie_g, e.mie_g),
            differs(s.multiple, e.multiple),
            differs(s.night_sky, e.night_sky),
        ];
        if v.sun_override {
            flags.extend([
                differs(v.sun_yaw, d.sun_yaw),
                differs(v.sun_pitch, d.sun_pitch),
                differs(v.sun_intensity, d.sun_intensity),
                v.sun_color != d.sun_color,
            ]);
        }
        flags.into_iter().filter(|changed| *changed).count()
    }

    /// Restores every setting the Sky page shows. The Blue Water environment
    /// struct also holds clouds and weather, so only the sky fields are copied back.
    pub(in crate::app) fn reset_sun_page(&mut self) {
        let d = VideoSettings::default();
        let e = crate::blue_water_environment::Settings::default();
        let v = &mut self.video;
        v.sky_backend = d.sky_backend;
        v.sky_fractal_override = d.sky_fractal_override;
        v.sun_yaw = d.sun_yaw;
        v.sun_pitch = d.sun_pitch;
        v.sun_intensity = d.sun_intensity;
        v.sun_color = d.sun_color;
        v.sun_visibility = d.sun_visibility;
        let s = &mut v.blue_water_environment;
        s.moon_phase = e.moon_phase;
        s.moon_custom = e.moon_custom;
        s.moon_elevation = e.moon_elevation;
        s.moon_azimuth = e.moon_azimuth;
        s.rayleigh = e.rayleigh;
        s.turbidity = e.turbidity;
        s.mie = e.mie;
        s.mie_g = e.mie_g;
        s.multiple = e.multiple;
        s.night_sky = e.night_sky;
        // Reset appearance without changing the Relight map master switch.
        self.sun_custom_initialized = true;
        self.baked_sun_debug_view = 0;
        self.sun_ray_until = None;
        self.sync_sun_ray();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app::egui_settings) fn egui_sun(&mut self, ui: &mut egui::Ui) {
        // A held slider that isn't moving produces no change events, so keep its
        // direction/diagnostic preview alive for as long as the drag continues.
        if self.sun_ray_until.is_some() && ui.ctx().dragged_id().is_some() {
            match self.environment_selected {
                ui::ENV_ROW_SUN_YAW | ui::ENV_ROW_SUN_PITCH => self.touch_sun_ray(),

                _ => {}
            }
        }
        let defaults = VideoSettings::default();
        let changed = self.sun_page_changes();
        theme::row(
            ui,
            "Sky settings",
            "These settings are saved and carry over to every map and every launch. Reset all returns this whole page to its shipped values. Double-click any slider or its label to reset just that one.",
            theme::Reset::None,
            |ui| {
                ui.add_enabled_ui(changed != 0, |ui| {
                    if theme::ghost_button(ui, "RESET ALL SKY SETTINGS").clicked() {
                        self.reset_sun_page();
                    }
                });
                ui.add_space(10.0);
                let (text, color) = if changed == 0 {
                    ("All at defaults".to_owned(), theme::TEXT_FAINT)
                } else {
                    (format!("{changed} changed from default"), theme::WARNING)
                };
                theme::glow_label(ui, &text, 12.5, color);
            },
        );

        // ------------------------------------------------------------ sky --
        theme::section(ui, "SKY", "Which sky is drawn behind the map.");
        theme::row(ui, "Sky renderer", "The map's authored BSP sky, or the Blue Water atmosphere. Blue Water also unlocks sky-referenced auto exposure (Color page, on by default) and sky world illumination on the Relighting page (off by default).", theme::Reset::None, |ui| {
            for (backend, name) in [(0, "JKA sky"), (1, "Blue Water sky")] {
                if theme::chip(ui, name, self.video.sky_backend == backend).clicked()
                    && self.video.sky_backend != backend
                {
                    self.video.sky_backend = backend;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                }
                ui.add_space(3.0);
            }
        });
        theme::row(
            ui,
            "Star Nest sky (experimental)",
            "Replaces BSP skybox and sky cloud stages with a procedural fractal nebula. Optional A/B test; real-world parallax, 12 x 10 shader loops. Does not replace volumetric clouds. Off by default.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.video.sky_fractal_override) {
                    self.video.sky_fractal_override = value;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                }
            },
        );

        // ------------------------------------------------------------ sun --
        theme::section(ui, "SUN", "Sun position and appearance. Relight map enables editing; live map illumination is configured on the Relighting page.");
        let authored = self.map_sun_editor_values();
        if theme::ghost_button(ui, "OPEN RELIGHTING").clicked() {
            self.video_section = VideoSection::Relighting;
        }
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

        if !self.video.sun_override {
            theme::hint(
                ui,
                "Enable Relight map on the Relighting page to edit the sun.",
                theme::TEXT_DIM,
            );
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
        } else {
            if theme::ghost_button(ui, "RESET SUN TO MAP").clicked() {
                let (yaw, pitch, intensity, color) = authored.unwrap_or((
                    defaults.sun_yaw,
                    defaults.sun_pitch,
                    defaults.sun_intensity,
                    defaults.sun_color,
                ));
                self.video.sun_yaw = yaw;
                self.video.sun_pitch = pitch;
                self.video.sun_intensity = intensity;
                self.video.sun_color = color;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            theme::row(
                ui,
                "Yaw / azimuth",
                "q3map sun azimuth in map space. 0° is east and 90° is north. Double-click to return to the map's sun.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_YAW),
                |ui| {
                    let mut value = self.video.sun_yaw;
                    let readout = format!("{value:.1}°");
                    let default = authored.map_or(defaults.sun_yaw, |v| v.0);
                    if theme::slider_default(ui, &mut value, 0.0..=360.0, &readout, default) {
                        self.environment_selected = ui::ENV_ROW_SUN_YAW;
                        self.set_sun_yaw(value);
                    }
                },
            );
            theme::row(
                ui,
                "Pitch / elevation",
                "q3map sun elevation above the horizon. 0° is on the horizon; 90° is directly overhead. Double-click to return to the map's sun.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_PITCH),
                |ui| {
                    let mut value = self.video.sun_pitch;
                    let readout = format!("{value:.1}°");
                    let default = authored.map_or(defaults.sun_pitch, |v| v.1);
                    if theme::slider_default(ui, &mut value, -90.0..=90.0, &readout, default) {
                        self.environment_selected = ui::ENV_ROW_SUN_PITCH;
                        self.set_sun_pitch(value);
                    }
                },
            );
            theme::row(
                ui,
                "Intensity",
                "Sun intensity for sky and effects. Enable Add live world sunlight on Relighting to illuminate map surfaces. Double-click to return to the map's sun.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_INTENSITY),
                |ui| {
                    let mut value = self.video.sun_intensity;
                    let readout = format!("{value:.0}");
                    let default = authored.map_or(defaults.sun_intensity, |v| v.2);
                    if theme::slider_default(ui, &mut value, 0.0..=ui::SUN_INTENSITY_MAX, &readout, default) {
                        self.environment_selected = ui::ENV_ROW_SUN_INTENSITY;
                        self.set_sun_intensity(value);
                    }
                },
            );
            let mut color = self.video.sun_color;
            let mut color_changed = false;
            color_row(
                ui,
                "Color",
                "Sun chromaticity. RGB is normalized exactly like q3map_sun, so intensity remains the separate brightness control.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_COLOR),
                &mut color,
                &mut color_changed,
            );
            if color_changed {
                self.environment_selected = ui::ENV_ROW_SUN_COLOR;
                self.set_sun_color(color);
            }
        }

        // ------------------------------------------- moon and atmosphere --
        if self.video.sky_backend == 1 {
            self.egui_native_moon(ui);
            self.egui_native_atmosphere(ui);
        }
    }
}
