//! Color cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_R_COLORCONTRAST {
        runtime: CvarDef::new(
                "r_colorContrast",
                "",
                "0..2",
                "Global color-grading contrast; 1 is neutral.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorcontrast(app)),
                |app, value| {
                    App::cvar_set_r_colorcontrast(app, APP_R_COLORCONTRAST.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_colorcontrast, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.contrast = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 2.0))
                            .unwrap_or(settings.color_grade.contrast)
                    }
                }),
                archive: Some((149, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.contrast)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorcontrast(&self) -> String {
                    format!("{:.3}", self.video.color_grade.contrast)
                }
                fn cvar_set_r_colorcontrast(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.contrast =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLOREXPOSURE {
        runtime: CvarDef::new(
                "r_colorExposure",
                "",
                "-4..4",
                "Manual scene exposure in EV before tone mapping; +1 doubles scene light.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorexposure(app)),
                |app, value| {
                    App::cvar_set_r_colorexposure(app, APP_R_COLOREXPOSURE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_colorexposure, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.exposure_ev = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-4.0, 4.0))
                            .unwrap_or(settings.color_grade.exposure_ev)
                    }
                }),
                archive: Some((148, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.exposure_ev)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorexposure(&self) -> String {
                    format!("{:.3}", self.video.color_grade.exposure_ev)
                }
                fn cvar_set_r_colorexposure(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.exposure_ev =
                            Self::parse_cvar_finite(name, value)?.clamp(-4.0, 4.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORHIGHLIGHTS {
        runtime: CvarDef::new(
                "r_colorHighlights",
                "",
                "-100..100",
                "Highlight tone-curve strength (not EV); 0 is neutral. Preserves tonal ordering.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorhighlights(app)),
                |app, value| {
                    App::cvar_set_r_colorhighlights(
                        app,
                        APP_R_COLORHIGHLIGHTS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_colorhighlights, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.highlights = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-100.0, 100.0))
                            .unwrap_or(settings.color_grade.highlights)
                    }
                }),
                archive: Some((155, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.highlights)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorhighlights(&self) -> String {
                    format!("{:.3}", self.video.color_grade.highlights)
                }
                fn cvar_set_r_colorhighlights(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.highlights =
                            Self::parse_cvar_finite(name, value)?.clamp(-100.0, 100.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORLUT {
        runtime: CvarDef::new(
                "r_colorLut",
                "",
                "preset",
                "Color grading LUT: built-in film look or a .cube name from base/LUTs.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorlut(app)),
                |app, value| App::cvar_set_r_colorlut(app, APP_R_COLORLUT.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_colorlut, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.color_lut = ColorLutPreset::from_config(value).unwrap_or(settings.color_lut);
                }),
                archive: Some((125, |state| {
                    let settings = state.video;
                    format!("{}", settings.color_lut.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorlut(&self) -> String {
                    self.video.color_lut.config_value().to_owned()
                }
                fn cvar_set_r_colorlut(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let preset = ColorLutPreset::from_config(value)
                            .ok_or_else(|| format!("{name}: unknown LUT preset"))?;
                        self.set_color_lut(preset);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORLUTSTRENGTH {
        runtime: CvarDef::new(
                "r_colorLutStrength",
                "",
                "0..1",
                "Color grading LUT strength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorlutstrength(app)),
                |app, value| {
                    App::cvar_set_r_colorlutstrength(
                        app,
                        APP_R_COLORLUTSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_colorlutstrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.color_lut_strength = value
                        .parse::<f32>()
                        .ok()
                        .filter(|strength| strength.is_finite())
                        .map(|strength| strength.clamp(0.0, 1.0))
                        .unwrap_or(settings.color_lut_strength);
                }),
                archive: Some((126, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_lut_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorlutstrength(&self) -> String {
                    format!("{:.3}", self.video.color_lut_strength)
                }
                fn cvar_set_r_colorlutstrength(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let strength = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected 0..1"))?;
                        self.set_color_lut_strength(strength);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORSATURATION {
        runtime: CvarDef::new(
                "r_colorSaturation",
                "",
                "0..2",
                "Global color saturation; 1 is neutral and 0 is greyscale.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorsaturation(app)),
                |app, value| {
                    App::cvar_set_r_colorsaturation(
                        app,
                        APP_R_COLORSATURATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_colorsaturation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.saturation = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 2.0))
                            .unwrap_or(settings.color_grade.saturation)
                    }
                }),
                archive: Some((150, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.saturation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorsaturation(&self) -> String {
                    format!("{:.3}", self.video.color_grade.saturation)
                }
                fn cvar_set_r_colorsaturation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.saturation =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORSHADOWS {
        runtime: CvarDef::new(
                "r_colorShadows",
                "",
                "-100..100",
                "Shadow tone-curve strength (not EV); 0 is neutral. Preserves tonal ordering.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorshadows(app)),
                |app, value| {
                    App::cvar_set_r_colorshadows(app, APP_R_COLORSHADOWS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_colorshadows, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.shadows = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-100.0, 100.0))
                            .unwrap_or(settings.color_grade.shadows)
                    }
                }),
                archive: Some((154, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.shadows)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorshadows(&self) -> String {
                    format!("{:.3}", self.video.color_grade.shadows)
                }
                fn cvar_set_r_colorshadows(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.shadows =
                            Self::parse_cvar_finite(name, value)?.clamp(-100.0, 100.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORTEMPERATURE {
        runtime: CvarDef::new(
                "r_colorTemperature",
                "",
                "2000..11000",
                "Color-grading white-balance temperature in kelvin; 6500 is neutral.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colortemperature(app)),
                |app, value| {
                    App::cvar_set_r_colortemperature(
                        app,
                        APP_R_COLORTEMPERATURE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_colortemperature, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.temperature = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(2000.0, 11000.0))
                            .unwrap_or(settings.color_grade.temperature)
                    }
                }),
                archive: Some((152, |state| {
                    let settings = state.video;
                    format!("{:.0}", settings.color_grade.temperature)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colortemperature(&self) -> String {
                    format!("{:.0}", self.video.color_grade.temperature)
                }
                fn cvar_set_r_colortemperature(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.temperature =
                            Self::parse_cvar_finite(name, value)?.clamp(2000.0, 11000.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORTINT {
        runtime: CvarDef::new(
                "r_colorTint",
                "",
                "-1..1",
                "Green/magenta white-balance tint; negative is green, positive is magenta.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colortint(app)),
                |app, value| App::cvar_set_r_colortint(app, APP_R_COLORTINT.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_colortint, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.tint = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-1.0, 1.0))
                            .unwrap_or(settings.color_grade.tint)
                    }
                }),
                archive: Some((153, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.tint)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colortint(&self) -> String {
                    format!("{:.3}", self.video.color_grade.tint)
                }
                fn cvar_set_r_colortint(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.tint =
                            Self::parse_cvar_finite(name, value)?.clamp(-1.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_COLORVIBRANCE {
        runtime: CvarDef::new(
                "r_colorVibrance",
                "",
                "-1..1",
                "Selective saturation that favors muted colors; 0 is neutral.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_colorvibrance(app)),
                |app, value| {
                    App::cvar_set_r_colorvibrance(app, APP_R_COLORVIBRANCE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_colorvibrance, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.color_grade.vibrance = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-1.0, 1.0))
                            .unwrap_or(settings.color_grade.vibrance)
                    }
                }),
                archive: Some((151, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.color_grade.vibrance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_colorvibrance(&self) -> String {
                    format!("{:.3}", self.video.color_grade.vibrance)
                }
                fn cvar_set_r_colorvibrance(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.color_grade.vibrance =
                            Self::parse_cvar_finite(name, value)?.clamp(-1.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONING {
        runtime: CvarDef::new(
                "r_splitToning",
                "",
                "0..1",
                "Enable separate shadow and highlight color tints.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoning(app)),
                |app, value| {
                    App::cvar_set_r_splittoning(app, APP_R_SPLITTONING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_splittoning, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.enabled =
                            parse_bool(value).unwrap_or(settings.split_toning.enabled)
                    }
                }),
                archive: Some((156, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.split_toning.enabled))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoning(&self) -> String {
                    u8::from(self.video.split_toning.enabled).to_string()
                }
                fn cvar_set_r_splittoning(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.split_toning.enabled = Self::parse_cvar_bool(name, value)?;
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGBALANCE {
        runtime: CvarDef::new(
                "r_splitToningBalance",
                "",
                "-1..1",
                "Negative favors highlight toning; positive favors shadow toning.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoningbalance(app)),
                |app, value| {
                    App::cvar_set_r_splittoningbalance(
                        app,
                        APP_R_SPLITTONINGBALANCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoningbalance, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.balance = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(-1.0, 1.0))
                            .unwrap_or(settings.split_toning.balance)
                    }
                }),
                archive: Some((162, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.balance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoningbalance(&self) -> String {
                    format!("{:.3}", self.video.split_toning.balance)
                }
                fn cvar_set_r_splittoningbalance(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.split_toning.balance =
                            Self::parse_cvar_finite(name, value)?.clamp(-1.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGHIGHLIGHTHUE {
        runtime: CvarDef::new(
                "r_splitToningHighlightHue",
                "",
                "0..360",
                "Highlight tint hue in degrees.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoninghighlighthue(app)),
                |app, value| {
                    App::cvar_set_r_splittoninghighlighthue(
                        app,
                        APP_R_SPLITTONINGHIGHLIGHTHUE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoninghighlighthue, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.highlight_hue = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 360.0))
                            .unwrap_or(settings.split_toning.highlight_hue)
                    }
                }),
                archive: Some((160, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.highlight_hue)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoninghighlighthue(&self) -> String {
                    format!("{:.3}", self.video.split_toning.highlight_hue)
                }
                fn cvar_set_r_splittoninghighlighthue(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.split_toning.highlight_hue =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 360.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGHIGHLIGHTSATURATION {
        runtime: CvarDef::new(
                "r_splitToningHighlightSaturation",
                "",
                "0..1",
                "Highlight tint saturation; zero leaves highlights untoned.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoninghighlightsaturation(app)),
                |app, value| {
                    App::cvar_set_r_splittoninghighlightsaturation(
                        app,
                        APP_R_SPLITTONINGHIGHLIGHTSATURATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoninghighlightsaturation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.highlight_saturation = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.split_toning.highlight_saturation)
                    }
                }),
                archive: Some((161, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.highlight_saturation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoninghighlightsaturation(&self) -> String {
                    format!("{:.3}", self.video.split_toning.highlight_saturation)
                }
                fn cvar_set_r_splittoninghighlightsaturation(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.split_toning.highlight_saturation =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGSHADOWHUE {
        runtime: CvarDef::new(
                "r_splitToningShadowHue",
                "",
                "0..360",
                "Shadow tint hue in degrees.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoningshadowhue(app)),
                |app, value| {
                    App::cvar_set_r_splittoningshadowhue(
                        app,
                        APP_R_SPLITTONINGSHADOWHUE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoningshadowhue, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.shadow_hue = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 360.0))
                            .unwrap_or(settings.split_toning.shadow_hue)
                    }
                }),
                archive: Some((158, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.shadow_hue)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoningshadowhue(&self) -> String {
                    format!("{:.3}", self.video.split_toning.shadow_hue)
                }
                fn cvar_set_r_splittoningshadowhue(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.split_toning.shadow_hue =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 360.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGSHADOWSATURATION {
        runtime: CvarDef::new(
                "r_splitToningShadowSaturation",
                "",
                "0..1",
                "Shadow tint saturation; zero leaves shadows untoned.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoningshadowsaturation(app)),
                |app, value| {
                    App::cvar_set_r_splittoningshadowsaturation(
                        app,
                        APP_R_SPLITTONINGSHADOWSATURATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoningshadowsaturation, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.shadow_saturation = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.split_toning.shadow_saturation)
                    }
                }),
                archive: Some((159, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.shadow_saturation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoningshadowsaturation(&self) -> String {
                    format!("{:.3}", self.video.split_toning.shadow_saturation)
                }
                fn cvar_set_r_splittoningshadowsaturation(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.split_toning.shadow_saturation =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_SPLITTONINGSTRENGTH {
        runtime: CvarDef::new(
                "r_splitToningStrength",
                "",
                "0..1",
                "Overall split-toning strength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_splittoningstrength(app)),
                |app, value| {
                    App::cvar_set_r_splittoningstrength(
                        app,
                        APP_R_SPLITTONINGSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_splittoningstrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.split_toning.strength = value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .map(|v| v.clamp(0.0, 1.0))
                            .unwrap_or(settings.split_toning.strength)
                    }
                }),
                archive: Some((157, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.split_toning.strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_splittoningstrength(&self) -> String {
                    format!("{:.3}", self.video.split_toning.strength)
                }
                fn cvar_set_r_splittoningstrength(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.split_toning.strength =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.sync_post_effects();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICLIGHTBRIGHTNESS {
        runtime: CvarDef::new("r_dynamicLightBrightness", "", "0.5..3.0", "Runtime dynamic light (blasters, sabers, explosions) brightness on the r_gamma scale. Follows r_gamma while r_dynamicLightBrightnessLock is 1.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_dynamiclightbrightness(app)), |app, value| App::cvar_set_r_dynamiclightbrightness(app, APP_R_DYNAMICLIGHTBRIGHTNESS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_dynamiclightbrightness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<f32>() {
                        if value.is_finite() {
                            settings.dynamic_light_brightness = value.clamp(GAMMA_MIN, GAMMA_MAX);
                        }
                    }
                }),
                archive: Some((181, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.dynamic_light_brightness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamiclightbrightness(&self) -> String {
                    format!("{:.3}", self.video.dynamic_light_brightness)
                }
                fn cvar_set_r_dynamiclightbrightness(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let value = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.set_dynamic_light_brightness(value);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_DYNAMICLIGHTBRIGHTNESSLOCK {
        runtime: CvarDef::new(
                "r_dynamicLightBrightnessLock",
                "",
                "0..1",
                "1 links r_dynamicLightBrightness to r_gamma; 0 lets it be set independently.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_dynamiclightbrightnesslock(app)),
                |app, value| {
                    App::cvar_set_r_dynamiclightbrightnesslock(
                        app,
                        APP_R_DYNAMICLIGHTBRIGHTNESSLOCK.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_dynamiclightbrightnesslock, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.dynamic_light_brightness_locked =
                        parse_bool(value).unwrap_or(settings.dynamic_light_brightness_locked);
                }),
                archive: Some((182, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.dynamic_light_brightness_locked))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_dynamiclightbrightnesslock(&self) -> String {
                    {
                        bool_value(self.video.dynamic_light_brightness_locked)
                    }
                }
                fn cvar_set_r_dynamiclightbrightnesslock(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let locked = Self::parse_cvar_bool(name, value)?;
                        self.set_dynamic_light_brightness_locked(locked);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_GAMMA {
        runtime: CvarDef::new(
                "r_gamma",
                "",
                "0.5..3.0",
                "Display gamma/brightness adjustment.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_gamma(app)),
                |app, value| App::cvar_set_r_gamma(app, APP_R_GAMMA.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_gamma, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(gamma) = value.parse::<f32>() {
                        if gamma.is_finite() {
                            settings.gamma = gamma.clamp(GAMMA_MIN, GAMMA_MAX);
                        }
                    }
                }),
                archive: Some((36, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.gamma)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_gamma(&self) -> String {
                    format!("{:.3}", self.video.gamma)
                }
                fn cvar_set_r_gamma(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.set_gamma(Self::parse_cvar_finite(name, value)?);
                    Ok(())
                }
            }
        ]
    }

    APP_R_GAMMAMETHOD {
        runtime: CvarDef::new("r_gammaMethod", "", "shader | baked | hardware | hardware_scene", "Brightness strategy: shader post, baked textures, Windows SDR display ramp, or hardware ramp with UI counter-compensation. Also accepts 0, 1, 2, 3.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_gammamethod(app)), |app, value| App::cvar_set_r_gammamethod(app, APP_R_GAMMAMETHOD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_gammamethod, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Some(method) = crate::gamma::GammaMethod::parse(value) {
                        settings.gamma_method = method;
                    }
                }),
                archive: Some((147, |state| {
                    let settings = state.video;
                    format!("{}", settings.gamma_method.config_value())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_gammamethod(&self) -> String {
                    self.video.gamma_method.config_value().to_owned()
                }
                fn cvar_set_r_gammamethod(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let method = crate::gamma::GammaMethod::parse(value).ok_or_else(|| {
                            format!("{name}: expected shader, baked, hardware, hardware_scene (or 0, 1, 2, 3)")
                        })?;
                        self.set_gamma_method(method);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MODELBRIGHTNESS {
        runtime: CvarDef::new("r_modelBrightness", "", "0.5..3.0", "Model lighting brightness on the r_gamma scale (like r_ambientScale). Follows r_gamma while r_modelBrightnessLock is 1.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_r_modelbrightness(app)), |app, value| App::cvar_set_r_modelbrightness(app, APP_R_MODELBRIGHTNESS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_r_modelbrightness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<f32>() {
                        if value.is_finite() {
                            settings.model_brightness = value.clamp(GAMMA_MIN, GAMMA_MAX);
                        }
                    }
                }),
                archive: Some((179, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.model_brightness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_modelbrightness(&self) -> String {
                    format!("{:.3}", self.video.model_brightness)
                }
                fn cvar_set_r_modelbrightness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let value = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.set_model_brightness(value);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_MODELBRIGHTNESSLOCK {
        runtime: CvarDef::new(
                "r_modelBrightnessLock",
                "",
                "0..1",
                "1 links r_modelBrightness to r_gamma; 0 lets it be set independently.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_modelbrightnesslock(app)),
                |app, value| {
                    App::cvar_set_r_modelbrightnesslock(
                        app,
                        APP_R_MODELBRIGHTNESSLOCK.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_modelbrightnesslock, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.model_brightness_locked =
                        parse_bool(value).unwrap_or(settings.model_brightness_locked);
                }),
                archive: Some((180, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.model_brightness_locked))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_modelbrightnesslock(&self) -> String {
                    bool_value(self.video.model_brightness_locked)
                }
                fn cvar_set_r_modelbrightnesslock(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let locked = Self::parse_cvar_bool(name, value)?;
                        self.set_model_brightness_locked(locked);
                    }
                    Ok(())
                }
            }
        ]
    }
}
