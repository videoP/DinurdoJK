//! Movement cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_CG_LOGSTRAFETRAIL {
        runtime: CvarDef::new(
                "cg_logStrafeTrail",
                "",
                "0 or trail name",
                "Record your active jaPRO race run to strafetrails/<name>.cfg on a background writer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_logstrafetrail(app)),
                |app, value| {
                    App::cvar_set_cg_logstrafetrail(
                        app,
                        APP_CG_LOGSTRAFETRAIL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_logstrafetrail, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.strafe_trail.log_name = value.trim().to_owned()
                }),
                archive: Some((279, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.strafe_trail.log_name)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_logstrafetrail(&self) -> String {
                    self.strafe_trails.settings.log_name.clone()
                }
                fn cvar_set_cg_logstrafetrail(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let active_dir =
                            jka_assets::pk3::game_write_directory(&self.base, self.game.as_deref());
                        self.strafe_trails.set_log_name(&active_dir, value)?;
                        self.strafe_trail_log_input = self.strafe_trails.settings.log_name.clone();
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVEMENTKEYS {
        runtime: CvarDef::new("cg_movementKeys", "", "0..4", "TaystJK movement-key HUD mode: 0 off, 1 original, 2 +attack, 3 compact centered, 4 compact movable.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_movementkeys(app)), |app, value| App::cvar_set_cg_movementkeys(app, APP_CG_MOVEMENTKEYS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_movementkeys, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(mode) = value.trim().parse::<u8>() {
                        settings.movement_keys.mode = mode.min(4);
                    }
                }),
                archive: Some((260, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.movement_keys.mode)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_movementkeys(&self) -> String {
                    self.movement_keys_hud.mode.to_string()
                }
                fn cvar_set_cg_movementkeys(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| "cg_movementKeys: expected 0..4".to_owned())?;
                        if mode > 4 {
                            return Err("cg_movementKeys: expected 0..4".to_owned());
                        }
                        self.movement_keys_hud.mode = mode;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVEMENTKEYSSIZE {
        runtime: CvarDef::new(
                "cg_movementKeysSize",
                "",
                "0.25..4",
                "TaystJK movement-key HUD scale.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_movementkeyssize(app)),
                |app, value| {
                    App::cvar_set_cg_movementkeyssize(
                        app,
                        APP_CG_MOVEMENTKEYSSIZE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_movementkeyssize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.movement_keys.size = finite()
                            .unwrap_or(settings.movement_keys.size)
                            .clamp(0.25, 4.0)
                    }
                }),
                archive: Some((263, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.movement_keys.size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_movementkeyssize(&self) -> String {
                    format!("{:.3}", self.movement_keys_hud.size)
                }
                fn cvar_set_cg_movementkeyssize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.movement_keys_hud.size =
                            Self::parse_cvar_finite(name, value)?.clamp(0.25, 4.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVEMENTKEYSWALK {
        runtime: CvarDef::new(
                "cg_movementKeysWalk",
                "",
                "0|1",
                "Show the walk/run key in the movement-key HUD.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_movementkeyswalk(app)),
                |app, value| {
                    App::cvar_set_cg_movementkeyswalk(
                        app,
                        APP_CG_MOVEMENTKEYSWALK.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_movementkeyswalk, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.movement_keys.walk =
                            parse_bool(value).unwrap_or(settings.movement_keys.walk)
                    }
                }),
                archive: Some((264, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.movement_keys.walk))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_movementkeyswalk(&self) -> String {
                    bool_value(self.movement_keys_hud.walk)
                }
                fn cvar_set_cg_movementkeyswalk(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.movement_keys_hud.walk = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVEMENTKEYSX {
        runtime: CvarDef::new(
                "cg_movementKeysX",
                "",
                "float",
                "TaystJK movement-key HUD horizontal offset.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_movementkeysx(app)),
                |app, value| {
                    App::cvar_set_cg_movementkeysx(app, APP_CG_MOVEMENTKEYSX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_movementkeysx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.movement_keys.x = finite()
                            .unwrap_or(settings.movement_keys.x)
                            .clamp(-640.0, 640.0)
                    }
                }),
                archive: Some((261, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.movement_keys.x)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_movementkeysx(&self) -> String {
                    format!("{:.3}", self.movement_keys_hud.x)
                }
                fn cvar_set_cg_movementkeysx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.movement_keys_hud.x =
                            Self::parse_cvar_finite(name, value)?.clamp(-640.0, 640.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVEMENTKEYSY {
        runtime: CvarDef::new(
                "cg_movementKeysY",
                "",
                "float",
                "TaystJK movement-key HUD vertical offset.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_movementkeysy(app)),
                |app, value| {
                    App::cvar_set_cg_movementkeysy(app, APP_CG_MOVEMENTKEYSY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_movementkeysy, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.movement_keys.y = finite()
                            .unwrap_or(settings.movement_keys.y)
                            .clamp(-480.0, 480.0)
                    }
                }),
                archive: Some((262, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.movement_keys.y)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_movementkeysy(&self) -> String {
                    format!("{:.3}", self.movement_keys_hud.y)
                }
                fn cvar_set_cg_movementkeysy(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.movement_keys_hud.y =
                            Self::parse_cvar_finite(name, value)?.clamp(-480.0, 480.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPER {
        runtime: CvarDef::new("cg_strafeHelper", "", "bitmask", "Strafehelper style/direction bitmask: TaystJK bits 0..21; Xonotic (1<<22), Q3 DeFRaG CGaz (1<<23); mode extras in 24..29; Warsow/Warfork triangular StrafeHUD bit 30.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_strafehelper(app)), |app, value| App::cvar_set_cg_strafehelper(app, APP_CG_STRAFEHELPER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_strafehelper, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(flags) = value.trim().parse::<u32>() {
                        settings.strafe_helper.flags = flags;
                    }
                }),
                archive: Some((265, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.strafe_helper.flags)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelper(&self) -> String {
                    self.strafe_helper.flags.to_string()
                }
                fn cvar_set_cg_strafehelper(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_helper.flags = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "cg_strafeHelper: expected a non-negative bitmask".to_owned())?;
                        ui::movement_components::normalize(&mut self.strafe_helper.flags,
                            &mut self.japro_cg.speedometer.flags);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPER_FPS {
        runtime: CvarDef::new("cg_strafeHelper_FPS", "", "0..1000", "Strafehelper physics FPS override; 0 uses movement cadence/server fixed Pmove for Xonotic/Q3DF/Warsow, and com_maxfps (125 uncapped) for TaystJK styles.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_strafehelper_fps(app)), |app, value| App::cvar_set_cg_strafehelper_fps(app, APP_CG_STRAFEHELPER_FPS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_strafehelper_fps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_helper.fps = finite()
                            .unwrap_or(settings.strafe_helper.fps)
                            .clamp(0.0, 1000.0)
                    }
                }),
                archive: Some((266, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_helper.fps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelper_fps(&self) -> String {
                    format!("{:.3}", self.strafe_helper.fps)
                }
                fn cvar_set_cg_strafehelper_fps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_helper.fps = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1000.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPERACTIVECOLOR {
        runtime: CvarDef::new(
                "cg_strafeHelperActiveColor",
                "",
                "R G B A",
                "TaystJK active Strafehelper line color.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelperactivecolor(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelperactivecolor(
                        app,
                        APP_CG_STRAFEHELPERACTIVECOLOR.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelperactivecolor, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Some(color) = parse_rgba8(value) {
                        settings.strafe_helper.active_color = color;
                    }
                }),
                archive: Some((271, |state| {
                    let presentation = state.presentation;
                    let [sr, sg, sb, sa] = presentation.strafe_helper.active_color;
                    format!("{sr} {sg} {sb} {sa}")
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelperactivecolor(&self) -> String {
                    {
                        let [r, g, b, a] = self.strafe_helper.active_color;
                        format!("{r} {g} {b} {a}")
                    }
                }
                fn cvar_set_cg_strafehelperactivecolor(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let values = value
                            .split_whitespace()
                            .map(str::parse::<i32>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| {
                                "cg_strafeHelperActiveColor: expected R G B A values from 0 to 255"
                                    .to_owned()
                            })?;
                        if values.len() != 4 {
                            return Err(
                                "cg_strafeHelperActiveColor: expected R G B A values from 0 to 255"
                                    .to_owned(),
                            );
                        }
                        self.strafe_helper.active_color = [
                            values[0].clamp(0, 255) as u8,
                            values[1].clamp(0, 255) as u8,
                            values[2].clamp(0, 255) as u8,
                            values[3].clamp(0, 255) as u8,
                        ];
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPERCUTOFF {
        runtime: CvarDef::new(
                "cg_strafeHelperCutoff",
                "",
                "0..480",
                "Strafehelper line cutoff.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelpercutoff(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelpercutoff(
                        app,
                        APP_CG_STRAFEHELPERCUTOFF.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelpercutoff, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_helper.cutoff = finite()
                            .unwrap_or(settings.strafe_helper.cutoff)
                            .clamp(0.0, 480.0)
                    }
                }),
                archive: Some((270, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_helper.cutoff)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelpercutoff(&self) -> String {
                    format!("{:.3}", self.strafe_helper.cutoff)
                }
                fn cvar_set_cg_strafehelpercutoff(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_helper.cutoff = Self::parse_cvar_finite(name, value)?.clamp(0.0, 480.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPERINACTIVEALPHA {
        runtime: CvarDef::new(
                "cg_strafeHelperInactiveAlpha",
                "",
                "0..255",
                "TaystJK inactive Strafehelper line alpha.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelperinactivealpha(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelperinactivealpha(
                        app,
                        APP_CG_STRAFEHELPERINACTIVEALPHA.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelperinactivealpha, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(alpha) = value.trim().parse::<i32>() {
                        settings.strafe_helper.inactive_alpha = alpha.clamp(0, 255) as u8;
                    }
                }),
                archive: Some((272, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.strafe_helper.inactive_alpha)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelperinactivealpha(&self) -> String {
                    self.strafe_helper.inactive_alpha.to_string()
                }
                fn cvar_set_cg_strafehelperinactivealpha(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_helper.inactive_alpha = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_strafeHelperInactiveAlpha: expected 0..255".to_owned())?
                            .clamp(0, 255) as u8;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPERLINEWIDTH {
        runtime: CvarDef::new(
                "cg_strafeHelperLineWidth",
                "",
                "0.25..5",
                "Strafehelper line width.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelperlinewidth(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelperlinewidth(
                        app,
                        APP_CG_STRAFEHELPERLINEWIDTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelperlinewidth, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_helper.line_width = finite()
                            .unwrap_or(settings.strafe_helper.line_width)
                            .clamp(0.25, 5.0)
                    }
                }),
                archive: Some((268, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_helper.line_width)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelperlinewidth(&self) -> String {
                    format!("{:.3}", self.strafe_helper.line_width)
                }
                fn cvar_set_cg_strafehelperlinewidth(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_helper.line_width =
                            Self::parse_cvar_finite(name, value)?.clamp(0.25, 5.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPEROFFSET {
        runtime: CvarDef::new(
                "cg_strafeHelperOffset",
                "",
                "float",
                "TaystJK Strafehelper angular offset in hundredths of a degree.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelperoffset(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelperoffset(
                        app,
                        APP_CG_STRAFEHELPEROFFSET.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelperoffset, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_helper.offset = finite()
                            .unwrap_or(settings.strafe_helper.offset)
                            .clamp(-1000.0, 1000.0)
                    }
                }),
                archive: Some((267, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_helper.offset)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelperoffset(&self) -> String {
                    format!("{:.3}", self.strafe_helper.offset)
                }
                fn cvar_set_cg_strafehelperoffset(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_helper.offset =
                            Self::parse_cvar_finite(name, value)?.clamp(-1000.0, 1000.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFEHELPERPRECISION {
        runtime: CvarDef::new(
                "cg_strafeHelperPrecision",
                "",
                "100..10000",
                "TaystJK Strafehelper projection precision/sensitivity.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafehelperprecision(app)),
                |app, value| {
                    App::cvar_set_cg_strafehelperprecision(
                        app,
                        APP_CG_STRAFEHELPERPRECISION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafehelperprecision, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.strafe_helper.precision = value
                            .trim()
                            .parse::<u32>()
                            .unwrap_or(settings.strafe_helper.precision)
                            .clamp(100, 10000)
                    }
                }),
                archive: Some((269, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.strafe_helper.precision)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafehelperprecision(&self) -> String {
                    self.strafe_helper.precision.to_string()
                }
                fn cvar_set_cg_strafehelperprecision(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_helper.precision = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| "cg_strafeHelperPrecision: expected 100..10000".to_owned())?
                            .clamp(100, 10000);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILDISTANCE {
        runtime: CvarDef::new(
                "cg_strafeTrailDistance",
                "",
                "256..131072",
                "DinurdoJK draw-distance cull for loaded/live strafe trails.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetraildistance(app)),
                |app, value| {
                    App::cvar_set_cg_strafetraildistance(
                        app,
                        APP_CG_STRAFETRAILDISTANCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetraildistance, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_trail.draw_distance = finite()
                            .unwrap_or(settings.strafe_trail.draw_distance)
                            .clamp(256.0, 131072.0)
                    }
                }),
                archive: Some((280, |state| {
                    let presentation = state.presentation;
                    format!("{:.1}", presentation.strafe_trail.draw_distance)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetraildistance(&self) -> String {
                    format!("{:.1}", self.strafe_trails.settings.draw_distance)
                }
                fn cvar_set_cg_strafetraildistance(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.draw_distance =
                            Self::parse_cvar_finite(name, value)?.clamp(256.0, 131_072.0);
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILFPS {
        runtime: CvarDef::new(
                "cg_strafeTrailFPS",
                "",
                "1..1000",
                "jaPRO SV_FPS metadata for recorded trails (used by legacy plum spacing).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetrailfps(app)),
                |app, value| {
                    App::cvar_set_cg_strafetrailfps(
                        app,
                        APP_CG_STRAFETRAILFPS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetrailfps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_trail.fps = finite()
                            .unwrap_or(settings.strafe_trail.fps)
                            .clamp(1.0, 1000.0)
                    }
                }),
                archive: Some((275, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_trail.fps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetrailfps(&self) -> String {
                    format!("{:.3}", self.strafe_trails.settings.fps)
                }
                fn cvar_set_cg_strafetrailfps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.fps =
                            Self::parse_cvar_finite(name, value)?.clamp(1.0, 1000.0);
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILGHOST {
        runtime: CvarDef::new(
                "cg_strafeTrailGhost",
                "",
                "0|1",
                "Draw strafe trails translucently.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetrailghost(app)),
                |app, value| {
                    App::cvar_set_cg_strafetrailghost(
                        app,
                        APP_CG_STRAFETRAILGHOST.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetrailghost, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.strafe_trail.ghost =
                            parse_bool(value).unwrap_or(settings.strafe_trail.ghost)
                    }
                }),
                archive: Some((277, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.strafe_trail.ghost))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetrailghost(&self) -> String {
                    bool_value(self.strafe_trails.settings.ghost)
                }
                fn cvar_set_cg_strafetrailghost(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.ghost = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILLIFE {
        runtime: CvarDef::new(
                "cg_strafeTrailLife",
                "",
                "0.1..3600 seconds",
                "jaPRO lifetime for live /strafeTrail player traces.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetraillife(app)),
                |app, value| {
                    App::cvar_set_cg_strafetraillife(
                        app,
                        APP_CG_STRAFETRAILLIFE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetraillife, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_trail.life_seconds = finite()
                            .unwrap_or(settings.strafe_trail.life_seconds)
                            .clamp(0.1, 3600.0)
                    }
                }),
                archive: Some((274, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_trail.life_seconds)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetraillife(&self) -> String {
                    format!("{:.3}", self.strafe_trails.settings.life_seconds)
                }
                fn cvar_set_cg_strafetraillife(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.life_seconds =
                            Self::parse_cvar_finite(name, value)?.clamp(0.1, 3600.0);
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILPLAYERS {
        runtime: CvarDef::new(
                "cg_strafeTrailPlayers",
                "",
                "32-bit mask",
                "jaPRO live-player strafe-trail bitmask; configure with /strafeTrail.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetrailplayers(app)),
                |app, value| {
                    App::cvar_set_cg_strafetrailplayers(
                        app,
                        APP_CG_STRAFETRAILPLAYERS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetrailplayers, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.strafe_trail.players = value
                            .trim()
                            .parse::<u32>()
                            .unwrap_or(settings.strafe_trail.players)
                    }
                }),
                archive: Some((278, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.strafe_trail.players)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetrailplayers(&self) -> String {
                    self.strafe_trails.settings.players.to_string()
                }
                fn cvar_set_cg_strafetrailplayers(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let mask = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected an unsigned integer bitmask"))?;
                        self.strafe_trails.set_players_mask(mask);
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILPLUMS {
        runtime: CvarDef::new(
                "cg_strafeTrailPlums",
                "",
                "0|1",
                "jaPRO whole-second number markers on loaded strafe trails.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetrailplums(app)),
                |app, value| {
                    App::cvar_set_cg_strafetrailplums(
                        app,
                        APP_CG_STRAFETRAILPLUMS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetrailplums, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.strafe_trail.plums =
                            parse_bool(value).unwrap_or(settings.strafe_trail.plums)
                    }
                }),
                archive: Some((276, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.strafe_trail.plums))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetrailplums(&self) -> String {
                    bool_value(self.strafe_trails.settings.plums)
                }
                fn cvar_set_cg_strafetrailplums(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.plums = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STRAFETRAILRADIUS {
        runtime: CvarDef::new(
                "cg_strafeTrailRadius",
                "",
                "0.1..100",
                "jaPRO strafe-trail line diameter.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_strafetrailradius(app)),
                |app, value| {
                    App::cvar_set_cg_strafetrailradius(
                        app,
                        APP_CG_STRAFETRAILRADIUS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_strafetrailradius, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.strafe_trail.radius = finite()
                            .unwrap_or(settings.strafe_trail.radius)
                            .clamp(0.1, 100.0)
                    }
                }),
                archive: Some((273, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.strafe_trail.radius)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_strafetrailradius(&self) -> String {
                    format!("{:.3}", self.strafe_trails.settings.radius)
                }
                fn cvar_set_cg_strafetrailradius(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.strafe_trails.settings.radius =
                            Self::parse_cvar_finite(name, value)?.clamp(0.1, 100.0);
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }
}
