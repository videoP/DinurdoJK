//! Runtime overrides of authored map illumination.
use crate::app::egui_settings::{
    segmented_row, theme, ui, App, DynamicShadowsMode, VideoSection, VideoSettings,
};

impl App {
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

    pub(in crate::app::egui_settings) fn egui_relighting(&mut self, ui: &mut egui::Ui) {
        let defaults = VideoSettings::default();
        const MODES: [(u8, &str); 3] = [(0, "Off"), (1, "Baked"), (2, "Realtime")];
        if let Some(mode) = segmented_row(ui, "Relighting",
            "Off preserves authored illumination. Baked uses cached sun/sky lightmaps (and Blue Water sky caching). Realtime uses Blue Water live sky lighting or vanilla change-triggered lightmaps with live CSM shadows. Saved settings are retained when switching modes.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_SOURCE), self.video.active_relighting_mode(), &MODES)
        {
            self.set_relighting_mode(mode);
        }
        let baked_mode = self.video.active_relighting_mode() == 1;
        if self.video.active_relighting_mode() == 2 {
            theme::hint(ui, "Realtime uses Blue Water's live sky pass or vanilla's change-triggered lightmap lighting with live world CSM. Cascaded Shadow Maps are required for world shadows.", theme::TEXT_DIM());
        }
        ui.add_enabled_ui(self.video.sun_override, |ui| {
            theme::section(ui, "TIME OF DAY", "Artistic 24-hour cycle; sunrise 6 AM, noon 12 PM, sunset 6 PM. Sun and moon positions update together; lunar phase and original map lighting stay independently adjustable.");
            theme::row(ui, "Time", "3 AM to the following 3 AM. Match system time uses the PC's local clock; Realtime follows it, Baked captures it at map load. Manual slider and shadow/sky selection are otherwise unchanged.", theme::Reset::None, |ui| {
                let matching = self.video.relight_match_system_time;
                let mut time = if matching {
                    self.relight_clock_effective.unwrap_or(self.video.relight_time)
                } else {
                    self.video.relight_time
                };
                let minutes = ((time.rem_euclid(24.0) * 60.0).round() as u32) % (24 * 60);
                let hours = minutes / 60;
                let hh = hours % 12;
                let readout = format!("{}:{:02} {}", if hh == 0 { 12 } else { hh }, minutes % 60, if hours < 12 { "AM" } else { "PM" });
                ui.horizontal(|ui| {
                    // Reserve room for the adjacent toggle: the theme's slider
                    // uses its available width to size the Taffy track.
                    let slider_width = (ui.available_width() - 150.0).max(210.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(slider_width, 24.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add_enabled_ui(!matching, |ui| {
                                if theme::slider_default(ui, &mut time, 3.0..=27.0, &readout, 15.0) {
                                    self.set_relighting_time((time * 4.0).round() * 0.25);
                                }
                            });
                        },
                    );
                    if theme::chip(ui, "MATCH SYSTEM TIME", matching).clicked() {
                        self.set_relighting_match_system_time(!matching);
                    }
                });
            });
            if self.video.relight_match_system_time && self.video.active_relighting_mode() == 1 {
                let hint = if self.relight_clock_effective.is_some() {
                    "Baked: system time was captured at map load and stays fixed until the next map load."
                } else {
                    "Baked: the system clock will first be sampled at the next map load. The displayed time is your saved manual value."
                };
                theme::hint(ui, hint, theme::TEXT_DIM());
            }
            if self.video.sky_backend != 1 || self.video.fast_sky != 0 {
                theme::hint(ui, "Blue Water Sky gives the clock a matching sunrise, sunset and night sky; other sky modes retain their own appearance.", theme::TEXT_DIM());
            }
        });
        if theme::ghost_button(ui, "OPEN SKY / SUN & MOON").clicked() {
            self.video_section = VideoSection::Sun;
        }
        if theme::ghost_button(ui, "RESET ALL RELIGHTING SETTINGS").clicked() {
            self.reset_relighting_page();
        }
        if !self.video.sun_override {
            theme::banner(ui, "Relighting is off. Your settings are retained; the map uses its authored lighting. Lamp shadows remain available in Lighting.", theme::TEXT_DIM());
        }
        ui.add_enabled_ui(self.video.sun_override, |ui| {
            let authored = self.map_sun_editor_values();
            theme::section(ui, "SUN REPLACEMENT", "Sun direction, color and intensity are edited on Sky. Subtraction removes the old baked sun; live sunlight adds the current sun separately.");
            ui.add_enabled_ui(baked_mode, |ui| {
                theme::row(ui, "Add world sunlight", "Add the current sun to world surfaces. Moving casters and optional nearby world shadows use Cascaded Shadow Maps. Subtract baked sunlight below to reduce duplicate sunlight. Realtime mode supplies its own sun/moon light.", theme::Reset::None, |ui| {
                    if let Some(enabled) = theme::switch(ui, self.video.sun_lighting) {
                        let _ = self.set_console_cvar("r_sunLighting", if enabled { "1" } else { "0" });
                    }
                });
            });
            let entity_csm = self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMapsEntities;
            ui.add_enabled_ui(entity_csm && baked_mode, |ui| {
                theme::row(
                    ui,
                    "Realtime nearby world shadows",
                    "Use realtime cascaded shadow maps for nearby map geometry, blending into baked shadows at the shadow-distance limit. Adds ongoing GPU cost. Realtime relighting always includes world casters when CSM is selected.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.video.static_world_csm) {
                            let _ = self.set_console_cvar("r_staticWorldCsm", if enabled { "1" } else { "0" });
                        }
                    },
                );
            });
            if !entity_csm {
                ui.small("Enable Cascaded Shadow Maps in Lighting to use static-world CSM.");
            }
            theme::hint(ui, "Moving models follow the current sky in Baked and Realtime, retaining room lighting from your selected Entity ambient lighting source. Sun/moon shadows use the selected shadow method.", theme::TEXT_DIM());
            // ------------------------------------------- baked lighting --
            // This is a subtraction from authored baked light and is independent
            // of the edited runtime sun direction and intensity.
            let detected = self.baked_sun_direction_status;
            let baked_sun_ready = authored.is_some()
                || matches!(detected, crate::renderer::BakedSunDirectionStatus::Found { .. });
            theme::section(
                ui,
                "BAKED LIGHTING",
                "Approximate removal of the map's baked direct sun. This control never adds a replacement light.",
            );
            if authored.is_none() && !baked_sun_ready {
                theme::banner(
                    ui,
                    "No q3map_sun in the current shaders. Detect the original baked sun from lightmap shadow edges below.",
                    theme::WARNING(),
                );
            } else if authored.is_some() {
                theme::banner(
                    ui,
                    "When compiler settings are available, removal projects the authored sun into the lightmap and subtracts before exposure and filtering. Other maps use an estimated fallback.",
                    theme::TEXT_DIM(),
                );
                if let Some((yaw, pitch, intensity, color)) = authored {
                    theme::banner(
                        ui,
                        &format!(
                            "MAP SUN  azimuth {yaw:.1}°  elevation {pitch:.1}°  intensity {intensity:.0}  RGB {:.2} {:.2} {:.2}  | live beam uses this direction and hue",
                            color[0], color[1], color[2]
                        ),
                        theme::TEXT_FAINT(),
                    );
                }
            }
            theme::row(ui, "Find baked sun direction",
                "One-time low-resolution CPU search of BSP shadow edges across azimuth/elevation, followed by coarse-to-fine refinement. Does not change the current sky sun. Reconstructs removal masks in the background; preserves the original map if the result is uncertain.",
                theme::Reset::None, |ui| {
                    ui.add_enabled_ui(!matches!(detected,
                        crate::renderer::BakedSunDirectionStatus::Searching |
                        crate::renderer::BakedSunDirectionStatus::Unavailable), |ui| {
                        if theme::ghost_button(ui, "FIND ORIGINAL SUN").clicked() {
                            self.render_command(crate::renderer::RenderCommand::FindBakedSunDirection);
                            self.video.baked_sun_removal_auto = true;
                            self.video.baked_sun_removal_contour = false;
                            self.sync_post_effects();
                            self.mark_config_dirty();
                        }
                    });
                });
            // The two sources are independent of the live sun/sky settings.
            // Keep both choices visible after detection so an expensive search
            // need not be repeated just to A/B against the authored direction.
            let found = matches!(detected, crate::renderer::BakedSunDirectionStatus::Found { .. });
            let using_detected = matches!(detected,
                crate::renderer::BakedSunDirectionStatus::Found { using_detected: true, .. });
            if authored.is_some() && (found || matches!(detected, crate::renderer::BakedSunDirectionStatus::Searching)) {
                theme::row(ui, "Baked sun source",
                    "Select the direction used to remove original baked sunlight. MAP SUN restores the actual shader-authored direction and recalibrates; DETECTED reuses the last matching result. Neither changes the live sky sun.",
                    theme::Reset::None, |ui| {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(!using_detected && !matches!(detected, crate::renderer::BakedSunDirectionStatus::Searching), "Map sun (authored)").clicked()
                                && (using_detected || matches!(detected, crate::renderer::BakedSunDirectionStatus::Searching)) {
                                self.render_command(crate::renderer::RenderCommand::SelectBakedSunDirection { detected: false });
                            }
                            ui.add_enabled_ui(found, |ui| {
                                if ui.selectable_label(using_detected, "Detected").clicked() && !using_detected {
                                    self.render_command(crate::renderer::RenderCommand::SelectBakedSunDirection { detected: true });
                                }
                            });
                        });
                    });
            }
            match detected {
                crate::renderer::BakedSunDirectionStatus::Searching => {
                    theme::hint(ui, "Comparing low-resolution sun shadow candidates (background CPU)...", theme::TEXT_DIM());
                }
                crate::renderer::BakedSunDirectionStatus::Found { azimuth, elevation, confidence, using_detected } => {
                    theme::banner(ui, &format!("BAKED SUN FOUND  azimuth {azimuth:.1}°  elevation {elevation:.1}°  edge fit {confidence:.2}  | using {} direction for removal", if using_detected { "detected" } else { "authored map" }), theme::TEXT_DIM());
                }
                crate::renderer::BakedSunDirectionStatus::Failed => {
                    theme::banner(ui, "Unable to distinguish a reliable original sun; see console for evidence. Original lightmap preserved. You can retry.", theme::WARNING());
                }
                _ => {}
            }
            if baked_sun_ready && !self.video.baked_sun_shadow_map {
                theme::banner(
                    ui,
                    "Legacy fallback ignores occlusion and can subtract inside baked shadows on uncalibrated surfaces. Choose Shadow map for occlusion in the fallback.",
                    theme::WARNING(),
                );
            }
            ui.add_enabled_ui(baked_sun_ready, |ui| {
                theme::row(ui, "Automatic sunlight removal",
                    "Auto uses a single robust sun/shadow edge fit. Contour Guided fits local strengths along shadow contours and conservatively estimates nearby bounce gradients; corrections blend smoothly on the same BSP lightmap chart. Both modes are background-baked and cached.",
                    theme::Reset::None, |ui| {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(self.video.baked_sun_removal_auto && !self.video.baked_sun_removal_contour, "Auto").clicked() {
                                let was_auto = self.video.baked_sun_removal_auto && !self.video.baked_sun_removal_contour;
                                self.video.baked_sun_removal_auto = !was_auto;
                                self.video.baked_sun_removal_contour = false;
                                self.sync_post_effects();
                                self.mark_config_dirty();
                            }
                            if ui.selectable_label(self.video.baked_sun_removal_contour, "Auto (Contour Guided)").clicked() {
                                self.video.baked_sun_removal_contour = !self.video.baked_sun_removal_contour;
                                if self.video.baked_sun_removal_contour { self.video.baked_sun_removal_auto = false; }
                                self.sync_post_effects();
                                self.mark_config_dirty();
                            }
                        });
                    });
                if self.video.baked_sun_removal_auto || self.video.baked_sun_removal_contour {
                    // Render a disabled *copy* of the standard slider. Never
                    // write back to the archived manual r_bakedSunRemoval value.
                    // Unlike the manual slider, Auto can fit above 1x.
                    theme::row(
                        ui,
                        "Subtract baked sunlight",
                        "Read-only map-calibrated removal strength. Auto fits sunlight/shadow contrast on the original lightmaps; disabling Auto restores the previously saved manual strength.",
                        theme::Reset::None,
                        |ui| match self.baked_sun_auto_status {
                            crate::renderer::BakedSunAutoStatus::Calibrated(strength) => {
                                let mut display = strength;
                                let upper = if strength > 1.0 { 2.0 } else { 1.0 };
                                ui.add_enabled_ui(false, |ui| {
                                    theme::slider_default(
                                        ui,
                                        &mut display,
                                        0.0..=upper,
                                        &format!("{strength:.2}x (Auto)"),
                                        strength,
                                    );
                                });
                            }
                            crate::renderer::BakedSunAutoStatus::ContourGuided => {
                                ui.label("Contour-guided field ready (variable strength + local bounce)");
                            }
                            crate::renderer::BakedSunAutoStatus::Calibrating => {
                                ui.label("Calibrating...");
                            }
                            crate::renderer::BakedSunAutoStatus::Unavailable => {
                                ui.label("N/A (insufficient evidence)");
                            }
                        },
                    );
                } else if self.baked_slider(
                    ui,
                    "Subtract baked sunlight",
                    "Manual removal strength (used when Auto is off): 0 leaves authored lighting unchanged; 1 removes the predicted direct sun. Relighting with clouds may raise the effective manual minimum. Auto uses an independently fitted multiplier.",
                    theme::Reset::Environment(ui::ENV_ROW_BAKED_SUN_REMOVAL),
                    0.0..=1.0,
                    defaults.baked_sun_removal,
                    |v| &mut v.baked_sun_removal,
                ) {
                    self.environment_selected = ui::ENV_ROW_BAKED_SUN_REMOVAL;
                    self.touch_baked_sun_debug();
                }
            });
            ui.add_enabled_ui(baked_mode, |ui| {
                theme::section(ui, "REMAINING BAKED LIGHT", "Baked mode only. Adjust residual baked illumination; these controls do not change texture colors or output gamma. Saved values are retained in Realtime.");
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
            });

            ui.collapsing("Advanced sun removal / diagnostics", |ui| {
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
            });
            theme::section(ui, "SKY ILLUMINATION", "Sky illumination uses cached lightmaps in Baked. Realtime uses live Blue Water lighting or a cached vanilla sun-elevation approximation with live CSM shadows.");
            theme::row(ui, "Protect authored room lighting", "A/B: OFF uses the original sky multiplier (including indoors). ON weights sky tint by traced sky openness, preserving enclosed room/point lighting. Newly simulated map lights are added after sky tint. With Blue Water Realtime, static BSP surfaces use the cached sky bake instead of the fullscreen multiplier; live CSM remains. This is an approximation, not complete indirect-light separation. For Blue Water only; vanilla already uses traced openness. Requires CSM and the sky bake.", theme::Reset::None, |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.sky_world_light_isolation) {
                    let _ = self.set_console_cvar("r_skyWorldLightIsolation", if enabled { "1" } else { "0" });
                }
            });
            if self.video.sky_world_light_isolation {
                if self.video.sky_backend != 1 || !self.video.dynamic_shadows.is_csm() {
                    theme::hint(ui, "For Blue Water, select Cascaded Shadow Maps to protect indoor lighting. Vanilla uses traced sky openness automatically.", theme::TEXT_DIM());
                } else if baked_mode && (!self.video.baked_sun_cache || self.video.sky_world_bake_ab == 0) {
                    theme::hint(ui, "Baked mode needs B: Sky bake and the baked sun cache enabled for room protection.", theme::TEXT_DIM());
                } else if !baked_mode {
                    theme::hint(ui, "Realtime world sky is cached and recomposed when the sky changes (up to once/second); realtime caster shadows are still live. No full-screen sky multiplier.", theme::TEXT_DIM());
                }
            }
            theme::row(ui, "Sky world lighting", "World brightness adjustment: 0% off, 100% full. Vanilla uses sun elevation and traced sky openness; Blue Water uses atmospheric irradiance. Baked updates cached lighting, Realtime uses live Blue Water lighting or vanilla cached gain with live CSM.", theme::Reset::None, |ui| {
                let mut strength = self.video.sky_world_lighting_strength;
                let readout = format!("{:.0}%", strength * 100.0);
                if theme::slider_default(ui, &mut strength, 0.0..=1.0, &readout, defaults.sky_world_lighting_strength) {
                    self.video.sky_world_lighting_strength = strength;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                }
            });
            ui.add_enabled_ui(baked_mode, |ui| {
            if let Some(target) = segmented_row(ui, "Sky approximation in lightmaps", "Baked mode only. A: original relighting, without sky gain in lightmaps. B: apply the Sky world lighting strength to BSP lightmaps and eligible vertex-lit surfaces. Requires CSM and baked sun cache. Vanilla uses an analytic sun-elevation approximation; Blue Water uses its atmosphere.", theme::Reset::None,
                self.video.sky_world_bake_ab != 0, &[(false, "A: Original"), (true, "B: Sky bake")]) {
                let _ = self.set_console_cvar("r_skyWorldBakeAB", if target { "1" } else { "0" });
            }
            });
            if baked_mode && self.video.sky_world_bake_ab != 0 && (!self.video.dynamic_shadows.is_csm() || !self.video.baked_sun_cache) {
                ui.small("B requires Cascaded Shadow Maps and r_bakedSunCache 1.");
            }
            theme::section(ui, "ENTITY FIXTURE EXPERIMENT", "Compare existing model lighting with a restrained per-pixel accent from authored map fixtures. Original world lightmaps stay intact; world accents below are independent.");
            if let Some(target) = segmented_row(ui, "Fixture lighting on models", "Session-only. Requires Relight map and Cascaded Shadow Maps. B preserves baked entity lighting and adds individually shadowed fixture accents on players and lit models. Hardware RT retains its existing model receiver. Wait for pipeline compilation before comparing.", theme::Reset::None,
                self.video.entity_fixture_lighting_ab, &[(false,"A: Existing"),(true,"B: Fixtures")]) {
                let _ = self.set_console_cvar("r_entityFixtureLightingAB", if target {"1"} else {"0"});
            }
            if self.video.entity_fixture_lighting_ab && !self.video.dynamic_shadows.is_csm() {
                ui.small("Select Cascaded Shadow Maps in Lighting to activate B.");
            }
            if self.video.entity_fixture_lighting_ab && self.video.dynamic_lights == crate::ui::DynamicLightsMode::RayTracedHardware {
                ui.small("Hardware ray tracing keeps its existing model lighting; this experiment applies to raster lighting.");
            }
            theme::section(ui, "ADDED MAP LIGHTS", "Each source switch independently controls runtime map illumination and its optional lightmap reconstruction. Lamp-shadow source selections in Lighting never add illumination by themselves.");
            self.egui_toggle_row(
                ui,
                "Shader source: glow stages",
                "Promote surfaces with the authored JKA shader-stage `glow` marker. Texture colour/energy and actual BSP surface geometry determine the emitted light.",
                ui::VIDEO_ROW_PROMOTE_SHADER_GLOW,
                self.video.promote_shader_glow,
            );
            self.egui_toggle_row(
                ui,
                "Shader source: additive stages",
                "Promote visible additive emission stages (GL_ONE GL_ONE or SRC_ALPHA ONE). This is broader than `glow` and is useful for finding legacy emissive materials that were never tagged for Dynamic Glow.",
                ui::VIDEO_ROW_PROMOTE_SHADER_ADDITIVE,
                self.video.promote_shader_additive,
            );
            self.egui_toggle_row(
                ui,
                "Shader source: emissive maps",
                "Promote modern _emissive/_emission/_emit/_glow companion textures. Mostly-black masks are energy/coverage weighted so the entire surface does not behave like a full-white emitter.",
                ui::VIDEO_ROW_PROMOTE_SHADER_EMISSIVE,
                self.video.promote_shader_emissive,
            );

            self.egui_toggle_row(
                ui,
                "Authored surface / area lights",
                "Runtime area emitters from mapper-authored q3map_surfacelight metadata. Shader-driven glow/additive/emissive promotion is controlled separately above, so the same surface is never emitted twice.",
                ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS,
                self.video.emissive_area_lights,
            );

        });
        if self.sun_ray_until.is_some()
            && ui.ctx().dragged_id().is_some()
            && self.environment_selected == ui::ENV_ROW_BAKED_SUN_REMOVAL
        {
            self.touch_baked_sun_debug();
        }
    }

    fn reset_relighting_page(&mut self) {
        let d = VideoSettings::default();
        self.video.entity_sun_lighting = d.entity_sun_lighting;
        self.video.baked_sun_shadow_map = d.baked_sun_shadow_map;
        self.video.baked_sun_removal = d.baked_sun_removal;
        self.video.baked_sun_removal_auto = d.baked_sun_removal_auto;
        self.video.baked_sun_removal_contour = d.baked_sun_removal_contour;
        self.video.baked_sun_edge_softness = d.baked_sun_edge_softness;
        self.video.baked_sun_direct_limit = d.baked_sun_direct_limit;
        self.video.baked_indirect_scale = d.baked_indirect_scale;
        self.video.baked_indirect_saturation = d.baked_indirect_saturation;
        self.video.baked_indirect_power = d.baked_indirect_power;
        self.video.baked_indirect_highlight_preserve = d.baked_indirect_highlight_preserve;
        self.video.sun_lighting = d.sun_lighting;
        self.video.relight_time = d.relight_time;
        self.video.relight_match_system_time = d.relight_match_system_time;
        self.relight_clock_effective = None;
        self.apply_relighting_celestial_time(d.relight_time);
        self.video.static_world_csm = d.static_world_csm;
        self.video.promote_shader_glow = d.promote_shader_glow;
        self.video.promote_shader_additive = d.promote_shader_additive;
        self.video.promote_shader_emissive = d.promote_shader_emissive;
        self.video.emissive_area_lights = d.emissive_area_lights;
        self.video.entity_fixture_lighting_ab = false;
        self.render_command(crate::renderer::RenderCommand::SetEntityFixtureLightingAB(false));
        self.video.sky_world_bake_ab = d.sky_world_bake_ab;
        self.video.sky_world_light_isolation = d.sky_world_light_isolation;
        self.video.sky_world_lighting_strength = d.sky_world_lighting_strength;
        self.baked_sun_debug_view = 0;
        self.sun_ray_until = None;
        self.set_sun_override(false, false);
    }
}
