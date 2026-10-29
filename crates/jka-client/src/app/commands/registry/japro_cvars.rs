//! Japro cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

settings_cvars! { JaproCgame;
    APP_CG_REMAPS {
        runtime: CvarDef::new(
                "cg_remaps",
                "1",
                "0 | 1 | other nonzero",
                "TaystJK shader-remap policy: 0 blocks incoming server remaps; 1 filters models/players/* from CS_SHADERSTATE; other non-zero values allow all. Local /remapShader is unaffected. Session-only; changes apply to subsequent server remaps immediately.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_remaps(app)),
                |app, value| App::cvar_set_cg_remaps(app, APP_CG_REMAPS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_remaps, None),
        // Not archived or loaded from old configs: always start each launch at 1.
        config: CvarBinding::NONE,
        settings: SETTINGS_CG_REMAPS = APP_CG_REMAPS
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.remaps.to_string()),
                    |state, value| JaproCgame::cvar_set_cg_remaps(state, APP_CG_REMAPS.entry.name, &value.to_string()),
                )
                .with_text_setter(JaproCgame::cvar_set_cg_remaps, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_remaps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|value| {
                        self.remaps = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                    })
                }
            }
            impl App {
                fn cvar_get_cg_remaps(&self) -> String {
                    (SETTINGS_CG_REMAPS.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_remaps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_REMAPS.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAINFX {
        runtime: CvarDef::new("cg_drainFX", "", "0..2", "Force-drain hand effect: 0 off, 1 stock mp/drain(wide).efx, 2 jaPRO mp/drain(wide)_japro.efx. Force lightning itself is unaffected.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_drainfx(app)), |app, value| App::cvar_set_cg_drainfx(app, APP_CG_DRAINFX.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_drainfx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((358, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.drain_fx)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_DRAINFX = APP_CG_DRAINFX
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.drain_fx.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_drainfx(
                            state,
                            APP_CG_DRAINFX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_drainfx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_drainfx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| self.drain_fx = v.clamp(0, 2) as u8)
                }
            }
            impl App {
                fn cvar_get_cg_drainfx(&self) -> String {
                    (SETTINGS_CG_DRAINFX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_drainfx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_DRAINFX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RENDERTOTEXTUREFX {
        runtime: CvarDef::new("cg_renderToTextureFX", "", "0 | 1 | 2 | 3 | 4 | 5", "0 TaystJK blue cloak; 1 TaystJK refraction; 2 Predator UV grid; 3 Predator object-space rings; 4 Predator world-space rings; 5 Predator view-space rings. Predator modes use experimental WGPU optics.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_rendertotexturefx(app)), |app, value| App::cvar_set_cg_rendertotexturefx(app, APP_CG_RENDERTOTEXTUREFX.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_rendertotexturefx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((359, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.render_to_texture_fx)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RENDERTOTEXTUREFX = APP_CG_RENDERTOTEXTUREFX
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.render_to_texture_fx.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_rendertotexturefx(
                            state,
                            APP_CG_RENDERTOTEXTUREFX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_rendertotexturefx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_rendertotexturefx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).and_then(|v| {
                        if v < 0 { self.render_to_texture_fx = 1; Ok(()) }
                        else if v <= 5 { self.render_to_texture_fx = v as u8; Ok(()) }
                        else { Err(format!("{name} accepts 0..5 (got {v})")) }
                    })
                }
            }
            impl App {
                fn cvar_get_cg_rendertotexturefx(&self) -> String {
                    (SETTINGS_CG_RENDERTOTEXTUREFX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_rendertotexturefx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RENDERTOTEXTUREFX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONFLAGALPHA {
        runtime: CvarDef::new("cg_thirdPersonFlagAlpha", "", "0..1", "Opacity of your carried flag in third person, including the followed player in demos.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_thirdpersonflagalpha(app)), |app, value| App::cvar_set_cg_thirdpersonflagalpha(app, APP_CG_THIRDPERSONFLAGALPHA.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_thirdpersonflagalpha, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((1000, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.third_person_flag_alpha)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_THIRDPERSONFLAGALPHA = APP_CG_THIRDPERSONFLAGALPHA
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.third_person_flag_alpha.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_thirdpersonflagalpha(
                            state,
                            APP_CG_THIRDPERSONFLAGALPHA.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_thirdpersonflagalpha, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_thirdpersonflagalpha(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.third_person_flag_alpha = v.clamp(0.0, 1.0))
                }
            }
            impl App {
                fn cvar_get_cg_thirdpersonflagalpha(&self) -> String {
                    (SETTINGS_CG_THIRDPERSONFLAGALPHA.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_thirdpersonflagalpha(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_THIRDPERSONFLAGALPHA.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_JUMPGOAL {
        runtime: CvarDef::new(
                "cg_jumpGoal",
                "",
                "speed",
                "First-jump pre-speed at or above which the readout turns green (0 = never).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_jumpgoal(app)),
                |app, value| App::cvar_set_cg_jumpgoal(app, APP_CG_JUMPGOAL.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_jumpgoal, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((353, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.jump_goal)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_JUMPGOAL = APP_CG_JUMPGOAL
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.jump_goal)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_jumpgoal(
                            state,
                            APP_CG_JUMPGOAL.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_jumpgoal, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_jumpgoal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.jump_goal = v.max(0.0))
                }
            }
            impl App {
                fn cvar_get_cg_jumpgoal(&self) -> String {
                    (SETTINGS_CG_JUMPGOAL.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_jumpgoal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_JUMPGOAL.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_LAGOMETER {
        runtime: CvarDef::new("cg_lagometer", "", "0..4", "jaPRO lagometer: 0 off (the connection-interrupted warning still shows), 1 graph, 2 graph with average ping and interpolation numbers, 3 numbers over a frameless graph, 4 (this client only) mode 3 plus packet loss and peak ping under the graph. Not shown against the local server.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_lagometer(app)), |app, value| App::cvar_set_cg_lagometer(app, APP_CG_LAGOMETER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_lagometer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((354, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.lagometer.mode)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_LAGOMETER = APP_CG_LAGOMETER
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.lagometer.mode.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_lagometer(
                            state,
                            APP_CG_LAGOMETER.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_lagometer, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_lagometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| {
                        self.lagometer.mode = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
                    })
                }
            }
            impl App {
                fn cvar_get_cg_lagometer(&self) -> String {
                    (SETTINGS_CG_LAGOMETER.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_lagometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_LAGOMETER.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_LAGOMETERX {
        runtime: CvarDef::new("cg_lagometerX", "", "640x480 px", "Distance of the lagometer's right edge from the right of the screen (also positions the old speed graph).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_lagometerx(app)), |app, value| App::cvar_set_cg_lagometerx(app, APP_CG_LAGOMETERX.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_lagometerx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((355, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.lagometer.x)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_LAGOMETERX = APP_CG_LAGOMETERX
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.lagometer.x.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_lagometerx(
                            state,
                            APP_CG_LAGOMETERX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_lagometerx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_lagometerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| {
                        self.lagometer.x = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
                    })
                }
            }
            impl App {
                fn cvar_get_cg_lagometerx(&self) -> String {
                    (SETTINGS_CG_LAGOMETERX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_lagometerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_LAGOMETERX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_LAGOMETERY {
        runtime: CvarDef::new("cg_lagometerY", "", "640x480 px", "Distance of the lagometer's bottom from the bottom of the screen (also positions the old speed graph).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_lagometery(app)), |app, value| App::cvar_set_cg_lagometery(app, APP_CG_LAGOMETERY.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_lagometery, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((356, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.lagometer.y)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_LAGOMETERY = APP_CG_LAGOMETERY
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.lagometer.y.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_lagometery(
                            state,
                            APP_CG_LAGOMETERY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_lagometery, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_lagometery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| {
                        self.lagometer.y = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
                    })
                }
            }
            impl App {
                fn cvar_get_cg_lagometery(&self) -> String {
                    (SETTINGS_CG_LAGOMETERY.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_lagometery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_LAGOMETERY.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACESTART {
        runtime: CvarDef::new(
                "cg_raceStart",
                "",
                "0|1",
                "Show the speed you crossed the race start line with.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racestart(app)),
                |app, value| {
                    App::cvar_set_cg_racestart(app, APP_CG_RACESTART.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racestart, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((341, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.japro.race_start))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACESTART = APP_CG_RACESTART
                .bind(
                    |state: &JaproCgame| CvarValue::String(u8::from(state.race_start).to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racestart(
                            state,
                            APP_CG_RACESTART.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racestart, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racestart(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value)
                        .map(|number| number != 0)
                        .map(|v| self.race_start = v)
                }
            }
            impl App {
                fn cvar_get_cg_racestart(&self) -> String {
                    (SETTINGS_CG_RACESTART.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racestart(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACESTART.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACESTARTX {
        runtime: CvarDef::new(
                "cg_raceStartX",
                "",
                "640x480 px",
                "Left edge of the start speed readout.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racestartx(app)),
                |app, value| {
                    App::cvar_set_cg_racestartx(app, APP_CG_RACESTARTX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racestartx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((342, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_start_x)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACESTARTX = APP_CG_RACESTARTX
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.race_start_x)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racestartx(
                            state,
                            APP_CG_RACESTARTX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racestartx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racestartx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.race_start_x = v.clamp(-640.0, 1280.0))
                }
            }
            impl App {
                fn cvar_get_cg_racestartx(&self) -> String {
                    (SETTINGS_CG_RACESTARTX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racestartx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACESTARTX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACESTARTY {
        runtime: CvarDef::new(
                "cg_raceStartY",
                "",
                "640x480 px",
                "Baseline of the start speed readout.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racestarty(app)),
                |app, value| {
                    App::cvar_set_cg_racestarty(app, APP_CG_RACESTARTY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racestarty, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((343, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_start_y)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACESTARTY = APP_CG_RACESTARTY
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.race_start_y)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racestarty(
                            state,
                            APP_CG_RACESTARTY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racestarty, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racestarty(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.race_start_y = v.clamp(-480.0, 960.0))
                }
            }
            impl App {
                fn cvar_get_cg_racestarty(&self) -> String {
                    (SETTINGS_CG_RACESTARTY.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racestarty(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACESTARTY.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACETIMER {
        runtime: CvarDef::new(
                "cg_raceTimer",
                "",
                "0..3",
                "jaPRO race timer: 0 off, 1 time, 2 time + max/avg/start speed, 3 same with milliseconds.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racetimer(app)),
                |app, value| {
                    App::cvar_set_cg_racetimer(app, APP_CG_RACETIMER.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racetimer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((337, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_timer)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACETIMER = APP_CG_RACETIMER
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.race_timer.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racetimer(
                            state,
                            APP_CG_RACETIMER.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racetimer, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racetimer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| self.race_timer = v.clamp(0, 3) as u8)
                }
            }
            impl App {
                fn cvar_get_cg_racetimer(&self) -> String {
                    (SETTINGS_CG_RACETIMER.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racetimer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACETIMER.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACETIMERSIZE {
        runtime: CvarDef::new(
                "cg_raceTimerSize",
                "",
                "0.1..3",
                "Text scale of the race timer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racetimersize(app)),
                |app, value| {
                    App::cvar_set_cg_racetimersize(app, APP_CG_RACETIMERSIZE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racetimersize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((338, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_timer_size)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACETIMERSIZE = APP_CG_RACETIMERSIZE
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.race_timer_size)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racetimersize(
                            state,
                            APP_CG_RACETIMERSIZE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racetimersize, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racetimersize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.race_timer_size = v.clamp(0.1, 3.0))
                }
            }
            impl App {
                fn cvar_get_cg_racetimersize(&self) -> String {
                    (SETTINGS_CG_RACETIMERSIZE.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racetimersize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACETIMERSIZE.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACETIMERX {
        runtime: CvarDef::new(
                "cg_raceTimerX",
                "",
                "640x480 px",
                "Left edge of the race timer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racetimerx(app)),
                |app, value| {
                    App::cvar_set_cg_racetimerx(app, APP_CG_RACETIMERX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racetimerx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((339, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_timer_x)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACETIMERX = APP_CG_RACETIMERX
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.race_timer_x)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racetimerx(
                            state,
                            APP_CG_RACETIMERX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racetimerx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racetimerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.race_timer_x = v.clamp(-640.0, 1280.0))
                }
            }
            impl App {
                fn cvar_get_cg_racetimerx(&self) -> String {
                    (SETTINGS_CG_RACETIMERX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racetimerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACETIMERX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACETIMERY {
        runtime: CvarDef::new(
                "cg_raceTimerY",
                "",
                "640x480 px",
                "Baseline of the race timer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racetimery(app)),
                |app, value| {
                    App::cvar_set_cg_racetimery(app, APP_CG_RACETIMERY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racetimery, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((340, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.race_timer_y)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_RACETIMERY = APP_CG_RACETIMERY
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.race_timer_y)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_racetimery(
                            state,
                            APP_CG_RACETIMERY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_racetimery, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_racetimery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.race_timer_y = v.clamp(-480.0, 960.0))
                }
            }
            impl App {
                fn cvar_get_cg_racetimery(&self) -> String {
                    (SETTINGS_CG_RACETIMERY.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_racetimery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_RACETIMERY.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPECHUD {
        runtime: CvarDef::new(
                "cg_specHud",
                "",
                "0|1",
                "While spectating or watching a demo, outline other living players through walls.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_spechud(app)),
                |app, value| {
                    App::cvar_set_cg_spechud(
                        app,
                        APP_CG_SPECHUD.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_spechud, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((419, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.japro.spec_hud))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPECHUD = APP_CG_SPECHUD
                .bind(
                    |state: &JaproCgame| CvarValue::String(u8::from(state.spec_hud).to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_spechud(
                            state,
                            APP_CG_SPECHUD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_spechud, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_spechud(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value)
                        .map(|number| number != 0)
                        .map(|v| self.spec_hud = v)
                }
            }
            impl App {
                fn cvar_get_cg_spechud(&self) -> String {
                    (SETTINGS_CG_SPECHUD.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_spechud(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPECHUD.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPECFOLLOWFASTEST {
        runtime: CvarDef::new(
                "cg_specFollowFastest",
                "",
                "0|1",
                "While spectating, keep following the fastest player (debounced).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_specfollowfastest(app)),
                |app, value| {
                    App::cvar_set_cg_specfollowfastest(
                        app,
                        APP_CG_SPECFOLLOWFASTEST.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_specfollowfastest, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((345, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.japro.follow_fastest))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPECFOLLOWFASTEST = APP_CG_SPECFOLLOWFASTEST
                .bind(
                    |state: &JaproCgame| CvarValue::String(u8::from(state.follow_fastest).to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_specfollowfastest(
                            state,
                            APP_CG_SPECFOLLOWFASTEST.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_specfollowfastest, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_specfollowfastest(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value)
                        .map(|number| number != 0)
                        .map(|v| self.follow_fastest = v)
                }
            }
            impl App {
                fn cvar_get_cg_specfollowfastest(&self) -> String {
                    (SETTINGS_CG_SPECFOLLOWFASTEST.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_specfollowfastest(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPECFOLLOWFASTEST.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETER {
        runtime: CvarDef::new("cg_speedometer", "", "bit mask", "jaPRO speedometer bits; configure with /speedometer <num>. 1 enable, 2 pre-speed, 4 jump height, 8 jump distance, 16 vertical speed, 32 yaw speed, 64 accel meter, 128 speed graph, 256 km/h, 512 mph, 1024 pre-speed jumps array, 2048 no colors, 4096/8192 array colors, 16384 old speed graph, 32768 XYZ speed.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_speedometer(app)), |app, value| App::cvar_set_cg_speedometer(app, APP_CG_SPEEDOMETER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_speedometer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((346, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.flags)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETER = APP_CG_SPEEDOMETER
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.speedometer.flags.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometer(
                            state,
                            APP_CG_SPEEDOMETER.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometer, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value)
                        .map(|v| self.speedometer.flags = v.clamp(0, i64::from(i32::MAX)) as u32)
                }
            }
            impl App {
                fn cvar_get_cg_speedometer(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETER.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETER.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    // The provider most recently selected wins, including console
                    // commands. Speed and acceleration remain independent.
                    if self.japro_cg.speedometer.flags & crate::speedometer::flag::ENABLE != 0 {
                        self.strafe_helper.flags &= !ui::SHELPER_Q3DF_SPEED;
                    }
                    if self.japro_cg.speedometer.flags & crate::speedometer::flag::ACCELMETER != 0 {
                        self.strafe_helper.flags &= !(ui::SHELPER_Q3DF_ACCEL | ui::SHELPER_ACCELMETER);
                    }
                    ui::movement_components::normalize(&mut self.strafe_helper.flags,
                        &mut self.japro_cg.speedometer.flags);
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERJUMPS {
        runtime: CvarDef::new(
                "cg_speedometerJumps",
                "",
                "0..511",
                "How many pre-speed jumps the jumps array keeps (bit 1024).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometerjumps(app)),
                |app, value| {
                    App::cvar_set_cg_speedometerjumps(
                        app,
                        APP_CG_SPEEDOMETERJUMPS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometerjumps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((350, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.jumps)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERJUMPS = APP_CG_SPEEDOMETERJUMPS
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.speedometer.jumps.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometerjumps(
                            state,
                            APP_CG_SPEEDOMETERJUMPS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometerjumps, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometerjumps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|v| self.speedometer.jumps = v.clamp(0, 511) as i32)
                }
            }
            impl App {
                fn cvar_get_cg_speedometerjumps(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERJUMPS.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometerjumps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERJUMPS.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERJUMPSX {
        runtime: CvarDef::new(
                "cg_speedometerJumpsX",
                "",
                "640x480 px",
                "Left edge of the pre-speed jumps array.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometerjumpsx(app)),
                |app, value| {
                    App::cvar_set_cg_speedometerjumpsx(
                        app,
                        APP_CG_SPEEDOMETERJUMPSX.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometerjumpsx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((351, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.jumps_x)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERJUMPSX = APP_CG_SPEEDOMETERJUMPSX
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.jumps_x)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometerjumpsx(
                            state,
                            APP_CG_SPEEDOMETERJUMPSX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometerjumpsx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometerjumpsx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.jumps_x = v.clamp(-640.0, 1280.0))
                }
            }
            impl App {
                fn cvar_get_cg_speedometerjumpsx(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERJUMPSX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometerjumpsx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERJUMPSX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERJUMPSY {
        runtime: CvarDef::new(
                "cg_speedometerJumpsY",
                "",
                "640x480 px",
                "Baseline of the pre-speed jumps array.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometerjumpsy(app)),
                |app, value| {
                    App::cvar_set_cg_speedometerjumpsy(
                        app,
                        APP_CG_SPEEDOMETERJUMPSY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometerjumpsy, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((352, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.jumps_y)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERJUMPSY = APP_CG_SPEEDOMETERJUMPSY
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.jumps_y)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometerjumpsy(
                            state,
                            APP_CG_SPEEDOMETERJUMPSY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometerjumpsy, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometerjumpsy(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.jumps_y = v.clamp(-480.0, 960.0))
                }
            }
            impl App {
                fn cvar_get_cg_speedometerjumpsy(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERJUMPSY.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometerjumpsy(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERJUMPSY.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERSIZE {
        runtime: CvarDef::new(
                "cg_speedometerSize",
                "",
                "0.1..3",
                "Text scale of the speedometer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometersize(app)),
                |app, value| {
                    App::cvar_set_cg_speedometersize(
                        app,
                        APP_CG_SPEEDOMETERSIZE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometersize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((349, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.size)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERSIZE = APP_CG_SPEEDOMETERSIZE
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.size)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometersize(
                            state,
                            APP_CG_SPEEDOMETERSIZE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometersize, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometersize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.size = v.clamp(0.1, 3.0))
                }
            }
            impl App {
                fn cvar_get_cg_speedometersize(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERSIZE.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometersize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERSIZE.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERX {
        runtime: CvarDef::new(
                "cg_speedometerX",
                "",
                "640x480 px",
                "Left edge of the speedometer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometerx(app)),
                |app, value| {
                    App::cvar_set_cg_speedometerx(app, APP_CG_SPEEDOMETERX.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometerx, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((347, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.x)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERX = APP_CG_SPEEDOMETERX
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.x)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometerx(
                            state,
                            APP_CG_SPEEDOMETERX.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometerx, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.x = v.clamp(-640.0, 1280.0))
                }
            }
            impl App {
                fn cvar_get_cg_speedometerx(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERX.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometerx(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERX.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPEEDOMETERY {
        runtime: CvarDef::new(
                "cg_speedometerY",
                "",
                "640x480 px",
                "Baseline of the speedometer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_speedometery(app)),
                |app, value| {
                    App::cvar_set_cg_speedometery(app, APP_CG_SPEEDOMETERY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_speedometery, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((348, |state| {
                    let presentation = state.presentation;
                    let speedometer = presentation.japro.speedometer;
                    format!("{}", speedometer.y)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SPEEDOMETERY = APP_CG_SPEEDOMETERY
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.speedometer.y)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_speedometery(
                            state,
                            APP_CG_SPEEDOMETERY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_speedometery, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_speedometery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.speedometer.y = v.clamp(-480.0, 960.0))
                }
            }
            impl App {
                fn cvar_get_cg_speedometery(&self) -> String {
                    (SETTINGS_CG_SPEEDOMETERY.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_speedometery(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SPEEDOMETERY.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STARTGOAL {
        runtime: CvarDef::new(
                "cg_startGoal",
                "",
                "speed",
                "The start speed readout turns green at or above this speed (0 = never).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_startgoal(app)),
                |app, value| {
                    App::cvar_set_cg_startgoal(app, APP_CG_STARTGOAL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_startgoal, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((344, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.start_goal)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_STARTGOAL = APP_CG_STARTGOAL
                .bind(
                    |state: &JaproCgame| CvarValue::String(format!("{}", state.start_goal)),
                    |state, value| {
                        JaproCgame::cvar_set_cg_startgoal(
                            state,
                            APP_CG_STARTGOAL.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_startgoal, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_startgoal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_float(name, value).map(|v| self.start_goal = v.max(0.0))
                }
            }
            impl App {
                fn cvar_get_cg_startgoal(&self) -> String {
                    (SETTINGS_CG_STARTGOAL.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_startgoal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_STARTGOAL.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CP_ALTDIMALPHA {
        runtime: CvarDef::new("cp_altDimAlpha", "", "1..255", "TaystJK JA+: alpha byte for players in the other dimension (default 40).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cp_altdimalpha(app)), |app, value| App::cvar_set_cp_altdimalpha(app, APP_CP_ALTDIMALPHA.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cp_altdimalpha, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((10426, |state| {
                    format!("{}", state.presentation.japro.alt_dim_alpha)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_ALTDIMALPHA = APP_CP_ALTDIMALPHA
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.alt_dim_alpha.to_string()),
                    |state, value| JaproCgame::cvar_set_cp_altdimalpha(state, APP_CP_ALTDIMALPHA.entry.name, &value.to_string()),
                )
                .with_text_setter(JaproCgame::cvar_set_cp_altdimalpha, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cp_altdimalpha(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value).map(|value| {
                        self.alt_dim_alpha = value.clamp(1, 255) as u8;
                    })
                }
            }
            impl App {
                fn cvar_get_cp_altdimalpha(&self) -> String {
                    (SETTINGS_CP_ALTDIMALPHA.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cp_altdimalpha(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_ALTDIMALPHA.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_STYLEPLAYER {
        runtime: CvarDef::new("cg_stylePlayer", "", "bit mask", "jaPRO player styling bits: 2 duel shell, 4 hide duelers, 8 hide racers in FFA, 16 hide non-racers while racing, 32 hide racers while racing, 64 solid racers, 128 solid FFA players while racing, 256 ghost duelers, 1024 hide non-duelers while dueling (stock servers), 65536 hide cosmetics, 131072 disable breathing effects, 1048576 seasonal cosmetics.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_styleplayer(app)), |app, value| App::cvar_set_cg_styleplayer(app, APP_CG_STYLEPLAYER.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_styleplayer, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((336, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.japro.style_player)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_STYLEPLAYER = APP_CG_STYLEPLAYER
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.style_player.to_string()),
                    |state, value| {
                        JaproCgame::cvar_set_cg_styleplayer(
                            state,
                            APP_CG_STYLEPLAYER.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cg_styleplayer, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cg_styleplayer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    japro_integer(name, value)
                        .map(|v| self.style_player = v.clamp(0, i64::from(i32::MAX)) as u32)
                }
            }
            impl App {
                fn cvar_get_cg_styleplayer(&self) -> String {
                    (SETTINGS_CG_STYLEPLAYER.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cg_styleplayer(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_STYLEPLAYER.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CL_COMMANDSIZE {
        runtime: CvarDef::new("cl_commandsize", "auto", "auto|64|128|256|512", "TaystJK-style command history window shared by prediction, network command selection and CONNECTION INTERRUPTED. AUTO scales with cl_commandRate and ping (minimum 64). Manual numbers snap UP to a supported power of two.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_commandsize(app)), |app, value| App::cvar_set_cl_commandsize(app, APP_CL_COMMANDSIZE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_commandsize, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.japro.set_cvar(name, value);
                }),
                archive: Some((357, |state| {
                    let presentation = state.presentation;
                    presentation.japro.lagometer.command_size_cvar()
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_COMMANDSIZE = APP_CL_COMMANDSIZE
                .bind(
                    |state: &JaproCgame| CvarValue::String(state.lagometer.command_size_cvar()),
                    |state, value| {
                        JaproCgame::cvar_set_cl_commandsize(
                            state,
                            APP_CL_COMMANDSIZE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(JaproCgame::cvar_set_cl_commandsize, None),
        implementations: [
            impl JaproCgame {
                fn cvar_set_cl_commandsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    if value.eq_ignore_ascii_case("auto") {
                        self.lagometer.command_size = crate::lagometer::AUTO_COMMAND_SIZE;
                        Ok(())
                    } else {
                        japro_integer(name, value).map(|v| {
                            self.lagometer.command_size =
                                crate::lagometer::Settings::normalize_command_size(v);
                        })
                    }
                }
            }
            impl App {
                fn cvar_get_cl_commandsize(&self) -> String {
                    (SETTINGS_CL_COMMANDSIZE.get)(&self.japro_cg).to_string()
                }
                fn cvar_set_cl_commandsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_COMMANDSIZE.set_text.unwrap())(&mut self.japro_cg, name, value)?;
                    if let Some(session) = self.game_session.as_mut() {
                        session.apply_client_options(
                            self.screen_shake,
                            self.audio.game,
                            self.video.footprints,
                            self.japro_cg,
                            self.network.plugin_disable,
                        );
                    }
                    self.mark_config_dirty();
                    self.publish_snapshot();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }
}
