//! Material cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_DETAILTEXTURE {
        runtime: CvarDef::new(
                "r_detailtexture",
                "",
                "custom",
                "Controls r_detailtexture.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_detailtexture(app)),
                |app, value| {
                    App::cvar_set_r_detailtexture(app, APP_R_DETAILTEXTURE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_detailtexture, None),
        config: CvarBinding {
                default: Some(|_state| {
                    "auto".into()
                 }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_detailtexture(&self) -> String {
                    "auto".into()
                }
                fn cvar_set_r_detailtexture(&mut self, name: &str, _value: &str) -> Result<(), String> {
                    {
                        return Err(format!("{name}: detail texture selection is fixed to AUTO"));
                    }
                }
            }
        ]
    }

    APP_R_DETAILTEXTUREFADE {
        runtime: CvarDef::new(
                "r_detailTextureFade",
                "",
                "0|1",
                "Fade the synthetic detail contribution back to neutral with camera distance.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_detailtexturefade(app)),
                |app, value| {
                    App::cvar_set_r_detailtexturefade(
                        app,
                        APP_R_DETAILTEXTUREFADE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_detailtexturefade, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.detail_texture_fade =
                        parse_bool(value).unwrap_or(settings.detail_texture_fade);
                }),
                archive: Some((15, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.detail_texture_fade))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_detailtexturefade(&self) -> String {
                    bool_value(self.video.detail_texture_fade)
                }
                fn cvar_set_r_detailtexturefade(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.detail_texture_fade = match value.trim().to_ascii_lowercase().as_str() {
                            "1" | "on" | "true" | "yes" => true,
                            "0" | "off" | "false" | "no" => false,
                            _ => return Err(format!("{name}: expected 0 or 1")),
                        };
                        self.render_command(RenderCommand::SetDetailTextureFade {
                            enabled: self.video.detail_texture_fade,
                            distance: self.video.detail_texture_fade_distance,
                        });
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DETAILTEXTUREFADEDISTANCE {
        runtime: CvarDef::new(
                "r_detailTextureFadeDistance",
                "",
                "64..8192",
                "JKA-unit distance where the ported power-4 detail fade reaches zero detail contribution.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_detailtexturefadedistance(app)),
                |app, value| {
                    App::cvar_set_r_detailtexturefadedistance(
                        app,
                        APP_R_DETAILTEXTUREFADEDISTANCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_detailtexturefadedistance, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(distance) = value.parse::<f32>() {
                        if distance.is_finite() {
                            settings.detail_texture_fade_distance = distance.clamp(64.0, 8192.0);
                        }
                    }
                }),
                archive: Some((16, |state| {
                    let settings = state.video;
                    format!("{:.0}", settings.detail_texture_fade_distance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_detailtexturefadedistance(&self) -> String {
                    {
                        format!("{:.0}", self.video.detail_texture_fade_distance)
                    }
                }
                fn cvar_set_r_detailtexturefadedistance(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let distance = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 64..8192"))?;
                        if !distance.is_finite() {
                            return Err(format!("{name}: expected 64..8192"));
                        }
                        self.video.detail_texture_fade_distance = distance.clamp(64.0, 8192.0);
                        self.render_command(RenderCommand::SetDetailTextureFade {
                            enabled: self.video.detail_texture_fade,
                            distance: self.video.detail_texture_fade_distance,
                        });
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_AUTODETAILTEXTURES {
        runtime: CvarDef::new("r_autoDetailTextures", "0", "0|1", "Enable neutral 2x synthetic detail textures on eligible opaque BSP materials without authored detail stages.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_autodetailtextures(app)), |app, value| App::cvar_set_r_autodetailtextures(app, APP_R_AUTODETAILTEXTURES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_autodetailtextures, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    // Older configs stored the selected blend mode. All modes
                    // now map to the one remaining neutral 2x implementation.
                    settings.detail_textures = parse_bool(value).or_else(|| match value.trim().to_ascii_lowercase().as_str() {
                        "neutral2x" | "enhanced" | "softlight" | "linear2x" | "modulate2x"
                        | "dstcolor_one" | "dstcolorone" | "multiply" | "modulate" | "2" | "3" | "4" => Some(true),
                        _ => None,
                    }).unwrap_or(settings.detail_textures);
                }),
                archive: Some((14, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.detail_textures))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_autodetailtextures(&self) -> String {
                    bool_value(self.video.detail_textures)
                }
                fn cvar_set_r_autodetailtextures(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.video.detail_textures = parse_bool(value)
                        .ok_or_else(|| format!("{name}: expected 0 or 1"))?;
                    self.render_command(RenderCommand::SetDetailTextures(self.video.detail_textures));
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    // TaystJK rd-vanilla/tr_shader.cpp FinishShader: this is *not* the
    // synthetic detail effect. It removes only stages authored with `detail`.
    APP_R_DETAILTEXTURES {
        runtime: CvarDef::new(
            "r_detailTextures", "1", "0|1",
            "Enable shader stages explicitly marked `detail` (TaystJK legacy behavior). Applies on the next map load or vid_restart; r_autoDetailTextures controls synthetic BSP micro-detail separately.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart,
            |_app: &App| CvarValue::String(bool_value(crate::materials::legacy_detail_textures_enabled())),
            |app, value| App::cvar_set_r_detailtextures(app, APP_R_DETAILTEXTURES.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_r_detailtextures, None),
        config: CvarBinding {
            load_video: Some(|_settings, _name, value, _context| {
                if let Some(enabled) = parse_bool(value) {
                    crate::materials::set_legacy_detail_textures_enabled(enabled);
                }
            }),
            archive: Some((36375, |_state| format!("{}", u8::from(crate::materials::legacy_detail_textures_enabled())))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_r_detailtextures(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    if enabled != crate::materials::legacy_detail_textures_enabled() {
                        crate::materials::set_legacy_detail_textures_enabled(enabled);
                        // Resolved stage lists are immutable after map preparation.
                        self.prepared_map_cache = None;
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_R_EXT_TEXTURE_FILTER_ANISOTROPIC {
        runtime: CvarDef::new(
                "r_ext_texture_filter_anisotropic",
                "",
                "0|2|4|8|16",
                "Anisotropic texture filtering level.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ext_texture_filter_anisotropic(app)),
                |app, value| {
                    App::cvar_set_r_ext_texture_filter_anisotropic(
                        app,
                        APP_R_EXT_TEXTURE_FILTER_ANISOTROPIC.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_ext_texture_filter_anisotropic, None),
        config: CvarBinding {
                load_video: Some(|_settings, _name, value, context| {
                    context.anisotropy = value.parse::<f32>().ok().filter(|v| v.is_finite());
                }),
                archive: Some((13, |state| {
                    let settings = state.video;
                    let anisotropy = crate::config::cvars::anisotropy(settings.texture_filter);
                    format!("{}", anisotropy)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ext_texture_filter_anisotropic(&self) -> String {
                    match self.video.texture_filter {
                        TextureFilter::Anisotropic2x => "2".into(),
                        TextureFilter::Anisotropic4x => "4".into(),
                        TextureFilter::Anisotropic8x => "8".into(),
                        TextureFilter::Anisotropic16x => "16".into(),
                        _ => "0".into(),
                    }
                }
                fn cvar_set_r_ext_texture_filter_anisotropic(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let level = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 0, 2, 4, 8, or 16"))?;
                        self.video.texture_filter = match level {
                            0 | 1 => TextureFilter::Trilinear,
                            2 => TextureFilter::Anisotropic2x,
                            4 => TextureFilter::Anisotropic4x,
                            8 => TextureFilter::Anisotropic8x,
                            16 => TextureFilter::Anisotropic16x,
                            _ => return Err(format!("{name}: expected 0, 2, 4, 8, or 16")),
                        };
                        self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_EXT_COMPRESS_TEXTURES {
        runtime: CvarDef::new("r_ext_compress_textures", "", "0..2", "Texture compression at upload: 0 off (RGBA8), 1 BC1/BC3 like jamp, 2 also compresses 8-bit lightmaps. Applies on the next map load or vid_restart.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart, |app| CvarValue::String(App::cvar_get_r_ext_compress_textures(app)), |app, value| App::cvar_set_r_ext_compress_textures(app, APP_R_EXT_COMPRESS_TEXTURES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_ext_compress_textures, Some(|name, value| { value
                        .trim()
                        .parse::<u8>()
                        .map(|level| level.min(2).to_string())
                        .map_err(|_| format!("{name}: expected 0, 1 or 2")) }))
            .with_restart_requirement(|app| app.video.texture_compression != app.applied_texture_compression),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(level) = value.parse::<u8>() {
                        settings.texture_compression = level.min(2);
                    }
                }),
                apply_latched: Some(|settings, value| {
                    if let Ok(level) = value.parse::<u8>() {
                        settings.texture_compression = level.min(2);
                    }
                }),
                archive: Some((12, |state| {
                    let settings = state.video;
                    format!("{}", settings.texture_compression)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ext_compress_textures(&self) -> String {
                    self.video.texture_compression.to_string()
                }
                fn cvar_set_r_ext_compress_textures(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let level = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| format!("{name}: expected 0, 1 or 2"))?
                            .min(2);
                        self.video.texture_compression = level;
                        self.render_command(RenderCommand::SetTextureCompression(level));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_PICMIP {
        runtime: CvarDef::new("r_picmip", "", "0..16", "Classic texture-quality mip bias: omit this many highest map-texture mip levels on upload; also scales the cached Blue Water sky (2K base per main face, 2x2 down face).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::MapLoadOrVidRestart, |app| CvarValue::String(App::cvar_get_r_picmip(app)), |app, value| App::cvar_set_r_picmip(app, APP_R_PICMIP.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_picmip, Some(|name, value| { value
                        .trim()
                        .parse::<u32>()
                        .map(|picmip| picmip.min(16).to_string())
                        .map_err(|_| format!("{name}: expected an integer from 0 to 16")) })),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(picmip) = value.parse::<u32>() {
                        settings.picmip = picmip.min(16);
                    }
                }),
                apply_latched: Some(|settings, value| {
                    if let Ok(picmip) = value.parse::<u32>() {
                        settings.picmip = picmip.min(16);
                    }
                }),
                archive: Some((12, |state| {
                    let settings = state.video;
                    format!("{}", settings.picmip)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_picmip(&self) -> String {
                    self.video.picmip.to_string()
                }
                fn cvar_set_r_picmip(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let picmip = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected an integer from 0 to 16"))?
                            .min(16);
                        self.video.picmip = picmip;
                        self.render_command(RenderCommand::SetPicmip(picmip));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_TEXTUREMODE {
        runtime: CvarDef::new(
                "r_textureMode",
                "",
                "GL_* filter mode",
                "Base texture filtering mode.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_texturemode(app)),
                |app, value| {
                    App::cvar_set_r_texturemode(app, APP_R_TEXTUREMODE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_texturemode, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.texture_filter = match value.to_ascii_uppercase().as_str() {
                        "GL_NEAREST" | "GL_NEAREST_MIPMAP_NEAREST" | "GL_NEAREST_MIPMAP_LINEAR" => {
                            TextureFilter::Nearest
                        }
                        "GL_LINEAR" | "GL_LINEAR_MIPMAP_NEAREST" => TextureFilter::Bilinear,
                        "GL_LINEAR_MIPMAP_LINEAR" => TextureFilter::Trilinear,
                        _ => settings.texture_filter,
                    };
                }),
                archive: Some((11, |state| {
                    let settings = state.video;
                    let texture_mode = crate::config::cvars::texture_mode(settings.texture_filter);
                    format!("{}", texture_mode)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_texturemode(&self) -> String {
                    match self.video.texture_filter {
                        TextureFilter::Nearest => "GL_NEAREST".into(),
                        TextureFilter::Bilinear => "GL_LINEAR_MIPMAP_NEAREST".into(),
                        _ => "GL_LINEAR_MIPMAP_LINEAR".into(),
                    }
                }
                fn cvar_set_r_texturemode(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.texture_filter = match value.trim().to_ascii_uppercase().as_str() {
                            "GL_NEAREST" | "GL_NEAREST_MIPMAP_NEAREST" | "GL_NEAREST_MIPMAP_LINEAR" => {
                                TextureFilter::Nearest
                            }
                            "GL_LINEAR" | "GL_LINEAR_MIPMAP_NEAREST" => TextureFilter::Bilinear,
                            "GL_LINEAR_MIPMAP_LINEAR" => TextureFilter::Trilinear,
                            _ => return Err(format!("{name}: unknown texture mode")),
                        };
                        self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
}
