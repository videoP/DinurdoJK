//! Sky page: sky renderer, sun, baked lighting, moon and atmosphere.
use crate::app::egui_settings::{
    color_row, segmented_row, theme, ui, App, EntityAmbientLightingMode, VideoSettings,
};

/// Float settings count as changed once they differ by more than a slider's
/// display precision.
fn differs(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-4
}

impl App {
    /// How many settings on the Sky page differ from their defaults. The custom
    /// sun's yaw/pitch/intensity/colour only count while Custom is selected,
    /// since otherwise the map's own sun is in charge.
    pub(in crate::app) fn sun_page_changes(&self) -> usize {
        let v = &self.video;
        let d = VideoSettings::default();
        let s = &v.blue_water_environment;
        let e = crate::blue_water_environment::Settings::default();
        let mut flags = vec![
            v.sky_backend != d.sky_backend,
            v.sky_fractal_override != d.sky_fractal_override,
            v.sun_override != d.sun_override,
            v.sun_visibility != d.sun_visibility,
            v.entity_sun_lighting != d.entity_sun_lighting,
            v.baked_sun_shadow_map != d.baked_sun_shadow_map,
            differs(v.baked_sun_removal, d.baked_sun_removal),
            differs(v.baked_sun_edge_softness, d.baked_sun_edge_softness),
            differs(v.baked_sun_direct_limit, d.baked_sun_direct_limit),
            differs(v.baked_indirect_scale, d.baked_indirect_scale),
            differs(v.baked_indirect_saturation, d.baked_indirect_saturation),
            differs(v.baked_indirect_power, d.baked_indirect_power),
            differs(v.baked_indirect_highlight_preserve, d.baked_indirect_highlight_preserve),
            differs(s.world_lighting, e.world_lighting),
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
        v.sun_override = d.sun_override;
        v.sun_yaw = d.sun_yaw;
        v.sun_pitch = d.sun_pitch;
        v.sun_intensity = d.sun_intensity;
        v.sun_color = d.sun_color;
        v.sun_visibility = d.sun_visibility;
        v.entity_sun_lighting = d.entity_sun_lighting;
        v.baked_sun_removal = d.baked_sun_removal;
        v.baked_sun_shadow_map = d.baked_sun_shadow_map;
        v.baked_sun_edge_softness = d.baked_sun_edge_softness;
        v.baked_sun_direct_limit = d.baked_sun_direct_limit;
        v.baked_indirect_scale = d.baked_indirect_scale;
        v.baked_indirect_saturation = d.baked_indirect_saturation;
        v.baked_indirect_power = d.baked_indirect_power;
        v.baked_indirect_highlight_preserve = d.baked_indirect_highlight_preserve;
        let s = &mut v.blue_water_environment;
        s.world_lighting = e.world_lighting;
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
        self.sun_custom_initialized = false;
        self.baked_sun_debug_view = 0;
        self.sun_ray_until = None;
        self.sync_sun_ray();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// A baked-lighting slider. Double-click the slider or its label to restore
    /// `default`.
    #[allow(clippy::too_many_arguments)]
    fn baked_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        reset: theme::Reset,
        range: std::ops::RangeInclusive<f32>,
        default: f32,
        field: fn(&mut VideoSettings) -> &mut f32,
    ) -> bool {
        let mut changed = false;
        theme::row(ui, label, tip, reset, |ui| {
            let mut value = *field(&mut self.video);
            let readout = format!("{value:.2}");
            if theme::slider_default(ui, &mut value, range, &readout, default) {
                *field(&mut self.video) = value;
                self.sync_post_effects();
                self.mark_config_dirty();
                changed = true;
            }
        });
        changed
    }

    pub(in crate::app::egui_settings) fn egui_sun(&mut self, ui: &mut egui::Ui) {
        // A held slider that isn't moving produces no change events, so keep its
        // direction/diagnostic preview alive for as long as the drag continues.
        if self.sun_ray_until.is_some() && ui.ctx().dragged_id().is_some() {
            match self.environment_selected {
                ui::ENV_ROW_SUN_YAW | ui::ENV_ROW_SUN_PITCH => self.touch_sun_ray(),
                ui::ENV_ROW_BAKED_SUN_REMOVAL => self.touch_baked_sun_debug(),
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
        theme::row(ui, "Sky renderer", "The map's authored BSP sky, or the Blue Water atmosphere. Blue Water also unlocks sky-referenced auto exposure (Color page, on by default) and the World lighting slider below (off by default).", theme::Reset::None, |ui| {
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
        theme::section(ui, "SUN", "Map shader uses the map's sun as authored. Custom changes the runtime sun. r_sunLighting 1 also enables live world sunlight; baked-light removal below remains independent.");
        let authored = self.map_sun_editor_values();
        const SOURCES: [(bool, &str); 2] = [(false, "Map shader"), (true, "Custom")];
        if let Some(target) = segmented_row(
            ui,
            "Sun source",
            "Map shader uses the strongest sun/q3map_sun/q3map_sunExt referenced by the current sky. Custom changes the runtime sun used by sky and effects. BSP baked lightmaps stay as authored unless Subtract baked sunlight is enabled; that slider only subtracts an estimate of the map's old direct sun and never adds Custom light. Baked voxel GI remains unchanged.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_SOURCE),
            self.video.sun_override,
            &SOURCES,
        ) {
            self.environment_selected = ui::ENV_ROW_SUN_SOURCE;
            self.set_sun_override(target, target);
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
                "Runtime sun intensity for sky/effects and, with r_sunLighting 1, direct world sunlight. Baked-sun removal stays independent. Double-click to return to the map's sun.",
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

            const ENTITY_SUN: [(bool, &str); 2] = [(false, "Baked"), (true, "Runtime sun")];
            ui.add_enabled_ui(
                self.video.entity_ambient_lighting == EntityAmbientLightingMode::BspLightgridClassic,
                |ui| {
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
                },
            );
        }

        // ------------------------------------------- baked lighting --
        // This is a subtraction from authored baked light and is independent
        // of whether the runtime sun source is the map shader or Custom.
        let baked_sun_ready = authored.is_some();
        theme::section(
            ui,
            "BAKED LIGHTING",
            "Approximate removal of the map's baked direct sun. This control never adds a replacement light.",
        );
        if authored.is_none() {
            theme::banner(
                ui,
                "Inactive: this map has no authored sun to remove from its baked lighting.",
                theme::WARNING,
            );
        } else {
            theme::banner(
                ui,
                "When compiler settings are available, removal projects the authored sun into the lightmap and subtracts before exposure and filtering. Other maps use an estimated fallback.",
                theme::TEXT_DIM,
            );
            if let Some((yaw, pitch, intensity, color)) = authored {
                theme::banner(
                    ui,
                    &format!(
                        "MAP SUN  azimuth {yaw:.1}°  elevation {pitch:.1}°  intensity {intensity:.0}  RGB {:.2} {:.2} {:.2}  | live beam uses this direction and hue",
                        color[0], color[1], color[2]
                    ),
                    theme::TEXT_FAINT,
                );
            }
        }
        if authored.is_some() && !self.video.baked_sun_shadow_map {
            theme::banner(
                ui,
                "Legacy fallback ignores occlusion and can subtract inside baked shadows on uncalibrated surfaces. Choose Shadow map for occlusion in the fallback.",
                theme::WARNING,
            );
        }
        ui.add_enabled_ui(baked_sun_ready, |ui| {
            if self.baked_slider(
                ui,
                "Subtract baked sunlight",
                "0 leaves baked lighting unchanged. 1 subtracts the full direct-sun estimate. Known compiler settings use the authored sun strength, color, direction and shadow sampling in the original lightmap UVs. Baked edge samples refine the projected soft shadow. Other surfaces use a measured fit or the conservative fallback below. This never adds the Custom sun; its intensity and CSM setting do not affect removal.",
                theme::Reset::Environment(ui::ENV_ROW_BAKED_SUN_REMOVAL),
                0.0..=1.0,
                defaults.baked_sun_removal,
                |v| &mut v.baked_sun_removal,
            ) {
                self.environment_selected = ui::ENV_ROW_BAKED_SUN_REMOVAL;
                self.touch_baked_sun_debug();
            }
        });
        const BAKED_PREVIEW: [(u8, &str); 3] = [
            (0, "Auto on drag"),
            (1, "Removed amount"),
            (2, "Sun visibility"),
        ];
        ui.add_enabled_ui(baked_sun_ready, |ui| {
            if let Some(target) = segmented_row(
                ui,
                "Live preview",
                "Auto shows the removal heatmap while you drag the subtraction slider. Removed amount: black means none, bright red means more light removed. Sun visibility: green means the authored sun reaches the surface, red means blocked, blue means the map is not ready or its projection failed, amber means Legacy mode has no occlusion data. The colored beam shows the authored map-sun direction and hue.",
                theme::Reset::None,
                self.baked_sun_debug_view,
                &BAKED_PREVIEW,
            ) {
                self.baked_sun_debug_view = target;
                self.sun_ray_until = None;
                self.sync_sun_ray();
            }
        });
        const BAKED_VISIBILITY: [(bool, &str); 2] = [(false, "Legacy"), (true, "Shadow map")];
        if let Some(target) = segmented_row(
            ui,
            "Old sun visibility",
            "Controls fallback visibility on surfaces without a measured fit. Legacy uses surface facing only. Shadow map uses a cached BSP shadow map and sky-admission map from the authored q3map_sun direction. Paired fits follow the original lightmap edges. This does not require Dynamic shadows (CSM).",
            theme::Reset::None,
            self.video.baked_sun_shadow_map,
            &BAKED_VISIBILITY,
        ) {
            self.video.baked_sun_shadow_map = target;
            self.sync_post_effects();
            self.mark_config_dirty();
            self.touch_baked_sun_debug();
        }
        if self.baked_slider(
            ui,
            "Old shadow edge softness",
            "Fallback shadow-map filter radius in texels. Larger values soften fallback visibility but may blur detail. Paired surface fits retain the original lightmap edges. Default 2.5. Does not change gamma.",
            theme::Reset::None,
            0.0..=24.0,
            defaults.baked_sun_edge_softness,
            |v| &mut v.baked_sun_edge_softness,
        ) {
            self.touch_baked_sun_debug();
        }
        if self.baked_slider(
            ui,
            "Fallback baked-light fraction",
            "Safety cap for surfaces without a reliable sunlit/shadowed fit. 0.48 limits the fallback estimate to 48% of a lightmap sample. Paired surfaces use their measured shadow baseline and direct-light bound, limited to available RGB light. Default 0.48.",
            theme::Reset::None,
            0.0..=0.9,
            defaults.baked_sun_direct_limit,
            |v| &mut v.baked_sun_direct_limit,
        ) {
            self.touch_baked_sun_debug();
        }
        self.baked_slider(
            ui,
            "Baked indirect brightness",
            "Brightness multiplier for residual baked illumination only, separate from gamma and runtime sun. Default 1.0. Does not change gamma.",
            theme::Reset::None,
            0.0..=2.0,
            defaults.baked_indirect_scale,
            |v| &mut v.baked_indirect_scale,
        );
        self.baked_slider(
            ui,
            "Baked indirect saturation",
            "Saturation multiplier for residual baked illumination only, not textures or dynamic sunlight. Default 1.0. Does not change gamma.",
            theme::Reset::None,
            0.0..=2.0,
            defaults.baked_indirect_saturation,
            |v| &mut v.baked_indirect_saturation,
        );
        self.baked_slider(
            ui,
            "Baked indirect power",
            "Nonlinear night-mode darkening of remaining baked light: 1.0 unchanged, 2.0 squared, 3.0 cubed. Preserves RGB hue by shaping linear luminance, not individual channels. Try 2.5 with brightness 1.0.",
            theme::Reset::None,
            1.0..=4.0,
            defaults.baked_indirect_power,
            |v| &mut v.baked_indirect_power,
        );
        self.baked_slider(
            ui,
            "Baked highlight preservation",
            "0 darkens all baked light with the power curve. 1 preserves high-intensity baked light pools (including likely lamps). This is luminance-based, not actual spotlight separation.",
            theme::Reset::None,
            0.0..=1.0,
            defaults.baked_indirect_highlight_preserve,
            |v| &mut v.baked_indirect_highlight_preserve,
        );

        // ------------------------------------------- moon and atmosphere --
        if self.video.sky_backend == 1 {
            self.egui_native_moon(ui);
            self.egui_native_atmosphere(ui);
        }
    }
}
