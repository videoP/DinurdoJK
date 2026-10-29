//! Video quality presets.
//!
//! A preset is just a list of console cvars, applied through the same
//! [`App::set_console_cvar`] path the console and config loader use, so a preset
//! click and a typed command produce identical renderer state.
//!
//! Presets cover cost levers and use the balanced frame queue. Resolution,
//! display mode, render backend, vsync, FPS cap, gamma, input timing and the
//! debug/instrumentation toggles are left alone: those are machine and
//! preference settings, not quality, and silently rewriting them would be hostile.
//! GPU-driven drawing and Hi-Z occlusion are optimization A/B controls, not quality levers.
//! Likewise, `cg_fxFPS` is a user preference and not part of quality presets.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QualityPreset {
    /// Every cost lever off, on the known-fast FastBaseline world renderer whenever
    /// the settings allow it. The benchmark control.
    Minimal,
    /// The same settings as `Minimal`, forced through the unified world renderer
    /// (`r_worldPath unified`) so the two can be compared directly.
    MinimalUnified,
    Low,
    Medium,
    High,
    Ultra,
}

impl QualityPreset {
    pub(super) const ALL: [Self; 6] = [
        Self::Minimal,
        Self::MinimalUnified,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Ultra,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Minimal => "Minimal (Legacy)",
            Self::MinimalUnified => "Minimal (Unified)",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// Cvar/value pairs defining the preset. Order matters where two cvars write
    /// the same field: `r_texturemode` is coarse and anisotropy refines it. Keep
    /// aliases that mutate the same state out of this table: for example,
    /// `r_dynamicShadows` already owns the derived cascaded-shadow boolean.
    pub(super) fn settings(self) -> Vec<(&'static str, &'static str)> {
        // Medium is Low with the requested visual upgrades. Derive it so the
        // rest of the Low profile stays aligned if it changes later.
        if self == Self::Medium {
            let mut settings = Self::Low.settings();
            for (name, value) in &mut settings {
                match *name {
                    "r_footprints" => *value = "3d",
                    "r_grass" => *value = "1",
                    "r_reflectionQuality" => *value = "low",
                    "r_glow" => *value = "authored",
                    "fx_physics" => *value = "2",
                    "r_promoteFxLights" => *value = "1",
                    "r_detailTextures" => *value = "1",
                    "r_pbr" => *value = "1",
                    "r_genNormalMaps" => *value = "1",
                    "r_deluxeMapping" => *value = "1",
                    "r_lodbias" => *value = "0",
                    "r_lodScale" => *value = "100",
                    "r_subdivisions" => *value = "1",
                    "r_lodCurveError" => *value = "2500",
                    _ => {}
                }
            }
            return settings;
        }

        // High is Medium with the requested upgrades.
        if self == Self::High {
            let mut settings = Self::Medium.settings();
            for (name, value) in &mut settings {
                match *name {
                    "r_ext_multisample" => *value = "4",
                    "fx_physics" => *value = "3",
                    "r_promoteFxLights" => *value = "2",
                    "r_dynamicLights" => *value = "clustered_lite",
                    "r_deluxeSpecular" => *value = "1",
                    "r_dynamicShadows" => *value = "csm_entities",
                    "r_glassPromotion" => *value = "1",
                    "r_hdr" => *value = "1",
                    "r_ocean" => *value = "1",
                    _ => {}
                }
            }
            return settings;
        }

        // Ultra is High with ray-traced lights, CSM (moving entities) shadows, RT regional TAA and 4x MSAA.
        if self == Self::Ultra {
            let mut settings = Self::High.settings();
            for (name, value) in &mut settings {
                match *name {
                    "r_ext_multisample" => *value = "4",
                    "r_taa" => *value = "0",
                    "r_dynamicLights" => *value = "ray_traced",
                    "r_dynamicShadows" => *value = "csm_entities",
                    "r_hdr" => *value = "1",
                    _ => {}
                }
            }
            settings.push(("r_rtSamples", "4"));
            return settings;
        }

        let table: &'static [(&'static str, &'static str)] = match self {
            Self::Minimal | Self::MinimalUnified => &[
                // The cvar's canonical "off" value is 0; the setter maps both
                // 0 and 1 to the renderer's single-sample state. Using 0 here
                // lets active_quality_preset() compare against the getter.
                ("r_ext_multisample", "0"),
                ("r_maxFrameLatency", "2"),
                ("r_picmip", "3"),
                ("fx_physics", "0"),
                ("r_promoteFxLights", "0"),
                ("r_texturemode", "GL_LINEAR_MIPMAP_LINEAR"),
                ("r_detailTextures", "off"),
                ("r_hdr", "0"),
                ("r_floatLightmap", "0"),
                ("r_tonemap", "0"),
                ("r_autoExposure", "0"),
                ("r_glow", "off"),
                ("r_halation", "0"),
                ("r_ssao", "0"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "0"),
                ("r_taa", "0"),
                ("r_contactShadows", "0"),
                ("r_drawfog", "2"),
                ("r_sunVisibility", "legacy"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "0"),
                ("r_cloudQuality", "0.25"),
                ("r_cloudShadows", "0"),
                ("r_grass", "0"),
                ("r_ocean", "0"),
                ("r_oceanMapSize", "128"),
                ("r_oceanMeshQuality", "0"),
                ("r_oceanUpdates", "24"),
                ("r_footprints", "off"),
                ("r_reflectionQuality", "off"),
                ("r_chromaticAberration", "0"),
                ("r_vignette", "0"),
                ("r_filmGrain", "0"),
                ("r_motionBlur", "0"),
                ("r_depthOfField", "0"),
                ("r_colorLut", "off"),
                ("r_lodbias", "3"),
                ("r_entityAmbientLighting", "bsp_lightgrid"),
                ("r_dynamicLights", "off"),
                ("r_pbr", "0"),
                ("r_genNormalMaps", "0"),
                ("r_deluxeMapping", "0"),
                ("r_deluxeSpecular", "0"),
                ("r_dynamicShadows", "off"),
                ("r_voxelProbeGI", "0"),
                ("r_localLightShadows", "0"),
                ("r_glassPromotion", "0"),
            ],
            Self::Low => &[
                ("r_ext_multisample", "2"),
                ("r_maxFrameLatency", "2"),
                ("r_picmip", "0"),
                ("fx_physics", "0"),
                ("r_promoteFxLights", "0"),
                ("r_ext_texture_filter_anisotropic", "16"),
                ("r_detailTextures", "off"),
                ("r_hdr", "0"),
                ("r_floatLightmap", "0"),
                ("r_tonemap", "0"),
                ("r_autoExposure", "0"),
                ("r_glow", "off"),
                ("r_halation", "0"),
                ("r_ssao", "0"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "0"),
                ("r_taa", "0"),
                ("r_contactShadows", "0"),
                ("r_drawfog", "2"),
                ("r_sunVisibility", "sky"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "0"),
                ("r_cloudQuality", "0.40"),
                ("r_cloudShadows", "1"),
                ("r_grass", "0"),
                ("r_ocean", "0"),
                ("r_oceanMapSize", "256"),
                ("r_oceanMeshQuality", "0"),
                ("r_oceanUpdates", "30"),
                ("r_footprints", "2d"),
                ("r_reflectionQuality", "legacy"),
                ("r_chromaticAberration", "0"),
                ("r_vignette", "0"),
                ("r_filmGrain", "0"),
                ("r_motionBlur", "0"),
                ("r_depthOfField", "0"),
                ("r_dofQuality", "adaptive"),
                ("r_colorLut", "fuji_velvia_50"),
                ("r_colorLutStrength", "0.500"),
                ("r_lodbias", "0"),
                ("r_entityAmbientLighting", "bsp_lightgrid"),
                ("r_dynamicLights", "legacy"),
                ("r_pbr", "0"),
                ("r_genNormalMaps", "0"),
                ("r_deluxeMapping", "0"),
                ("r_deluxeSpecular", "0"),
                ("r_dynamicShadows", "blob"),
                ("r_voxelProbeGI", "0"),
                ("r_localLightShadows", "0"),
                ("r_glassPromotion", "0"),
            ],
            Self::High => unreachable!("High derives from Medium above"),
            Self::Ultra => unreachable!("Ultra derives from High above"),
            Self::Medium => unreachable!("Medium derives from Low above"),
        };
        let mut settings = table.to_vec();
        // Preserve seam compatibility at every tier. Medium and above use the
        // menu's maximum patch quality and the highest native model detail.
        let (subdivisions, curve_error, model_scale) = match self {
            Self::Minimal | Self::MinimalUnified => ("12", "125", "5"),
            Self::Low => ("8", "125", "5"),
            Self::Medium | Self::High | Self::Ultra => unreachable!("derived presets return above"),
        };
        settings.extend([
            ("r_subdivisions", subdivisions),
            ("r_lodCurveError", curve_error),
            ("r_patchStitching", "1"),
            ("r_lodScale", model_scale),
        ]);
        // The two Minimal presets differ only in which world renderer draws.
        match self {
            Self::Minimal => settings.push(("r_worldPath", "auto")),
            Self::MinimalUnified => settings.push(("r_worldPath", "unified")),
            _ => {}
        }
        settings
    }
}

