//! Renderer cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;
use crate::ui::InlineTransformMode;

// All runtime definitions in this registry are App cvars. Fix the state type
// before closure inference for setters and dynamic cvar bindings.
type CvarDef = crate::console::cvars::CvarDef<App>;

app_cvars! {
    APP_R_DEBUGWORLDVERTEXALPHAONE {
        runtime: CvarDef::new("r_debugWorldVertexAlphaOne", "", "0..1", "A/B shader test: force BSP world vertex alpha to 1. Original vertex data is restored by setting 0. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_debugworldvertexalphaone(app)), |app, value| App::cvar_set_r_debugworldvertexalphaone(app, APP_R_DEBUGWORLDVERTEXALPHAONE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_debugworldvertexalphaone, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_get_r_debugworldvertexalphaone(&self) -> String {
                    bool_value(self.video.debug_world_vertex_alpha_one)
                }
                fn cvar_set_r_debugworldvertexalphaone(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.video.debug_world_vertex_alpha_one = enabled;
                    self.render_command(RenderCommand::SetDebugWorldVertexAlphaOne(enabled));
                    self.console_status = format!("BSP VERTEX ALPHA A/B: {}", if enabled { "FORCED 1.0" } else { "AUTHORED" });
                    Ok(())
                }
            }
        ]
    }

    APP_DEVELOPER {
        runtime: CvarDef::new("developer", "", "0..3", "Global diagnostic level: 0 quiet, 1 basic lifecycle, 2 verbose per-entity/job output, 3 trace. Levels above 0 also enable developer-only inspector/overlay tools.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_developer(app)), |app, value| App::cvar_set_developer(app, APP_DEVELOPER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_developer, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    let level = value
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .map(|level| level.min(3))
                        .or_else(|| parse_bool(value).map(u8::from))
                        .unwrap_or(settings.developer_level);
                    settings.developer_level = level;
                    settings.developer_tools = level != 0;
                }),
                archive: Some((19, |state| {
                    let settings = state.video;
                    format!("{}", settings.developer_level)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_developer(&self) -> String {
                    self.video.developer_level.to_string()
                }
                fn cvar_set_developer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let level = match Self::parse_console_bool(value) {
                            Some(enabled) => u8::from(enabled),
                            None => value
                                .trim()
                                .parse::<u8>()
                                .map_err(|_| format!("{name}: expected 0..3 (off/on are also accepted)"))?,
                        };
                        if level > 3 {
                            return Err(format!("{name}: expected 0..3"));
                        }
                        self.video.developer_level = level;
                        self.video.developer_tools = level != 0;
                        crate::logging::set_developer_level(level);
                        if !self.video.developer_tools {
                            self.forget_trace();
                            self.render_command(RenderCommand::ClearSurfaceInspection);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_FS_ALLOWASSETOVERRIDES {
        runtime: CvarDef::new("fs_allowAssetOverrides", "", "0..1", "Allow higher-priority addon/loose assets to replace qpaths supplied by retail assets0.pk3 through assets3.pk3. Active .mtr dependencies remain source-affine.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart, |app| CvarValue::String(App::cvar_get_fs_allowassetoverrides(app)), |app, value| App::cvar_set_fs_allowassetoverrides(app, APP_FS_ALLOWASSETOVERRIDES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_fs_allowassetoverrides, Some(|name, value| { App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string()) })),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.allow_asset_overrides =
                            parse_bool(value).unwrap_or(settings.allow_asset_overrides)
                    }
                }),
                apply_latched: Some(|settings, value| {
                    settings.allow_asset_overrides =
                        App::parse_console_bool(value).unwrap_or(settings.allow_asset_overrides)
                }),
                archive: Some((137, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.allow_asset_overrides))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_fs_allowassetoverrides(&self) -> String {
                    bool_value(self.video.allow_asset_overrides)
                }
                fn cvar_set_fs_allowassetoverrides(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.allow_asset_overrides = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_STAGECOLLAPSEEXTRA {
        runtime: CvarDef::new(
            "r_stageCollapseExtra", "", "0..1",
            "Experimental OpenJK reverse lightmap modulation fold. Live cvar: rebuilds GPU world batches from the cached source (no BSP/asset reparse); may briefly stall during upload.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(App::cvar_get_r_stagecollapseextra(app)),
            |app, value| App::cvar_set_r_stagecollapseextra(app, APP_R_STAGECOLLAPSEEXTRA.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_stagecollapseextra, Some(|name, value| {
            App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string())
        })),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.stage_collapse_extra = parse_bool(value).unwrap_or(settings.stage_collapse_extra);
            }),
            archive: Some((422, |state| {
                format!("{}", u8::from(state.video.stage_collapse_extra))
            })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_stagecollapseextra(&self) -> String {
                    bool_value(self.video.stage_collapse_extra)
                }
                fn cvar_set_r_stagecollapseextra(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if enabled == self.video.stage_collapse_extra {
                        return Ok(());
                    }
                    self.video.stage_collapse_extra = enabled;
                    let has_candidates = self.prepared_map_cache.as_ref().is_some_and(|cache| {
                        cache.map.batches.iter()
                            .chain(cache.map.pvs_batches.iter())
                            .chain(cache.map.inline_batches.iter())
                            .any(|batch| batch.reverse_lightmap_fold_role == 1)
                    });
                    // Avoid tearing down any GPU state when this map cannot
                    // possibly benefit. Remapped materials need the normal
                    // rebuild because their candidates are derived separately.
                    if !has_candidates && self.shader_remaps.is_empty() && self.loading.is_none() {
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "EXTRA SHADER STAGE COLLAPSE: {} (no eligible authored pairs in cache)",
                            if enabled { "ON" } else { "OFF" },
                        );
                        return Ok(());
                    }
                    // Use the established R_RemapShader presentation-only
                    // swap. If a map upload is in flight, coalesce changes and
                    // rebuild once it completes. Never restart gameplay.
                    if let Err(error) = self.rebuild_shader_remapped_world() {
                        self.video.stage_collapse_extra = !enabled;
                        return Err(format!("{name}: {error}"));
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    self.console_status = format!(
                        "EXTRA SHADER STAGE COLLAPSE: {} (cached render-world swap)",
                        if enabled { "ON" } else { "OFF" },
                    );
                    Ok(())
                }
            }
        ]
    }

    APP_R_ADDITIVESTAGEFUSION {
        runtime: CvarDef::new(
            "r_additiveStageFusion", "", "0..1",
            "A/B: fuse equivalent consecutive additive BSP shader stages into one draw. Default 1. Live toggle reuploads the cached render world (may briefly stall); no map reparse.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.additive_stage_fusion)),
            |app, value| App::cvar_set_r_additivestagefusion(app, APP_R_ADDITIVESTAGEFUSION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_additivestagefusion, Some(|name, value| {
            App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string())
        })),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.additive_stage_fusion = parse_bool(value).unwrap_or(settings.additive_stage_fusion);
            }),
            archive: Some((427, |state| format!("{}", u8::from(state.video.additive_stage_fusion)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_additivestagefusion(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if enabled == self.video.additive_stage_fusion {
                        return Ok(());
                    }
                    self.video.additive_stage_fusion = enabled;
                    if let Err(error) = self.rebuild_shader_remapped_world() {
                        self.video.additive_stage_fusion = !enabled;
                        return Err(format!("{name}: {error}"));
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    self.console_status = format!(
                        "ADDITIVE STAGE FUSION: {} (cached render-world swap)",
                        if enabled { "ON" } else { "OFF" },
                    );
                    Ok(())
                }
            }
        ]
    }

    APP_R_GPUADAPTER {
        runtime: CvarDef::new(
            "r_gpuAdapter", "", "0..3", "Graphics processor for Unified: 0 automatic, 1 dedicated, 2 integrated/APU, 3 CPU/software. Requires vid_restart.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::VidRestart,
            |app: &App| CvarValue::String(app.video.gpu_adapter.to_string()),
            |app, value| App::cvar_set_r_gpuadapter(app, APP_R_GPUADAPTER.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_gpuadapter, Some(|name, value| {
            crate::renderer::gpu_adapter::parse(value)
                .map(|v| v.to_string()).ok_or_else(|| format!("{name}: expected 0..3 (auto/dedicated/integrated/software)"))
        })),
        config: CvarBinding {
            load_video: Some(|video, _name, value, _context| {
                if let Some(mode) = crate::renderer::gpu_adapter::parse(value) { video.gpu_adapter = mode; }
            }),
            apply_latched: Some(|video, value| {
                if let Some(mode) = crate::renderer::gpu_adapter::parse(value) { video.gpu_adapter = mode; }
            }),
            archive: Some((465, |state| state.video.gpu_adapter.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_gpuadapter(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.gpu_adapter = crate::renderer::gpu_adapter::parse(value)
                        .ok_or_else(|| format!("{name}: expected 0..3"))?;
                    self.mark_config_dirty();
                    self.console_status = format!("GRAPHICS PROCESSOR: {} (APPLY VIDEO SETTINGS OR VID_RESTART)",
                        crate::renderer::gpu_adapter::label(self.video.gpu_adapter));
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BACKEND {
        runtime: CvarDef::new(
                "r_backend",
                "",
                "vulkan|dx12",
                "Graphics backend selected for the next vid_restart.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::VidRestart,
                |app| CvarValue::String(App::cvar_get_r_backend(app)),
                |app, value| App::cvar_set_r_backend(app, APP_R_BACKEND.entry.name, &value.to_string()),
            )
            .with_text_setter(
                App::cvar_set_r_backend,
                Some(|name, value| {
                    RendererBackend::from_config(value)
                        .map(|backend| backend.config_value().to_owned())
                        .ok_or_else(|| format!("{name}: expected vulkan or dx12"))
                }),
            ),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.renderer_backend =
                            RendererBackend::from_config(value).unwrap_or(settings.renderer_backend)
                    }
                }),
                apply_latched: Some(|settings, value| {
                    if let Some(backend) = RendererBackend::from_config(value) {
                        settings.renderer_backend = backend;
                    }
                }),
                archive: Some((7, |state| {
                    let settings = state.video;
                    format!("{}", settings.renderer_backend.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_backend(&self) -> String {
                    self.video.renderer_backend.config_value().to_owned()
                }
                fn cvar_set_r_backend(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.renderer_backend = RendererBackend::from_config(value)
                            .ok_or_else(|| format!("{name}: expected vulkan or dx12"))?;
                        self.mark_config_dirty();
                        self.console_status = format!(
                            "RENDER BACKEND: {} (RUN VID_RESTART TO APPLY)",
                            self.video.renderer_backend.label()
                        );
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CASCADEDSHADOWS {
        runtime: CvarDef::new(
                "r_cascadedShadows",
                "",
                "0..1",
                "Legacy boolean alias for Dynamic Shadows: Cascaded Shadow Maps.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_cascadedshadows(app)),
                |app, value| {
                    App::cvar_set_r_cascadedshadows(
                        app,
                        APP_R_CASCADEDSHADOWS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_cascadedshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.cascaded_shadows = parse_bool(value).unwrap_or(settings.cascaded_shadows)
                    }
                }),
                archive: Some((145, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.cascaded_shadows))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_cascadedshadows(&self) -> String {
                    bool_value(self.video.cascaded_shadows)
                }
                fn cvar_set_r_cascadedshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.cascaded_shadows = Self::parse_cvar_bool(name, value)?;
                        self.video.dynamic_shadows = if self.video.cascaded_shadows {
                            DynamicShadowsMode::CascadedShadowMaps
                        } else {
                            DynamicShadowsMode::Off
                        };
                        self.sync_cascaded_shadows();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CUSTOMHEIGHT {
        runtime: CvarDef::new(
                "r_customheight",
                "",
                "240..",
                "Windowed render height.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::VidRestart,
                |app| CvarValue::String(App::cvar_get_r_customheight(app)),
                |app, value| {
                    App::cvar_set_r_customheight(app, APP_R_CUSTOMHEIGHT.entry.name, &value.to_string())
                },
            )
            .with_text_setter(
                App::cvar_set_r_customheight,
                Some(|name, value| {
                    value
                        .trim()
                        .parse::<u32>()
                        .map(|pixels| pixels.max(240).to_string())
                        .map_err(|_| format!("{name}: expected an integer pixel size"))
                }),
            ),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    context.custom_height = value.parse::<u32>().ok().filter(|v| *v >= 240)
                }),
                apply_latched: Some(|settings, value| {
                    if let Ok(height) = value.parse::<u32>() {
                        settings.resolution[1] = height.max(240);
                    }
                }),
                archive: Some((2, |state| {
                    let settings = state.video;
                    format!("{}", settings.resolution[1])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_customheight(&self) -> String {
                    self.video.resolution[1].to_string()
                }
                fn cvar_set_r_customheight(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_customheight".to_owned();
                    {
                        let parsed = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected an integer pixel size"))?;
                        if lower == "r_customwidth" {
                            self.video.resolution[0] = parsed.max(320);
                        } else {
                            self.video.resolution[1] = parsed.max(240);
                        }
                        self.mark_config_dirty();
                        self.console_status = format!(
                            "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                            self.video.resolution[0], self.video.resolution[1]
                        );
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_CUSTOMWIDTH {
        runtime: CvarDef::new(
                "r_customwidth",
                "",
                "320..",
                "Windowed render width.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::VidRestart,
                |app| CvarValue::String(App::cvar_get_r_customwidth(app)),
                |app, value| {
                    App::cvar_set_r_customwidth(app, APP_R_CUSTOMWIDTH.entry.name, &value.to_string())
                },
            )
            .with_text_setter(
                App::cvar_set_r_customwidth,
                Some(|name, value| {
                    value
                        .trim()
                        .parse::<u32>()
                        .map(|pixels| pixels.max(320).to_string())
                        .map_err(|_| format!("{name}: expected an integer pixel size"))
                }),
            ),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    context.custom_width = value.parse::<u32>().ok().filter(|v| *v >= 320)
                }),
                apply_latched: Some(|settings, value| {
                    if let Ok(width) = value.parse::<u32>() {
                        settings.resolution[0] = width.max(320);
                    }
                }),
                archive: Some((1, |state| {
                    let settings = state.video;
                    format!("{}", settings.resolution[0])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_customwidth(&self) -> String {
                    self.video.resolution[0].to_string()
                }
                fn cvar_set_r_customwidth(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_customwidth".to_owned();
                    {
                        let parsed = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected an integer pixel size"))?;
                        if lower == "r_customwidth" {
                            self.video.resolution[0] = parsed.max(320);
                        } else {
                            self.video.resolution[1] = parsed.max(240);
                        }
                        self.mark_config_dirty();
                        self.console_status = format!(
                            "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                            self.video.resolution[0], self.video.resolution[1]
                        );
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DELUXEMAPPING {
        runtime: CvarDef::new(
                "r_deluxeMapping",
                "",
                "0..1",
                "Use q3map2/Rend2 directional lightmaps when present, with BSP lightgrid fallback.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_deluxemapping(app)),
                |app, value| {
                    App::cvar_set_r_deluxemapping(app, APP_R_DELUXEMAPPING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_deluxemapping, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.deluxe_mapping = parse_bool(value).unwrap_or(settings.deluxe_mapping)
                    }
                }),
                archive: Some((139, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.deluxe_mapping))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_deluxemapping(&self) -> String {
                    bool_value(self.video.deluxe_mapping)
                }
                fn cvar_set_r_deluxemapping(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.deluxe_mapping = Self::parse_cvar_bool(name, value)?;
                        self.sync_pbr();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DELUXESPECULAR {
        runtime: CvarDef::new(
                "r_deluxeSpecular",
                "",
                "0..1",
                "Scale specular response from directional baked lighting.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_deluxespecular(app)),
                |app, value| {
                    App::cvar_set_r_deluxespecular(app, APP_R_DELUXESPECULAR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_deluxespecular, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.deluxe_specular = value
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(0.0, 1.0))
                        .unwrap_or(settings.deluxe_specular);
                }),
                archive: Some((140, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.deluxe_specular)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_deluxespecular(&self) -> String {
                    format!("{:.3}", self.video.deluxe_specular)
                }
                fn cvar_set_r_deluxespecular(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let scale = value
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        if !scale.is_finite() {
                            return Err(format!("{name}: expected 0..1"));
                        }
                        self.video.deluxe_specular = scale.clamp(0.0, 1.0);
                        self.sync_pbr();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DEPTHOFFIELD {
        runtime: CvarDef::new(
                "r_depthOfField",
                "",
                "0..1",
                "Gaussian depth-of-field strength with crosshair autofocus.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_depthoffield(app)),
                |app, value| {
                    App::cvar_set_r_depthoffield(app, APP_R_DEPTHOFFIELD.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_depthoffield, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.depth_of_field_strength = value
                            .parse::<f32>()
                            .ok()
                            .filter(|strength| strength.is_finite())
                            .map(|strength| strength.clamp(0.0, 1.0))
                            .unwrap_or(settings.depth_of_field_strength);
                        // Legacy configs predate r_dofMode. A nonzero strength
                        // was equivalent to On, so maintain that behavior.
                        if settings.depth_of_field_strength > 0.001 && settings.dof_mode == DofMode::Off {
                            settings.dof_mode = DofMode::On;
                        }
                    }
                }),
                archive: Some((112, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.depth_of_field_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_depthoffield(&self) -> String {
                    format!("{:.3}", self.video.depth_of_field_strength)
                }
                fn cvar_set_r_depthoffield(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let strength = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        self.set_depth_of_field_strength(strength);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DISTANCECULLSCALE {
        runtime: CvarDef::new(
                "r_distanceCullScale",
                "",
                "0..10",
                "0 uses the map/default distanceCull; nonzero values scale that distance.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_distancecullscale(app)),
                |app, value| {
                    App::cvar_set_r_distancecullscale(
                        app,
                        APP_R_DISTANCECULLSCALE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_distancecullscale, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(scale) = value.parse::<f32>() {
                        if scale.is_finite() {
                            settings.distance_cull_scale = scale.clamp(0.0, MAX_DISTANCE_CULL_SCALE);
                        }
                    }
                }),
                archive: Some((63, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.distance_cull_scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_distancecullscale(&self) -> String {
                    format!("{:.3}", self.video.distance_cull_scale)
                }
                fn cvar_set_r_distancecullscale(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let scale = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.set_distance_cull_scale(scale);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DRAWCLIPBRUSHES {
        runtime: CvarDef::new("r_drawClipBrushes", "", "0..1", "Draw clip-only brushes (player clip orange, shot clip magenta, monster/bot clip yellow). Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_drawclipbrushes(app)),
         |app, value| App::cvar_set_r_drawclipbrushes(app, APP_R_DRAWCLIPBRUSHES.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_drawclipbrushes, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.draw_clip_brushes)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_drawclipbrushes(&self) -> String {
                    bool_value(self.video.draw_clip_brushes)
                }
                fn cvar_set_r_drawclipbrushes(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.draw_clip_brushes = Self::parse_cvar_bool(name, value)?;
                        self.sync_debug_volumes();
                        self.console_status = format!(
                            "DRAW CLIP BRUSHES: {}",
                            if self.video.draw_clip_brushes {
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

    APP_R_DRAWENTITIES {
        runtime: CvarDef::new("r_drawEntities", "", "0..1", "NetRadiant-style 3D overlay: a colored box and classname label above every map entity (same data as the `entities` command), colored by category, with lines to its target/targetname links. Brush movers track their live position when networked. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_drawentities(app)),
         |app, value| App::cvar_set_r_drawentities(app, APP_R_DRAWENTITIES.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_drawentities, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.draw_entities)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_drawentities(&self) -> String {
                    bool_value(self.video.draw_entities)
                }
                fn cvar_set_r_drawentities(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.set_draw_entities(Self::parse_cvar_bool(name, value)?);
                        self.console_status = format!(
                            "DRAW ENTITIES: {}",
                            if self.video.draw_entities {
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

    APP_R_DRAWMAPMODELS {
        runtime: CvarDef::new("r_drawMapModels", "", "0..1", "Draw server-placed MD3 map models (misc_model_* props). Brush models and geometry compiled into the BSP are unaffected.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_drawmapmodels(app)), |app, value| App::cvar_set_r_drawmapmodels(app, APP_R_DRAWMAPMODELS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_drawmapmodels, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.draw_map_models = parse_bool(value).unwrap_or(settings.draw_map_models);
                }),
                archive: Some((178, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.draw_map_models))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_drawmapmodels(&self) -> String {
                    bool_value(self.video.draw_map_models)
                }
                fn cvar_set_r_drawmapmodels(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.draw_map_models = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "MAP MODELS: {}",
                            if self.video.draw_map_models {
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

    APP_R_DRAWINLINEMODELS {
        runtime: CvarDef::new("r_drawInlineModels", "", "0..1", "Render-only A/B: draw server-controlled BSP inline movers (doors, lifts, func_rotating). Does not change physics or static world.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.draw_inline_models)),
            |app, value| App::cvar_set_r_drawinlinemodels(app, APP_R_DRAWINLINEMODELS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_drawinlinemodels, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.draw_inline_models = parse_bool(value).unwrap_or(settings.draw_inline_models);
            }),
            archive: Some((425, |state| format!("{}", u8::from(state.video.draw_inline_models)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_drawinlinemodels(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if self.video.draw_inline_models != enabled {
                        self.video.draw_inline_models = enabled;
                        self.render_command(RenderCommand::SetDrawInlineModels(enabled));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    self.console_status = format!("INLINE BSP MODELS: {}", if enabled { "ON" } else { "OFF" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_INLINETRANSFORM {
        runtime: CvarDef::new("r_inlineTransform", "", "cpu|workers|gpu", "Inline BSP mover transforms: scalar CPU reference, CPU worker pool, or GPU compute (default). Applies immediately; compare pre.inline CPU time and gpu_pass_inline_transform_ms in captures.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.inline_transform_mode.config_value().to_owned()),
            |app, value| App::cvar_set_r_inlinetransform(app, APP_R_INLINETRANSFORM.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_inlinetransform, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.inline_transform_mode = InlineTransformMode::from_config(value)
                    .unwrap_or(settings.inline_transform_mode);
            }),
            archive: Some((431, |state| state.video.inline_transform_mode.config_value().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_inlinetransform(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = InlineTransformMode::from_config(value)
                        .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                    if self.video.inline_transform_mode != mode {
                        self.video.inline_transform_mode = mode;
                        self.render_command(RenderCommand::SetInlineTransformMode(mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    self.console_status = format!("INLINE MOVER TRANSFORMS: {}", mode.label());
                    Ok(())
                }
            }
        ]
    }

    APP_R_INLINEGPUTRANSFORM {
        runtime: CvarDef::new("r_inlineGpuTransform", "", "0..1", "Compatibility A/B switch: 0 selects CPU, 1 selects GPU. Use r_inlineTransform cpu|workers|gpu for all three modes.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.inline_transform_mode == InlineTransformMode::Gpu)),
            |app, value| App::cvar_set_r_inlinegputransform(app, APP_R_INLINEGPUTRANSFORM.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_inlinegputransform, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_set_r_inlinegputransform(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    let mode = if enabled { "gpu" } else { "cpu" };
                    self.cvar_set_r_inlinetransform(name, mode)
                }
            }
        ]
    }

    APP_R_WORLDSORTMODE {
        runtime: CvarDef::new("r_worldSortMode", "", "0|1", "Session A/B: 0 authored-sort baseline; 1 additionally group independent opaque/masked stages by their ACTUAL compiled pipeline inside the same authored shader sort bucket. Never reorders blended, sky, depth-equal, multi-stage, mover, water, mirror or ocean stages. Sort estimates appear in renderperf captures and JKA PERF logs.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.world_sort_mode.to_string()),
            |app, value| App::cvar_set_r_worldsortmode(app, APP_R_WORLDSORTMODE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_worldsortmode, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_set_r_worldsortmode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .ok().filter(|&mode| mode <= 1)
                        .ok_or_else(|| format!("{name}: expected 0 or 1"))?;
                    self.video.world_sort_mode = mode;
                    self.render_command(RenderCommand::SetWorldSortMode(mode));
                    self.console_status = format!("WORLD SORT: {}", if mode == 0 { "AUTHORED SORT" } else { "AUTHORED + SAFE PIPELINE" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_POSTSPECIALIZE {
        runtime: CvarDef::new("r_postSpecialize", "", "0|1", "Session A/B: 1 (default) draws full post with a pipeline compiled for the effects currently enabled, so disabled effects cost no registers; 0 always uses the general post pipeline. Same image either way.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(u8::from(app.video.post_specialize).to_string()),
            |app, value| App::cvar_set_r_postspecialize(app, APP_R_POSTSPECIALIZE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_postspecialize, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_set_r_postspecialize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = match value.trim() {
                        "0" => false,
                        "1" => true,
                        _ => return Err(format!("{name}: expected 0 or 1")),
                    };
                    self.video.post_specialize = enabled;
                    self.render_command(RenderCommand::SetPostSpecialize(enabled));
                    self.console_status = format!("POST SPECIALIZE: {}", if enabled { "ON" } else { "OFF" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_FRAGMENTSHADERMODE {
        runtime: CvarDef::new("r_fragmentShaderMode", "", "0..4", "World shaders: 0 general; 1 material specialization; 2 specialization plus minimal entries; 3 shared basic/enhanced families; 4 generated minimal vertex/fragment pairs per encountered stage recipe. Modes 3/4 target Minimal Unified, Low and Medium, with reported fallbacks for other features. Switches after background compilation; quality settings are preserved.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.fragment_shader_mode.to_string()),
            |app, value| App::cvar_set_r_fragmentshadermode(app, APP_R_FRAGMENTSHADERMODE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_fragmentshadermode, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.fragment_shader_mode = mode.min(4);
                }
            }),
            archive: Some((428, |state| format!("{}", state.video.fragment_shader_mode))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_fragmentshadermode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .filter(|mode| *mode <= 4)
                        .ok_or_else(|| format!("{name}: expected 0..4"))?;
                    if self.video.fragment_shader_mode != mode {
                        self.video.fragment_shader_mode = mode;
                        self.render_command(RenderCommand::SetFragmentShaderMode(mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    self.console_status = format!("FRAGMENT SHADERS: {}", match mode {
                        1 => "TAYSTJK SPECIALIZED",
                        2 => "SPECIALIZED + MINIMAL ENTRIES",
                        3 => "SHARED FAMILIES",
                        4 => "GENERATED STAGE PROGRAMS",
                        _ => "GENERAL",
                    });
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORCLEARMODE {
        runtime: CvarDef::new("r_colorClearMode", "", "0..1", "World colour clear: 0 clear every frame, 1 skip it when every sky on the map draws a sky box or the map has no sky (vanilla/TaystJK behaviour; holes into the void then show the previous frame). Fastsky always clears.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.color_clear_mode.to_string()),
            |app, value| App::cvar_set_r_colorclearmode(app, APP_R_COLORCLEARMODE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_colorclearmode, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.color_clear_mode = mode.min(1);
                }
            }),
            archive: Some((429, |state| format!("{}", state.video.color_clear_mode))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_colorclearmode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .filter(|mode| *mode <= 1)
                        .ok_or_else(|| format!("{name}: expected 0 or 1"))?;
                    if self.video.color_clear_mode != mode {
                        self.video.color_clear_mode = mode;
                        self.render_command(RenderCommand::SetColorClearMode(mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    self.console_status = format!("WORLD COLOUR CLEAR: {}", if mode == 1 { "SKIP UNDER SKY BOX" } else { "ALWAYS" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_DRAWBSPMODELSURFACES {
        runtime: CvarDef::new("r_drawBspModelSurfaces", "", "0..1", "Render-only heuristic: draw static BSP surfaces whose original shader name begins models/. Not exact model provenance; may hide map geometry using models/ materials.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.draw_bsp_model_surfaces)),
            |app, value| App::cvar_set_draw_bsp_model_surfaces(app, APP_R_DRAWBSPMODELSURFACES.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_draw_bsp_model_surfaces, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.draw_bsp_model_surfaces = parse_bool(value).unwrap_or(settings.draw_bsp_model_surfaces);
            }),
            archive: Some((426, |state| format!("{}", u8::from(state.video.draw_bsp_model_surfaces)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_draw_bsp_model_surfaces(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.video.draw_bsp_model_surfaces = enabled;
                    self.render_command(RenderCommand::SetWorldStageDebug {
                        draw_bsp_model_surfaces: enabled,
                        probe: self.video.world_stage_probe,
                    });
                    self.mark_config_dirty();
                    self.publish_ui();
                    self.console_status = format!("BUILT-IN BSP MODEL-PATH SURFACES: {}", if enabled { "ON" } else { "OFF" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_FASTSKY {
        runtime: CvarDef::new("r_fastsky", "", "0..10", "TaystJK fast sky: 0 authored skybox/cloud stages; 1 black; 2 green; 3 yellow; 4 blue; 5 cyan; 6 magenta; 7 white; 8 orange; 9 grey; 10 red. Skips sky drawing in Unified, without adding a rendering pass.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.fast_sky.to_string()),
            |app, value| App::cvar_set_fast_sky(app, APP_R_FASTSKY.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_fast_sky, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.fast_sky = mode.min(10);
                }
            }),
            archive: Some((427, |state| state.video.fast_sky.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_fast_sky(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .map_err(|_| "r_fastsky expects a number from 0 through 10".to_owned())?;
                    if mode > 10 {
                        return Err("r_fastsky expects a number from 0 through 10".to_owned());
                    }
                    if self.video.fast_sky != mode {
                        self.video.fast_sky = mode;
                        self.render_command(RenderCommand::SetFastSky(mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    self.console_status = format!("FAST SKY: {mode}");
                    Ok(())
                }
            }
        ]
    }

    APP_R_WORLDSTAGEPROBE {
        runtime: CvarDef::new("r_worldStageProbe", "", "0..8", "Session-only GPU A/B: 0 full; 1 omit transparent; 2 omit opaque/masked (changes depth); 3 omit multiply; 4 omit all additive; 5 omit alpha-composite; 6 omit other blends; 7 omit additive on authored water surfaces only; 8 omit additive on non-water BSP surfaces only. Compare at the same camera. Modes 3..8 retain depth-producing stages.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.world_stage_probe.to_string()),
            |app, value| App::cvar_set_world_stage_probe(app, APP_R_WORLDSTAGEPROBE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_world_stage_probe, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_set_world_stage_probe(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>().map_err(|_| "r_worldStageProbe expects a mode from 0 through 8".to_owned())?;
                    if mode > 8 { return Err("r_worldStageProbe expects a mode from 0 through 8".to_owned()); }
                    self.video.world_stage_probe = mode;
                    self.render_command(RenderCommand::SetWorldStageDebug {
                        draw_bsp_model_surfaces: self.video.draw_bsp_model_surfaces,
                        probe: mode,
                    });
                    self.console_status = format!("WORLD STAGE GPU PROBE: {mode}");
                    Ok(())
                }
            }
        ]
    }

    APP_R_DRAWTRIGGERS {
        runtime: CvarDef::new("r_drawTriggers", "", "0..1", "Draw trigger_* volumes as colored translucent brushes (push green, teleport purple, hurt red, multiple blue, once cyan). Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_drawtriggers(app)),
         |app, value| App::cvar_set_r_drawtriggers(app, APP_R_DRAWTRIGGERS.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_drawtriggers, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.draw_triggers)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_drawtriggers(&self) -> String {
                    bool_value(self.video.draw_triggers)
                }
                fn cvar_set_r_drawtriggers(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.draw_triggers = Self::parse_cvar_bool(name, value)?;
                        self.sync_debug_volumes();
                        self.console_status = format!(
                            "DRAW TRIGGERS: {}",
                            if self.video.draw_triggers {
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

    APP_R_DYNAMICLIGHTFALLOFF {
        runtime: CvarDef::new("r_dynamicLightFalloff", "", "0..1", "Falloff of runtime dynamic lights (sabers, blasters, explosions) in forward_plus/ray_traced. 0 stock JKA 1 - d^2/r^2, 1 physical inverse-square with a smooth window that reaches zero at the radius with no visible edge.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_dynamiclightfalloff(app)), |app, value| App::cvar_set_r_dynamiclightfalloff(app, APP_R_DYNAMICLIGHTFALLOFF.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_dynamiclightfalloff, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(mode @ (0 | 1)) = value.parse::<u32>() {
                        settings.dynamic_light_falloff = mode;
                    }
                }),
                archive: Some((164, |state| {
                    let settings = state.video;
                    format!("{}", settings.dynamic_light_falloff)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamiclightfalloff(&self) -> String {
                    self.video.dynamic_light_falloff.to_string()
                }
                fn cvar_set_r_dynamiclightfalloff(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let mode = value
                            .parse::<u32>()
                            .ok()
                            .filter(|n| matches!(n, 0 | 1))
                            .ok_or_else(|| format!("{name}: expected 0|1"))?;
                        self.video.dynamic_light_falloff = mode;
                        self.render_command(RenderCommand::SetDynamicLightFalloff(mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICLIGHTS {
        runtime: CvarDef::new(
                "r_dynamicLights",
                "",
                "off|legacy|vertex|clustered_lite|forward_plus|ray_traced",
                "Dynamic-lighting technique for runtime-authored lights.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dynamiclights(app)),
                |app, value| {
                    App::cvar_set_r_dynamiclights(app, APP_R_DYNAMICLIGHTS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_dynamiclights, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(mode) = DynamicLightsMode::from_config(value) {
                        settings.dynamic_lights = mode;
                    }
                }),
                archive: Some((130, |state| {
                    let settings = state.video;
                    format!("{}", settings.dynamic_lights.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamiclights(&self) -> String {
                    self.video.dynamic_lights.config_value().to_owned()
                }
                fn cvar_set_r_dynamiclights(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.dynamic_lights = DynamicLightsMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected off|legacy|vertex|clustered_lite|forward_plus|ray_traced"))?;
                        self.sync_dynamic_lighting();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICLIGHTDEBUG {
        runtime: CvarDef::new(
                "r_dynamicLightDebug",
                "",
                "0..1",
                "Debug runtime dynamic-light contribution on neutral dark BSP receivers. Suppresses normal world shading so point/spot/segment RGB, falloff and current shadowing are easy to inspect.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dynamiclightdebug(app)),
                |app, value| App::cvar_set_r_dynamiclightdebug(app, APP_R_DYNAMICLIGHTDEBUG.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_dynamiclightdebug, None),
        config: CvarBinding {
                default: Some(|state| bool_value(state.video.dynamic_light_debug)),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamiclightdebug(&self) -> String {
                    bool_value(self.video.dynamic_light_debug)
                }
                fn cvar_set_r_dynamiclightdebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.dynamic_light_debug = Self::parse_cvar_bool(name, value)?;
                    self.render_command(RenderCommand::SetDynamicLightDebug(
                        self.video.dynamic_light_debug,
                    ));
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PROMOTEFXLIGHTS {
        runtime: CvarDef::new(
                "r_promoteFxLights",
                "",
                "0..2",
                "0 off; 1 conservative FX inference; 2 aggressive FX + live projectile light promotion. Authored Light{} is preserved.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_promotefxlights(app)),
                |app, value| {
                    App::cvar_set_r_promotefxlights(app, APP_R_PROMOTEFXLIGHTS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_promotefxlights, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.promote_fx_lights = value.trim().parse::<u8>().ok()
                        .filter(|level| *level <= 2).unwrap_or(settings.promote_fx_lights);
                }),
                archive: Some((417, |state| {
                    let settings = state.video;
                    format!("{}", settings.promote_fx_lights)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_promotefxlights(&self) -> String {
                    self.video.promote_fx_lights.to_string()
                }
                fn cvar_set_r_promotefxlights(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.promote_fx_lights = value.trim().parse::<u8>()
                        .ok().filter(|value| *value <= 2)
                        .ok_or_else(|| format!("{name}: expected 0, 1 or 2"))?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.promote_fx_lights = self.video.promote_fx_lights;
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PROMOTESHADERLIGHTS {
        runtime: CvarDef::new(
                "r_promoteShaderLights",
                "",
                "0..1",
                "Promote visible BSP shader emission into runtime area lights. Glow/additive/emissive evidence is classified even on q3map_surfacelight materials so it can be A/B tested; when authored area lights are enabled, the authored emitter wins to avoid double lighting.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_promoteshaderlights(app)),
                |app, value| App::cvar_set_r_promoteshaderlights(app, APP_R_PROMOTESHADERLIGHTS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_promoteshaderlights, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.promote_shader_lights = parse_bool(value).unwrap_or(settings.promote_shader_lights);
                }),
                archive: Some((10420, |state| format!("{}", u8::from(state.video.promote_shader_lights)))),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_promoteshaderlights(&self) -> String { bool_value(self.video.promote_shader_lights) }
                fn cvar_set_r_promoteshaderlights(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.promote_shader_lights = Self::parse_cvar_bool(name, value)?;
                    self.sync_shader_light_promotion();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PROMOTESHADERGLOW {
        runtime: CvarDef::new(
                "r_promoteShaderGlow",
                "",
                "0..1",
                "Allow shader-light promotion from authored shader-stage glow markers.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_promoteshaderglow(app)),
                |app, value| App::cvar_set_r_promoteshaderglow(app, APP_R_PROMOTESHADERGLOW.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_promoteshaderglow, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.promote_shader_glow = parse_bool(value).unwrap_or(settings.promote_shader_glow);
                }),
                archive: Some((10421, |state| format!("{}", u8::from(state.video.promote_shader_glow)))),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_promoteshaderglow(&self) -> String { bool_value(self.video.promote_shader_glow) }
                fn cvar_set_r_promoteshaderglow(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.promote_shader_glow = Self::parse_cvar_bool(name, value)?;
                    self.sync_shader_light_promotion();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PROMOTESHADERADDITIVE {
        runtime: CvarDef::new(
                "r_promoteShaderAdditive",
                "",
                "0..1",
                "Allow shader-light promotion from visible additive GL_ONE GL_ONE or SRC_ALPHA ONE stages.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_promoteshaderadditive(app)),
                |app, value| App::cvar_set_r_promoteshaderadditive(app, APP_R_PROMOTESHADERADDITIVE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_promoteshaderadditive, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.promote_shader_additive = parse_bool(value).unwrap_or(settings.promote_shader_additive);
                }),
                archive: Some((10422, |state| format!("{}", u8::from(state.video.promote_shader_additive)))),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_promoteshaderadditive(&self) -> String { bool_value(self.video.promote_shader_additive) }
                fn cvar_set_r_promoteshaderadditive(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.promote_shader_additive = Self::parse_cvar_bool(name, value)?;
                    self.sync_shader_light_promotion();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PROMOTESHADEREMISSIVE {
        runtime: CvarDef::new(
                "r_promoteShaderEmissive",
                "",
                "0..1",
                "Allow shader-light promotion from modern _emissive/_emission/_emit/_glow companion textures.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_promoteshaderemissive(app)),
                |app, value| App::cvar_set_r_promoteshaderemissive(app, APP_R_PROMOTESHADEREMISSIVE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_promoteshaderemissive, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.promote_shader_emissive = parse_bool(value).unwrap_or(settings.promote_shader_emissive);
                }),
                archive: Some((10423, |state| format!("{}", u8::from(state.video.promote_shader_emissive)))),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_promoteshaderemissive(&self) -> String { bool_value(self.video.promote_shader_emissive) }
                fn cvar_set_r_promoteshaderemissive(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.promote_shader_emissive = Self::parse_cvar_bool(name, value)?;
                    self.sync_shader_light_promotion();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICSHADOWS {
        runtime: CvarDef::new("r_dynamicShadows", "", "off|blob|entity|csm|ray_traced", "Dynamic-shadow mode; entity = entity-only shadow map aimed by the baked lightgrid; csm = Bevy-style cascaded shadow maps.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_dynamicshadows(app)), |app, value| App::cvar_set_r_dynamicshadows(app, APP_R_DYNAMICSHADOWS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_dynamicshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, context| {
                    if let Some(mode) = DynamicShadowsMode::from_config(value) {
                        settings.dynamic_shadows = mode;
                        context.dynamic_shadows_mode_seen = true;
                    }
                }),
                archive: Some((141, |state| {
                    let settings = state.video;
                    format!("{}", settings.dynamic_shadows.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamicshadows(&self) -> String {
                    self.video.dynamic_shadows.config_value().to_owned()
                }
                fn cvar_set_r_dynamicshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.dynamic_shadows = DynamicShadowsMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected off|blob|entity|csm|ray_traced"))?;
                        self.video.cascaded_shadows =
                            self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps;
                        self.sync_cascaded_shadows();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_EMISSIVEAREALIGHTS {
        runtime: CvarDef::new(
                "r_emissiveAreaLights",
                "",
                "0..1",
                "Enable mapper-authored q3map_surfacelight surfaces as runtime area lights. Shader promotion is controlled separately by r_promoteShaderLights.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_emissivearealights(app)),
                |app, value| {
                    App::cvar_set_r_emissivearealights(
                        app,
                        APP_R_EMISSIVEAREALIGHTS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_emissivearealights, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.emissive_area_lights =
                            parse_bool(value).unwrap_or(settings.emissive_area_lights)
                    }
                }),
                archive: Some((142, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.emissive_area_lights))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_emissivearealights(&self) -> String {
                    bool_value(self.video.emissive_area_lights)
                }
                fn cvar_set_r_emissivearealights(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.emissive_area_lights = Self::parse_cvar_bool(name, value)?;
                        self.sync_emissive_area_lights();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_ENTITYAMBIENTLIGHTING {
        runtime: CvarDef::new("r_entityAmbientLighting", "", "off|bsp_lightgrid|bevy_irradiance_volume", "Entity ambient-lighting source; bsp_lightgrid uses OpenJK-style classic BSP entity lighting.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_entityambientlighting(app)), |app, value| App::cvar_set_r_entityambientlighting(app, APP_R_ENTITYAMBIENTLIGHTING.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_entityambientlighting, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.entity_ambient_lighting = EntityAmbientLightingMode::from_config(value)
                            .unwrap_or(settings.entity_ambient_lighting)
                    }
                }),
                archive: Some((129, |state| {
                    let settings = state.video;
                    format!("{}", settings.entity_ambient_lighting.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_entityambientlighting(&self) -> String {
                    {
                        self.video.entity_ambient_lighting.config_value().to_owned()
                    }
                }
                fn cvar_set_r_entityambientlighting(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.entity_ambient_lighting = EntityAmbientLightingMode::from_config(value)
                            .ok_or_else(|| {
                                format!("{name}: expected off|bsp_lightgrid|bevy_irradiance_volume")
                            })?;
                        self.sync_entity_ambient_lighting();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_ENTITYSHADOWLIGHT {
        runtime: CvarDef::new("r_entityShadowLight", "", "lightgrid|authored", "Entity-map shadow light: baked lightgrid direction, or the strongest authored map light's real position.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_entityshadowlight(app)), |app, value| App::cvar_set_r_entityshadowlight(app, APP_R_ENTITYSHADOWLIGHT.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_entityshadowlight, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.entity_shadow_light =
                            EntityShadowLight::from_config(value).unwrap_or(settings.entity_shadow_light)
                    }
                }),
                archive: Some((146, |state| {
                    let settings = state.video;
                    format!("{}", settings.entity_shadow_light.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_entityshadowlight(&self) -> String {
                    self.video.entity_shadow_light.config_value().to_owned()
                }
                fn cvar_set_r_entityshadowlight(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.entity_shadow_light = EntityShadowLight::from_config(value)
                            .ok_or_else(|| format!("{name}: expected lightgrid|authored"))?;
                        self.sync_entity_shadow_light();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDSUNREMOVAL {
        runtime: CvarDef::new("r_bakedSunRemoval", "", "0..1", "Subtract authored direct sun in BSP lightmap UVs and compiler lighting units when bake settings are known; use measured fits or conservative estimates otherwise. 0 preserves baked lighting; 1 applies the full estimate. Does not add a replacement light.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_bakedsunremoval(app)), |app, value| App::cvar_set_r_bakedsunremoval(app, APP_R_BAKEDSUNREMOVAL.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_bakedsunremoval, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_sun_removal = v.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((59, |state| format!("{:.3}", state.video.baked_sun_removal))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_bakedsunremoval(&self) -> String {
                    format!("{:.3}", self.video.baked_sun_removal)
                }
                fn cvar_set_r_bakedsunremoval(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_sun_removal = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_SUNLIGHTING {
        runtime: CvarDef::new("r_sunLighting", "", "0|1", "Add live directional sunlight to BSP lightmaps after baked-sun removal. Independent of removal strength and its cache. Use CSM or RT sun shadows for occlusion. Default 0 preserves legacy baked lighting.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(if app.video.sun_lighting { "1".into() } else { "0".into() }), |app, value| App::cvar_set_r_sunlighting(app, APP_R_SUNLIGHTING.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_sunlighting, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.sun_lighting = v; }
            }),
            archive: Some((26046, |state| if state.video.sun_lighting { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_sunlighting(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.sun_lighting = parse_bool(value).ok_or_else(|| format!("{name}: expected 0|1"))?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_SUNSHADOWSTRENGTH {
        runtime: CvarDef::new("r_sunShadowStrength", "", "0..1", "Live sun shadow contrast before surface-facing attenuation. Default 0.3 preserves the existing response; 1 strengthens CSM/RT sun shadows. Intensity scaling still applies, with final strength capped at 1. Does not change baked-sun removal.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(format!("{:.3}", app.video.sun_shadow_strength)), |app, value| App::cvar_set_r_sunshadowstrength(app, APP_R_SUNSHADOWSTRENGTH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_sunshadowstrength, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.sun_shadow_strength = v.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((26045, |state| format!("{:.3}", state.video.sun_shadow_strength))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_sunshadowstrength(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.sun_shadow_strength = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDSUNCACHE {
        runtime: CvarDef::new("r_bakedSunCache", "", "0|1", "Cache projected BSP sunlight subtraction in GPU lightmaps when the removal amount or source changes. 0 evaluates per pixel; 1 uses cached filtered lightmaps. Preview remains live. Unsupported bake formats use the shader path. Default 1.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(if app.video.baked_sun_cache { "1".into() } else { "0".into() }), |app, value| App::cvar_set_r_bakedsuncache(app, APP_R_BAKEDSUNCACHE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_bakedsuncache, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.baked_sun_cache = v; }
            }),
            archive: Some((26044, |state| if state.video.baked_sun_cache { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_bakedsuncache(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_sun_cache = parse_bool(value)
                        .ok_or_else(|| format!("{name}: expected 0|1"))?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDSUNSHADOWMAP {
        runtime: CvarDef::new("r_bakedSunShadowMap", "", "0|1", "Fallback visibility for surfaces without a measured baked-sun fit: 1 = cached original-sun shadow map; 0 = unshadowed approximation. Static BSP-only map, rebuilt on map or authored-sun change.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_bakedsunshadowmap(app)), |app, value| App::cvar_set_r_bakedsunshadowmap(app, APP_R_BAKEDSUNSHADOWMAP.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_bakedsunshadowmap, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(v) = parse_bool(value) { settings.baked_sun_shadow_map = v; }
            }),
            archive: Some((60, |state| if state.video.baked_sun_shadow_map { "1".into() } else { "0".into() })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_bakedsunshadowmap(&self) -> String {
                    if self.video.baked_sun_shadow_map { "1".into() } else { "0".into() }
                }
                fn cvar_set_r_bakedsunshadowmap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_sun_shadow_map = parse_bool(value)
                        .ok_or_else(|| format!("{name}: expected 0|1"))?;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDSUNEDGESOFTNESS {
        runtime: CvarDef::new("r_bakedSunEdgeSoftness", "", "0..24", "Fallback original-sun shadow edge softness in depth-map texels. Paired surface fits retain authored lightmap edges.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedsunedgesoftness_get(app)), |app, value| App::cvar_r_bakedsunedgesoftness_set(app, APP_R_BAKEDSUNEDGESOFTNESS.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedsunedgesoftness_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_sun_edge_softness = v.clamp(0.0, 24.0); }
                }
            }),
            archive: Some((26036, |state| format!("{:.3}", state.video.baked_sun_edge_softness))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedsunedgesoftness_get(&self) -> String {
                    format!("{:.3}", self.video.baked_sun_edge_softness)
                }
                fn cvar_r_bakedsunedgesoftness_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_sun_edge_softness = Self::parse_cvar_finite(name, value)?.clamp(0.0, 24.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDSUNDIRECTLIMIT {
        runtime: CvarDef::new("r_bakedSunDirectLimit", "", "0..0.9", "Safety cap on the fraction removed from unpaired BSP lightmap samples. Paired surface fits use their measured shadow baseline and direct-light bound.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedsundirectlimit_get(app)), |app, value| App::cvar_r_bakedsundirectlimit_set(app, APP_R_BAKEDSUNDIRECTLIMIT.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedsundirectlimit_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_sun_direct_limit = v.clamp(0.0, 0.9); }
                }
            }),
            archive: Some((26037, |state| format!("{:.3}", state.video.baked_sun_direct_limit))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedsundirectlimit_get(&self) -> String {
                    format!("{:.3}", self.video.baked_sun_direct_limit)
                }
                fn cvar_r_bakedsundirectlimit_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_sun_direct_limit = Self::parse_cvar_finite(name, value)?.clamp(0.0, 0.9);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDINDIRECTSCALE {
        runtime: CvarDef::new("r_bakedIndirectScale", "", "0..2", "Brightness multiplier for residual baked illumination only, separate from gamma and runtime sun.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedindirectscale_get(app)), |app, value| App::cvar_r_bakedindirectscale_set(app, APP_R_BAKEDINDIRECTSCALE.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedindirectscale_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_indirect_scale = v.clamp(0.0, 2.0); }
                }
            }),
            archive: Some((26038, |state| format!("{:.3}", state.video.baked_indirect_scale))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedindirectscale_get(&self) -> String {
                    format!("{:.3}", self.video.baked_indirect_scale)
                }
                fn cvar_r_bakedindirectscale_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_indirect_scale = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDINDIRECTSATURATION {
        runtime: CvarDef::new("r_bakedIndirectSaturation", "", "0..2", "Saturation multiplier for residual baked illumination only, not textures or dynamic sunlight.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedindirectsaturation_get(app)), |app, value| App::cvar_r_bakedindirectsaturation_set(app, APP_R_BAKEDINDIRECTSATURATION.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedindirectsaturation_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_indirect_saturation = v.clamp(0.0, 2.0); }
                }
            }),
            archive: Some((26039, |state| format!("{:.3}", state.video.baked_indirect_saturation))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedindirectsaturation_get(&self) -> String {
                    format!("{:.3}", self.video.baked_indirect_saturation)
                }
                fn cvar_r_bakedindirectsaturation_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_indirect_saturation = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }


    APP_R_BAKEDINDIRECTPOWER {
        runtime: CvarDef::new("r_bakedIndirectPower", "", "1..4", "Power curve for baked indirect linear luminance. 1 = unchanged; higher values darken dim baked illumination while preserving hue.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedindirectpower_get(app)), |app, value| App::cvar_r_bakedindirectpower_set(app, APP_R_BAKEDINDIRECTPOWER.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedindirectpower_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_indirect_power = v.clamp(1.0, 4.0); }
                }
            }),
            archive: Some((26040, |state| format!("{:.3}", state.video.baked_indirect_power))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedindirectpower_get(&self) -> String {
                    format!("{:.3}", self.video.baked_indirect_power)
                }
                fn cvar_r_bakedindirectpower_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_indirect_power = Self::parse_cvar_finite(name, value)?.clamp(1.0, 4.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_BAKEDINDIRECTHIGHLIGHTPRESERVE {
        runtime: CvarDef::new("r_bakedIndirectHighlightPreserve", "", "0..1", "Restore high-luminance baked lighting after the night power curve. 0 = none; 1 = full highlight retention.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_r_bakedindirecthighlightpreserve_get(app)), |app, value| App::cvar_r_bakedindirecthighlightpreserve_set(app, APP_R_BAKEDINDIRECTHIGHLIGHTPRESERVE.entry.name, &value.to_string())).with_text_setter(App::cvar_r_bakedindirecthighlightpreserve_set, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.baked_indirect_highlight_preserve = v.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((26041, |state| format!("{:.3}", state.video.baked_indirect_highlight_preserve))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_r_bakedindirecthighlightpreserve_get(&self) -> String {
                    format!("{:.3}", self.video.baked_indirect_highlight_preserve)
                }
                fn cvar_r_bakedindirecthighlightpreserve_set(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.baked_indirect_highlight_preserve = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_ENTITYSUNLIGHTING {
        runtime: CvarDef::new("r_entitySunLighting", "", "0..1", "Remove the baked map sun from the entity lightgrid and re-light entities with the runtime sun (needs r_entityAmbientLighting bsp_lightgrid).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_entitysunlighting(app)), |app, value| App::cvar_set_r_entitysunlighting(app, APP_R_ENTITYSUNLIGHTING.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_entitysunlighting, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.entity_sun_lighting =
                            parse_bool(value).unwrap_or(settings.entity_sun_lighting)
                    }
                }),
                archive: Some((58, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.entity_sun_lighting))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_entitysunlighting(&self) -> String {
                    bool_value(self.video.entity_sun_lighting)
                }
                fn cvar_set_r_entitysunlighting(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_entitysunlighting".to_owned();
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

    APP_R_EXT_MULTISAMPLE {
        runtime: CvarDef::new(
                "r_ext_multisample",
                "",
                "0|2|4|8...",
                "MSAA sample count; 0 means off.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ext_multisample(app)),
                |app, value| {
                    App::cvar_set_r_ext_multisample(
                        app,
                        APP_R_EXT_MULTISAMPLE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_ext_multisample, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(samples) = value.parse::<u32>() {
                        settings.msaa_samples = samples.max(1);
                    }
                }),
                archive: Some((10, |state| {
                    let settings = state.video;
                    let msaa = if settings.msaa_samples <= 1 {
                        0
                    } else {
                        settings.msaa_samples
                    };
                    format!("{}", msaa)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ext_multisample(&self) -> String {
                    {
                        if self.video.msaa_samples <= 1 {
                            "0".into()
                        } else {
                            self.video.msaa_samples.to_string()
                        }
                    }
                }
                fn cvar_set_r_ext_multisample(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let samples = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected an integer sample count"))?;
                        let samples = if samples <= 1 { 1 } else { samples };
                        if !self.supported_msaa.contains(&samples) {
                            return Err(format!(
                                "{name}: unsupported sample count {samples}; supported {:?}",
                                self.supported_msaa
                            ));
                        }
                        self.video.msaa_samples = samples;
                        if samples > 1 {
                            self.video.fxaa = false;
                            self.video.smaa = false;
                            self.video.taa = false;
                        }
                        self.render_command(RenderCommand::SetMsaa(samples));
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FILMGRAIN {
        runtime: CvarDef::new(
                "r_filmGrain",
                "",
                "0..1",
                "Film grain strength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_filmgrain(app)),
                |app, value| App::cvar_set_r_filmgrain(app, APP_R_FILMGRAIN.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_filmgrain, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.film_grain_strength = value
                            .parse::<f32>()
                            .ok()
                            .filter(|strength| strength.is_finite())
                            .map(|strength| strength.clamp(0.0, 1.0))
                            .or_else(|| parse_bool(value).map(|enabled| if enabled { 0.35 } else { 0.0 }))
                            .unwrap_or(settings.film_grain_strength)
                    }
                }),
                archive: Some((110, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.film_grain_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_filmgrain(&self) -> String {
                    format!("{:.3}", self.video.film_grain_strength)
                }
                fn cvar_set_r_filmgrain(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let strength = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        self.set_film_grain_strength(strength);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FLARES {
        runtime: CvarDef::new(
                "r_flares",
                "",
                "0..1",
                "OpenJK/TaystJK world flares (RBSP MST_FLARE); r_worldFlares can independently enable the same GPU renderer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_flares(app)),
                |app, value| App::cvar_set_r_flares(app, APP_R_FLARES.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_flares, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.flares = parse_bool(value).unwrap_or(settings.flares)
                }),
                archive: Some((133, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.flares))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_flares(&self) -> String {
                    bool_value(self.video.flares)
                }
                fn cvar_set_r_flares(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.flares = Self::parse_cvar_bool(name, value)?;
                        self.sync_world_flares();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status =
                            format!("WORLD FLARES: {}", if self.video.world_flares_active() { "ON" } else { "OFF" });
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SABERFLARES {
        runtime: CvarDef::new(
                "r_saberFlares", "1", "0..1",
                "Screen-space saber clash flare; leaves authored saber hit FX, sound and marks unchanged.",
                CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
                |app: &App| CvarValue::String(bool_value(app.video.saber_flares)),
                |app: &mut App, value: CvarValue| App::cvar_set_r_saberflares(app, APP_R_SABERFLARES.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_r_saberflares, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.saber_flares = parse_bool(value).unwrap_or(settings.saber_flares);
            }),
            archive: Some((423, |state| format!("{}", u8::from(state.video.saber_flares)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_saberflares(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.saber_flares = Self::parse_cvar_bool(name, value)?;
                    self.mark_config_dirty();
                    self.console_status = format!("SABER FLARES: {}", if self.video.saber_flares { "ON" } else { "OFF" });
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_WORLDFLARES {
        runtime: CvarDef::new(
                "r_worldFlares", "", "0..1",
                "Optional world-flare override (same GPU renderer as r_flares); does not control saber overlays.",
                CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
                |app: &App| CvarValue::String(bool_value(app.video.world_flares)),
                |app: &mut App, value: CvarValue| App::cvar_set_r_worldflares(app, APP_R_WORLDFLARES.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_r_worldflares, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.world_flares = parse_bool(value).unwrap_or(settings.world_flares);
            }),
            archive: Some((420, |state| format!("{}", u8::from(state.video.world_flares)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_worldflares(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.world_flares = Self::parse_cvar_bool(name, value)?;
                    self.sync_world_flares();
                    self.mark_config_dirty();
                    self.console_status = format!("WORLD FLARES: {}", if self.video.world_flares_active() { "ON" } else { "OFF" });
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_FLOATLIGHTMAP {
        runtime: CvarDef::new(
                "r_floatLightmap",
                "",
                "0..1",
                "Use FP16 lightmaps while HDR is enabled; prefer Rend2 .hdr companions when present.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::MapLoadOrVidRestart,
                |app| CvarValue::String(App::cvar_get_r_floatlightmap(app)),
                |app, value| {
                    App::cvar_set_r_floatlightmap(app, APP_R_FLOATLIGHTMAP.entry.name, &value.to_string())
                },
            )
            .with_text_setter(
                App::cvar_set_r_floatlightmap,
                Some(|name, value| {
                    App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string())
                }),
            ),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.float_lightmap = parse_bool(value).unwrap_or(settings.float_lightmap)
                    }
                }),
                apply_latched: Some(|settings, value| {
                    settings.float_lightmap =
                        App::parse_console_bool(value).unwrap_or(settings.float_lightmap)
                }),
                archive: Some((38, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.float_lightmap))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_floatlightmap(&self) -> String {
                    bool_value(self.video.float_lightmap)
                }
                fn cvar_set_r_floatlightmap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.float_lightmap = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        if self.map_prepare_restart_required() {
                            self.console_status =
                                "FLOAT LIGHTMAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FOOTPRINTS {
        runtime: CvarDef::new(
                "r_footprints",
                "",
                "off|2d|3d",
                "Footprints: off, stock JKA 2D mark shader, or 3D snow deformation.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_footprints(app)),
                |app, value| {
                    App::cvar_set_r_footprints(app, APP_R_FOOTPRINTS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_footprints, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.footprints =
                            FootprintMode::from_config(value).unwrap_or(settings.footprints)
                    }
                }),
                archive: Some((106, |state| {
                    let settings = state.video;
                    format!("{}", settings.footprints.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_footprints(&self) -> String {
                    self.video.footprints.config_value().to_owned()
                }
                fn cvar_set_r_footprints(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.footprints = FootprintMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected off|2d|3d"))?;
                        self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                        self.footprints_chosen();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FULLBRIGHT {
        runtime: CvarDef::new(
                "r_fullbright",
                "",
                "0..1",
                "Classic world-lighting master: 1 replaces baked lightmap contribution with white.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_fullbright(app)),
                |app, value| {
                    App::cvar_set_r_fullbright(app, APP_R_FULLBRIGHT.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_fullbright, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.world_lighting = !parse_bool(value).unwrap_or(!settings.world_lighting);
                }),
                archive: Some((166, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(!settings.world_lighting))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fullbright(&self) -> String {
                    bool_value(!self.video.world_lighting)
                }
                fn cvar_set_r_fullbright(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_fullbright".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_fullbright" => self.video.world_lighting = !enabled,
                            "r_vertexlight" => self.video.vertex_lighting = enabled,
                            "r_lightmap" => self.video.lightmap_only = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_classic_world_lighting();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FULLSCREEN {
        runtime: CvarDef::new(
                "r_fullscreen",
                "",
                "0|1|2",
                "Fullscreen mode: 0 windowed, 1 borderless, 2 exclusive.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::VidRestart,
                |app| CvarValue::String(App::cvar_get_r_fullscreen(app)),
                |app, value| {
                    App::cvar_set_r_fullscreen(app, APP_R_FULLSCREEN.entry.name, &value.to_string())
                },
            )
            .with_text_setter(
                App::cvar_set_r_fullscreen,
                Some(|name, value| {
                    FullscreenMode::from_config(value)
                        .map(|mode| mode.config_value().to_string())
                        .ok_or_else(|| {
                            format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")
                        })
                }),
            ),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.fullscreen =
                            FullscreenMode::from_config(value).unwrap_or(settings.fullscreen)
                    }
                }),
                apply_latched: Some(|settings, value| {
                    if let Some(mode) = FullscreenMode::from_config(value) {
                        settings.fullscreen = mode;
                    }
                }),
                archive: Some((6, |state| {
                    let settings = state.video;
                    format!("{}", settings.fullscreen.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_fullscreen(&self) -> String {
                    self.video.fullscreen.config_value().to_string()
                }
                fn cvar_set_r_fullscreen(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = FullscreenMode::from_config(value).ok_or_else(|| {
                            format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")
                        })?;
                        if mode != self.video.fullscreen {
                            self.set_fullscreen_mode(mode);
                        }
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GENNORMALMAPS {
        runtime: CvarDef::new(
                "r_genNormalMaps",
                "",
                "0..1",
                "Generate fallback normal maps from diffuse textures when authored normals are absent.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::MapLoadOrVidRestart,
                |app| CvarValue::String(App::cvar_get_r_gennormalmaps(app)),
                |app, value| {
                    App::cvar_set_r_gennormalmaps(app, APP_R_GENNORMALMAPS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(
                App::cvar_set_r_gennormalmaps,
                Some(|name, value| {
                    App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string())
                }),
            ),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.gen_normal_maps = parse_bool(value).unwrap_or(settings.gen_normal_maps)
                    }
                }),
                apply_latched: Some(|settings, value| {
                    settings.gen_normal_maps =
                        App::parse_console_bool(value).unwrap_or(settings.gen_normal_maps)
                }),
                archive: Some((138, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.gen_normal_maps))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_gennormalmaps(&self) -> String {
                    bool_value(self.video.gen_normal_maps)
                }
                fn cvar_set_r_gennormalmaps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.gen_normal_maps = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        if self.map_prepare_restart_required() {
                            self.console_status =
                                "GENERATED NORMAL MAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART"
                                    .into();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_EMOTECUBIC {
        runtime: CvarDef::new("cg_emoteCubic", "1", "0|1", "Bounded four-frame Catmull-Rom interpolation for emote body, props and source contact heights at render rate. Does not change playback speed or audio timing.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.emote_cubic)),
            |app,value|App::cvar_set_cg_emotecubic(app,APP_CG_EMOTECUBIC.entry.name,&value.to_string())).with_text_setter(App::cvar_set_cg_emotecubic,None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context|{settings.emote_cubic=parse_bool(value).unwrap_or(settings.emote_cubic);}),
            archive:Some((27346,|state|bool_value(state.video.emote_cubic))),
            ..CvarBinding::NONE
        },
        implementations:[impl App {
            fn cvar_set_cg_emotecubic(&mut self,name:&str,value:&str)->Result<(),String>{
                self.video.emote_cubic=Self::parse_cvar_bool(name,value)?;
                crate::emotes::set_cubic_interpolation(self.video.emote_cubic);
                self.mark_config_dirty();self.publish_ui();Ok(())
            }
        }]
    }
    APP_CG_EMOTEEFFECTS {
        runtime: CvarDef::new("cg_emoteEffects", "1", "0|1", "Draw baked Niagara particle effects (sparkles, smoke, confetti, lights) on emotes. Effects are compiled offline; this does not run Unreal code.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.emote_effects)),
            |app,value|App::cvar_set_cg_emoteeffects(app,APP_CG_EMOTEEFFECTS.entry.name,&value.to_string())).with_text_setter(App::cvar_set_cg_emoteeffects,None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context|{settings.emote_effects=parse_bool(value).unwrap_or(settings.emote_effects);}),
            archive:Some((27347,|state|bool_value(state.video.emote_effects))),
            ..CvarBinding::NONE
        },
        implementations:[impl App {
            fn cvar_set_cg_emoteeffects(&mut self,name:&str,value:&str)->Result<(),String>{
                self.video.emote_effects=Self::parse_cvar_bool(name,value)?;
                crate::emotes::set_effects_enabled(self.video.emote_effects);
                self.mark_config_dirty();self.publish_ui();Ok(())
            }
        }]
    }
    APP_R_GHOUL2ANIMSMOOTH {
        runtime: CvarDef::new("r_ghoul2AnimSmooth", "", "0..1 (exclusive)", "jaPRO CBoneCache::SmoothLow renderer bone-history filter (jaPRO default 0.3): blends each player's final composed Ghoul2 pose toward last frame's filtered pose before skinning/attachments. Only active strictly between 0 and 1, matching jaPRO: 0 or >=1 disables it (1 is 'off', not 'maximum'). Does not apply while ragdolled.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_ghoul2animsmooth(app)), |app, value| App::cvar_set_r_ghoul2animsmooth(app, APP_R_GHOUL2ANIMSMOOTH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_ghoul2animsmooth, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        // jaPRO stores this cvar unclamped and only activates smoothing
                        // when it reads strictly inside (0, 1) at use; 1.0+ is a no-op
                        // there, not "maximum". Don't clamp the upper bound here either.
                        settings.ghoul2_anim_smooth = value
                            .trim()
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.max(0.0))
                            .unwrap_or(settings.ghoul2_anim_smooth);
                    }
                }),
                archive: Some((28, |state| {
                    let settings = state.video;
                    format!("{}", settings.ghoul2_anim_smooth)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ghoul2animsmooth(&self) -> String {
                    self.video.ghoul2_anim_smooth.to_string()
                }
                fn cvar_set_r_ghoul2animsmooth(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let factor = value
                            .trim()
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .ok_or_else(|| format!("{name}: expected a number"))?;
                        // jaPRO only activates smoothing strictly inside (0, 1); 1.0+
                        // is a no-op there, not "maximum" smoothing, so don't clamp
                        // the upper bound here either.
                        self.video.ghoul2_anim_smooth = factor.max(0.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status =
                            format!("GHOUL2 ANIM SMOOTH: {}", self.video.ghoul2_anim_smooth);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GHOUL2BATCHDRAWS {
        runtime: CvarDef::new("r_ghoul2BatchDraws", "", "0|1|2", "GPU Ghoul2 draw batching: 0 off, 1 adaptive (default), 2 force. Adaptive only groups repeated opaque/masked mesh/material/LOD surfaces when the crowd is likely to benefit.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_ghoul2batchdraws(app)), |app, value| App::cvar_set_r_ghoul2batchdraws(app, APP_R_GHOUL2BATCHDRAWS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_ghoul2batchdraws, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ghoul2_batch_draws =
                        Ghoul2BatchMode::from_config(value).unwrap_or(settings.ghoul2_batch_draws);
                }),
                archive: Some((27, |state| {
                    let settings = state.video;
                    format!("{}", settings.ghoul2_batch_draws.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ghoul2batchdraws(&self) -> String {
                    self.video.ghoul2_batch_draws.config_value().to_owned()
                }
                fn cvar_set_r_ghoul2batchdraws(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ghoul2_batch_draws = Ghoul2BatchMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected 0|1|2 or off|adaptive|force"))?;
                        self.render_command(RenderCommand::SetGhoul2BatchDraws(
                            self.video.ghoul2_batch_draws,
                        ));
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "GHOUL2 BATCH DRAWS: {}",
                            self.video.ghoul2_batch_draws.label()
                        );
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MODELSTATECACHE {
        runtime: CvarDef::new("r_modelStateCache", "", "0..1", "A/B repeated texture and Ghoul2 mesh binding suppression in model/FX draw phases. Session-only; default 0.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.model_state_cache)),
            |app, value| App::cvar_set_r_modelstatecache(app, APP_R_MODELSTATECACHE.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_modelstatecache, None),
        config: CvarBinding {
            default: Some(|state| bool_value(state.video.model_state_cache)),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_modelstatecache(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.model_state_cache = Self::parse_cvar_bool(name, value)?;
                    self.render_command(RenderCommand::SetModelStateCache(self.video.model_state_cache));
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICBATCHDRAWS {
        runtime: CvarDef::new("r_dynamicBatchDraws", "", "0..1", "Merging adjacent compatible CPU model index ranges and GPU FX instance ranges without reordering. Session-only A/B override; default 1.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.dynamic_batch_draws)),
            |app, value| App::cvar_set_r_dynamicbatchdraws(app, APP_R_DYNAMICBATCHDRAWS.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_dynamicbatchdraws, None),
        config: CvarBinding {
            default: Some(|state| bool_value(state.video.dynamic_batch_draws)),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_dynamicbatchdraws(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.dynamic_batch_draws = Self::parse_cvar_bool(name, value)?;
                    self.render_command(RenderCommand::SetDynamicBatchDraws(self.video.dynamic_batch_draws));
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_WORLDCOMPACTMINDRAWS {
        runtime: CvarDef::new("r_worldCompactMinDraws", "", "1|2", "CPU world compaction: 1 baseline, 2 draws singleton visible groups directly. Keeps map-load packing and GPU-driven behavior. Session-only; default 1.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.world_compact_min_draws.to_string()),
            |app, value| App::cvar_set_r_worldcompactmindraws(app, APP_R_WORLDCOMPACTMINDRAWS.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_worldcompactmindraws, None),
        config: CvarBinding {
            default: Some(|state| state.video.world_compact_min_draws.to_string()),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_worldcompactmindraws(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let minimum = value.trim().parse::<u32>().ok().filter(|value| (1..=2).contains(value))
                        .ok_or_else(|| format!("{name}: expected 1|2"))?;
                    self.video.world_compact_min_draws = minimum;
                    self.render_command(RenderCommand::SetWorldCompactMinDraws(minimum));
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_PERFTHREADPAGE {
        runtime: CvarDef::new("r_perfThreadPage", "", "0..", "Zero-based page of every live process thread in cg_drawFPS 2. Windows percentages are CPU time per core, excluding waits. Session-only; default 0.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.perf_thread_page.to_string()),
            |app, value| App::cvar_set_r_perfthreadpage(app, APP_R_PERFTHREADPAGE.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_perfthreadpage, None),
        config: CvarBinding { default: Some(|state| state.video.perf_thread_page.to_string()), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_perfthreadpage(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.perf_thread_page = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected a nonnegative page number"))?;
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_ENCODERFINISHTHREADS {
        runtime: CvarDef::new("r_encoderFinishThreads", "", "0|1|2", "A/B finalization: 0 original encoder; 1 split world/model-post encoders finished serially; 2 finish world on a persistent worker concurrently with remaining recording/finish (default). Same-frame ordered submit. Unified MSAA1 CPU raster, no ocean/optics/RT/planar views. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.encoder_finish_threads.to_string()),
            |app, value| App::cvar_set_r_encoderfinishthreads(app, APP_R_ENCODERFINISHTHREADS.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_encoderfinishthreads, None),
        config: CvarBinding { default: Some(|state| state.video.encoder_finish_threads.to_string()), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_encoderfinishthreads(&mut self, name: &str, value: &str) -> Result<(), String> {
                let mode = value.trim().parse::<u32>().ok().filter(|mode| *mode <= 2)
                    .ok_or_else(|| format!("{name}: expected 0|1|2"))?;
                self.video.encoder_finish_threads = mode;
                self.render_command(RenderCommand::SetEncoderFinishThreads(mode));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_MODELRENDERBUNDLES {
        runtime: CvarDef::new("r_modelRenderBundles", "", "0..1", "A/B reusable opaque/masked indexed-model command recording; exact draw/resource checks and two-frame stability. RT/glow/excluded views retain direct submission. Session-only prototype; default 0.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.model_render_bundles)),
            |app, value| App::cvar_set_r_modelrenderbundles(app, APP_R_MODELRENDERBUNDLES.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_modelrenderbundles, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.model_render_bundles)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_modelrenderbundles(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.model_render_bundles = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetModelRenderBundles(self.video.model_render_bundles));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_MD3AUTOLOD {
        runtime: CvarDef::new("r_md3AutoLod", "", "0..1", "Shared automatic LOD switch: 0 authored MD3 LODs only and full-detail baked BSP models; 1 (default) generate/use missing MD3 LODs and simplify eligible baked BSP models. Authored MD3 LODs always take priority.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.md3_auto_lod)),
            |app, value| App::cvar_set_r_md3autolod(app, APP_R_MD3AUTOLOD.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_md3autolod, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.md3_auto_lod = parse_bool(value).unwrap_or(settings.md3_auto_lod);
            }),
            archive: Some((430, |state| bool_value(state.video.md3_auto_lod))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_r_md3autolod(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.md3_auto_lod = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetBspModelLod {
                    scale: self.video.lod_scale, bias: self.video.ghoul2_lod_bias,
                    debug: self.video.md3_lod_debug,
                    enabled: self.video.md3_auto_lod && self.video.bsp_model_lod,
                });
                self.console_status = format!("AUTOMATIC MODEL LODS: {}", if self.video.md3_auto_lod { "ON (ENTITY MD3 + BSP)" } else { "OFF (AUTHORED MD3 ONLY)" });
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_R_BSPMODELLOD {
        runtime: CvarDef::new("r_bspModelLod", "", "0..1", "Session-only A/B override for baked BSP model LOD. 0 forces baked geometry to full detail; 1 allows it when r_md3AutoLod is also on. Never affects MD3 entities.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.bsp_model_lod)),
            |app, value| App::cvar_set_r_bspmodellod(app, APP_R_BSPMODELLOD.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_bspmodellod, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.bsp_model_lod)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_bspmodellod(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.bsp_model_lod = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetBspModelLod {
                    scale: self.video.lod_scale, bias: self.video.ghoul2_lod_bias,
                    debug: self.video.md3_lod_debug,
                    enabled: self.video.md3_auto_lod && self.video.bsp_model_lod,
                });
                self.console_status = format!("BSP MODEL LOD: {}", if self.video.bsp_model_lod { "ALLOWED BY SHARED GATE" } else { "FORCED FULL DETAIL" });
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_R_MD3LODDEBUG {
        runtime: CvarDef::new("r_md3LodDebug", "", "0..1", "Log MD3 and compiled BSP-model LOD selection changes and generated/cache LOD job completions. Session only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.md3_lod_debug)),
            |app, value| App::cvar_set_r_md3loddebug(app, APP_R_MD3LODDEBUG.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_md3loddebug, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.md3_lod_debug)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_md3loddebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.md3_lod_debug = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetBspModelLod {
                    scale: self.video.lod_scale, bias: self.video.ghoul2_lod_bias, debug: self.video.md3_lod_debug,
                    enabled: self.video.md3_auto_lod && self.video.bsp_model_lod,
                });
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_SURFACESPRITES {
        runtime: CvarDef::new("r_surfaceSprites", "", "0..1", "Draw the map's native JKA surfaceSprites (ground grass/vegetation cards and splash effects). 0 hides them; Godot procedural grass (r_grass) is separate. Applied live.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.surface_sprites)),
            |app, value| App::cvar_set_r_surfacesprites(app, APP_R_SURFACESPRITES.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_surfacesprites, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.surface_sprites = parse_bool(value).unwrap_or(settings.surface_sprites);
            }),
            archive: Some((93, |state| bool_value(state.video.surface_sprites))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_r_surfacesprites(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.surface_sprites = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetSurfaceSprites(self.video.surface_sprites));
                self.console_status = format!("JKA SURFACE SPRITES: {}",
                    if self.video.surface_sprites { "ON" } else { "OFF" });
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_SURFACESPRITESGPU {
        runtime: CvarDef::new("r_surfaceSpritesGpu", "", "0..1", "A/B: 1 (default) bakes native vertical/flattened/oriented surfaceSprites into GPU-resident instances at map load and only culls 512-unit chunks per frame; 0 regenerates and uploads every sprite on the CPU each frame (original path). Effect-kind sprites always use the CPU path. Session only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.surface_sprites_gpu)),
            |app, value| App::cvar_set_r_surfacespritesgpu(app, APP_R_SURFACESPRITESGPU.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_surfacespritesgpu, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.surface_sprites_gpu)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_surfacespritesgpu(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.surface_sprites_gpu = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetSurfaceSpritesGpu(self.video.surface_sprites_gpu));
                self.console_status = format!("SURFACE SPRITES: {}",
                    if self.video.surface_sprites_gpu { "GPU RESIDENT" } else { "CPU PER FRAME" });
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_STATICMD3INSTANCES {
        runtime: CvarDef::new("r_staticMd3Instances", "", "0..1", "A/B: 0 original misc_model_static CPU MD3 path; 1 GPU-resident model-space geometry and per-placement transforms for repeated opaque/masked props. Existing CPU PVS/frustum/LOD culling remains; unusual stages and RT fall back. Session only, default 0.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.static_md3_instances)),
            |app, value| App::cvar_set_r_staticmd3instances(app, APP_R_STATICMD3INSTANCES.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_staticmd3instances, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.static_md3_instances)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_staticmd3instances(&mut self, name: &str, value: &str) -> Result<(), String> {
                let enabled = Self::parse_cvar_bool(name, value)?;
                self.video.static_md3_instances = enabled;
                if let Some(session) = self.game_session.as_mut() {
                    session.entity_presenter.set_static_md3_instances(enabled);
                }
                self.console_status = format!("STATIC MD3 GPU INSTANCES: {} (CPU VISIBILITY + LOD)",
                    if enabled { "ON" } else { "BASELINE" });
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_MD3GEOMETRYCACHE {
        runtime: CvarDef::new("r_md3GeometryCache", "", "0..1", "A/B reuse unchanged transformed/lit MD3 geometry and resident GPU stream ranges. Animation/transforms/lights invalidate exact cached inputs. Raster stream cache; RT keeps its original preparation. Session-only A/B override; default 1.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.md3_geometry_cache)),
            |app, value| App::cvar_set_r_md3geometrycache(app, APP_R_MD3GEOMETRYCACHE.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_md3geometrycache, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.md3_geometry_cache)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_md3geometrycache(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.md3_geometry_cache = Self::parse_cvar_bool(name, value)?;
                if let Some(presenter) = self.asset_preview_entity_presenter.as_mut() {
                    presenter.set_md3_geometry_cache(self.video.md3_geometry_cache);
                }
                self.render_command(RenderCommand::SetMd3GeometryCache(self.video.md3_geometry_cache));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_MODELINDEXUPLOADCACHE {
        runtime: CvarDef::new("r_modelIndexUploadCache", "", "0..1", "A/B skip unchanged index uploads and upload only a changed span on the raster CPU-model path. Session-only; default 1 (each upload is a GPU copy, ~9 us on an iGPU).", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.model_index_upload_cache)),
            |app, value| App::cvar_set_r_modelindexuploadcache(app, APP_R_MODELINDEXUPLOADCACHE.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_modelindexuploadcache, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.model_index_upload_cache)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_modelindexuploadcache(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.model_index_upload_cache = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetModelIndexUploadCache(self.video.model_index_upload_cache));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_GHOUL2BATCHKEYCACHE {
        runtime: CvarDef::new("r_ghoul2BatchKeyCache", "", "0..1", "A/B Ghoul2 cached group IDs + linear visible bucketing (1), or legacy full-frame hash/sort (0). Session-only; default 1.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.ghoul2_batch_key_cache)),
            |app, value| App::cvar_set_r_ghoul2batchkeycache(app, APP_R_GHOUL2BATCHKEYCACHE.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_ghoul2batchkeycache, None),
        config: CvarBinding { default: Some(|state| bool_value(state.video.ghoul2_batch_key_cache)), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_ghoul2batchkeycache(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ghoul2_batch_key_cache = Self::parse_cvar_bool(name, value)?;
                self.render_command(RenderCommand::SetGhoul2BatchKeyCache(self.video.ghoul2_batch_key_cache));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_MODELPREPARETHREADS {
        runtime: CvarDef::new("r_modelPrepareThreads", "", "0|1|2|4", "CPU model packing: 0 original serial path (default), 1 serial prepared jobs, 2/4 bounded worker threads for vertex lighting/index rebasing. Raster path only. Session-only A/B override.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.model_prepare_threads.to_string()),
            |app, value| App::cvar_set_r_modelpreparethreads(app, APP_R_MODELPREPARETHREADS.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_modelpreparethreads, None),
        config: CvarBinding { default: Some(|state| state.video.model_prepare_threads.to_string()), ..CvarBinding::NONE },
        implementations: [impl App {
            fn cvar_set_r_modelpreparethreads(&mut self, name: &str, value: &str) -> Result<(), String> {
                let threads = value.trim().parse::<u32>().ok().filter(|value| matches!(value, 0 | 1 | 2 | 4))
                    .ok_or_else(|| format!("{name}: expected 0|1|2|4"))?;
                self.video.model_prepare_threads = threads;
                self.render_command(RenderCommand::SetModelPrepareThreads(threads));
                self.publish_ui();
                Ok(())
            }
        }]
    }

    APP_R_GHOUL2EARLYCULL {
        runtime: CvarDef::new("r_ghoul2EarlyCull", "", "0..1", "Skip full Ghoul2 pose evaluation and render submission for models outside the view frustum while preserving lightweight animation state.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_ghoul2earlycull(app)), |app, value| App::cvar_set_r_ghoul2earlycull(app, APP_R_GHOUL2EARLYCULL.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_ghoul2earlycull, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ghoul2_early_cull = parse_bool(value).unwrap_or(settings.ghoul2_early_cull);
                }),
                archive: Some((25, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ghoul2_early_cull))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ghoul2earlycull(&self) -> String {
                    bool_value(self.video.ghoul2_early_cull)
                }
                fn cvar_set_r_ghoul2earlycull(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ghoul2_early_cull = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "GHOUL2 EARLY CULL: {}",
                            if self.video.ghoul2_early_cull {
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

    APP_R_GHOUL2SKINNING {
        runtime: CvarDef::new(
                "r_ghoul2Skinning",
                "",
                "cpu|workers|gpu",
                "Ghoul2 GLM skinning path: CPU reference, CPU worker pool, or WGPU vertex shader.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ghoul2skinning(app)),
                |app, value| {
                    App::cvar_set_r_ghoul2skinning(app, APP_R_GHOUL2SKINNING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_ghoul2skinning, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ghoul2_skinning =
                        Ghoul2SkinningMode::from_config(value).unwrap_or(settings.ghoul2_skinning);
                }),
                archive: Some((24, |state| {
                    let settings = state.video;
                    format!("{}", settings.ghoul2_skinning.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ghoul2skinning(&self) -> String {
                    self.video.ghoul2_skinning.config_value().to_owned()
                }
                fn cvar_set_r_ghoul2skinning(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ghoul2_skinning = Ghoul2SkinningMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status =
                            format!("GHOUL2 SKINNING: {}", self.video.ghoul2_skinning.label());
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GPUDRIVEN {
        runtime: CvarDef::new(
                "r_gpuDriven",
                "",
                "0..1",
                "GPU-visible draw compaction.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_gpudriven(app)),
                |app, value| App::cvar_set_r_gpudriven(app, APP_R_GPUDRIVEN.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_gpudriven, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.gpu_driven = parse_bool(value).unwrap_or(settings.gpu_driven)
                }),
                archive: Some((127, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.gpu_driven))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_gpudriven(&self) -> String {
                    bool_value(self.video.gpu_driven)
                }
                fn cvar_set_r_gpudriven(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_gpudriven".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        if lower == "r_gpudriven" {
                            self.video.gpu_driven = enabled;
                        } else {
                            self.video.hiz_occlusion = enabled;
                        }
                        self.sync_gpu_visibility();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GPUTIMINGS {
        runtime: CvarDef::new(
                "r_gpuTimings",
                "",
                "0..1",
                "GPU timestamp-query profiler; no per-frame query/readback cost when disabled.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_gputimings(app)),
                |app, value| {
                    App::cvar_set_r_gputimings(app, APP_R_GPUTIMINGS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_gputimings, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.gpu_timings = parse_bool(value).unwrap_or(settings.gpu_timings);
                }),
                archive: Some((23, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.gpu_timings))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_gputimings(&self) -> String {
                    bool_value(self.video.gpu_timings)
                }
                fn cvar_set_r_gputimings(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.gpu_timings = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_HIZOCCLUSION {
        runtime: CvarDef::new(
                "r_hizOcclusion",
                "",
                "0..1",
                "Hi-Z occlusion culling.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_hizocclusion(app)),
                |app, value| {
                    App::cvar_set_r_hizocclusion(app, APP_R_HIZOCCLUSION.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_hizocclusion, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.hiz_occlusion = parse_bool(value).unwrap_or(settings.hiz_occlusion)
                    }
                }),
                archive: Some((128, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.hiz_occlusion))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_hizocclusion(&self) -> String {
                    bool_value(self.video.hiz_occlusion)
                }
                fn cvar_set_r_hizocclusion(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_hizocclusion".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        if lower == "r_gpudriven" {
                            self.video.gpu_driven = enabled;
                        } else {
                            self.video.hiz_occlusion = enabled;
                        }
                        self.sync_gpu_visibility();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_JUMPHEIGHTSHADE {
        runtime: CvarDef::new("r_jumpHeightShade", "", "0..1", "jaPRO SP physics: tint flat surfaces by jump height while airborne (green = ideal landing, red = deeper, blue = reachable above the jump line).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_jumpheightshade(app)), |app, value| App::cvar_set_r_jumpheightshade(app, APP_R_JUMPHEIGHTSHADE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_jumpheightshade, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.jump_height_shade = parse_bool(value).unwrap_or(settings.jump_height_shade)
                    }
                }),
                archive: Some((292, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.jump_height_shade))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_jumpheightshade(&self) -> String {
                    bool_value(self.jump_height_shade)
                }
                fn cvar_set_r_jumpheightshade(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.jump_height_shade = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_snapshot();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_LIGHTMAP {
        runtime: CvarDef::new(
                "r_lightmap",
                "",
                "0..1",
                "Show baked lightmap or vertex-light data without the diffuse texture where available.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_lightmap(app)),
                |app, value| App::cvar_set_r_lightmap(app, APP_R_LIGHTMAP.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_lightmap, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.lightmap_only = parse_bool(value).unwrap_or(settings.lightmap_only);
                }),
                archive: Some((168, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.lightmap_only))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_lightmap(&self) -> String {
                    bool_value(self.video.lightmap_only)
                }
                fn cvar_set_r_lightmap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_lightmap".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_fullbright" => self.video.world_lighting = !enabled,
                            "r_vertexlight" => self.video.vertex_lighting = enabled,
                            "r_lightmap" => self.video.lightmap_only = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_classic_world_lighting();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GLASSPROMOTION {
        runtime: CvarDef::new(
            "r_glassPromotion", "", "0..1",
            "Promote q3map_material Glass shaders (BSP, MD3, Ghoul2) to a transmissive draw over the opaque scene. 0 keeps the authored shader stages. Applies on next map load.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart,
            |_app: &App| CvarValue::String(bool_value(crate::materials::glass_promotion_enabled())),
            |app, value| App::cvar_set_r_glasspromotion(app, APP_R_GLASSPROMOTION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_glasspromotion, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| {
                if let Some(enabled) = parse_bool(value) {
                    crate::materials::set_glass_promotion_enabled(enabled);
                }
            }),
            archive: Some((26004, |_state| format!("{}", u8::from(crate::materials::glass_promotion_enabled())))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_glasspromotion(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if enabled != crate::materials::glass_promotion_enabled() {
                        crate::materials::set_glass_promotion_enabled(enabled);
                        // Glass is resolved while materials compile. The cached
                        // prepared map (reused by vid_restart and same-map reloads)
                        // still holds the old resolution, so drop it.
                        self.prepared_map_cache = None;
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_GLASSSTYLE {
        runtime: CvarDef::new(
            "r_glassStyle", "", "bevy|frosted|liquid|ocean",
            "Live glass style. Applies a preset and resets glass tuning controls: bevy=physical transmission, frosted=blur, liquid=static distortion/color separation, ocean=animated water-like distortion.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::settings().style.name().to_owned()),
            |app, value| App::cvar_set_glass_style(app, APP_R_GLASSSTYLE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_style, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| {
                if let Some(style) = crate::glass::Style::parse(value) { crate::glass::set_style(style); }
            }),
            archive: Some((26005, |_state| crate::glass::settings().style.name().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_glass_style(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let style = crate::glass::Style::parse(value).ok_or_else(|| "r_glassStyle: bevy, frosted, liquid, or ocean".to_owned())?;
                    crate::glass::set_style(style);
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
                fn cvar_set_glass_control(&mut self, name: &str, value: &str) -> Result<(), String> {
                    crate::glass::set_value(name, value)?;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
                fn cvar_set_glass_tint(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    crate::glass::set_tint(value)?;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_GLASSBLUR {
        runtime: CvarDef::new(
            "r_glassBlur", "", "0..64", "Additional transmission blur radius in screen pixels. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassBlur").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSBLUR.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassBlur", value); }),
            archive: Some((26006, |_state| crate::glass::value("r_glassBlur").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSDISTORTION {
        runtime: CvarDef::new(
            "r_glassDistortion", "", "0..8", "Refraction displacement multiplier; 0 disables distortion. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassDistortion").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSDISTORTION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassDistortion", value); }),
            archive: Some((26007, |_state| crate::glass::value("r_glassDistortion").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSDISPERSION {
        runtime: CvarDef::new(
            "r_glassDispersion", "", "0..0.25", "Red/blue separation as a fraction of refraction displacement. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassDispersion").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSDISPERSION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassDispersion", value); }),
            archive: Some((26008, |_state| crate::glass::value("r_glassDispersion").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSROUGHNESS {
        runtime: CvarDef::new(
            "r_glassRoughness", "", "0..1", "Surface roughness controls Bevy-style transmission blur. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRoughness").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSROUGHNESS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRoughness", value); }),
            archive: Some((26009, |_state| crate::glass::value("r_glassRoughness").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSTHICKNESS {
        runtime: CvarDef::new(
            "r_glassThickness", "", "0.1..128", "Approximate glass thickness in map units; increases refraction. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassThickness").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSTHICKNESS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassThickness", value); }),
            archive: Some((26010, |_state| crate::glass::value("r_glassThickness").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSIOR {
        runtime: CvarDef::new(
            "r_glassIOR", "", "1..2.5", "Index of refraction; 1.5 is ordinary glass. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassIOR").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSIOR.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassIOR", value); }),
            archive: Some((26011, |_state| crate::glass::value("r_glassIOR").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSTINTSTRENGTH {
        runtime: CvarDef::new(
            "r_glassTintStrength", "", "0..1", "Amount of authored chromatic tint and r_glassTint applied to transmission. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassTintStrength").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSTINTSTRENGTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassTintStrength", value); }),
            archive: Some((26012, |_state| crate::glass::value("r_glassTintStrength").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSREFLECTION {
        runtime: CvarDef::new(
            "r_glassReflection", "", "0..1", "Fresnel environment contribution; 0 disables it. Live for all promoted glass styles.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassReflection").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSREFLECTION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassReflection", value); }),
            archive: Some((26013, |_state| crate::glass::value("r_glassReflection").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSTINT {
        runtime: CvarDef::new(
            "r_glassTint", "", "R G B (0..1)", "Live linear RGB transmission tint. Strength is r_glassTintStrength.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| { let c = crate::glass::settings().tint; CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_glass_tint(app, APP_R_GLASSTINT.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_tint, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_tint(value); }),
            archive: Some((26014, |_state| { let c = crate::glass::settings().tint; format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSFROST {
        runtime: CvarDef::new(
            "r_glassFrost", "", "0..1", "Blurred-versus-sharp transmission mix; 0 keeps sharp refraction. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassFrost").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSFROST.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassFrost", value); }),
            archive: Some((26015, |_state| crate::glass::value("r_glassFrost").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSOPACITY {
        runtime: CvarDef::new(
            "r_glassOpacity", "", "0..1", "Solid haze/tint fill; 0 adds no haze, 1 fills with r_glassTint. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassOpacity").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSOPACITY.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassOpacity", value); }),
            archive: Some((26016, |_state| crate::glass::value("r_glassOpacity").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEFFECTSTRENGTH {
        runtime: CvarDef::new(
            "r_glassEffectStrength", "", "0..1", "Overall glass effect strength; 0 shows the undistorted scene. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassEffectStrength").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEFFECTSTRENGTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEffectStrength", value); }),
            archive: Some((26017, |_state| crate::glass::value("r_glassEffectStrength").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEDGEWIDTH {
        runtime: CvarDef::new(
            "r_glassEdgeWidth", "", "0..128", "Width of the refractive bevel in screen pixels. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassEdgeWidth").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEDGEWIDTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEdgeWidth", value); }),
            archive: Some((26018, |_state| crate::glass::value("r_glassEdgeWidth").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEDGESTRENGTH {
        runtime: CvarDef::new(
            "r_glassEdgeStrength", "", "0..8", "Bevel refraction strength along actual visible glass boundaries. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassEdgeStrength").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEDGESTRENGTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEdgeStrength", value); }),
            archive: Some((26019, |_state| crate::glass::value("r_glassEdgeStrength").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSMAXDISPLACEMENT {
        runtime: CvarDef::new(
            "r_glassMaxDisplacement", "", "0..128", "Maximum refraction displacement in screen pixels. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassMaxDisplacement").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSMAXDISPLACEMENT.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassMaxDisplacement", value); }),
            archive: Some((26020, |_state| crate::glass::value("r_glassMaxDisplacement").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSMAGNIFICATION {
        runtime: CvarDef::new(
            "r_glassMagnification", "", "-1..1", "Zoom the transmitted scene around the visible pane center; 0 disables. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassMagnification").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSMAGNIFICATION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassMagnification", value); }),
            archive: Some((26021, |_state| crate::glass::value("r_glassMagnification").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMSTRENGTH {
        runtime: CvarDef::new(
            "r_glassRimStrength", "", "0..2", "Directional rim highlight strength. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimStrength").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMSTRENGTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimStrength", value); }),
            archive: Some((26022, |_state| crate::glass::value("r_glassRimStrength").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMWIDTH {
        runtime: CvarDef::new(
            "r_glassRimWidth", "", "0..32", "Rim highlight width in screen pixels. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimWidth").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMWIDTH.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimWidth", value); }),
            archive: Some((26023, |_state| crate::glass::value("r_glassRimWidth").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMSOFTNESS {
        runtime: CvarDef::new(
            "r_glassRimSoftness", "", "0.1..32", "Rim highlight feather width in screen pixels. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimSoftness").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMSOFTNESS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimSoftness", value); }),
            archive: Some((26024, |_state| crate::glass::value("r_glassRimSoftness").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMANGLE {
        runtime: CvarDef::new(
            "r_glassRimAngle", "", "0..360", "Rim highlight direction in screen degrees. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimAngle").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMANGLE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimAngle", value); }),
            archive: Some((26025, |_state| crate::glass::value("r_glassRimAngle").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEDGEBLUR {
        runtime: CvarDef::new(
            "r_glassEdgeBlur", "", "0..64", "Additional progressive blur radius near glass boundaries. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassEdgeBlur").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEDGEBLUR.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEdgeBlur", value); }),
            archive: Some((26026, |_state| crate::glass::value("r_glassEdgeBlur").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEDGEBLURMODE {
        runtime: CvarDef::new(
            "r_glassEdgeBlurMode", "", "0..1", "Progressive blur distribution: 0 all boundaries, 1 top/bottom boundaries. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassEdgeBlurMode").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEDGEBLURMODE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEdgeBlurMode", value); }),
            archive: Some((26027, |_state| crate::glass::value("r_glassEdgeBlurMode").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSWAVINESS {
        runtime: CvarDef::new(
            "r_glassWaviness", "", "0..4", "Procedural surface waves; 0 keeps the flat center clear. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassWaviness").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSWAVINESS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassWaviness", value); }),
            archive: Some((26028, |_state| crate::glass::value("r_glassWaviness").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSANIMATIONSPEED {
        runtime: CvarDef::new(
            "r_glassAnimationSpeed", "", "0..4", "Surface wave animation rate; 0 freezes waves. Live for promoted glass.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassAnimationSpeed").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSANIMATIONSPEED.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassAnimationSpeed", value); }),
            archive: Some((26029, |_state| crate::glass::value("r_glassAnimationSpeed").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSEDGEPROFILE {
        runtime: CvarDef::new(
            "r_glassEdgeProfile", "", "convex|squircle|concave|lip", "Shape of the refractive glass bevel. Applies live.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::settings().edge_profile.name().to_owned()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSEDGEPROFILE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassEdgeProfile", value); }),
            archive: Some((26030, |_state| crate::glass::settings().edge_profile.name().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSFOREGROUNDGUARD {
        runtime: CvarDef::new(
            "r_glassForegroundGuard", "", "0..1", "Suppress refraction offsets near foreground actors to prevent silhouette halos. Applies live.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassForegroundGuard").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSFOREGROUNDGUARD.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassForegroundGuard", value); }),
            archive: Some((26031, |_state| crate::glass::value("r_glassForegroundGuard").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMSATURATION {
        runtime: CvarDef::new(
            "r_glassRimSaturation", "", "0..4", "Color saturation at the rim when r_glassRimType is 0.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimSaturation").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMSATURATION.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimSaturation", value); }),
            archive: Some((26032, |_state| crate::glass::value("r_glassRimSaturation").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSRIMTYPE {
        runtime: CvarDef::new(
            "r_glassRimType", "", "0|1", "0 color-enhanced rim, 1 white rim. Applies live.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassRimType").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSRIMTYPE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassRimType", value); }),
            archive: Some((26033, |_state| crate::glass::value("r_glassRimType").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_GLASSDISPERSIONBASE {
        runtime: CvarDef::new(
            "r_glassDispersionBase", "", "0..4", "Base multiplier for chromatic separation; multiplies r_glassDispersion.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassDispersionBase").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSDISPERSIONBASE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassDispersionBase", value); }),
            archive: Some((26034, |_state| crate::glass::value("r_glassDispersionBase").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }


    APP_R_GLASSBACKGROUNDRECOVERY {
        runtime: CvarDef::new(
            "r_glassBackgroundRecovery", "", "0|1", "Save the world before opaque models to fill foreground refraction/blur holes. Adds one fullscreen capture when needed; 0 disables it.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |_app: &App| CvarValue::String(crate::glass::value("r_glassBackgroundRecovery").to_string()),
            |app, value| App::cvar_set_glass_control(app, APP_R_GLASSBACKGROUNDRECOVERY.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_glass_control, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| { let _ = crate::glass::set_value("r_glassBackgroundRecovery", value); }),
            archive: Some((26035, |_state| crate::glass::value("r_glassBackgroundRecovery").to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_R_TINTEDGLASS {
        runtime: CvarDef::new(
            "r_tintedGlass", "", "0..1",
            "RGB transmission for hardware RT and raster shadowed local lights. Preserves opaque blockers and stacks panes. Overrides: tinted_glass_overrides.txt",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.tinted_glass)),
            |app, value| App::cvar_set_r_tintedglass(app, APP_R_TINTEDGLASS.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_tintedglass, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.tinted_glass = parse_bool(value).unwrap_or(settings.tinted_glass);
            }),
            archive: Some((420, |state| format!("{}", u8::from(state.video.tinted_glass)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_tintedglass(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.tinted_glass = Self::parse_cvar_bool(name, value)?;
                    self.sync_tinted_glass();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_TINTEDGLASSDEBUG {
        runtime: CvarDef::new(
            "r_tintedGlassDebug", "", "0..2",
            "0=off, 1=highlight receivers reached through glass, 2=RGB absorption diagnostic.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.tinted_glass_debug.to_string()),
            |app, value| App::cvar_set_r_tintedglass_debug(app, APP_R_TINTEDGLASSDEBUG.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_tintedglass_debug, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.tinted_glass_debug = value.trim().parse::<u32>().unwrap_or(settings.tinted_glass_debug).min(2);
            }),
            archive: Some((421, |state| state.video.tinted_glass_debug.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_tintedglass_debug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = value.parse::<u32>().map_err(|_| format!("{} expects 0, 1, or 2", name))?;
                    if mode > 2 { return Err(format!("{} expects 0, 1, or 2", name)); }
                    self.video.tinted_glass_debug = mode;
                    self.sync_tinted_glass_debug();
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_LOCALLIGHTSHADOWS {
        runtime: CvarDef::new(
                "r_localLightShadows",
                "",
                "0..1",
                "Cubemap shadows for selected local lights.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_locallightshadows(app)),
                |app, value| {
                    App::cvar_set_r_locallightshadows(
                        app,
                        APP_R_LOCALLIGHTSHADOWS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_locallightshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.local_light_shadows =
                            parse_bool(value).unwrap_or(settings.local_light_shadows)
                    }
                }),
                archive: Some((144, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.local_light_shadows))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_locallightshadows(&self) -> String {
                    bool_value(self.video.local_light_shadows)
                }
                fn cvar_set_r_locallightshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.local_light_shadows = Self::parse_cvar_bool(name, value)?;
                        self.sync_local_light_shadows();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_LODBIAS {
        runtime: CvarDef::new(
                "r_lodbias",
                "",
                "0..8",
                "TaystJK Ghoul2/MD3/BSP-model LOD bias; higher values select cheaper authored or generated geometry.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_lodbias(app)),
                |app, value| App::cvar_set_r_lodbias(app, APP_R_LODBIAS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_lodbias, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ghoul2_lod_bias = value
                        .parse::<i32>()
                        .ok()
                        .map(|bias| bias.max(0))
                        .unwrap_or(settings.ghoul2_lod_bias);
                }),
                archive: Some((26, |state| {
                    let settings = state.video;
                    format!("{}", settings.ghoul2_lod_bias)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_lodbias(&self) -> String {
                    self.video.ghoul2_lod_bias.to_string()
                }
                fn cvar_set_r_lodbias(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let bias = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected a non-negative integer"))?;
                        self.video.ghoul2_lod_bias = bias.max(0);
                        self.render_command(RenderCommand::SetBspModelLod {
                            scale: self.video.lod_scale, bias: self.video.ghoul2_lod_bias, debug: self.video.md3_lod_debug,
                    enabled: self.video.md3_auto_lod && self.video.bsp_model_lod,
                        });
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!("MODEL LOD BIAS: {}", self.video.ghoul2_lod_bias);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_LODSCALE {
        runtime: CvarDef::new("r_lodScale", "", "0.1..100", "TaystJK Ghoul2, MD3 and compiled BSP-model projected-radius LOD scale (default 5). Larger keeps higher-detail models farther away; r_lodbias is added afterward.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_lodscale(app)), |app, value| App::cvar_set_r_lodscale(app, APP_R_LODSCALE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_lodscale, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.lod_scale = normalize_lod_scale(value, settings.lod_scale);
                }),
                archive: Some((175, |state| {
                    let settings = state.video;
                    format!("{}", settings.lod_scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_lodscale(&self) -> String {
                    self.video.lod_scale.to_string()
                }
                fn cvar_set_r_lodscale(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "r_lodScale expects a number".to_owned())?;
                        self.video.lod_scale = config::normalize_lod_scale(value, self.video.lod_scale);
                        self.render_command(RenderCommand::SetBspModelLod {
                            scale: self.video.lod_scale, bias: self.video.ghoul2_lod_bias, debug: self.video.md3_lod_debug,
                    enabled: self.video.md3_auto_lod && self.video.bsp_model_lod,
                        });
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!("MODEL LOD SCALE: {}", self.video.lod_scale);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MAPLIGHTSIMULATION {
        runtime: CvarDef::new(
                "r_mapLightSimulation",
                "",
                "0..1",
                "Source .map only: preview authored light entities with the runtime clustered renderer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_maplightsimulation(app)),
                |app, value| {
                    App::cvar_set_r_maplightsimulation(
                        app,
                        APP_R_MAPLIGHTSIMULATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_maplightsimulation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.map_light_simulation =
                        parse_bool(value).unwrap_or(settings.map_light_simulation);
                }),
                archive: Some((131, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.map_light_simulation))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_maplightsimulation(&self) -> String {
                    bool_value(self.video.map_light_simulation)
                }
                fn cvar_set_r_maplightsimulation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.map_light_simulation = Self::parse_cvar_bool(name, value)?;
                        self.sync_map_light_simulation();
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            ".MAP LIGHT SIMULATION: {}",
                            if self.video.map_light_simulation {
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

    APP_R_EARLYSUBMIT {
        runtime: CvarDef::new("r_earlySubmit", "", "0..1", "Render the frame offscreen and submit it before waiting for a swapchain image, then copy it in. Lets the GPU start while the CPU waits. Ignored on frames with menus, CRT, SMAA or HDR output. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_earlysubmit(app)),
         |app, value| App::cvar_set_r_earlysubmit(app, APP_R_EARLYSUBMIT.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_earlysubmit, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.early_submit)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_earlysubmit(&self) -> String {
                    bool_value(self.video.early_submit)
                }
                fn cvar_set_r_earlysubmit(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        // Session-only (not config-dirty) A/B switch.
                        self.video.early_submit = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetEarlySubmit(self.video.early_submit));
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PRESENTCOPY {
        runtime: CvarDef::new("r_presentCopy", "", "0..1", "Render into an offscreen image and copy it into the swapchain image at the end of the frame (TaystJK Vulkan structure). Ignored on frames with menus, CRT, SMAA or HDR output. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_presentcopy(app)),
         |app, value| App::cvar_set_r_presentcopy(app, APP_R_PRESENTCOPY.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_presentcopy, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.present_copy)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_presentcopy(&self) -> String {
                    bool_value(self.video.present_copy)
                }
                fn cvar_set_r_presentcopy(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.present_copy = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetPresentCopy(self.video.present_copy));
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MAXFRAMELATENCY {
        runtime: CvarDef::new("r_maxFrameLatency", "", "1|2|3", "Maximum WGPU surface frames in flight: 1 lowest latency, 2 balanced, 3 maximum throughput.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_maxframelatency(app)), |app, value| App::cvar_set_r_maxframelatency(app, APP_R_MAXFRAMELATENCY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_maxframelatency, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<u32>() {
                        if (1..=3).contains(&value) {
                            settings.max_frame_latency = value;
                        }
                    }
                }),
                archive: Some((9, |state| {
                    let settings = state.video;
                    format!("{}", settings.max_frame_latency)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_maxframelatency(&self) -> String {
                    self.video.max_frame_latency.to_string()
                }
                fn cvar_set_r_maxframelatency(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let latency = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 1, 2, or 3"))?;
                        if !(1..=3).contains(&latency) {
                            return Err(format!("{name}: expected 1, 2, or 3"));
                        }
                        self.set_max_frame_latency(latency);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MODE {
        runtime: CvarDef::new(
                "r_mode",
                "",
                "-1",
                "Custom resolution marker; dimensions are controlled by r_customwidth and r_customheight.",
                CvarType::Enum(&["-1"]),
                CvarFlags::ARCHIVE.union(CvarFlags::FIXED),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_mode(app)),
                |app, value| App::cvar_set_r_mode(app, APP_R_MODE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_mode, None),
        config: CvarBinding {
                archive: Some((0, |_state| "-1".to_owned())),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_mode(&self) -> String {
                    "-1".into()
                }
                fn cvar_set_r_mode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected an integer"))?;
                        if mode != -1 {
                            return Err(format!(
                                "{name}: only -1 (custom, r_customWidth/r_customHeight) is supported"
                            ));
                        }
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MODERNSABERS {
        runtime: CvarDef::new("r_modernSabers", "", "0..1", "Continuous view-facing saber glow ribbon; 0 keeps the OpenJK-compatible sprite-chain presentation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_modernsabers(app)), |app, value| App::cvar_set_r_modernsabers(app, APP_R_MODERNSABERS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_modernsabers, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.modern_sabers = parse_bool(value).unwrap_or(settings.modern_sabers)
                    }
                }),
                archive: Some((132, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.modern_sabers))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_modernsabers(&self) -> String {
                    bool_value(self.video.modern_sabers)
                }
                fn cvar_set_r_modernsabers(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.modern_sabers = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session
                                .weapon_fx
                                .set_modern_sabers(self.video.modern_sabers);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "MODERN SABER RENDERING: {}",
                            if self.video.modern_sabers {
                                "ON"
                            } else {
                                "OFF (OPENJK)"
                            }
                        );
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MOTIONBLUR {
        runtime: CvarDef::new(
                "r_motionBlur",
                "",
                "0..1",
                "Camera/world motion blur strength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_motionblur(app)),
                |app, value| {
                    App::cvar_set_r_motionblur(app, APP_R_MOTIONBLUR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_motionblur, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.motion_blur_strength = value
                            .parse::<f32>()
                            .ok()
                            .filter(|strength| strength.is_finite())
                            .map(|strength| strength.clamp(0.0, 1.0))
                            .unwrap_or(settings.motion_blur_strength)
                    }
                }),
                archive: Some((111, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.motion_blur_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_motionblur(&self) -> String {
                    format!("{:.3}", self.video.motion_blur_strength)
                }
                fn cvar_set_r_motionblur(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let strength = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        self.set_motion_blur_strength(strength);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_NOVIS {
        runtime: CvarDef::new(
                "r_novis",
                "",
                "0..1",
                "Disable PVS culling; legacy compatibility alias.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_novis(app)),
                |app, value| App::cvar_set_r_novis(app, APP_R_NOVIS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_novis, None),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| context.novis = parse_bool(value)),
                archive: Some((29, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.pvs_mode == PvsMode::Off))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_novis(&self) -> String {
                    bool_value(self.video.pvs_mode == PvsMode::Off)
                }
                fn cvar_set_r_novis(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.pvs_mode = if Self::parse_cvar_bool(name, value)? {
                            PvsMode::Off
                        } else {
                            PvsMode::Auto
                        };
                        self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PBR {
        runtime: CvarDef::new(
                "r_pbr",
                "",
                "0..1",
                "Master Rend2-style PBR switch; when off, .mtr overrides are not used.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::MapLoadOrVidRestart,
                |app| CvarValue::String(App::cvar_get_r_pbr(app)),
                |app, value| App::cvar_set_r_pbr(app, APP_R_PBR.entry.name, &value.to_string()),
            )
            .with_text_setter(
                App::cvar_set_r_pbr,
                Some(|name, value| {
                    App::parse_cvar_bool(name, value).map(|enabled| u8::from(enabled).to_string())
                }),
            ),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.pbr = parse_bool(value).unwrap_or(settings.pbr)
                }),
                apply_latched: Some(|settings, value| {
                    settings.pbr = App::parse_console_bool(value).unwrap_or(settings.pbr)
                }),
                archive: Some((136, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.pbr))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_pbr(&self) -> String {
                    bool_value(self.video.pbr)
                }
                fn cvar_set_r_pbr(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.pbr = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PERFTRACE {
        runtime: CvarDef::new(
                "r_perfTrace",
                "",
                "0..1",
                "Periodic renderer performance trace in the console.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_perftrace(app)),
                |app, value| App::cvar_set_r_perftrace(app, APP_R_PERFTRACE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_perftrace, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.perf_trace = parse_bool(value).unwrap_or(settings.perf_trace);
                }),
                archive: Some((21, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.perf_trace))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_perftrace(&self) -> String {
                    bool_value(self.video.perf_trace)
                }
                fn cvar_set_r_perftrace(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.perf_trace = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PLANARDEBUG {
        runtime: CvarDef::new(
                "r_planardebug",
                "off",
                "off|candidates|selected|texture|applied|binding",
                "Planar reflection debug view.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_planardebug(app)),
                |app, value| {
                    App::cvar_set_r_planardebug(app, APP_R_PLANARDEBUG.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_planardebug, None),
        config: CvarBinding {
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_planardebug(&self) -> String {
                    self.video
                        .planar_reflection_debug
                        .label()
                        .to_ascii_lowercase()
                }
                fn cvar_set_r_planardebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.planar_reflection_debug = PlanarReflectionDebugMode::from_config(value)
                            .ok_or_else(|| {
                                format!("{name}: expected off|candidates|selected|texture|applied|binding")
                            })?;
                        self.sync_planar_reflection_debug();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_POM {
        runtime: CvarDef::new("r_pom", "", "0..1", "Parallax occlusion mapping when r_pbr is on; 0 keeps PBR shading without POM height sampling. Session-only.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_pom(app)),
         |app, value| App::cvar_set_r_pom(app, APP_R_POM.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_pom, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.pom)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_pom(&self) -> String {
                    bool_value(self.video.pom)
                }
                fn cvar_set_r_pom(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        // Session-only (not config-dirty): parallax occlusion with r_pbr.
                        self.video.pom = Self::parse_cvar_bool(name, value)?;
                        self.render_command(RenderCommand::SetPom(self.video.pom));
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PUDDLEQUALITY {
        runtime: CvarDef::new("r_puddleQuality", "", "standard|high", "high shades puddles and wet ground with the GodotOcean water response and streaked reflections.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_puddlequality(app)), |app, value| App::cvar_set_r_puddlequality(app, APP_R_PUDDLEQUALITY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_puddlequality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.puddle_quality =
                            PuddleQuality::from_config(value).unwrap_or(settings.puddle_quality)
                    }
                }),
                archive: Some((89, |state| {
                    let settings = state.video;
                    format!("{}", settings.puddle_quality.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_puddlequality(&self) -> String {
                    self.video.puddle_quality.config_value().to_owned()
                }
                fn cvar_set_r_puddlequality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.puddle_quality = PuddleQuality::from_config(value)
                            .ok_or_else(|| format!("{name}: expected standard|high"))?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PUDDLESCATTER {
        runtime: CvarDef::new("r_puddleScatter", "", "0..1", "How readily rain collects in scattered puddles on large flat ground; 0 keeps only basin puddles.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_puddlescatter(app)), |app, value| App::cvar_set_r_puddlescatter(app, APP_R_PUDDLESCATTER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_puddlescatter, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.puddle_scatter = value
                            .parse::<f32>()
                            .ok()
                            .filter(|amount| amount.is_finite())
                            .map(|amount| amount.clamp(0.0, 1.0))
                            .unwrap_or(settings.puddle_scatter)
                    }
                }),
                archive: Some((90, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.puddle_scatter)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_puddlescatter(&self) -> String {
                    format!("{:.3}", self.video.puddle_scatter)
                }
                fn cvar_set_r_puddlescatter(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let amount = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        self.set_puddle_scatter(amount);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_FACEPLANECULL {
        runtime: CvarDef::new(
            "r_facePlaneCull", "", "0..1",
            "BSP plane culling: packed-batch rejection plus CPU subrange rejection of planar faces (GPU-driven uses batch-only fallback).",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(App::cvar_get_r_faceplanecull(app)),
            |app, value| App::cvar_set_r_faceplanecull(app, APP_R_FACEPLANECULL.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_faceplanecull, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(enabled) = parse_bool(value) {
                    settings.face_plane_cull = enabled;
                }
            }),
            archive: Some((31, |state| format!("{}", u8::from(state.video.face_plane_cull)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_r_faceplanecull(&self) -> String {
                    bool_value(self.video.face_plane_cull)
                }
                fn cvar_set_r_faceplanecull(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if self.video.face_plane_cull != enabled {
                        self.video.face_plane_cull = enabled;
                        self.render_command(RenderCommand::SetFacePlaneCull(enabled));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PVSMODE {
        runtime: CvarDef::new("r_pvsMode", "", "off|minimal|full|auto", "Potentially-visible-set culling mode; Auto uses map-load portal/cluster draw plans with shared compatible merged batches.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_pvsmode(app)), |app, value| App::cvar_set_r_pvsmode(app, APP_R_PVSMODE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_pvsmode, None),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    {
                        context.pvs_mode = match value.to_ascii_lowercase().as_str() {
                            "off" | "0" => Some(PvsMode::Off),
                            "minimal" | "1" => Some(PvsMode::Minimal),
                            "full" | "2" => Some(PvsMode::Full),
                            // Auto 1-3 were removed; Auto 4 became Auto.
                            "auto" | "3" | "auto2" | "4" | "auto3" | "5" | "auto4" | "batched"
                            | "pvsbatched" | "6" => Some(PvsMode::Auto),
                            _ => context.pvs_mode,
                        };
                    }
                }),
                archive: Some((30, |state| {
                    let settings = state.video;
                    let pvs_mode = settings.pvs_mode.config_value();
                    format!("{}", pvs_mode)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_pvsmode(&self) -> String {
                    self.video.pvs_mode.config_value().to_owned()
                }
                fn cvar_set_r_pvsmode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.pvs_mode = match value.trim().to_ascii_lowercase().as_str() {
                            "off" | "0" => PvsMode::Off,
                            "minimal" | "1" => PvsMode::Minimal,
                            "full" | "2" => PvsMode::Full,
                            "auto" | "3" | "auto2" | "4" | "auto3" | "5" | "auto4" | "batched"
                            | "pvsbatched" | "6" => PvsMode::Auto,
                            _ => return Err(format!("{name}: expected off|minimal|full|auto")),
                        };
                        self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_REFLECTIONDEBUG {
        runtime: CvarDef::new("r_reflectionDebug", "", "0..1", "Show per-surface reflection resolver result (magenta planar, green SSR, blue probe, gray SSR-eligible/no-hit).", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_r_reflectiondebug(app)),
         |app, value| App::cvar_set_r_reflectiondebug(app, APP_R_REFLECTIONDEBUG.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_r_reflectiondebug, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.video.reflection_debug)
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_reflectiondebug(&self) -> String {
                    bool_value(self.video.reflection_debug)
                }
                fn cvar_set_r_reflectiondebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_reflectiondebug".to_owned();
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

    APP_R_REFLECTIONQUALITY {
        runtime: CvarDef::new("r_reflectionQuality", "", "off|low|medium|high|ultra", "Master reflection resolver quality/budget: probes -> SSR -> dynamically allocated planar reflections.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_reflectionquality(app)), |app, value| App::cvar_set_r_reflectionquality(app, APP_R_REFLECTIONQUALITY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_reflectionquality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.reflection_quality =
                            ReflectionQuality::from_config(value).unwrap_or(settings.reflection_quality)
                    }
                }),
                archive: Some((107, |state| {
                    let settings = state.video;
                    format!("{}", settings.reflection_quality.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_reflectionquality(&self) -> String {
                    self.video.reflection_quality.config_value().to_owned()
                }
                fn cvar_set_r_reflectionquality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.reflection_quality = ReflectionQuality::from_config(value)
                            .ok_or_else(|| format!("{name}: expected off|legacy|low|medium|high|ultra"))?;
                        self.reflection_quality_changed();
                        self.publish_ui();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_R_SABERIMPACTFX {
        runtime: CvarDef::new("r_saberImpactFx", "", "0..1", "Render authored saber hit/block EFX trees; useful for isolated FX performance A/B testing.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_saberimpactfx(app)), |app, value| App::cvar_set_r_saberimpactfx(app, APP_R_SABERIMPACTFX.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_saberimpactfx, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.saber_impact_fx = parse_bool(value).unwrap_or(settings.saber_impact_fx)
                    }
                }),
                archive: Some((134, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.saber_impact_fx))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_saberimpactfx(&self) -> String {
                    bool_value(self.video.saber_impact_fx)
                }
                fn cvar_set_r_saberimpactfx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.saber_impact_fx = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session
                                .weapon_fx
                                .set_saber_impact_fx(self.video.saber_impact_fx);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "SABER IMPACT EFFECTS: {}",
                            if self.video.saber_impact_fx {
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

    APP_R_SABERMARKS {
        runtime: CvarDef::new("r_saberMarks", "", "off|legacy|enhanced", "Saber/world contact effects: off, OpenJK-compatible marks, or a smooth heat-accumulating molten relief/sludge effect.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_sabermarks(app)), |app, value| App::cvar_set_r_sabermarks(app, APP_R_SABERMARKS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_sabermarks, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.saber_marks =
                            SaberMarkMode::from_config(value).unwrap_or(settings.saber_marks)
                    }
                }),
                archive: Some((135, |state| {
                    let settings = state.video;
                    format!("{}", settings.saber_marks.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_sabermarks(&self) -> String {
                    self.video.saber_marks.config_value().to_owned()
                }
                fn cvar_set_r_sabermarks(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.saber_marks = ui::SaberMarkMode::from_config(value)
                            .ok_or_else(|| format!("{name}: expected off|legacy|enhanced"))?;
                        if let Some(session) = self.game_session.as_mut() {
                            session.weapon_fx.set_saber_marks(self.video.saber_marks);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                        self.console_status = format!(
                            "SABER MARKS: {}",
                            self.video.saber_marks.label().to_ascii_uppercase()
                        );
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SHADER_COMPLEXITY {
        runtime: CvarDef::new("r_shaderComplexity", "", "0..1", "Session-only BSP fragment-accumulating heatmap. Per-pixel sum of estimated visible stage costs, NOT driver GPU timing/instruction count. 0 off, 1 on.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_shadercomplexity(app)), |app, value| App::cvar_set_r_shadercomplexity(app, APP_R_SHADER_COMPLEXITY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_shadercomplexity, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_get_r_shadercomplexity(&self) -> String {
                    bool_value(self.video.shader_complexity)
                }
                fn cvar_set_r_shadercomplexity(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.video.shader_complexity = enabled;
                    self.render_command(RenderCommand::SetShaderComplexity(enabled));
                    self.console_status = format!("BSP shader complexity: {} (estimated stage costs accumulated per visible pixel)", if enabled { "ON" } else { "OFF" });
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_SHOWTRIS {
        runtime: CvarDef::new("r_showtris", "", "0..127", "Wireframe category bitmask: 1 map, 2 players, 4 entities, 8 effects, 16 grass, 32 ocean, 64 deformation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_showtris(app)), |app, value| App::cvar_set_r_showtris(app, APP_R_SHOWTRIS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_showtris, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(mask) = value.trim().parse::<u32>() {
                        settings.wireframe_mask = mask & crate::ui::wireframe::ALL;
                    } else if let Some(enabled) = parse_bool(value) {
                        settings.wireframe_mask = if enabled {
                            crate::ui::wireframe::MAP
                        } else {
                            0
                        };
                    }
                }),
                archive: Some((17, |state| {
                    let settings = state.video;
                    format!("{}", settings.wireframe_mask)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_showtris(&self) -> String {
                    self.video.wireframe_mask.to_string()
                }
                fn cvar_set_r_showtris(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let mask = value.trim().parse::<u32>().map_err(|_| {
                            format!(
                                "{name}: expected an integer bitmask (0..{})",
                                ui::wireframe::ALL
                            )
                        })?;
                        if mask & !ui::wireframe::ALL != 0 {
                            return Err(format!(
                                "{name}: unsupported wireframe bits; valid range is 0..{}",
                                ui::wireframe::ALL
                            ));
                        }
                        if mask != 0 && !self.wireframe_supported {
                            return Err("r_showtris: wireframe overlay is not supported by this GPU".into());
                        }
                        self.video.wireframe_mask = mask;
                        self.render_command(RenderCommand::SetWireframeMask(mask));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SKIPUI {
        runtime: CvarDef::new(
                "r_skipUi",
                "",
                "0..1",
                "Diagnostic: skip all UI vertex generation and UI rendering.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_skipui(app)),
                |app, value| App::cvar_set_r_skipui(app, APP_R_SKIPUI.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_skipui, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.skip_ui = parse_bool(value).unwrap_or(settings.skip_ui);
                }),
                archive: Some((18, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.skip_ui))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_skipui(&self) -> String {
                    bool_value(self.video.skip_ui)
                }
                fn cvar_set_r_skipui(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.skip_ui = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SWAPINTERVAL {
        runtime: CvarDef::new(
                "r_swapInterval",
                "",
                "0|1|2|3",
                "Presentation mode: 0 off, 1 on, 2 fast/mailbox, 3 adaptive.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_swapinterval(app)),
                |app, value| {
                    App::cvar_set_r_swapinterval(app, APP_R_SWAPINTERVAL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_swapinterval, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.vsync = VsyncMode::from_config(value).unwrap_or(settings.vsync)
                    }
                }),
                archive: Some((8, |state| {
                    let settings = state.video;
                    format!("{}", settings.vsync.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_swapinterval(&self) -> String {
                    self.video.vsync.config_value().to_string()
                }
                fn cvar_set_r_swapinterval(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let previous_vsync = self.video.vsync;
                        self.video.vsync = VsyncMode::from_config(value).ok_or_else(|| {
                            format!("{name}: expected 0/off, 1/on, 2/fast, or 3/adaptive")
                        })?;
                        self.render_command(RenderCommand::SetVsync(self.video.vsync));
                        self.mark_config_dirty();
                        if self.video.vsync != previous_vsync {
                            // Entering FAST on DX12 can make the standing cap unreachable.
                            self.set_fps_cap(self.video.fps_cap);
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PIPELINE_TRACE {
        runtime: CvarDef::new("r_pipelineTrace", "0", "0|1", "Log native BSP/RT pipeline BEGIN/END and background shader job progress. Session-only; enable before the slow settings change.", CvarType::String, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.pipeline_trace)),
            |app: &mut App, value| App::cvar_set_r_pipeline_trace(app, APP_R_PIPELINE_TRACE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_r_pipeline_trace, None),
        config: CvarBinding {
            default: Some(|state| bool_value(state.video.pipeline_trace)),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_pipeline_trace(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.video.pipeline_trace = enabled;
                    crate::pipeline_jobs::set_pipeline_trace(enabled);
                    self.console_status = format!("SHADER COMPILE TRACE: {}", if enabled { "ON" } else { "OFF" });
                    Ok(())
                }
            }
        ]
    }

    APP_R_VERBOSE {
        runtime: CvarDef::new("r_verbose", "", "0..3", "Renderer-only diagnostic level: 0 quiet, 1 basic renderer lifecycle, 2 verbose renderer internals, 3 trace. A matching developer level also enables renderer output.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_verbose(app)), |app, value| App::cvar_set_r_verbose(app, APP_R_VERBOSE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_verbose, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.renderer_verbose = value
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .map(|level| level.min(3))
                        .or_else(|| parse_bool(value).map(u8::from))
                        .unwrap_or(settings.renderer_verbose);
                }),
                archive: Some((20, |state| {
                    let settings = state.video;
                    format!("{}", settings.renderer_verbose)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_verbose(&self) -> String {
                    self.video.renderer_verbose.to_string()
                }
                fn cvar_set_r_verbose(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let level = match Self::parse_console_bool(value) {
                            Some(enabled) => u8::from(enabled),
                            None => value
                                .trim()
                                .parse::<u8>()
                                .map_err(|_| format!("{name}: expected 0..3 (off/on are also accepted)"))?,
                        };
                        if level > 3 {
                            return Err(format!("{name}: expected 0..3"));
                        }
                        self.video.renderer_verbose = level;
                        crate::logging::set_renderer_verbose_level(level);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_VERTEXLIGHT {
        runtime: CvarDef::new("r_vertexLight", "", "0..1", "Use BSP vertex lighting instead of baked world lightmaps; as in JKA, runtime dlights are suppressed while enabled.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_vertexlight(app)), |app, value| App::cvar_set_r_vertexlight(app, APP_R_VERTEXLIGHT.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_vertexlight, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.vertex_lighting = parse_bool(value).unwrap_or(settings.vertex_lighting);
                }),
                archive: Some((167, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.vertex_lighting))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_vertexlight(&self) -> String {
                    bool_value(self.video.vertex_lighting)
                }
                fn cvar_set_r_vertexlight(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_vertexlight".to_owned();
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        match lower.as_str() {
                            "r_fullbright" => self.video.world_lighting = !enabled,
                            "r_vertexlight" => self.video.vertex_lighting = enabled,
                            "r_lightmap" => self.video.lightmap_only = enabled,
                            _ => unreachable!(),
                        }
                        self.sync_classic_world_lighting();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_VIGNETTE {
        runtime: CvarDef::new(
                "r_vignette",
                "",
                "0..1",
                "Vignette post effect.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_vignette(app)),
                |app, value| App::cvar_set_r_vignette(app, APP_R_VIGNETTE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_vignette, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.vignette = parse_bool(value).unwrap_or(settings.vignette)
                }),
                archive: Some((109, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.vignette))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_vignette(&self) -> String {
                    bool_value(self.video.vignette)
                }
                fn cvar_set_r_vignette(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_vignette".to_owned();
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

    APP_R_VOXELPROBEGI {
        runtime: CvarDef::new("r_voxelProbeGI", "", "0..1", "Voxel/probe indirect lighting; enabling it on a map prepared without GI requires vid_restart.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_voxelprobegi(app)), |app, value| App::cvar_set_r_voxelprobegi(app, APP_R_VOXELPROBEGI.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_voxelprobegi, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.voxel_probe_gi = parse_bool(value).unwrap_or(settings.voxel_probe_gi)
                    }
                }),
                archive: Some((143, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.voxel_probe_gi))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_voxelprobegi(&self) -> String {
                    bool_value(self.video.voxel_probe_gi)
                }
                fn cvar_set_r_voxelprobegi(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.voxel_probe_gi = Self::parse_cvar_bool(name, value)?;
                        self.sync_voxel_probe_gi();
                        self.mark_config_dirty();
                        if self.map_prepare_restart_required() {
                            self.console_status =
                                "VOXEL / PROBE GI: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                        }
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_WINDOWMAXIMIZED {
        runtime: CvarDef::new(
                "r_windowMaximized",
                "",
                "0..1",
                "Restore the window maximized on launch.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_windowmaximized(app)),
                |app, value| {
                    App::cvar_set_r_windowmaximized(
                        app,
                        APP_R_WINDOWMAXIMIZED.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_windowmaximized, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.window_maximized = parse_bool(value).unwrap_or(settings.window_maximized)
                    }
                }),
                archive: Some((5, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.window_maximized))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_windowmaximized(&self) -> String {
                    bool_value(self.video.window_maximized)
                }
                fn cvar_set_r_windowmaximized(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.window_maximized = Self::parse_cvar_bool(name, value)?;
                        if self.applied_fullscreen.is_windowed() {
                            if let Some(window) = &self.window {
                                window.set_maximized(self.video.window_maximized);
                            }
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_WINDOWX {
        runtime: CvarDef::new(
                "r_windowX",
                "",
                "desktop pixels",
                "Last windowed X position; auto means automatic.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_windowx(app)),
                |app, value| App::cvar_set_r_windowx(app, APP_R_WINDOWX.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_windowx, None),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    context.window_x = value.parse::<i32>().ok()
                }),
                archive: Some((3, |state| {
                    let settings = state.video;
                    let window_x = settings
                        .window_position
                        .map(|p| p[0].to_string())
                        .unwrap_or_else(|| "auto".into());
                    format!("{}", window_x)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_windowx(&self) -> String {
                    self.video
                        .window_position
                        .map_or_else(|| "auto".into(), |p| p[0].to_string())
                }
                fn cvar_set_r_windowx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_windowx".to_owned();
                    {
                        if value.eq_ignore_ascii_case("auto") {
                            self.video.window_position = None;
                        } else {
                            let parsed = value.trim().parse::<i32>().map_err(|_| {
                                format!("{name}: expected a desktop pixel coordinate or auto")
                            })?;
                            let mut position = self.video.window_position.unwrap_or([0, 0]);
                            if lower == "r_windowx" {
                                position[0] = parsed;
                            } else {
                                position[1] = parsed;
                            }
                            self.video.window_position = Some(position);
                            if self.applied_fullscreen.is_windowed() {
                                if let Some(window) = &self.window {
                                    window.set_outer_position(PhysicalPosition::new(
                                        position[0],
                                        position[1],
                                    ));
                                }
                            }
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_WINDOWY {
        runtime: CvarDef::new(
                "r_windowY",
                "",
                "desktop pixels",
                "Last windowed Y position; auto means automatic.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_windowy(app)),
                |app, value| App::cvar_set_r_windowy(app, APP_R_WINDOWY.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_windowy, None),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    context.window_y = value.parse::<i32>().ok()
                }),
                archive: Some((4, |state| {
                    let settings = state.video;
                    let window_y = settings
                        .window_position
                        .map(|p| p[1].to_string())
                        .unwrap_or_else(|| "auto".into());
                    format!("{}", window_y)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_windowy(&self) -> String {
                    self.video
                        .window_position
                        .map_or_else(|| "auto".into(), |p| p[1].to_string())
                }
                fn cvar_set_r_windowy(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_windowy".to_owned();
                    {
                        if value.eq_ignore_ascii_case("auto") {
                            self.video.window_position = None;
                        } else {
                            let parsed = value.trim().parse::<i32>().map_err(|_| {
                                format!("{name}: expected a desktop pixel coordinate or auto")
                            })?;
                            let mut position = self.video.window_position.unwrap_or([0, 0]);
                            if lower == "r_windowx" {
                                position[0] = parsed;
                            } else {
                                position[1] = parsed;
                            }
                            self.video.window_position = Some(position);
                            if self.applied_fullscreen.is_windowed() {
                                if let Some(window) = &self.window {
                                    window.set_outer_position(PhysicalPosition::new(
                                        position[0],
                                        position[1],
                                    ));
                                }
                            }
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_WORLDPATH {
        runtime: CvarDef::new("r_worldPath", "", "auto|unified", "World renderer: auto keeps the FastBaseline path when eligible; unified forces the general renderer (Minimal (Unified) preset).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_worldpath(app)), |app, value| App::cvar_set_r_worldpath(app, APP_R_WORLDPATH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_worldpath, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.force_unified_world = value.trim().eq_ignore_ascii_case("unified");
                }),
                archive: Some((22, |state| {
                    let settings = state.video;
                    format!(
                        "{}",
                        if settings.force_unified_world {
                            "unified"
                        } else {
                            "auto"
                        }
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_worldpath(&self) -> String {
                    if self.video.force_unified_world {
                        "unified"
                    } else {
                        "auto"
                    }
                    .to_owned()
                }
                fn cvar_set_r_worldpath(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.force_unified_world = match value.trim().to_ascii_lowercase().as_str() {
                            "auto" => false,
                            "unified" => true,
                            _ => return Err(format!("{name}: expected auto|unified")),
                        };
                        self.render_command(RenderCommand::SetForceUnifiedWorld(
                            self.video.force_unified_world,
                        ));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
}
