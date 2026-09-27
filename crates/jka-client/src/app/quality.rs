//! Video quality presets.
//!
//! A preset is just a list of console cvars, applied through the same
//! [`App::set_console_cvar`] path the console and config loader use, so a preset
//! click and a typed command produce identical renderer state.
//!
//! Presets deliberately cover cost levers only. Resolution, display mode, render
//! backend, vsync, frame queue, FPS cap, gamma, input timing and the debug/instrumentation
//! toggles are left alone: those are machine and preference settings, not
//! quality, and silently rewriting them would be hostile.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QualityPreset {
    Minimal,
    Low,
    Medium,
    High,
}

impl QualityPreset {
    pub(super) const ALL: [Self; 4] = [Self::Minimal, Self::Low, Self::Medium, Self::High];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// Cvar/value pairs defining the preset. Order matters where two cvars write
    /// the same field: `r_texturemode` is coarse and anisotropy refines it. Keep
    /// aliases that mutate the same state out of this table: for example,
    /// `r_dynamicShadows` already owns the derived cascaded-shadow boolean.
    pub(super) fn settings(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Minimal => &[
                // The cvar's canonical "off" value is 0; the setter maps both
                // 0 and 1 to the renderer's single-sample state. Using 0 here
                // lets active_quality_preset() compare against the getter.
                ("r_ext_multisample", "0"),
                ("r_texturemode", "GL_LINEAR_MIPMAP_NEAREST"),
                ("r_hdr", "0"),
                ("r_floatLightmap", "0"),
                ("r_tonemap", "0"),
                ("r_autoExposure", "0"),
                ("r_bloom", "0"),
                ("r_halation", "0"),
                ("r_ssao", "0"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "0"),
                ("r_taa", "0"),
                ("r_contactShadows", "0"),
                ("r_fogMode", "legacy"),
                ("r_sunVisibility", "legacy"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "0"),
                ("r_cloudQuality", "0.25"),
                ("r_cloudShadows", "0"),
                ("r_rain", "0"),
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
                ("cg_fxFPS", "30"),
                ("r_colorLut", "off"),
                ("r_gpuDriven", "0"),
                ("r_hizOcclusion", "0"),
                ("r_lodbias", "3"),
                ("r_entityAmbientLighting", "bsp_lightgrid"),
                ("r_dynamicLights", "off"),
                ("r_pbr", "0"),
                ("r_genNormalMaps", "0"),
                ("r_deluxeMapping", "0"),
                ("r_deluxeSpecular", "0"),
                ("r_dynamicShadows", "off"),
                ("r_emissiveAreaLights", "0"),
                ("r_voxelProbeGI", "0"),
                ("r_localLightShadows", "0"),
            ],
            Self::Low => &[
                ("r_ext_multisample", "2"),
                ("r_ext_texture_filter_anisotropic", "16"),
                ("r_hdr", "0"),
                ("r_floatLightmap", "0"),
                ("r_tonemap", "0"),
                ("r_autoExposure", "0"),
                ("r_bloom", "0"),
                ("r_halation", "0"),
                ("r_ssao", "0"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "0"),
                ("r_taa", "0"),
                ("r_contactShadows", "0"),
                ("r_fogMode", "legacy"),
                ("r_sunVisibility", "sky"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "0"),
                ("r_cloudQuality", "0.40"),
                ("r_cloudShadows", "1"),
                ("r_rain", "0"),
                ("r_grass", "1"),
                ("r_ocean", "0"),
                ("r_oceanMapSize", "256"),
                ("r_oceanMeshQuality", "0"),
                ("r_oceanUpdates", "30"),
                ("r_footprints", "3d"),
                ("r_reflectionQuality", "low"),
                ("r_chromaticAberration", "0"),
                ("r_vignette", "0"),
                ("r_filmGrain", "0"),
                ("r_motionBlur", "0"),
                ("r_depthOfField", "0"),
                ("r_dofQuality", "adaptive"),
                ("cg_fxFPS", "60"),
                ("r_colorLut", "fuji_velvia_50"),
                ("r_colorLutStrength", "0.500"),
                ("r_gpuDriven", "0"),
                ("r_hizOcclusion", "0"),
                ("r_lodbias", "2"),
                ("r_entityAmbientLighting", "bsp_lightgrid"),
                ("r_dynamicLights", "legacy"),
                ("r_pbr", "0"),
                ("r_genNormalMaps", "0"),
                ("r_deluxeMapping", "0"),
                ("r_deluxeSpecular", "0"),
                ("r_dynamicShadows", "blob_stencil"),
                ("r_emissiveAreaLights", "0"),
                ("r_voxelProbeGI", "0"),
                ("r_localLightShadows", "0"),
            ],
            Self::Medium => &[
                // SMAA is the preset AA mode. Post-process AA and MSAA are
                // mutually exclusive in set_console_cvar(), so describe the
                // normalized single-sample state instead of asking for both.
                ("r_ext_multisample", "0"),
                ("r_ext_texture_filter_anisotropic", "16"),
                ("r_hdr", "1"),
                ("r_floatLightmap", "0"),
                ("r_tonemap", "1"),
                ("r_autoExposure", "1"),
                ("r_bloom", "1"),
                ("r_halation", "0"),
                ("r_ssao", "1"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "1"),
                ("r_taa", "0"),
                ("r_contactShadows", "0"),
                ("r_fogMode", "legacy"),
                ("r_sunVisibility", "sky"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "1"),
                ("r_cloudQuality", "0.70"),
                ("r_cloudRenderResolution", "half"),
                ("r_cloudShadows", "1"),
                ("r_cloudTemporal", "1"),
                ("r_cloudTemporalDepthFix", "1"),
                ("r_rain", "0"),
                ("r_grass", "1"),
                ("r_ocean", "0"),
                ("r_oceanMapSize", "512"),
                ("r_oceanMeshQuality", "1"),
                ("r_oceanUpdates", "48"),
                ("r_footprints", "3d"),
                ("r_reflectionQuality", "medium"),
                ("r_chromaticAberration", "0"),
                ("r_vignette", "1"),
                ("r_filmGrain", "0"),
                ("r_motionBlur", "0"),
                ("r_depthOfField", "0"),
                ("r_dofQuality", "adaptive"),
                ("cg_fxFPS", "90"),
                ("r_colorLut", "fuji_velvia_50"),
                ("r_colorLutStrength", "0.500"),
                ("r_gpuDriven", "1"),
                ("r_hizOcclusion", "1"),
                ("r_lodbias", "1"),
                ("r_entityAmbientLighting", "bevy_irradiance_volume"),
                ("r_dynamicLights", "forward_plus"),
                ("r_pbr", "0"),
                ("r_genNormalMaps", "0"),
                ("r_deluxeMapping", "0"),
                ("r_deluxeSpecular", "0"),
                ("r_dynamicShadows", "csm"),
                ("r_emissiveAreaLights", "0"),
                ("r_voxelProbeGI", "0"),
                ("r_localLightShadows", "0"),
            ],
            Self::High => &[
                // TAA is the preset AA mode; enabling it intentionally disables
                // MSAA, so the preset must match that final state.
                ("r_ext_multisample", "0"),
                ("r_ext_texture_filter_anisotropic", "16"),
                ("r_hdr", "1"),
                ("r_floatLightmap", "1"),
                ("r_tonemap", "1"),
                ("r_autoExposure", "1"),
                ("r_bloom", "1"),
                ("r_halation", "1"),
                ("r_ssao", "1"),
                ("r_staticBspAo", "0"),
                ("r_staticBspAoSamples", "8"),
                ("r_staticBspAoResolution", "1"),
                ("r_fxaa", "0"),
                ("r_smaa", "0"),
                ("r_taa", "1"),
                ("r_contactShadows", "1"),
                ("r_fogMode", "volumetric"),
                ("r_sunVisibility", "filtered"),
                ("r_distanceCullScale", "0"),
                ("r_clouds", "1"),
                ("r_cloudQuality", "1.0"),
                ("r_cloudRenderResolution", "full"),
                ("r_cloudShadows", "1"),
                ("r_cloudTemporal", "1"),
                ("r_cloudTemporalDepthFix", "1"),
                ("r_rain", "0"),
                ("r_grass", "1"),
                ("r_ocean", "1"),
                ("r_oceanMapSize", "1024"),
                ("r_oceanMeshQuality", "1"),
                ("r_oceanUpdates", "60"),
                ("r_oceanSeaSpray", "1"),
                ("r_oceanWindFoam", "1"),
                ("r_footprints", "3d"),
                ("r_reflectionQuality", "high"),
                ("r_chromaticAberration", "0"),
                ("r_vignette", "1"),
                ("r_filmGrain", "0"),
                ("r_motionBlur", "0"),
                ("r_depthOfField", "0"),
                ("r_dofQuality", "high"),
                ("cg_fxFPS", "120"),
                ("r_colorLut", "fuji_velvia_50"),
                ("r_colorLutStrength", "0.500"),
                ("r_gpuDriven", "1"),
                ("r_hizOcclusion", "1"),
                ("r_lodbias", "0"),
                ("r_entityAmbientLighting", "bevy_irradiance_volume"),
                ("r_dynamicLights", "forward_plus"),
                ("r_pbr", "1"),
                ("r_genNormalMaps", "1"),
                ("r_deluxeMapping", "1"),
                ("r_deluxeSpecular", "1"),
                ("r_dynamicShadows", "csm_bevy"),
                ("r_emissiveAreaLights", "1"),
                ("r_voxelProbeGI", "1"),
                ("r_localLightShadows", "1"),
            ],
        }
    }
}

/// Two cvar values are equal when they name the same setting. Numeric values are
/// compared as numbers so "0" and "0.000" match; everything else is a label.
fn cvar_values_match(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
        (Ok(left), Ok(right)) => (left - right).abs() < 0.0005,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

impl App {
    pub(super) fn apply_quality_preset(&mut self, preset: QualityPreset) {
        let mut apply_failed = false;
        for (name, value) in preset.settings() {
            if let Err(error) = self.set_console_cvar(name, value) {
                // A preset naming a cvar the build no longer has is a bug in the
                // table, not something the player can act on. Keep applying the
                // rest so one bad entry does not prevent the remaining quality
                // settings from landing, but do not claim the preset is active.
                apply_failed = true;
                eprintln!("[QUALITY] {} preset: {error}", preset.label());
            }
        }

        // Capture the canonical values *after* all setters have run. Some cvars
        // normalize aliases, clamp values, or intentionally update related
        // settings. The selected chip should describe the actual state produced
        // by the preset, not require every getter to reproduce the literal input
        // spelling from the table.
        if apply_failed {
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
