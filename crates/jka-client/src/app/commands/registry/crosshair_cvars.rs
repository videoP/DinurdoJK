//! Crosshair cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_CG_CROSSHAIRCOLOR {
        runtime: CvarDef::new(
                "cg_crosshairColor",
                "",
                "R G B A (0..255)",
                "TaystJK-style crosshair RGBA color.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_crosshaircolor(app)),
                |app, value| {
                    App::cvar_set_cg_crosshaircolor(
                        app,
                        APP_CG_CROSSHAIRCOLOR.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_crosshaircolor, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Some(color) = parse_rgba8(value) {
                        settings.crosshair.color = color;
                    }
                }),
                archive: Some((233, |state| {
                    let presentation = state.presentation;
                    let [r, g, b, a] = presentation.crosshair.color;
                    format!("{r} {g} {b} {a}")
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshaircolor(&self) -> String {
                    {
                        let [r, g, b, a] = self.crosshair.color;
                        format!("{r} {g} {b} {a}")
                    }
                }
                fn cvar_set_cg_crosshaircolor(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let values = value
                            .split_whitespace()
                            .map(str::parse::<i32>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| {
                                "cg_crosshairColor: expected R G B A values from 0 to 255".to_owned()
                            })?;
                        if values.len() != 4 {
                            return Err(
                                "cg_crosshairColor: expected R G B A values from 0 to 255".to_owned()
                            );
                        }
                        self.crosshair.color = [
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

    APP_CG_CROSSHAIRIDENTIFYTARGET {
        runtime: CvarDef::new("cg_crosshairIdentifyTarget", "", "0..1", "jaPRO: colour the crosshair by what it is on (red enemy, green friend, yellow neutral, grey duelist). With 0 it keeps cg_crosshairColor.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_crosshairidentifytarget(app)), |app, value| App::cvar_set_cg_crosshairidentifytarget(app, APP_CG_CROSSHAIRIDENTIFYTARGET.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_crosshairidentifytarget, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.crosshair.identify_target =
                            parse_bool(value).unwrap_or(settings.crosshair.identify_target)
                    }
                }),
                archive: Some((235, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.crosshair.identify_target))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshairidentifytarget(&self) -> String {
                    bool_value(self.crosshair.identify_target)
                }
                fn cvar_set_cg_crosshairidentifytarget(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.crosshair.identify_target = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CROSSHAIRSCOPE {
        runtime: CvarDef::new(
            "cg_crossHairScope", "0", "0|1|2",
            "TaystJK Disruptor scope: 0 classic 4:3 scope and rotating insert (no crosshair), 1 normal crosshair without the scope mask, 2 full-screen scope mask (no crosshair). Ammo ticks and charge meter remain in all modes.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::Int(App::cvar_get_cg_crosshairscope(app)),
            |app, value| App::cvar_set_cg_crosshairscope(app, APP_CG_CROSSHAIRSCOPE.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_crosshairscope, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.crosshair.scope_mode = mode.min(crate::ui::CROSSHAIR_SCOPE_MAX);
                }
            }),
            archive: Some((2291, |state| state.presentation.crosshair.scope_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshairscope(&self) -> i32 {
                    i32::from(self.crosshair.scope_mode)
                }
                fn cvar_set_cg_crosshairscope(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let error = || format!("cg_crossHairScope: expected 0, 1 or {}", ui::CROSSHAIR_SCOPE_MAX);
                    let mode = value.trim().parse::<u8>().map_err(|_| error())?;
                    if mode > ui::CROSSHAIR_SCOPE_MAX { return Err(error()); }
                    self.crosshair.scope_mode = mode;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CROSSHAIRIMAGE {
        runtime: CvarDef::new("cg_crosshairImage", "", "0..10", "Image crosshair: 0 draws the cg_drawCrosshair shape; 1..10 draws gfx/2d/crosshaira..crosshairj (j needs japro-assets.pk3). cg_drawCrosshair 0 still hides it.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_crosshairimage(app)), |app, value| App::cvar_set_cg_crosshairimage(app, APP_CG_CROSSHAIRIMAGE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_crosshairimage, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(image) = value.trim().parse::<u8>() {
                        settings.crosshair.image = image.min(crate::ui::CROSSHAIR_IMAGE_COUNT);
                    }
                }),
                archive: Some((230, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.crosshair.image)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshairimage(&self) -> String {
                    self.crosshair.image.to_string()
                }
                fn cvar_set_cg_crosshairimage(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let error = || {
                            format!(
                                "cg_crosshairImage: expected an integer from 0 to {}",
                                ui::CROSSHAIR_IMAGE_COUNT
                            )
                        };
                        let image = value.trim().parse::<u8>().map_err(|_| error())?;
                        if image > ui::CROSSHAIR_IMAGE_COUNT {
                            return Err(error());
                        }
                        self.crosshair.image = image;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CROSSHAIRSIZE {
        runtime: CvarDef::new(
                "cg_crosshairSize",
                "",
                "4..96",
                "Crosshair size using the stock JKA cg_crosshairSize convention.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_crosshairsize(app)),
                |app, value| {
                    App::cvar_set_cg_crosshairsize(app, APP_CG_CROSSHAIRSIZE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_crosshairsize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.crosshair.size =
                            finite().unwrap_or(settings.crosshair.size).clamp(4.0, 96.0);
                    }
                }),
                archive: Some((231, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.crosshair.size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshairsize(&self) -> String {
                    format!("{:.3}", self.crosshair.size)
                }
                fn cvar_set_cg_crosshairsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.crosshair.size = Self::parse_cvar_finite(name, value)?.clamp(4.0, 96.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CROSSHAIRSTRENGTH {
        runtime: CvarDef::new("cg_crosshairStrength", "", "0..2", "Crosshair visibility. 1 is as authored; below fades it, above strengthens the faint stock image crosshairs.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_crosshairstrength(app)), |app, value| App::cvar_set_cg_crosshairstrength(app, APP_CG_CROSSHAIRSTRENGTH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_crosshairstrength, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.crosshair.strength = finite()
                            .unwrap_or(settings.crosshair.strength)
                            .clamp(0.0, crate::ui::CROSSHAIR_STRENGTH_MAX);
                    }
                }),
                archive: Some((232, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.crosshair.strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_crosshairstrength(&self) -> String {
                    format!("{:.3}", self.crosshair.strength)
                }
                fn cvar_set_cg_crosshairstrength(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.crosshair.strength =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, ui::CROSSHAIR_STRENGTH_MAX);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWCROSSHAIR {
        runtime: CvarDef::new("cg_drawCrosshair", "", "0..7", "Crosshair shape: 0 off; 1 classic; 2 dot; 3 plus; 4 classic+dot; 5 brackets; 6 box; 7 line (jaPRO strafehelper line crosshair). A non-zero cg_crosshairImage draws that image instead of the shape.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawcrosshair(app)), |app, value| App::cvar_set_cg_drawcrosshair(app, APP_CG_DRAWCROSSHAIR.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawcrosshair, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(style) = value.trim().parse::<u8>() {
                        settings.crosshair.style = style.min(crate::ui::CROSSHAIR_STYLE_MAX);
                    }
                }),
                archive: Some((229, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.crosshair.style)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawcrosshair(&self) -> String {
                    self.crosshair.style.to_string()
                }
                fn cvar_set_cg_drawcrosshair(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let error = || {
                            format!(
                                "cg_drawCrosshair: expected an integer from 0 to {}",
                                ui::CROSSHAIR_STYLE_MAX
                            )
                        };
                        let style = value.trim().parse::<u8>().map_err(|_| error())?;
                        if style > ui::CROSSHAIR_STYLE_MAX {
                            return Err(error());
                        }
                        self.crosshair.style = style;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWCROSSHAIRNAMES {
        runtime: CvarDef::new("cg_drawCrosshairNames", "", "float", "jaPRO: name of the player under the crosshair. 0 off; a positive value is the seconds the name stays after you aim away; a negative value shows it only while aimed at.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawcrosshairnames(app)), |app, value| App::cvar_set_cg_drawcrosshairnames(app, APP_CG_DRAWCROSSHAIRNAMES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawcrosshairnames, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.crosshair.names = finite()
                            .unwrap_or(settings.crosshair.names)
                            .clamp(-1000.0, 1000.0);
                    }
                }),
                archive: Some((236, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.crosshair.names)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawcrosshairnames(&self) -> String {
                    format!("{}", self.crosshair.names)
                }
                fn cvar_set_cg_drawcrosshairnames(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        // > 0: seconds a name lingers; < 0: only while aimed at.
                        self.crosshair.names = Self::parse_cvar_finite(name, value)?.clamp(-1000.0, 1000.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWCROSSHAIRNAMESCOLOURS {
        runtime: CvarDef::new("cg_drawCrosshairNamesColours", "", "0..1", "jaPRO: 1 draws the name with its own colour codes; 0 strips them and colours it red/green by friend or foe.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawcrosshairnamescolours(app)), |app, value| App::cvar_set_cg_drawcrosshairnamescolours(app, APP_CG_DRAWCROSSHAIRNAMESCOLOURS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawcrosshairnamescolours, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.crosshair.names_colours =
                            parse_bool(value).unwrap_or(settings.crosshair.names_colours)
                    }
                }),
                archive: Some((237, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.crosshair.names_colours))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawcrosshairnamescolours(&self) -> String {
                    bool_value(self.crosshair.names_colours)
                }
                fn cvar_set_cg_drawcrosshairnamescolours(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.crosshair.names_colours = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWCROSSHAIRNAMESOPACITY {
        runtime: CvarDef::new(
                "cg_drawCrosshairNamesOpacity",
                "",
                "0..1",
                "jaPRO: opacity of the crosshair name.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawcrosshairnamesopacity(app)),
                |app, value| {
                    App::cvar_set_cg_drawcrosshairnamesopacity(
                        app,
                        APP_CG_DRAWCROSSHAIRNAMESOPACITY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawcrosshairnamesopacity, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.crosshair.names_opacity = finite()
                            .unwrap_or(settings.crosshair.names_opacity)
                            .clamp(0.0, 1.0);
                    }
                }),
                archive: Some((238, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.crosshair.names_opacity)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawcrosshairnamesopacity(&self) -> String {
                    format!("{}", self.crosshair.names_opacity)
                }
                fn cvar_set_cg_drawcrosshairnamesopacity(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.crosshair.names_opacity =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWPLAYERNAMES {
        runtime: CvarDef::new("cg_drawPlayerNames", "", "0..2", "TaystJK: draw unobstructed player names above players. 0 off, 1 names, 2 names plus health bars.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawplayernames(app)), |app, value| App::cvar_set_cg_drawplayernames(app, APP_CG_DRAWPLAYERNAMES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawplayernames, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.player_names.mode = value
                        .trim()
                        .parse::<i32>()
                        .unwrap_or(settings.player_names.mode)
                        .clamp(0, 2);
                }),
                archive: Some((239, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.player_names.mode)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawplayernames(&self) -> String {
                    self.player_names.mode.to_string()
                }
                fn cvar_set_cg_drawplayernames(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_drawPlayerNames: expected 0, 1, or 2".to_owned())?;
                        if !(0..=2).contains(&mode) {
                            return Err("cg_drawPlayerNames: expected 0, 1, or 2".to_owned());
                        }
                        self.player_names.mode = mode;
                        if mode == 0 {
                            self.player_name_visible = [false; 32];
                        } else {
                            self.player_name_scan_at = Instant::now() - Duration::from_secs(1);
                        }
                        self.mark_config_dirty();
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWPLAYERNAMESSCALE {
        runtime: CvarDef::new(
                "cg_drawPlayerNamesScale",
                "",
                "0.05..4",
                "TaystJK: scale of world-space player-name labels.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawplayernamesscale(app)),
                |app, value| {
                    App::cvar_set_cg_drawplayernamesscale(
                        app,
                        APP_CG_DRAWPLAYERNAMESSCALE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawplayernamesscale, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.player_names.scale = finite()
                            .unwrap_or(settings.player_names.scale)
                            .clamp(0.05, 4.0);
                    }
                }),
                archive: Some((240, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.player_names.scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawplayernamesscale(&self) -> String {
                    format!("{:.3}", self.player_names.scale)
                }
                fn cvar_set_cg_drawplayernamesscale(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.player_names.scale = Self::parse_cvar_finite(name, value)?.clamp(0.05, 4.0);
                        self.mark_config_dirty();
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }
}
