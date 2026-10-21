//! Postprocess cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

// Normalized post strength. Accept the historical boolean spellings as
// endpoints when loading older user configs, but retain fractional values.
fn normalized_post_strength(value: &str) -> Option<f32> {
    if let Ok(number) = value.trim().parse::<f32>() {
        return number.is_finite().then_some(number.clamp(0.0, 1.0));
    }
    parse_bool(value).map(f32::from)
}

#[cfg(test)]
mod strength_tests {
    use super::normalized_post_strength;

    #[test]
    fn legacy_boolean_endpoints_and_fractional_strengths() {
        assert_eq!(normalized_post_strength("0"), Some(0.0));
        assert_eq!(normalized_post_strength("1"), Some(1.0));
        assert_eq!(normalized_post_strength("0.35"), Some(0.35));
        assert_eq!(normalized_post_strength("off"), Some(0.0));
        assert_eq!(normalized_post_strength("on"), Some(1.0));
        assert_eq!(normalized_post_strength("2"), Some(1.0));
        assert_eq!(normalized_post_strength("-1"), Some(0.0));
        assert_eq!(normalized_post_strength("NaN"), None);
    }
}

app_cvars! {
    APP_R_AUTOEXPOSURE {
        runtime: CvarDef::new(
                "r_autoExposure",
                "",
                "0..1",
                "Auto exposure strength 0..1 (0 disabled); 1 preserves existing adaptation.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_autoexposure(app)),
                |app, value| {
                    App::cvar_set_r_autoexposure(app, APP_R_AUTOEXPOSURE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_autoexposure, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.auto_exposure = normalized_post_strength(value).unwrap_or(settings.auto_exposure)
                    }
                }),
                archive: Some((40, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.auto_exposure)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_autoexposure(&self) -> String {
                    format!("{:.3}", self.video.auto_exposure)
                }
                fn cvar_set_r_autoexposure(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.auto_exposure = normalized_post_strength(value)
                        .ok_or_else(|| format!("{name}: expected strength 0..1"))?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_CHROMATICABERRATION {
        runtime: CvarDef::new(
                "r_chromaticAberration",
                "",
                "0..1",
                "Photographic purple-fringing strength on bright high-contrast edges.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_chromaticaberration(app)),
                |app, value| {
                    App::cvar_set_r_chromaticaberration(
                        app,
                        APP_R_CHROMATICABERRATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_chromaticaberration, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.chromatic_aberration = value
                            .parse::<f32>()
                            .ok()
                            .filter(|strength| strength.is_finite())
                            .map(|strength| strength.max(0.0))
                            .or_else(|| parse_bool(value).map(|enabled| if enabled { 1.0 } else { 0.0 }))
                            .unwrap_or(settings.chromatic_aberration)
                    }
                }),
                archive: Some((108, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.chromatic_aberration)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_chromaticaberration(&self) -> String {
                    format!("{:.3}", self.video.chromatic_aberration)
                }
                fn cvar_set_r_chromaticaberration(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let strength = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a non-negative number"))?;
                        self.set_chromatic_aberration_strength(strength);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CONTACTSHADOWDEBUG {
        runtime: CvarDef::new("r_contactShadowDebug", "", "0..1", "Show why the contact-shadow pass returned per pixel: grey=factor, blue=faces away from sun, red=ray sub-pixel/off screen, yellow=origin behind camera, magenta=no depth (runtime diagnostic; not persisted).", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_contactshadowdebug(app)),
         |app, value| App::cvar_set_r_contactshadowdebug(app, APP_R_CONTACTSHADOWDEBUG.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_contactshadowdebug, None),
        config: CvarBinding {
                default: Some(|state| {
                    state.video.contact_shadow_debug.to_string()
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_contactshadowdebug(&self) -> String {
                    self.video.contact_shadow_debug.to_string()
                }
                fn cvar_set_r_contactshadowdebug(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.contact_shadow_debug = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| "r_contactShadowDebug takes 0 or 1".to_owned())?
                            .min(1);
                        self.render_command(RenderCommand::SetContactShadowDebug(
                            self.video.contact_shadow_debug,
                        ));
                        println!("Contact shadow debug={}", self.video.contact_shadow_debug);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SSRDEBUG {
        runtime: CvarDef::new("r_ssrDebug", "", "0..9", "Isolate the screen-space reflection pass: 0 off, 1 resolved SSR radiance alone, 2 blend weight as greyscale, 3 raw half-resolution history ahead of every gate, 4 the full-res gates (red=not a receiver, blue=frontmost rejected, green=allowed), 5 the SSR pass's stored depth (red=erased before tracing, green=traced), 6 normal view with temporal accumulation disabled, 7 why each ray ended (grey=not a receiver, white=planar won the slot, dark red=not glossy enough, dark blue=no strength, red=no crossing, brown=ray walked off screen, orange=sky fallback, magenta=refined off screen, cyan=no depth there, yellow=rejected as too deep, green=accepted), 8 normal view with the per-pixel step jitter pinned to its midpoint, 9 the reconstructed geometric normal as RGB (runtime diagnostic; not persisted).", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_ssrdebug(app)),
         |app, value| App::cvar_set_r_ssrdebug(app, APP_R_SSRDEBUG.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_ssrdebug, None),
        config: CvarBinding {
                default: Some(|state| {
                    state.video.ssr_debug.to_string()
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ssrdebug(&self) -> String {
                    self.video.ssr_debug.to_string()
                }
                fn cvar_set_r_ssrdebug(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.ssr_debug = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| "r_ssrDebug takes 0..9".to_owned())?
                            .min(9);
                        self.render_command(RenderCommand::SetSsrDebug(self.video.ssr_debug));
                        println!("SSR debug={}", self.video.ssr_debug);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CONTACTSHADOWS {
        runtime: CvarDef::new(
                "r_contactShadows",
                "",
                "0..1",
                "Short-range screen-space contact shadows toward the map sun.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_contactshadows(app)),
                |app, value| {
                    App::cvar_set_r_contactshadows(app, APP_R_CONTACTSHADOWS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_contactshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.contact_shadows = parse_bool(value).unwrap_or(settings.contact_shadows)
                    }
                }),
                archive: Some((53, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.contact_shadows))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_contactshadows(&self) -> String {
                    bool_value(self.video.contact_shadows)
                }
                fn cvar_set_r_contactshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_contactshadows".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DOFMODE {
        runtime: CvarDef::new(
                "r_dofMode",
                "",
                "off|underwater|on (0|1|2)",
                "Depth of field: off, fixed-distance underwater only, or on with the usual focus behavior.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dofmode(app)),
                |app, value| App::cvar_set_r_dofmode(app, APP_R_DOFMODE.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_r_dofmode, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.dof_mode = DofMode::from_config(value).unwrap_or(settings.dof_mode);
                }),
                archive: Some((115, |state| state.video.dof_mode.config_value().to_owned())),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dofmode(&self) -> String {
                    self.video.dof_mode.config_value().to_owned()
                }
                fn cvar_set_r_dofmode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = DofMode::from_config(value)
                        .ok_or_else(|| format!("{name}: expected off|underwater|on (0|1|2)"))?;
                    self.set_dof_mode(mode);
                    Ok(())
                }
            }
        ]
    }

    APP_R_DOFAUTOFOCUS {
        runtime: CvarDef::new("r_dofAutoFocus", "", "0..1", "Autofocus depth of field on the surface under the crosshair. Uses the same BSP/entity trace as the crosshair and works in remote play and demos, not only Solo.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_dofautofocus(app)), |app, value| App::cvar_set_r_dofautofocus(app, APP_R_DOFAUTOFOCUS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_dofautofocus, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.dof_autofocus = parse_bool(value).unwrap_or(settings.dof_autofocus)
                    }
                }),
                archive: Some((113, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.dof_autofocus))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dofautofocus(&self) -> String {
                    bool_value(self.video.dof_autofocus)
                }
                fn cvar_set_r_dofautofocus(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.dof_autofocus = Self::parse_cvar_bool(name, value)?;
                        self.crosshair_scan_at = Instant::now() - Duration::from_secs(1);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DOFQUALITY {
        runtime: CvarDef::new(
                "r_dofQuality",
                "",
                "performance|adaptive|high",
                "DOF Gaussian sampling quality. Adaptive spends extra samples only on large blur radii.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dofquality(app)),
                |app, value| {
                    App::cvar_set_r_dofquality(app, APP_R_DOFQUALITY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_dofquality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.dof_quality =
                            DofQuality::from_config(value).unwrap_or(settings.dof_quality)
                    }
                }),
                archive: Some((114, |state| {
                    let settings = state.video;
                    format!("{}", settings.dof_quality.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dofquality(&self) -> String {
                    self.video.dof_quality.config_value().to_owned()
                }
                fn cvar_set_r_dofquality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.dof_quality = DofQuality::from_config(value)
                            .ok_or_else(|| format!("{name}: expected performance|adaptive|high"))?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GLOW {
        runtime: CvarDef::new(
                "r_glow",
                "",
                "off|authored|bloom|combined",
                "Glow source: off, JKA-authored shader glow, modern HDR bloom, or both.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_glow(app)),
                |app, value| App::cvar_set_r_glow(app, APP_R_GLOW.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_glow, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.glow = GlowMode::from_config(value).unwrap_or(settings.glow)
                }),
                archive: Some((41, |state| {
                    let settings = state.video;
                    format!("{}", settings.glow.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_glow(&self) -> String {
                    self.video.glow.config_value().to_owned()
                }
                fn cvar_set_r_glow(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let Some(mode) = GlowMode::from_config(value) else {
                            return Err("usage: r_glow <off|authored|bloom|combined>".into());
                        };
                        self.video.glow = mode;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_HALATION {
        runtime: CvarDef::new(
                "r_halation",
                "",
                "0..1",
                "Halation post-processing.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_halation(app)),
                |app, value| App::cvar_set_r_halation(app, APP_R_HALATION.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_halation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.halation = parse_bool(value).unwrap_or(settings.halation)
                }),
                archive: Some((42, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.halation))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_halation(&self) -> String {
                    bool_value(self.video.halation)
                }
                fn cvar_set_r_halation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_halation".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_HDR {
        runtime: CvarDef::new(
                "r_hdr",
                "",
                "0..2",
                "HDR mode: 0 Off, 1 HDR Math Only (SDR output), 2 Full HDR Output (r_hdrStyle). Apply Video Settings when switching native output.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_hdr(app)),
                |app, value| App::cvar_set_r_hdr(app, APP_R_HDR.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_hdr, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(mode @ 0..=2) = value.trim().parse::<u32>() {
                        settings.set_hdr_mode(mode);
                    }
                }),
                archive: Some((37, |state| {
                    let settings = state.video;
                    settings.hdr_mode().to_string()
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_hdr(&self) -> String {
                    self.video.hdr_mode().to_string()
                }
                pub(in crate::app) fn cvar_set_r_hdr(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode: u32 = value.trim().parse().map_err(|_| format!("{name}: expected 0, 1, or 2"))?;
                    if mode > 2 { return Err(format!("{name}: expected 0, 1, or 2")); }
                    if mode == 2 && self.hdr_modes_known
                        && self.video.renderer_backend == self.applied_renderer_backend
                        && self.video.gpu_adapter == self.applied_gpu_adapter
                        && (self.hdr_available_modes & (1 << (self.video.hdr_style + 1))) == 0
                    {
                        return Err("Full HDR output unavailable for this display/backend".to_owned());
                    }
                    self.video.set_hdr_mode(mode);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }

            }
        ]
    }

    APP_R_RTTRACERATE {
        runtime: CvarDef::new("r_rtTraceRate", "", "1|2|4", "RT sun and local-light update rate: 1 Full, 2 Half, 4 Quarter. Retains full output resolution and reuses validated visibility history between fresh traces. Changes, edges and disocclusions may trace more often; colored glass uses direct rays.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(app.video.rt_trace_rate.to_string()), |app, value| App::cvar_set_r_rttracerate(app, APP_R_RTTRACERATE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_rttracerate, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(rate @ (1 | 2 | 4)) = value.parse::<u32>() { settings.rt_trace_rate = rate; }
            }),
            archive: Some((165, |state| state.video.rt_trace_rate.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_rttracerate(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let rate = value.parse::<u32>().ok().filter(|n| matches!(n, 1 | 2 | 4))
                        .ok_or_else(|| format!("{name}: expected 1|2|4"))?;
                    self.video.rt_trace_rate = rate;
                    self.render_command(RenderCommand::SetRtTraceRate(rate));
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    // Preserve old configs: historical "half" actually retraced one quarter per frame.
    APP_R_RTRESOLUTION {
        runtime: CvarDef::new("r_rtResolution", "", "full|half", "Legacy alias: full selects r_rtTraceRate 1; half selects 4. Prefer r_rtTraceRate 1|2|4.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app: &App| CvarValue::String(if app.video.rt_trace_rate == 1 { "full" } else { "half" }.to_owned()), |app, value| App::cvar_set_r_rtresolution(app, APP_R_RTRESOLUTION.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_rtresolution, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                match value.to_ascii_lowercase().as_str() {
                    "full" | "1" => settings.rt_trace_rate = 1,
                    "half" | "0.5" => settings.rt_trace_rate = 4,
                    _ => {}
                }
            }),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_rtresolution(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let rate = match value.to_ascii_lowercase().as_str() {
                        "full" | "1" => "1",
                        "half" | "0.5" => "4",
                        _ => return Err(format!("{name}: expected full|half (prefer r_rtTraceRate 1|2|4)")),
                    };
                    self.cvar_set_r_rttracerate("r_rtTraceRate", rate)
                }
            }
        ]
    }

    APP_R_RTSAMPLES {
        runtime: CvarDef::new("r_rtSamples", "", "1|2|4", "Samples per finite-emitter RT sun/local light. 1 is stochastic (best with TAA); 2/4 reduce noise at proportional ray cost; hard point lights use one ray.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_rtsamples(app)), |app, value| App::cvar_set_r_rtsamples(app, APP_R_RTSAMPLES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_rtsamples, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(samples @ (1 | 2 | 4)) = value.parse::<u32>() {
                        settings.rt_samples = samples;
                    }
                }),
                archive: Some((163, |state| {
                    let settings = state.video;
                    format!("{}", settings.rt_samples)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_rtsamples(&self) -> String {
                    self.video.rt_samples.to_string()
                }
                fn cvar_set_r_rtsamples(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let samples = value
                            .parse::<u32>()
                            .ok()
                            .filter(|n| matches!(n, 1 | 2 | 4))
                            .ok_or_else(|| format!("{name}: expected 1|2|4"))?;
                        self.video.rt_samples = samples;
                        self.render_command(RenderCommand::SetRtSamples(samples));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_RT_TAA {
        runtime: CvarDef::new("r_rtTaa", "", "0..2", "Regional RT dynamic-light temporal resolve: 0 Off, 1 On, 2 influence mask. Keeps MSAA and an unjittered camera; reuses existing forward RT samples. Enabling disables global TAA. The mask is a conservative light volume, including shadowed surfaces.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_rttaa(app)), |app, value| App::cvar_set_r_rttaa(app, APP_R_RT_TAA.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_rttaa, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode @ (0..=2)) = value.parse::<u32>() { settings.rt_taa = mode; }
            }),
            archive: Some((421, |state| format!("{}", state.video.rt_taa))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_rttaa(&self) -> String { self.video.rt_taa.to_string() }
                fn cvar_set_r_rttaa(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.parse::<u32>().ok().filter(|m| *m <= 2)
                        .ok_or_else(|| format!("{name}: expected 0..2"))?;
                    self.video.rt_taa = mode;
                    if mode != 0 {
                        self.video.taa = false;
                        self.video.fxaa = false;
                        self.video.smaa = false;
                    }
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }



    APP_R_SMAA {
        runtime: CvarDef::new(
                "r_smaa",
                "",
                "0..1",
                "SMAA anti-aliasing.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_smaa(app)),
                |app, value| App::cvar_set_r_smaa(app, APP_R_SMAA.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_smaa, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.smaa = parse_bool(value).unwrap_or(settings.smaa)
                }),
                archive: Some((51, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.smaa))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_smaa(&self) -> String {
                    bool_value(self.video.smaa)
                }
                fn cvar_set_r_smaa(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_smaa".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SSAO {
        runtime: CvarDef::new(
                "r_ssao",
                "",
                "0..1",
                "Screen-space ambient occlusion.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ssao(app)),
                |app, value| App::cvar_set_r_ssao(app, APP_R_SSAO.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_ssao, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ssao = parse_bool(value).unwrap_or(settings.ssao)
                }),
                archive: Some((43, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ssao))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ssao(&self) -> String {
                    bool_value(self.video.ssao)
                }
                fn cvar_set_r_ssao(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_ssao".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_STATICBSPAO {
        runtime: CvarDef::new(
                "r_staticBspAo",
                "",
                "0..1",
                "Cached static BSP ambient occlusion.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspao(app)),
                |app, value| {
                    App::cvar_set_r_staticbspao(app, APP_R_STATICBSPAO.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspao, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.static_bsp_ao = parse_bool(value).unwrap_or(settings.static_bsp_ao)
                    }
                }),
                archive: Some((44, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.static_bsp_ao))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspao(&self) -> String {
                    bool_value(self.video.static_bsp_ao)
                }
                fn cvar_set_r_staticbspao(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_staticbspao".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_STATICBSPAOCURRENTCELL {
        runtime: CvarDef::new(
                "r_staticBspAoCurrentCell",
                "",
                "custom",
                "Controls r_staticBspAoCurrentCell.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaocurrentcell(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaocurrentcell(
                        app,
                        APP_R_STATICBSPAOCURRENTCELL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaocurrentcell, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.static_bsp_ao_current_cell =
                        parse_bool(value).unwrap_or(settings.static_bsp_ao_current_cell);
                }),
                archive: Some((49, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.static_bsp_ao_current_cell))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaocurrentcell(&self) -> String {
                    bool_value(self.video.static_bsp_ao_current_cell)
                }
                fn cvar_set_r_staticbspaocurrentcell(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.static_bsp_ao_current_cell = Self::parse_cvar_bool(name, value)?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_STATICBSPAOMODE {
        runtime: CvarDef::new(
                "r_staticbspaomode",
                "",
                "custom",
                "Controls r_staticbspaomode.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaomode(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaomode(
                        app,
                        APP_R_STATICBSPAOMODE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaomode, None),
        config: CvarBinding {
                default: Some(|state| {
                    {
                        if state.video.static_bsp_ao_lightmap {
                            "lightmap".into()
                        } else {
                            "vertex".into()
                        }
                    }
                 }),
                load_video: Some(|settings, _name, value, _context| {
                    settings.static_bsp_ao_lightmap = match value.to_ascii_lowercase().as_str() {
                        "lightmap" | "lightmap-space" | "1" => true,
                        "vertex" | "per-vertex" | "0" => false,
                        _ => settings.static_bsp_ao_lightmap,
                    };
                }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaomode(&self) -> String {
                    {
                        if self.video.static_bsp_ao_lightmap {
                            "lightmap".into()
                        } else {
                            "vertex".into()
                        }
                    }
                }
                fn cvar_set_r_staticbspaomode(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.static_bsp_ao_lightmap = match value.to_ascii_lowercase().as_str() {
                            "lightmap" | "lightmap-space" | "1" => true,
                            "vertex" | "per-vertex" | "0" => false,
                            _ => return Err("usage: r_staticBspAoMode <vertex|lightmap>".into()),
                        };
                        if self.video.static_bsp_ao {
                            self.video.ssao = false;
                        }
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_STATICBSPAORANGE {
        runtime: CvarDef::new(
                "r_staticBspAoRange",
                "",
                "custom",
                "Controls r_staticBspAoRange.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaorange(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaorange(
                        app,
                        APP_R_STATICBSPAORANGE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaorange, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<u32>() {
                        const VALUES: [u32; 4] = [50, 100, 150, 200];
                        settings.static_bsp_ao_range = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(value))
                            .unwrap_or(&100);
                    }
                }),
                archive: Some((48, |state| {
                    let settings = state.video;
                    format!("{}", settings.static_bsp_ao_range)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaorange(&self) -> String {
                    self.video.static_bsp_ao_range.to_string()
                }
                fn cvar_set_r_staticbspaorange(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .parse::<u32>()
                            .map_err(|_| "usage: r_staticBspAoRange <50|100|150|200>".to_string())?;
                        let Some(&range) = [50_u32, 100, 150, 200].iter().find(|&&v| v == requested) else {
                            return Err("usage: r_staticBspAoRange <50|100|150|200>".into());
                        };
                        self.video.static_bsp_ao_range = range;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_STATICBSPAORESOLUTION {
        runtime: CvarDef::new(
                "r_staticBspAoResolution",
                "",
                "custom",
                "Controls r_staticBspAoResolution.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaoresolution(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaoresolution(
                        app,
                        APP_R_STATICBSPAORESOLUTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaoresolution, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(scale) = value.parse::<u32>() {
                        const VALUES: [u32; 3] = [1, 3, 5];
                        settings.static_bsp_ao_resolution = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(scale))
                            .unwrap_or(&3);
                    }
                }),
                archive: Some((46, |state| {
                    let settings = state.video;
                    format!("{}", settings.static_bsp_ao_resolution)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaoresolution(&self) -> String {
                    self.video.static_bsp_ao_resolution.to_string()
                }
                fn cvar_set_r_staticbspaoresolution(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let requested = value
                            .parse::<u32>()
                            .map_err(|_| "usage: r_staticBspAoResolution <1|3|5>".to_string())?;
                        let Some(&scale) = [1_u32, 3, 5].iter().find(|&&scale| scale == requested) else {
                            return Err("usage: r_staticBspAoResolution <1|3|5>".into());
                        };
                        self.video.static_bsp_ao_resolution = scale;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_STATICBSPAOSAMPLES {
        runtime: CvarDef::new(
                "r_staticBspAoSamples",
                "",
                "custom",
                "Controls r_staticBspAoSamples.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaosamples(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaosamples(
                        app,
                        APP_R_STATICBSPAOSAMPLES.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaosamples, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(samples) = value.parse::<u32>() {
                        const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                        settings.static_bsp_ao_samples = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(samples))
                            .unwrap_or(&8);
                    }
                }),
                archive: Some((45, |state| {
                    let settings = state.video;
                    format!("{}", settings.static_bsp_ao_samples)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaosamples(&self) -> String {
                    self.video.static_bsp_ao_samples.to_string()
                }
                fn cvar_set_r_staticbspaosamples(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let requested = value
                            .parse::<u32>()
                            .map_err(|_| "usage: r_staticBspAoSamples <8|16|32|64|128>".to_string())?;
                        const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                        let Some(&samples) = VALUES.iter().find(|&&samples| samples == requested) else {
                            return Err("usage: r_staticBspAoSamples <8|16|32|64|128>".into());
                        };
                        self.video.static_bsp_ao_samples = samples;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_STATICBSPAOSTRENGTH {
        runtime: CvarDef::new(
                "r_staticBspAoStrength",
                "",
                "custom",
                "Controls r_staticBspAoStrength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_staticbspaostrength(app)),
                |app, value| {
                    App::cvar_set_r_staticbspaostrength(
                        app,
                        APP_R_STATICBSPAOSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_staticbspaostrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<u32>() {
                        const VALUES: [u32; 4] = [25, 50, 75, 100];
                        settings.static_bsp_ao_strength = *VALUES
                            .iter()
                            .min_by_key(|&&candidate| candidate.abs_diff(value))
                            .unwrap_or(&75);
                    }
                }),
                archive: Some((47, |state| {
                    let settings = state.video;
                    format!("{}", settings.static_bsp_ao_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_staticbspaostrength(&self) -> String {
                    self.video.static_bsp_ao_strength.to_string()
                }
                fn cvar_set_r_staticbspaostrength(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let requested = value
                            .parse::<u32>()
                            .map_err(|_| "usage: r_staticBspAoStrength <25|50|75|100>".to_string())?;
                        let Some(&strength) = [25_u32, 50, 75, 100].iter().find(|&&v| v == requested)
                        else {
                            return Err("usage: r_staticBspAoStrength <25|50|75|100>".into());
                        };
                        self.video.static_bsp_ao_strength = strength;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_TAA {
        runtime: CvarDef::new(
                "r_taa",
                "",
                "0..1",
                "Temporal anti-aliasing.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_taa(app)),
                |app, value| App::cvar_set_r_taa(app, APP_R_TAA.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_taa, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.taa = parse_bool(value).unwrap_or(settings.taa)
                }),
                archive: Some((52, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.taa))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_taa(&self) -> String {
                    bool_value(self.video.taa)
                }
                fn cvar_set_r_taa(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_taa".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_tonemap" => self.video.tone_mapping = if enabled { 1.0 } else { 0.0 },
                            "r_autoexposure" => self.video.auto_exposure = if enabled { 1.0 } else { 0.0 },
                            "r_halation" => self.video.halation = enabled,
                            "r_ssao" => {
                                self.video.ssao = enabled;
                                if enabled {
                                    self.video.static_bsp_ao = false;
                                }
                            }
                            "r_staticbspao" => {
                                self.video.static_bsp_ao = enabled;
                                if enabled {
                                    self.video.ssao = false;
                                }
                            }
                            "r_fxaa" => {
                                self.video.fxaa = enabled;
                                if enabled {
                                    self.video.smaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_smaa" => {
                                self.video.smaa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.taa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_taa" => {
                                self.video.taa = enabled;
                                if enabled {
                                    self.video.fxaa = false;
                                    self.video.smaa = false;
                                    self.video.msaa_samples = 1;
                                    self.render_command(RenderCommand::SetMsaa(1));
                                }
                            }
                            "r_contactshadows" => self.video.contact_shadows = enabled,
                            "r_cloudshadows" => self.video.cloud_shadows = enabled,
                            "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                            "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                            "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                            "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                            "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                            "r_rain" => self.video.rain = enabled,
                            "r_sunoverride" => {
                                self.set_sun_override(enabled, enabled);
                                return Ok(());
                            }
                            "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                            "r_reflectiondebug" => self.video.reflection_debug = enabled,
                            "r_vignette" => self.video.vignette = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_post_effects();
                        if lower != "r_reflectiondebug" {
                            self.mark_config_dirty();
                        }
                        // r_floatLightmap is only effective while HDR is active, matching
                        // Rend2. If that changes the prepared lightmap representation,
                        // video_restart_required() exposes the normal renderer-restart path.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_TONEMAP {
        runtime: CvarDef::new(
                "r_tonemap",
                "",
                "custom",
                "Tone mapping strength 0..1 (0 disabled, 1 original ACES/rolloff).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_tonemap(app)),
                |app, value| App::cvar_set_r_tonemap(app, APP_R_TONEMAP.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_tonemap, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.tone_mapping = normalized_post_strength(value).unwrap_or(settings.tone_mapping);
                }),
                archive: Some((39, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.tone_mapping)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_tonemap(&self) -> String {
                    format!("{:.3}", self.video.tone_mapping)
                }
                fn cvar_set_r_tonemap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.tone_mapping = normalized_post_strength(value)
                        .ok_or_else(|| format!("{name}: expected strength 0..1"))?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
}
