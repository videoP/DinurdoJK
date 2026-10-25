//! Effects cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

// Fix the CvarDef state type before the getter/setter closures are inferred.
// This mirrors the renderer_cvars registry.
type CvarDef = crate::console::cvars::CvarDef<App>;

app_cvars! {
    APP_R_MARKCOUNT {
        runtime: CvarDef::new("r_markcount", "", "0..4096", "TaystJK wall mark count (default 100). Zero disables authored EFX decals.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(app.video.mark_count.to_string()), |app, value| App::cvar_set_r_markcount(app, APP_R_MARKCOUNT.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_markcount, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(n) = value.trim().parse::<u32>() { settings.mark_count = n.min(4096); }
            }),
            archive: Some((174, |state| state.video.mark_count.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_markcount(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let n: u32 = value.trim().parse().map_err(|_| "r_markcount expects 0..4096".to_owned())?;
                    self.video.mark_count = n.min(4096);
                    self.apply_mark_count();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
    APP_R_FXAA {
        runtime: CvarDef::new(
                "r_fxaa",
                "",
                "0..1",
                "FXAA anti-aliasing.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_fxaa(app)),
                |app, value| App::cvar_set_r_fxaa(app, APP_R_FXAA.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_fxaa, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fxaa = parse_bool(value).unwrap_or(settings.fxaa)
                }),
                archive: Some((50, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.fxaa))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fxaa(&self) -> String {
                    bool_value(self.video.fxaa)
                }
                fn cvar_set_r_fxaa(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_fxaa".to_owned();
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

    APP_CG_FXFPS {
        runtime: CvarDef::new("cg_fxFPS", "", "0 or 15..250", "Continuous projectile/trail EFX sampling rate. 0 restores legacy JKA presentation-frame-driven density.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_fxfps(app)), |app, value| App::cvar_set_cg_fxfps(app, APP_CG_FXFPS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_fxfps, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_fps = normalize_fx_fps(value, settings.fx_fps);
                }),
                archive: Some((169, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_fps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fxfps(&self) -> String {
                    self.video.fx_fps.to_string()
                }
                fn cvar_set_cg_fxfps(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value.trim().parse::<u32>().map_err(|_| {
                            format!(
                                "cg_fxFPS expects 0 (Legacy JKA) or {}..{} Hz",
                                crate::fx::FX_FPS_MIN,
                                crate::fx::FX_FPS_MAX
                            )
                        })?;
                        self.video.fx_fps = if requested == crate::fx::FX_FPS_LEGACY_JKA {
                            crate::fx::FX_FPS_LEGACY_JKA
                        } else {
                            requested.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
                        };
                        self.apply_fx_fps_settings();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                            "FX FPS: LEGACY JKA (ONE CONTINUOUS EFFECT INVOCATION PER PRESENTATION FRAME)"
                                .into()
                        } else {
                            format!("FX FPS: {} HZ", self.video.fx_fps)
                        };
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FXFPSSCOPE {
        runtime: CvarDef::new("cg_fxFPSScope", "", "0..1", "Which render-frame-driven effects cg_fxFPS resamples: 0 continuous projectile/trail EFX only, 1 also eligible stock frame-driven effects such as saber/world sparks and wall marks.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_fxfpsscope(app)), |app, value| App::cvar_set_cg_fxfpsscope(app, APP_CG_FXFPSSCOPE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_fxfpsscope, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_fps_scope = normalize_fx_fps_scope(value, settings.fx_fps_scope);
                }),
                archive: Some((170, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_fps_scope)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fxfpsscope(&self) -> String {
                    self.video.fx_fps_scope.to_string()
                }
                fn cvar_set_cg_fxfpsscope(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "cg_fxFPSScope expects 0..1".to_owned())?;
                        self.video.fx_fps_scope = requested.min(crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN);
                        self.apply_fx_fps_settings();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status =
                            if self.video.fx_fps_scope == crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN {
                                "FX FPS SCOPE: ALL FRAME-DRIVEN FX".into()
                            } else {
                                "FX FPS SCOPE: CONTINUOUS EFX".into()
                            };
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_FX_COUNTSCALE {
        runtime: CvarDef::new("fx_countScale", "", "0..1", "Stock OpenJK EFX count scale: multiplies authored count ranges wider than 1. Never scales upward; every primitive keeps at least one spawn.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_fx_countscale(app)), |app, value| App::cvar_set_fx_countscale(app, APP_FX_COUNTSCALE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_fx_countscale, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_count_scale = normalize_fx_count_scale(value, settings.fx_count_scale);
                }),
                archive: Some((173, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_count_scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_fx_countscale(&self) -> String {
                    self.video.fx_count_scale.to_string()
                }
                fn cvar_set_fx_countscale(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "fx_countScale expects 0..1".to_owned())?;
                        self.video.fx_count_scale =
                            config::normalize_fx_count_scale(value, self.video.fx_count_scale);
                        self.apply_fx_lod();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!("FX COUNT SCALE: {}", self.video.fx_count_scale);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_FX_LOD {
        runtime: CvarDef::new("fx_lod", "", "0..2", "EFX level of detail, decided when an effect spawns (live particles are never thinned): 0 stock, 1 honor authored cullRange (stock OpenJK ignores it), 2 also scale Particle/Tail populations down by projected screen size.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_fx_lod(app)), |app, value| App::cvar_set_fx_lod(app, APP_FX_LOD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_fx_lod, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_lod = normalize_fx_lod(value, settings.fx_lod);
                }),
                archive: Some((172, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_lod)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_fx_lod(&self) -> String {
                    self.video.fx_lod.to_string()
                }
                fn cvar_set_fx_lod(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "fx_lod expects 0..2".to_owned())?;
                        self.video.fx_lod = config::normalize_fx_lod(value, self.video.fx_lod);
                        self.apply_fx_lod();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status =
                            format!("FX LOD: {}", crate::fx::lod_label(self.video.fx_lod));
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_FX_PHYSICS {
        runtime: CvarDef::new("fx_physics", "", "0..3", "FX particle physics (TaystJK): 0 off, 1 same as 0 (stock has no non-expensive collision), 2 trace primitives flagged expensivePhysics, 3 force the world trace on every physics primitive.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_fx_physics(app)), |app, value| App::cvar_set_fx_physics(app, APP_FX_PHYSICS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_fx_physics, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_physics = normalize_fx_physics(value, settings.fx_physics);
                }),
                archive: Some((171, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_physics)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_fx_physics(&self) -> String {
                    self.video.fx_physics.to_string()
                }
                fn cvar_set_fx_physics(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "fx_physics expects 0..3".to_owned())?;
                        self.video.fx_physics = config::normalize_fx_physics(value, self.video.fx_physics);
                        self.apply_fx_physics();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "FX PHYSICS: {}",
                            crate::fx::physics_label(self.video.fx_physics)
                        );
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FXGEOMETRY {
        runtime: CvarDef::new("r_fxGeometry", "", "cpu|workers|gpu", "EFX geometry submission path: reference CPU tessellation, threaded CPU tessellation, or GPU-instanced sprites, saber glows and spark lines/tails. This is geometry only, not GPU simulation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_fxgeometry(app)), |app, value| App::cvar_set_r_fxgeometry(app, APP_R_FXGEOMETRY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_fxgeometry, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_geometry =
                        FxGeometryMode::from_config(value).unwrap_or(settings.fx_geometry);
                }),
                archive: Some((176, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_geometry.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fxgeometry(&self) -> String {
                    self.video.fx_geometry.config_value().to_owned()
                }
                fn cvar_set_r_fxgeometry(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.fx_geometry = FxGeometryMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!("FX GEOMETRY: {}", self.video.fx_geometry.label());
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FXLODSCALE {
        runtime: CvarDef::new("r_fxLodScale", "", "0.1..100", "EFX distance LOD scale, like r_lodscale: multiplies every authored cullRange (stock OpenJK ignores cullRange) and shifts the adaptive density curve. Larger keeps more detail farther away; 1 is the raw authored range. Needs fx_lod 1 or 2.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_fxlodscale(app)), |app, value| App::cvar_set_r_fxlodscale(app, APP_R_FXLODSCALE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_fxlodscale, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_lod_scale = normalize_lod_scale(value, settings.fx_lod_scale);
                }),
                archive: Some((174, |state| {
                    let settings = state.video;
                    format!("{}", settings.fx_lod_scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fxlodscale(&self) -> String {
                    self.video.fx_lod_scale.to_string()
                }
                fn cvar_set_r_fxlodscale(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "r_fxLodScale expects a number".to_owned())?;
                        self.video.fx_lod_scale =
                            config::normalize_lod_scale(value, self.video.fx_lod_scale);
                        self.apply_fx_lod();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!("FX LOD SCALE: {}", self.video.fx_lod_scale);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FXPARTICLESIMULATION {
        runtime: CvarDef::new("r_fxParticleSimulation", "", "0..1", "Opt-in GPU-resident motion for compatible noncolliding EFX sprite primitives. Requires r_fxGeometry gpu. Other primitives retain CPU fallback; experimental until FX visual parity and frame-time A/B testing.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_fxparticlesimulation(app)), |app, value| App::cvar_set_r_fxparticlesimulation(app, APP_R_FXPARTICLESIMULATION.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_fxparticlesimulation, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.fx_particle_simulation = parse_bool(value).unwrap_or(settings.fx_particle_simulation);
            }),
            archive: Some((178, |state| format!("{}", u8::from(state.video.fx_particle_simulation)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_fxparticlesimulation(&self) -> String {
                    bool_value(self.video.fx_particle_simulation)
                }
                fn cvar_set_r_fxparticlesimulation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.fx_particle_simulation = Self::parse_cvar_bool(name, value)?;
                    self.mark_config_dirty();
                    self.publish_ui();
                    self.console_status = format!("GPU RESIDENT FX: {} (requires r_fxGeometry gpu)",
                        if self.video.fx_particle_simulation { "ON" } else { "OFF" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_FXOCCLUSION {
        runtime: CvarDef::new("r_fxOcclusion", "", "0..1 (2 reserved)", "FX raster A/B: 0 original tessellation, 1 stationary map fx_runner groups with live descendant bounds, rear-plane rejection only. GPU Hi-Z wall-occlusion mode 2 is NOT implemented and is rejected. Defaults 0.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.fx_occlusion.to_string()),
            |app, value| App::cvar_set_r_fxocclusion(app, APP_R_FXOCCLUSION.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_fxocclusion, None),
        config: CvarBinding { default: Some(|state| state.video.fx_occlusion.to_string()), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_fxocclusion(&mut self, _name: &str, value: &str) -> Result<(), String> {
                let mode = value.trim().parse::<u8>().map_err(|_| "r_fxOcclusion expects 0 or 1".to_owned())?;
                if mode > 1 {
                    return Err("r_fxOcclusion 2 (GPU Hi-Z) unavailable: raster query/draw integration is not implemented yet".to_owned());
                }
                self.video.fx_occlusion = mode;
                if let Some(session) = self.game_session.as_mut() {
                    session.weapon_fx.set_runner_culling(mode == 1);
                }
                self.publish_ui();
                self.console_status = format!("FX CPU OCCLUSION A/B: {}", if mode == 0 {"ORIGINAL"} else {"MAP RUNNERS (rear-plane only)"});
                Ok(())
            }
        }]
    }

    APP_R_FXZEROALPHADISCARD {
        runtime: CvarDef::new("r_fxZeroAlphaDiscard", "", "0..1", "GPU EFX sprite A/B: discard exactly-zero-alpha source fragments before fog/blending where alpha controls contribution.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_fxzeroalphadiscard(app)), |app, value| App::cvar_set_r_fxzeroalphadiscard(app, APP_R_FXZEROALPHADISCARD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_fxzeroalphadiscard, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fx_zero_alpha_discard =
                        parse_bool(value).unwrap_or(settings.fx_zero_alpha_discard);
                }),
                archive: Some((177, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.fx_zero_alpha_discard))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fxzeroalphadiscard(&self) -> String {
                    bool_value(self.video.fx_zero_alpha_discard)
                }
                fn cvar_set_r_fxzeroalphadiscard(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.fx_zero_alpha_discard = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetFxZeroAlphaDiscard(
                            self.video.fx_zero_alpha_discard,
                        ));
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "FX ZERO-ALPHA DISCARD: {}",
                            if self.video.fx_zero_alpha_discard {
                                "ON"
                            } else {
                                "OFF"
                            }
                        );
                    }
                    Ok(())
                }
            }
        ]
    }
}
