//! Presentation cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_CG_DRAW2D {
        runtime: super::hud::APP_DRAW_2D.copy(),
        config: CvarBinding {
            load_video: Some(|settings, _, value, _| {
                settings.draw_2d = parse_bool(value).unwrap_or(settings.draw_2d);
            }),
            archive: Some((453, |state| {
                format!("{}", u8::from(state.video.draw_2d))
            })),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_DRAWHUD {
        runtime: super::hud::APP_DRAW_HUD.copy(),
        config: CvarBinding {
            load_video: Some(|settings, _, value, _| {
                settings.draw_hud = parse_bool(value).unwrap_or(settings.draw_hud);
            }),
            archive: Some((454, |state| {
                format!("{}", u8::from(state.video.draw_hud))
            })),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_DRAWFPS {
        runtime: super::hud::APP_DRAW_FPS.copy(),
        config: CvarBinding {
                load_video: Some(|settings, _, value, _| {
                    if let Ok(value) = value.parse::<u8>() {
                        settings.draw_fps = value.min(2);
                    }
                }),
                archive: Some((32, |state| {
                    let settings = state.video;
                    format!("{}", settings.draw_fps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [

        ]
    }

    APP_CG_DRAWSCORES {
        runtime: CvarDef::new("cg_drawScores", "", "0..3", "TaystJK score HUD: 0 off; 1 classic team scores; 2 classic with red/blue score colours; 3 centred team boxes (and duel HUD in GT_DUEL).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawscores(app)), |app, value| App::cvar_set_cg_drawscores(app, APP_CG_DRAWSCORES.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawscores, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.draw_scores = value
                            .trim()
                            .parse::<i32>()
                            .ok()
                            .map(|v| v.clamp(0, 3))
                            .unwrap_or(settings.draw_scores)
                    }
                }),
                archive: Some((378, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.draw_scores)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawscores(&self) -> String {
                    self.draw_scores.to_string()
                }
                fn cvar_set_cg_drawscores(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.draw_scores = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_drawScores expects 0..3".to_owned())?
                            .clamp(0, 3);
                        self.mini_scores_shown = self.mini_scores_ui();
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }


    APP_CG_DRAWSCORE {
        runtime: CvarDef::new("cg_drawScore", "2", "0..2", "TaystJK: score on the player HUD; 0 off, 1 score only, 2 includes team score and team lead/deficit.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(app.draw_score.to_string()), |app, value| App::cvar_set_draw_score(app, "cg_drawScore", &value.to_string())).with_text_setter(App::cvar_set_draw_score, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Ok(value) = value.trim().parse::<i32>() {
                    settings.draw_score = value.clamp(0, 2);
                }
            }),
            archive: Some((379, |state| state.presentation.draw_score.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_draw_score(&mut self, _: &str, value: &str) -> Result<(), String> {
                self.draw_score = value.trim().parse::<i32>().map_err(|_| "cg_drawScore expects 0..2".to_owned())?.clamp(0, 2);
                self.mark_config_dirty();
                self.publish_transient_ui();
                Ok(())
            }
        }]
    }

    APP_CG_DRAWSCOREX {
        runtime: CvarDef::new("cg_drawScoreX", "0", "float", "TaystJK: horizontal offset of the player-HUD score.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(app.draw_score_x.to_string()), |app, value| App::cvar_set_draw_score_x(app, "cg_drawScoreX", &value.to_string())).with_text_setter(App::cvar_set_draw_score_x, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Ok(value) = value.trim().parse::<f32>() {
                    if value.is_finite() { settings.draw_score_x = value; }
                }
            }),
            archive: Some((380, |state| state.presentation.draw_score_x.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_draw_score_x(&mut self, _: &str, value: &str) -> Result<(), String> {
                let offset = value.trim().parse::<f32>().map_err(|_| "cg_drawScoreX expects a float".to_owned())?;
                if !offset.is_finite() { return Err("cg_drawScoreX expects a finite float".into()); }
                self.draw_score_x = offset;
                self.mark_config_dirty();
                self.publish_transient_ui();
                Ok(())
            }
        }]
    }

    APP_CG_DRAWSCOREY {
        runtime: CvarDef::new("cg_drawScoreY", "0", "float", "TaystJK: vertical offset of the player-HUD score.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(app.draw_score_y.to_string()), |app, value| App::cvar_set_draw_score_y(app, "cg_drawScoreY", &value.to_string())).with_text_setter(App::cvar_set_draw_score_y, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Ok(value) = value.trim().parse::<f32>() {
                    if value.is_finite() { settings.draw_score_y = value; }
                }
            }),
            archive: Some((381, |state| state.presentation.draw_score_y.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_draw_score_y(&mut self, _: &str, value: &str) -> Result<(), String> {
                let offset = value.trim().parse::<f32>().map_err(|_| "cg_drawScoreY expects a float".to_owned())?;
                if !offset.is_finite() { return Err("cg_drawScoreY expects a finite float".into()); }
                self.draw_score_y = offset;
                self.mark_config_dirty();
                self.publish_transient_ui();
                Ok(())
            }
        }]
    }

    APP_CG_DRAWENEMYINFO {
        runtime: CvarDef::new("cg_drawEnemyInfo", "1", "0|1", "TaystJK: top-right duel opponent, leader, and Jedi Master information.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app: &App| CvarValue::String(u8::from(app.draw_enemy_info).to_string()), |app, value| App::cvar_set_draw_enemy_info(app, "cg_drawEnemyInfo", &value.to_string())).with_text_setter(App::cvar_set_draw_enemy_info, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                settings.draw_enemy_info = parse_bool(value).unwrap_or(settings.draw_enemy_info);
            }),
            archive: Some((382, |state| u8::from(state.presentation.draw_enemy_info).to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_draw_enemy_info(&mut self, _: &str, value: &str) -> Result<(), String> {
                self.draw_enemy_info = parse_bool(value).ok_or_else(|| "cg_drawEnemyInfo expects 0 or 1".to_owned())?;
                self.mark_config_dirty();
                self.publish_transient_ui();
                Ok(())
            }
        }]
    }

    APP_CG_DRAWTEAMOVERLAY {
        runtime: CvarDef::new("cg_drawTeamOverlay", "", "0..6", "TaystJK team overlay: 0 off; 1/2 classic, 3/4 alternate, 5/6 compact scalable; even modes omit your own row.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drawteamoverlay(app)), |app, value| App::cvar_set_cg_drawteamoverlay(app, APP_CG_DRAWTEAMOVERLAY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drawteamoverlay, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.team_overlay.mode = value
                            .trim()
                            .parse::<i32>()
                            .ok()
                            .map(|v| v.clamp(0, 6))
                            .unwrap_or(settings.team_overlay.mode)
                    }
                }),
                archive: Some((370, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.team_overlay.mode)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlay(&self) -> String {
                    self.team_overlay.mode.to_string()
                }
                fn cvar_set_cg_drawteamoverlay(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.team_overlay.mode = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_drawTeamOverlay expects 0..6".to_owned())?
                            .clamp(0, 6);
                        // CG_UpdateCvars parity: mirror the visual cvar into the hidden
                        // CVAR_USERINFO teamoverlay bit so the server sends/stops tinfo.
                        let requested = self.team_overlay.mode > 0;
                        if self.network.team_overlay != requested {
                            self.network.team_overlay = requested;
                            self.request_userinfo_send();
                        }
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYFORCE {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayForce",
                "",
                "0..1",
                "Show the jaPRO force-power column/bar in cg_drawTeamOverlay when the server provides it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlayforce(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlayforce(
                        app,
                        APP_CG_DRAWTEAMOVERLAYFORCE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlayforce, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.team_overlay.force =
                            parse_bool(value).unwrap_or(settings.team_overlay.force)
                    }
                }),
                archive: Some((376, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.team_overlay.force))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlayforce(&self) -> String {
                    bool_value(self.team_overlay.force)
                }
                fn cvar_set_cg_drawteamoverlayforce(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.team_overlay.force = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYMAXHP {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayMaxHP",
                "",
                ">=1",
                "Health-bar reference maximum for cg_drawTeamOverlay modes 5/6.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlaymaxhp(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlaymaxhp(
                        app,
                        APP_CG_DRAWTEAMOVERLAYMAXHP.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlaymaxhp, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.team_overlay.max_hp =
                            finite().unwrap_or(settings.team_overlay.max_hp).max(1.0)
                    }
                }),
                archive: Some((375, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.team_overlay.max_hp)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlaymaxhp(&self) -> String {
                    format!("{:.3}", self.team_overlay.max_hp)
                }
                fn cvar_set_cg_drawteamoverlaymaxhp(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.team_overlay.max_hp = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "cg_drawTeamOverlayMaxHP expects a number".to_owned())?
                            .max(1.0);
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYSCALE {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayScale",
                "",
                "0.5..2.5",
                "Scale for cg_drawTeamOverlay modes 5/6.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlayscale(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlayscale(
                        app,
                        APP_CG_DRAWTEAMOVERLAYSCALE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlayscale, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.team_overlay.scale = finite()
                            .unwrap_or(settings.team_overlay.scale)
                            .clamp(0.5, 2.5)
                    }
                }),
                archive: Some((374, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.team_overlay.scale)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlayscale(&self) -> String {
                    format!("{:.3}", self.team_overlay.scale)
                }
                fn cvar_set_cg_drawteamoverlayscale(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.team_overlay.scale = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| "cg_drawTeamOverlayScale expects a number".to_owned())?
                            .clamp(0.5, 2.5);
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYWEAPONS {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayWeapons",
                "",
                "0..1",
                "Show teammate weapon icons in cg_drawTeamOverlay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlayweapons(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlayweapons(
                        app,
                        APP_CG_DRAWTEAMOVERLAYWEAPONS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlayweapons, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.team_overlay.weapons =
                            parse_bool(value).unwrap_or(settings.team_overlay.weapons)
                    }
                }),
                archive: Some((373, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.team_overlay.weapons))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlayweapons(&self) -> String {
                    bool_value(self.team_overlay.weapons)
                }
                fn cvar_set_cg_drawteamoverlayweapons(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.team_overlay.weapons = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYX {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayX",
                "",
                "integer",
                "Horizontal anchor for the TaystJK team overlay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlayx(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlayx(
                        app,
                        APP_CG_DRAWTEAMOVERLAYX.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlayx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.team_overlay.x = value.trim().parse().unwrap_or(settings.team_overlay.x)
                    }
                }),
                archive: Some((371, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.team_overlay.x)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlayx(&self) -> String {
                    self.team_overlay.x.to_string()
                }
                fn cvar_set_cg_drawteamoverlayx(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.team_overlay.x = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_drawTeamOverlayX expects an integer".to_owned())?;
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTEAMOVERLAYY {
        runtime: CvarDef::new(
                "cg_drawTeamOverlayY",
                "",
                "integer",
                "Vertical anchor for the TaystJK team overlay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawteamoverlayy(app)),
                |app, value| {
                    App::cvar_set_cg_drawteamoverlayy(
                        app,
                        APP_CG_DRAWTEAMOVERLAYY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_drawteamoverlayy, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.team_overlay.y = value.trim().parse().unwrap_or(settings.team_overlay.y)
                    }
                }),
                archive: Some((372, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.team_overlay.y)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawteamoverlayy(&self) -> String {
                    self.team_overlay.y.to_string()
                }
                fn cvar_set_cg_drawteamoverlayy(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.team_overlay.y = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_drawTeamOverlayY expects an integer".to_owned())?;
                        self.mark_config_dirty();
                        self.publish_transient_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWTIMER {
        runtime: super::hud::APP_DRAW_TIMER.copy(),
        config: CvarBinding {
                load_video: Some(|settings, _, value, _| {
                    settings.draw_timer = parse_bool(value).unwrap_or(settings.draw_timer);
                }),
                archive: Some((33, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.draw_timer))
                })),
                ..CvarBinding::NONE
            },
        implementations: [

        ]
    }
}