/// Two cvar values are equal when they name the same setting. Numeric values are
/// compared as numbers so "0" and "0.000" match; everything else is a label.
pub(super) fn cvar_values_match(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
        (Ok(left), Ok(right)) => (left - right).abs() < 0.0005,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

impl App {
    pub(super) fn apply_quality_preset(&mut self, preset: QualityPreset) {
        self.apply_quality_preset_with_scope(preset, None);
    }

    pub(super) fn apply_quality_preset_to_page(
        &mut self,
        preset: QualityPreset,
        section: super::egui_menu::VideoSection,
    ) {
        self.apply_quality_preset_with_scope(preset, Some(section));
    }

    fn apply_quality_preset_with_scope(
        &mut self,
        preset: QualityPreset,
        section: Option<super::egui_menu::VideoSection>,
    ) {
        let settings: Vec<_> = preset.settings().into_iter().filter(|(name, _)| {
            section.is_none_or(|section| super::egui_settings::preset_cvar_section(name) == Some(section))
                // Inside Render Studio a preset only touches the Studio profile.
                && (self.render_studio.graphics_gameplay.is_none()
                    || super::render_studio_cfg::studio_graphics_cvar(name))
        }).collect();
        if settings.is_empty() {
            return;
        }
        let mut apply_failed = false;
        let title = match section {
            Some(section) => format!("{} quality preset on {section:?}", preset.label()),
            None => format!("{} quality preset", preset.label()),
        };
        let batch_started = Instant::now();
        let mut timings: Vec<(&str, f64)> = Vec::with_capacity(settings.len());
        self.begin_settings_batch(&title);
        for (name, value) in &settings {
            self.render_command(RenderCommand::BatchLabel((*name).to_owned()));
            let setting_started = Instant::now();
            // Applying a preset overrides pending console values for its own controls.
            self.latched_console_cvars.remove(&name.to_ascii_lowercase());
            let result = self.set_console_cvar(name, value);
            timings.push((name, setting_started.elapsed().as_secs_f64() * 1000.0));
            if let Err(error) = result {
                // A preset naming a cvar the build no longer has is a bug in the
                // table, not something the player can act on. Keep applying the
                // rest so one bad entry does not prevent the remaining quality
                // settings from landing, but do not claim the preset is active.
                apply_failed = true;
                eprintln!("[QUALITY] {} preset: {error}", preset.label());
            }
        }

        self.end_settings_batch();
        let applied_ms = batch_started.elapsed().as_secs_f64() * 1000.0;
        timings.sort_by(|a, b| b.1.total_cmp(&a.1));
        devprintln!(
            2,
            "[BATCH] {title}: main thread applied {} setting(s) in {applied_ms:.2} ms (render-thread work is reported separately)",
            timings.len(),
        );
        for (name, ms) in timings.iter().filter(|(_, ms)| *ms >= 0.05).take(20) {
            devprintln!(2, "[BATCH]   main {name}: {ms:.2} ms");
        }

        // Capture the canonical values *after* all setters have run. Some cvars
        // normalize aliases, clamp values, or intentionally update related
        // settings. The selected chip should describe the actual state produced
        // by the preset, not require every getter to reproduce the literal input
        // spelling from the table.
        if apply_failed || section.is_some() {
            // Page-only changes do not imply that other categories match the
            // same preset. The global bar should return to Custom unless the
            // whole configuration independently matches a known preset.
            self.quality_preset_selected = None;
            self.quality_preset_values.clear();
        } else {
            let values: Vec<_> = preset
                .settings()
                .iter()
                .filter_map(|(name, _)| {
                    self.console_cvar_value(name)
                        .map(|value| ((*name).to_owned(), value))
                })
                .collect();
            if values.len() == preset.settings().len() {
                self.quality_preset_selected = Some(preset);
                self.quality_preset_values = values;
            } else {
                self.quality_preset_selected = None;
                self.quality_preset_values.clear();
            }
        }

        self.mark_config_dirty();
        // The row computed its selected state before handling this click. Force
        // a fresh egui frame so the newly-applied preset lights immediately.
        self.egui_repaint_requested = true;
        self.egui_ctx.request_repaint();
        self.publish_ui();
    }

    /// The preset the current settings correspond to, or `None` when they have
    /// been customized. An explicitly-applied preset is tracked using the
    /// canonical values its setters actually produced; changing any controlled
    /// setting breaks that snapshot. The table comparison remains as a fallback
    /// so a matching config loaded at startup can still be recognized.
    pub(super) fn active_quality_preset(&self) -> Option<QualityPreset> {
        if let Some(preset) = self.quality_preset_selected {
            if self.quality_preset_values.len() == preset.settings().len()
                && self.quality_preset_values.iter().all(|(name, value)| {
                    self.console_cvar_value(name)
                        .is_some_and(|current| cvar_values_match(&current, value))
                })
            {
                return Some(preset);
            }
        }

        QualityPreset::ALL.into_iter().find(|preset| {
            preset.settings().iter().all(|(name, value)| {
                self.console_cvar_value(name)
                    .is_some_and(|current| cvar_values_match(&current, value))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::QualityPreset;

    #[cfg(target_os = "windows")]
    #[test]
    fn applied_medium_high_ultra_use_highest_model_detail() {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
            .with_any_thread(true).build().unwrap();
        let fixture = crate::cvar_compatibility::Fixture::new();
        let mut app = crate::app::App::new(fixture.0.clone(), None,
            crate::scene::MapSource::Bsp("mp/ffa3".into()), false, event_loop.create_proxy()).unwrap();
        app.supported_msaa = vec![1, 2, 4, 8];
        app.video.ghoul2_lod_bias = 2;
        app.video.lod_scale = 1.0;
        app.latched_console_cvars.insert("r_subdivisions".into(), "80".into());
        for (preset, subdivisions, curve_error) in [
            (QualityPreset::Medium, "1", "2500"),
            (QualityPreset::High, "1", "2500"),
            (QualityPreset::Ultra, "1", "2500"),
        ] {
            app.apply_quality_preset(preset);
            assert_eq!(app.video.ghoul2_lod_bias, 0);
            assert_eq!(app.video.lod_scale, crate::fx::LOD_SCALE_MAX);
            assert_eq!(app.console_cvar_value("r_lodbias").as_deref(), Some("0"));
            assert_eq!(app.console_cvar_value("r_lodScale").as_deref(), Some("100"));
            assert_eq!(app.console_cvar_value("r_subdivisions").as_deref(), Some(subdivisions));
            assert_eq!(app.console_cvar_value("r_lodCurveError").as_deref(), Some(curve_error));
            assert_eq!(app.active_quality_preset(), Some(preset));
            assert!(!app.latched_console_cvars.contains_key("r_subdivisions"));
            println!("{} applied: model detail=Highest, model scale=100, curve detail={subdivisions}, curve error={curve_error}", preset.label());
        }
    }

    #[test]
    fn quality_presets_keep_medium_high_ultra_patch_and_model_detail_at_maximum() {
        let mut previous_subdivisions = f32::INFINITY;
        let mut previous_curve_error = 0.0;
        for preset in [QualityPreset::Minimal, QualityPreset::Low, QualityPreset::Medium, QualityPreset::High, QualityPreset::Ultra] {
            let settings = preset.settings();
            let value = |key| settings.iter().find(|(name, _)| *name == key).unwrap().1;
            let subdivisions = value("r_subdivisions").parse::<f32>().unwrap();
            let curve_error = value("r_lodCurveError").parse::<f32>().unwrap();
            assert!(subdivisions <= previous_subdivisions);
            assert!(curve_error >= previous_curve_error);
            assert_eq!(value("r_patchStitching"), "1");
            assert!(settings.iter().all(|(name, _)| !["r_nocurves", "r_speeds", "r_showtris"].contains(name)));
            if matches!(preset, QualityPreset::Medium | QualityPreset::High | QualityPreset::Ultra) {
                assert_eq!(value("r_subdivisions"), "1");
                assert_eq!(value("r_lodCurveError"), "2500");
                assert_eq!(value("r_lodbias"), "0");
                assert_eq!(value("r_lodScale").parse::<f32>().unwrap(), crate::fx::LOD_SCALE_MAX);
            }
            if preset == QualityPreset::Ultra {
                assert_eq!(value("r_dynamicShadows"), "csm_entities");
            }
            let mut names = std::collections::BTreeSet::new();
            assert!(settings.iter().all(|(name, _)| names.insert(*name)), "duplicate preset setting");
            previous_subdivisions = subdivisions;
            previous_curve_error = curve_error;
        }
        let medium = QualityPreset::Medium.settings();
        assert!(medium.contains(&("r_subdivisions", "1")));
        assert!(medium.contains(&("r_lodCurveError", "2500")));
    }

    #[test]
    fn quality_presets_do_not_touch_optimization_controls() {
        for preset in QualityPreset::ALL {
            let settings = preset.settings();
            for (cvar, _, _) in super::super::egui_settings::OPTIMIZATION_CVARS {
                assert!(settings.iter().all(|(name, _)| !name.eq_ignore_ascii_case(cvar)),
                    "{} preset must not change {cvar}", preset.label());
            }
        }
    }

    #[test]
    fn quality_presets_leave_fx_fps_unchanged_and_use_expected_lod_bias() {
        assert_eq!(crate::fx::FX_FPS_DEFAULT, 90);
        for preset in QualityPreset::ALL {
            let settings = preset.settings();
            assert!(settings.iter().all(|(name, _)| !name.eq_ignore_ascii_case("cg_fxFPS")));
            let expected_bias = match preset {
                QualityPreset::Minimal | QualityPreset::MinimalUnified => "3",
                QualityPreset::Low
                | QualityPreset::Medium
                | QualityPreset::High
                | QualityPreset::Ultra => "0",
            };
            assert_eq!(
                settings.iter().find(|(name, _)| *name == "r_lodbias").map(|(_, value)| *value),
                Some(expected_bias),
                "{} preset",
                preset.label(),
            );
        }
    }
}
