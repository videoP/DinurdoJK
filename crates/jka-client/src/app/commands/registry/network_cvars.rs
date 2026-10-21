//! Network cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

settings_cvars! { NetworkSettings;
    APP_CG_DISPLAYCAMERAPOSITION {
        runtime: CvarDef::new(
                "cg_displayCameraPosition",
                "",
                "read-only",
                "jaPRO userinfo: cg_thirdPerson, range and vertical offset (kept current automatically).",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_displaycameraposition(app)),
                |app, value| {
                    App::cvar_set_cg_displaycameraposition(
                        app,
                        APP_CG_DISPLAYCAMERAPOSITION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_displaycameraposition, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_DISPLAYCAMERAPOSITION = APP_CG_DISPLAYCAMERAPOSITION
                    .bind(
                        |state: &NetworkSettings| CvarValue::String(state.display_camera_position.clone()),
                        |state, value| {
                            NetworkSettings::cvar_set_cg_displaycameraposition(
                                state,
                                APP_CG_DISPLAYCAMERAPOSITION.entry.name,
                                &value.to_string(),
                            )
                        },
                    )
                    .with_text_setter(NetworkSettings::cvar_set_cg_displaycameraposition, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_displaycameraposition(
                    &mut self,
                    name: &str,
                    _value: &str,
                ) -> Result<(), String> {
                    ({ Err(format!("{name} is read-only")) }).map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_displaycameraposition(&self) -> String {
                    (SETTINGS_CG_DISPLAYCAMERAPOSITION.get)(&self.network).to_string()
                }
                fn cvar_set_cg_displaycameraposition(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (SETTINGS_CG_DISPLAYCAMERAPOSITION.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_DISPLAYNETSETTINGS {
        runtime: CvarDef::new(
                "cg_displayNetSettings",
                "",
                "read-only",
                "jaPRO userinfo: cl_maxPackets, cl_timeNudge and com_maxFPS (kept current automatically).",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_displaynetsettings(app)),
                |app, value| {
                    App::cvar_set_cg_displaynetsettings(
                        app,
                        APP_CG_DISPLAYNETSETTINGS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_displaynetsettings, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_DISPLAYNETSETTINGS = APP_CG_DISPLAYNETSETTINGS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.display_net_settings.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_displaynetsettings(
                            state,
                            APP_CG_DISPLAYNETSETTINGS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_displaynetsettings, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_displaynetsettings(
                    &mut self,
                    name: &str,
                    _value: &str,
                ) -> Result<(), String> {
                    ({ Err(format!("{name} is read-only")) }).map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_displaynetsettings(&self) -> String {
                    (SETTINGS_CG_DISPLAYNETSETTINGS.get)(&self.network).to_string()
                }
                fn cvar_set_cg_displaynetsettings(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (SETTINGS_CG_DISPLAYNETSETTINGS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_ERRORDECAY {
        runtime: CvarDef::new(
                "cg_errorDecay",
                "",
                "0..500",
                "Milliseconds over which prediction corrections are smoothed.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_errordecay(app)),
                |app, value| {
                    App::cvar_set_cg_errordecay(app, APP_CG_ERRORDECAY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_errordecay, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((327, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.error_decay)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_ERRORDECAY = APP_CG_ERRORDECAY
                .bind(
                    |state: &NetworkSettings| CvarValue::String(format!("{}", state.error_decay)),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_errordecay(
                            state,
                            APP_CG_ERRORDECAY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_errordecay, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_errordecay(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .map(|v| {
                            self.error_decay = v.clamp(0.0, 500.0);
                            false
                        })
                        .ok_or_else(|| format!("{name}: expected a number")))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_errordecay(&self) -> String {
                    (SETTINGS_CG_ERRORDECAY.get)(&self.network).to_string()
                }
                fn cvar_set_cg_errordecay(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_ERRORDECAY.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_GROUNDTRACEDEBUG {
        runtime: CvarDef::new("cg_groundTraceDebug", "", "0..2", "Print this client's ground-trace predictions: 1 = ground changes (takeoff/landing/entity) and mismatches against the server snapshot for the same command, 2 = every new command.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_groundtracedebug(app)), |app, value| App::cvar_set_cg_groundtracedebug(app, APP_CG_GROUNDTRACEDEBUG.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_groundtracedebug, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_GROUNDTRACEDEBUG = APP_CG_GROUNDTRACEDEBUG
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.ground_trace_debug.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_groundtracedebug(
                            state,
                            APP_CG_GROUNDTRACEDEBUG.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_groundtracedebug, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_groundtracedebug(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.ground_trace_debug = atoi(value.as_bytes()).clamp(0, 2) as u8;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_groundtracedebug(&self) -> String {
                    (SETTINGS_CG_GROUNDTRACEDEBUG.get)(&self.network).to_string()
                }
                fn cvar_set_cg_groundtracedebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_GROUNDTRACEDEBUG.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_HITCHRECORD {
        runtime: CvarDef::new("cg_hitchrecord", "", "0..1", "Keep a rolling buffer of per-frame prediction/view state for `hitchmark`, and print ^3HITCH^7 lines to console when one is detected. 0 disables both.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_hitchrecord(app)), |app, value| App::cvar_set_cg_hitchrecord(app, APP_CG_HITCHRECORD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_hitchrecord, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_HITCHRECORD = APP_CG_HITCHRECORD
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.hitch_record).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_hitchrecord(
                            state,
                            APP_CG_HITCHRECORD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_hitchrecord, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_hitchrecord(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.hitch_record = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_hitchrecord(&self) -> String {
                    (SETTINGS_CG_HITCHRECORD.get)(&self.network).to_string()
                }
                fn cvar_set_cg_hitchrecord(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_HITCHRECORD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_MODELFRAMEDEBUG {
        runtime: CvarDef::new("cg_modelFrameDebug", "", "0..1", "Log head, saber hilt and blade anchors for every presented render frame to <game>/hitch/model-frames-*.csv, with JKA world coordinates and screen pixels. Set 0 to finish and flush.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_modelframedebug(app)), |app, value| App::cvar_set_cg_modelframedebug(app, APP_CG_MODELFRAMEDEBUG.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_modelframedebug, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_MODELFRAMEDEBUG = APP_CG_MODELFRAMEDEBUG
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.model_frame_debug).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cg_modelframedebug(
                            state,
                            APP_CG_MODELFRAMEDEBUG.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_modelframedebug, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_modelframedebug(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.model_frame_debug = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_modelframedebug(&self) -> String {
                    (SETTINGS_CG_MODELFRAMEDEBUG.get)(&self.network).to_string()
                }
                fn cvar_set_cg_modelframedebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_MODELFRAMEDEBUG.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_NOPREDICT {
        runtime: CvarDef::new(
                "cg_noPredict",
                "",
                "0..1",
                "Disable client-side movement prediction (use interpolated server state).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_nopredict(app)),
                |app, value| {
                    App::cvar_set_cg_nopredict(app, APP_CG_NOPREDICT.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_nopredict, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((328, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.no_predict))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_NOPREDICT = APP_CG_NOPREDICT
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.no_predict).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_nopredict(
                            state,
                            APP_CG_NOPREDICT.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_nopredict, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_nopredict(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.no_predict = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_nopredict(&self) -> String {
                    (SETTINGS_CG_NOPREDICT.get)(&self.network).to_string()
                }
                fn cvar_set_cg_nopredict(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_NOPREDICT.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_PHYSICSDIAG {
        runtime: CvarDef::new("cg_physicsDiag", "", "0..2", "Replay each snapshot interval from the previous snapshot and compare with the server's state: 1 = print intervals that do not match exactly (origin/velocity/ground/flags deltas), 2 = print every one. Isolates pure physics mismatch (e.g. at cl_commandRate 1000).", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_physicsdiag(app)), |app, value| App::cvar_set_cg_physicsdiag(app, APP_CG_PHYSICSDIAG.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_physicsdiag, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_PHYSICSDIAG = APP_CG_PHYSICSDIAG
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.physics_diag.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_physicsdiag(
                            state,
                            APP_CG_PHYSICSDIAG.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_physicsdiag, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_physicsdiag(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.physics_diag = atoi(value.as_bytes()).clamp(0, 2) as u8;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_physicsdiag(&self) -> String {
                    (SETTINGS_CG_PHYSICSDIAG.get)(&self.network).to_string()
                }
                fn cvar_set_cg_physicsdiag(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_PHYSICSDIAG.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_CORRECTPREDICTEDEVENTS {
        runtime: CvarDef::new(
                "cg_correctPredictedEvents",
                "",
                "0..1",
                "Re-fire a predicted event when a replay or snapshot puts a different event at a sequence index this client already played (OpenJK CG_CheckChangedPredictableEvents). Raven shipped that call commented out and every JKA fork inherited it, so 0 matches them and a mispredicted one-shot keeps whatever played first; Quake 3 ran it. 1 corrects it, which can play a sound or effect those builds never played. Never affects Pmove or the sent command.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_correctpredictedevents(app)),
                |app, value| {
                    App::cvar_set_cg_correctpredictedevents(
                        app,
                        APP_CG_CORRECTPREDICTEDEVENTS.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_correctpredictedevents, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((424, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.correct_predicted_events))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_CORRECTPREDICTEDEVENTS = APP_CG_CORRECTPREDICTEDEVENTS
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.correct_predicted_events).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cg_correctpredictedevents(
                            state,
                            APP_CG_CORRECTPREDICTEDEVENTS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_correctpredictedevents, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_correctpredictedevents(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.correct_predicted_events = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_correctpredictedevents(&self) -> String {
                    (SETTINGS_CG_CORRECTPREDICTEDEVENTS.get)(&self.network).to_string()
                }
                fn cvar_set_cg_correctpredictedevents(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_CORRECTPREDICTEDEVENTS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    if let Some(session) = self.game_session.as_mut() {
                        session
                            .client_game
                            .set_predicted_event_correction(self.network.correct_predicted_events);
                    }

                    Ok(())
                }
            }
        ]
    }

    APP_CG_PREDICTIONDEBUG {
        runtime: CvarDef::new(
                "cg_predictionDebug",
                "",
                "0..1",
                "Show live prediction/ground-trace diagnostics and print rich miss details.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_predictiondebug(app)),
                |app, value| {
                    App::cvar_set_cg_predictiondebug(
                        app,
                        APP_CG_PREDICTIONDEBUG.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_predictiondebug, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_PREDICTIONDEBUG = APP_CG_PREDICTIONDEBUG
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.prediction_debug).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cg_predictiondebug(
                            state,
                            APP_CG_PREDICTIONDEBUG.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_predictiondebug, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_predictiondebug(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.prediction_debug = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_predictiondebug(&self) -> String {
                    (SETTINGS_CG_PREDICTIONDEBUG.get)(&self.network).to_string()
                }
                fn cvar_set_cg_predictiondebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_PREDICTIONDEBUG.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.publish_transient_ui();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_PREDICTIONMISSHIGHLIGHT {
        runtime: CvarDef::new("cg_predictionMissHighlight", "", "0..1", "Flash a red screen-edge warning when a prediction miss exceeds cg_predictionMissThreshold.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_predictionmisshighlight(app)), |app, value| App::cvar_set_cg_predictionmisshighlight(app, APP_CG_PREDICTIONMISSHIGHLIGHT.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_predictionmisshighlight, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_PREDICTIONMISSHIGHLIGHT = APP_CG_PREDICTIONMISSHIGHLIGHT
                    .bind(
                        |state: &NetworkSettings| {
                            CvarValue::String(u8::from(state.prediction_miss_highlight).to_string())
                        },
                        |state, value| {
                            NetworkSettings::cvar_set_cg_predictionmisshighlight(
                                state,
                                APP_CG_PREDICTIONMISSHIGHLIGHT.entry.name,
                                &value.to_string(),
                            )
                        },
                    )
                    .with_text_setter(NetworkSettings::cvar_set_cg_predictionmisshighlight, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_predictionmisshighlight(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (Ok({
                        self.prediction_miss_highlight = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_predictionmisshighlight(&self) -> String {
                    (SETTINGS_CG_PREDICTIONMISSHIGHLIGHT.get)(&self.network).to_string()
                }
                fn cvar_set_cg_predictionmisshighlight(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (SETTINGS_CG_PREDICTIONMISSHIGHLIGHT.set_text.unwrap())(
                        &mut self.network,
                        name,
                        value,
                    )?;
                    self.mark_config_dirty();
                    self.publish_transient_ui();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_PREDICTIONMISSTHRESHOLD {
        runtime: CvarDef::new(
                "cg_predictionMissThreshold",
                "",
                "0..4096",
                "Prediction correction distance in JKA units required for the visual miss warning.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_predictionmissthreshold(app)),
                |app, value| {
                    App::cvar_set_cg_predictionmissthreshold(
                        app,
                        APP_CG_PREDICTIONMISSTHRESHOLD.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_predictionmissthreshold, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_PREDICTIONMISSTHRESHOLD = APP_CG_PREDICTIONMISSTHRESHOLD
                    .bind(
                        |state: &NetworkSettings| {
                            CvarValue::String(format!("{}", state.prediction_miss_threshold))
                        },
                        |state, value| {
                            NetworkSettings::cvar_set_cg_predictionmissthreshold(
                                state,
                                APP_CG_PREDICTIONMISSTHRESHOLD.entry.name,
                                &value.to_string(),
                            )
                        },
                    )
                    .with_text_setter(NetworkSettings::cvar_set_cg_predictionmissthreshold, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_predictionmissthreshold(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .map(|v| {
                            self.prediction_miss_threshold = v.clamp(0.0, 4096.0);
                            false
                        })
                        .ok_or_else(|| format!("{name}: expected a number")))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_predictionmissthreshold(&self) -> String {
                    (SETTINGS_CG_PREDICTIONMISSTHRESHOLD.get)(&self.network).to_string()
                }
                fn cvar_set_cg_predictionmissthreshold(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (SETTINGS_CG_PREDICTIONMISSTHRESHOLD.set_text.unwrap())(
                        &mut self.network,
                        name,
                        value,
                    )?;
                    self.mark_config_dirty();
                    self.publish_transient_ui();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_PREDICTITEMS {
        runtime: CvarDef::new(
                "cg_predictItems",
                "",
                "0..1",
                "Tell the server this client predicts item pickups (userinfo).",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_predictitems(app)),
                |app, value| {
                    App::cvar_set_cg_predictitems(app, APP_CG_PREDICTITEMS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_predictitems, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((323, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.predict_items))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_PREDICTITEMS = APP_CG_PREDICTITEMS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.predict_items).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_predictitems(
                            state,
                            APP_CG_PREDICTITEMS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_predictitems, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_predictitems(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.predict_items = atoi(value.as_bytes()) != 0;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_predictitems(&self) -> String {
                    (SETTINGS_CG_PREDICTITEMS.get)(&self.network).to_string()
                }
                fn cvar_set_cg_predictitems(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_PREDICTITEMS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CG_SHOWMISS {
        runtime: CvarDef::new(
                "cg_showMiss",
                "",
                "0..1",
                "Print prediction misses.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_showmiss(app)),
                |app, value| App::cvar_set_cg_showmiss(app, APP_CG_SHOWMISS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_showmiss, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CG_SHOWMISS = APP_CG_SHOWMISS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.show_miss).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cg_showmiss(
                            state,
                            APP_CG_SHOWMISS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cg_showmiss, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cg_showmiss(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.show_miss = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cg_showmiss(&self) -> String {
                    (SETTINGS_CG_SHOWMISS.get)(&self.network).to_string()
                }
                fn cvar_set_cg_showmiss(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CG_SHOWMISS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CHAR_COLOR_BLUE {
        runtime: CvarDef::new(
                "char_color_blue",
                "",
                "0..255",
                "Player tint (blue) sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_char_color_blue(app)),
                |app, value| {
                    App::cvar_set_char_color_blue(app, APP_CHAR_COLOR_BLUE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_char_color_blue, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((321, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.char_color[2])
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CHAR_COLOR_BLUE = APP_CHAR_COLOR_BLUE
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.char_color[2].to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_char_color_blue(
                            state,
                            APP_CHAR_COLOR_BLUE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_char_color_blue, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_char_color_blue(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 255).map(|v| {
                        self.char_color[2] = v as u8;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_char_color_blue(&self) -> String {
                    (SETTINGS_CHAR_COLOR_BLUE.get)(&self.network).to_string()
                }
                fn cvar_set_char_color_blue(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CHAR_COLOR_BLUE.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    if let Some(server) = self.local_server.as_mut() {
                        server.set_char_color(self.network.char_color);
                    }
                    self.profile_preview_key = None;
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CHAR_COLOR_GREEN {
        runtime: CvarDef::new(
                "char_color_green",
                "",
                "0..255",
                "Player tint (green) sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_char_color_green(app)),
                |app, value| {
                    App::cvar_set_char_color_green(app, APP_CHAR_COLOR_GREEN.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_char_color_green, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((320, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.char_color[1])
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CHAR_COLOR_GREEN = APP_CHAR_COLOR_GREEN
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.char_color[1].to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_char_color_green(
                            state,
                            APP_CHAR_COLOR_GREEN.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_char_color_green, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_char_color_green(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 255).map(|v| {
                        self.char_color[1] = v as u8;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_char_color_green(&self) -> String {
                    (SETTINGS_CHAR_COLOR_GREEN.get)(&self.network).to_string()
                }
                fn cvar_set_char_color_green(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CHAR_COLOR_GREEN.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    if let Some(server) = self.local_server.as_mut() {
                        server.set_char_color(self.network.char_color);
                    }
                    self.profile_preview_key = None;
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CHAR_COLOR_RED {
        runtime: CvarDef::new("char_color_red", "", "0..255", "Player tint (red) sent in userinfo; tints the entity-coloured parts of a player model, e.g. jedi_zf armour.", CvarType::String, CvarFlags::ARCHIVE.union(CvarFlags::USERINFO), LatchScope::None, |app| CvarValue::String(App::cvar_get_char_color_red(app)), |app, value| App::cvar_set_char_color_red(app, APP_CHAR_COLOR_RED.entry.name, &value.to_string())).with_text_setter(App::cvar_set_char_color_red, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((319, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.char_color[0])
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CHAR_COLOR_RED = APP_CHAR_COLOR_RED
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.char_color[0].to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_char_color_red(
                            state,
                            APP_CHAR_COLOR_RED.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_char_color_red, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_char_color_red(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 255).map(|v| {
                        self.char_color[0] = v as u8;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_char_color_red(&self) -> String {
                    (SETTINGS_CHAR_COLOR_RED.get)(&self.network).to_string()
                }
                fn cvar_set_char_color_red(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CHAR_COLOR_RED.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    if let Some(server) = self.local_server.as_mut() {
                        server.set_char_color(self.network.char_color);
                    }
                    self.profile_preview_key = None;
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_ALLOWDOWNLOAD {
        runtime: CvarDef::new("cl_allowDownload", "", "0..1", "Allow legacy Jedi Academy UDP PK3 autodownloads (svc_download). Used when HTTP is unavailable or fails.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_allowdownload(app)), |app, value| App::cvar_set_cl_allowdownload(app, APP_CL_ALLOWDOWNLOAD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_allowdownload, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((331, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.allow_legacy_downloads))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_ALLOWDOWNLOAD = APP_CL_ALLOWDOWNLOAD
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.allow_legacy_downloads).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cl_allowdownload(
                            state,
                            APP_CL_ALLOWDOWNLOAD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_allowdownload, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_allowdownload(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.allow_legacy_downloads = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_allowdownload(&self) -> String {
                    (SETTINGS_CL_ALLOWDOWNLOAD.get)(&self.network).to_string()
                }
                fn cvar_set_cl_allowdownload(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_ALLOWDOWNLOAD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_ALLOWHTTPDOWNLOAD {
        runtime: CvarDef::new(
                "cl_allowHttpDownload",
                "",
                "0..1",
                "Allow HTTP PK3 autodownloads advertised by the server. HTTP is preferred when available.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_allowhttpdownload(app)),
                |app, value| {
                    App::cvar_set_cl_allowhttpdownload(
                        app,
                        APP_CL_ALLOWHTTPDOWNLOAD.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cl_allowhttpdownload, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((330, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.allow_http_downloads))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_ALLOWHTTPDOWNLOAD = APP_CL_ALLOWHTTPDOWNLOAD
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.allow_http_downloads).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cl_allowhttpdownload(
                            state,
                            APP_CL_ALLOWHTTPDOWNLOAD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_allowhttpdownload, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_allowhttpdownload(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (Ok({
                        self.allow_http_downloads = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_allowhttpdownload(&self) -> String {
                    (SETTINGS_CL_ALLOWHTTPDOWNLOAD.get)(&self.network).to_string()
                }
                fn cvar_set_cl_allowhttpdownload(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_ALLOWHTTPDOWNLOAD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_ALLOWMISSINGMAP {
        runtime: CvarDef::new("cl_allowMissingMap", "", "0..1", "Keep a live server connection active without world geometry when the server BSP is not installed locally.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_allowmissingmap(app)), |app, value| App::cvar_set_cl_allowmissingmap(app, APP_CL_ALLOWMISSINGMAP.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_allowmissingmap, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((329, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.allow_missing_map))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_ALLOWMISSINGMAP = APP_CL_ALLOWMISSINGMAP
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.allow_missing_map).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cl_allowmissingmap(
                            state,
                            APP_CL_ALLOWMISSINGMAP.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_allowmissingmap, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_allowmissingmap(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.allow_missing_map = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_allowmissingmap(&self) -> String {
                    (SETTINGS_CL_ALLOWMISSINGMAP.get)(&self.network).to_string()
                }
                fn cvar_set_cl_allowmissingmap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_ALLOWMISSINGMAP.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_CHATBUBBLESELF {
        runtime: CvarDef::new("cl_chatBubbleSelf", "", "0..1", "jaPRO: show your own chat balloon to others while the console, chat or a menu has the keyboard.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_chatbubbleself(app)), |app, value| App::cvar_set_cl_chatbubbleself(app, APP_CL_CHATBUBBLESELF.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_chatbubbleself, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((334, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.chat_bubble_self))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_CHATBUBBLESELF = APP_CL_CHATBUBBLESELF
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.chat_bubble_self).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cl_chatbubbleself(
                            state,
                            APP_CL_CHATBUBBLESELF.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_chatbubbleself, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_chatbubbleself(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.chat_bubble_self = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_chatbubbleself(&self) -> String {
                    (SETTINGS_CL_CHATBUBBLESELF.get)(&self.network).to_string()
                }
                fn cvar_set_cl_chatbubbleself(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_CHATBUBBLESELF.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_CHATBUBBLEUNFOCUSED {
        runtime: CvarDef::new(
                "cl_chatBubbleUnfocused",
                "",
                "0..1",
                "jaPRO: also show the chat balloon while the game window is unfocused.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_chatbubbleunfocused(app)),
                |app, value| {
                    App::cvar_set_cl_chatbubbleunfocused(
                        app,
                        APP_CL_CHATBUBBLEUNFOCUSED.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cl_chatbubbleunfocused, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((335, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.chat_bubble_unfocused))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_CHATBUBBLEUNFOCUSED = APP_CL_CHATBUBBLEUNFOCUSED
                .bind(
                    |state: &NetworkSettings| {
                        CvarValue::String(u8::from(state.chat_bubble_unfocused).to_string())
                    },
                    |state, value| {
                        NetworkSettings::cvar_set_cl_chatbubbleunfocused(
                            state,
                            APP_CL_CHATBUBBLEUNFOCUSED.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_chatbubbleunfocused, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_chatbubbleunfocused(
                    &mut self,
                    _name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (Ok({
                        self.chat_bubble_unfocused = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_chatbubbleunfocused(&self) -> String {
                    (SETTINGS_CL_CHATBUBBLEUNFOCUSED.get)(&self.network).to_string()
                }
                fn cvar_set_cl_chatbubbleunfocused(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    (SETTINGS_CL_CHATBUBBLEUNFOCUSED.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_COMMANDPACING {
        runtime: CvarDef::new("cl_commandPacing", "", "0..1", "1 = stamp usercmds on an exact cl.serverTime cadence (race physics steps by the command gap); 0 = wall-clock pacing (gaps jitter 5-10 ms at 125).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_commandpacing(app)), |app, value| App::cvar_set_cl_commandpacing(app, APP_CL_COMMANDPACING.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_commandpacing, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((333, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.network.command_pacing))
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_COMMANDPACING = APP_CL_COMMANDPACING
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.command_pacing).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cl_commandpacing(
                            state,
                            APP_CL_COMMANDPACING.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_commandpacing, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_commandpacing(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.command_pacing = atoi(value.as_bytes()) != 0;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_commandpacing(&self) -> String {
                    (SETTINGS_CL_COMMANDPACING.get)(&self.network).to_string()
                }
                fn cvar_set_cl_commandpacing(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_COMMANDPACING.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_COMMANDRATE {
        runtime: CvarDef::new("cl_commandRate", "", "15..1000", "Usercmds built per second, i.e. the movement physics rate. Set it yourself; it is not tied to com_maxfps (which only caps rendering here).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_commandrate(app)), |app, value| App::cvar_set_cl_commandrate(app, APP_CL_COMMANDRATE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_commandrate, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((332, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.command_rate)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_COMMANDRATE = APP_CL_COMMANDRATE
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.command_rate.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cl_commandrate(
                            state,
                            APP_CL_COMMANDRATE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_commandrate, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_commandrate(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 15, 1000).map(|v| {
                        self.command_rate = v as u32;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_commandrate(&self) -> String {
                    (SETTINGS_CL_COMMANDRATE.get)(&self.network).to_string()
                }
                fn cvar_set_cl_commandrate(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_COMMANDRATE.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.next_command_time = None;
                    if let Some(server) = self.local_server.as_mut() {
                        server.set_command_rate(self.network.command_rate)?;
                    }
                    self.publish_ui();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_IDRIVE {
        runtime: CvarDef::new("cl_idrive", "1", "0|1", "Opposite movement keys: 0 cancel, 1 most recently pressed direction wins (solo and multiplayer).", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::Bool(app.network.idrive),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!("validated cvar type") };
                app.network.idrive = enabled;
                app.mark_config_dirty();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _, value| {
                if let Some(enabled) = parse_bool(value) { settings.network.idrive = enabled; }
            }),
            archive: Some((3500, |state| format!("{}", u8::from(state.presentation.network.idrive)))),
            ..CvarBinding::NONE
        },
        settings: SETTINGS_CL_IDRIVE = APP_CL_IDRIVE.bind(
            |state: &NetworkSettings| CvarValue::Bool(state.idrive),
            |state, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!("validated cvar type") };
                state.idrive = enabled;
                Ok(())
            },
        ),
        implementations: []
    }

    APP_CL_MAXPACKETS {
        runtime: CvarDef::new(
                "cl_maxpackets",
                "",
                "15..1000",
                "Maximum client packets per second.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_maxpackets(app)),
                |app, value| {
                    App::cvar_set_cl_maxpackets(app, APP_CL_MAXPACKETS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cl_maxpackets, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((308, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.max_packets)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_MAXPACKETS = APP_CL_MAXPACKETS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.max_packets.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cl_maxpackets(
                            state,
                            APP_CL_MAXPACKETS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_maxpackets, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_maxpackets(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 15, 1000).map(|v| {
                        self.max_packets = v as u32;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_maxpackets(&self) -> String {
                    (SETTINGS_CL_MAXPACKETS.get)(&self.network).to_string()
                }
                fn cvar_set_cl_maxpackets(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_MAXPACKETS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.mark_japro_net_userinfo_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_PACKETDUP {
        runtime: CvarDef::new(
                "cl_packetdup",
                "",
                "0..5",
                "Repeat the usercmds of this many earlier packets to survive packet loss.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_packetdup(app)),
                |app, value| {
                    App::cvar_set_cl_packetdup(app, APP_CL_PACKETDUP.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cl_packetdup, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((309, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.packet_dup)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_PACKETDUP = APP_CL_PACKETDUP
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.packet_dup.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cl_packetdup(
                            state,
                            APP_CL_PACKETDUP.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_packetdup, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_packetdup(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 5).map(|v| {
                        self.packet_dup = v as u32;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_packetdup(&self) -> String {
                    (SETTINGS_CL_PACKETDUP.get)(&self.network).to_string()
                }
                fn cvar_set_cl_packetdup(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_PACKETDUP.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_CL_TIMENUDGE {
        runtime: CvarDef::new(
                "cl_timeNudge",
                "",
                "-900..900",
                "Milliseconds of extra (+) or less (-) interpolation delay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_timenudge(app)),
                |app, value| {
                    App::cvar_set_cl_timenudge(app, APP_CL_TIMENUDGE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cl_timenudge, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((310, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.time_nudge)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CL_TIMENUDGE = APP_CL_TIMENUDGE
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.time_nudge.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cl_timenudge(
                            state,
                            APP_CL_TIMENUDGE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cl_timenudge, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cl_timenudge(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, -900, 900).map(|v| {
                        self.time_nudge = v as i32;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cl_timenudge(&self) -> String {
                    (SETTINGS_CL_TIMENUDGE.get)(&self.network).to_string()
                }
                fn cvar_set_cl_timenudge(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CL_TIMENUDGE.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.mark_japro_net_userinfo_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_COLOR1 {
        runtime: CvarDef::new("color1", "", "0..6[cosmetic]", "Primary saber colour: 0 red, 1 orange, 2 yellow, 3 green, 4 blue, 5 purple, 6 RGB (jaPRO / JA+ servers; see cp_sbRGB1).", CvarType::String, CvarFlags::ARCHIVE.union(CvarFlags::USERINFO), LatchScope::None, |app| CvarValue::String(App::cvar_get_color1(app)), |app, value| App::cvar_set_color1(app, APP_COLOR1.entry.name, &value.to_string())).with_text_setter(App::cvar_set_color1, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((313, |state| {
                    let presentation = state.presentation;
                    crate::global_cosmetics::encode(presentation.network.color1, &presentation.network.global_hat)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_COLOR1 = APP_COLOR1
                .bind(
                    |state: &NetworkSettings| CvarValue::String(crate::global_cosmetics::encode(state.color1, &state.global_hat)),
                    |state, value| {
                        NetworkSettings::cvar_set_color1(state, APP_COLOR1.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_color1, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_color1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let (color, hat) = crate::global_cosmetics::parse_cvar(value)
                        .map_err(|error| format!("{name}: {error}"))?;
                    self.color1 = color;
                    self.global_hat = hat;
                    Ok(())
                }
            }
            impl App {
                fn cvar_get_color1(&self) -> String {
                    (SETTINGS_COLOR1.get)(&self.network).to_string()
                }
                fn cvar_set_color1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_COLOR1.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_COLOR2 {
        runtime: CvarDef::new(
                "color2",
                "",
                "0..6[cosmetic]",
                "Secondary saber colour (same values as color1; RGB uses cp_sbRGB2).",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_color2(app)),
                |app, value| App::cvar_set_color2(app, APP_COLOR2.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_color2, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((314, |state| {
                    let presentation = state.presentation;
                    crate::global_cosmetics::encode(presentation.network.color2, &presentation.network.global_cape)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_COLOR2 = APP_COLOR2
                .bind(
                    |state: &NetworkSettings| CvarValue::String(crate::global_cosmetics::encode(state.color2, &state.global_cape)),
                    |state, value| {
                        NetworkSettings::cvar_set_color2(state, APP_COLOR2.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_color2, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_color2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let (color, cape) = crate::global_cosmetics::parse_cvar(value)
                        .map_err(|error| format!("{name}: {error}"))?;
                    self.color2 = color;
                    self.global_cape = cape;
                    Ok(())
                }
            }
            impl App {
                fn cvar_get_color2(&self) -> String {
                    (SETTINGS_COLOR2.get)(&self.network).to_string()
                }
                fn cvar_set_color2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_COLOR2.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CP_CLANPWD {
        runtime: CvarDef::new(
                "cp_clanPwd",
                "",
                "string",
                "jaPRO clan password, sent to jaPRO servers. Not saved to the config.",
                CvarType::String,
                CvarFlags::NONE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cp_clanpwd(app)),
                |app, value| App::cvar_set_cp_clanpwd(app, APP_CP_CLANPWD.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cp_clanpwd, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_CLANPWD = APP_CP_CLANPWD
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.clan_pwd.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_cp_clanpwd(
                            state,
                            APP_CP_CLANPWD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cp_clanpwd, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cp_clanpwd(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.clan_pwd = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cp_clanpwd(&self) -> String {
                    (SETTINGS_CP_CLANPWD.get)(&self.network).to_string()
                }
                fn cvar_set_cp_clanpwd(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_CLANPWD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CP_COSMETICS {
        runtime: CvarDef::new(
                "cp_cosmetics",
                "",
                "bitfield",
                "jaPRO model cosmetics bitfield, sent to jaPRO servers.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cp_cosmetics(app)),
                |app, value| {
                    App::cvar_set_cp_cosmetics(app, APP_CP_COSMETICS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cp_cosmetics, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((324, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.cosmetics as i32)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_COSMETICS = APP_CP_COSMETICS
                .bind(
                    |state: &NetworkSettings| CvarValue::String((state.cosmetics as i32).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cp_cosmetics(
                            state,
                            APP_CP_COSMETICS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cp_cosmetics, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cp_cosmetics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, i64::from(i32::MIN), i64::from(i32::MAX)).map(|v| {
                        self.cosmetics = v as i32 as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cp_cosmetics(&self) -> String {
                    (SETTINGS_CP_COSMETICS.get)(&self.network).to_string()
                }
                fn cvar_set_cp_cosmetics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_COSMETICS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CP_PLUGINDISABLE {
        runtime: CvarDef::new(
                "cp_pluginDisable",
                "",
                "0..2147483647",
                "JAPRO preference bitfield; exposed by the Setup -> Movement and Setup -> JAPRO sections.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cp_plugindisable(app)),
                |app, value| {
                    App::cvar_set_cp_plugindisable(app, APP_CP_PLUGINDISABLE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cp_plugindisable, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((304, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.plugin_disable)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_PLUGINDISABLE = APP_CP_PLUGINDISABLE
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.plugin_disable.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cp_plugindisable(
                            state,
                            APP_CP_PLUGINDISABLE.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cp_plugindisable, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cp_plugindisable(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, i32::MAX as i64).map(|v| {
                        self.plugin_disable = v as i32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cp_plugindisable(&self) -> String {
                    (SETTINGS_CP_PLUGINDISABLE.get)(&self.network).to_string()
                }
                fn cvar_set_cp_plugindisable(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_PLUGINDISABLE.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    if let Some(session) = self.game_session.as_mut() {
                        session
                            .client_game
                            .set_plugin_disable(self.network.plugin_disable);
                    }
                    self.profile_preview_key = None;
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CP_SBRGB1 {
        runtime: CvarDef::new(
                "cp_sbRGB1",
                "",
                "0..16777215",
                "jaPRO custom blade colour for color1 6, packed red + green * 256 + blue * 65536.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cp_sbrgb1(app)),
                |app, value| App::cvar_set_cp_sbrgb1(app, APP_CP_SBRGB1.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cp_sbrgb1, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((315, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.sb_rgb1)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_SBRGB1 = APP_CP_SBRGB1
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.sb_rgb1.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cp_sbrgb1(
                            state,
                            APP_CP_SBRGB1.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cp_sbrgb1, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cp_sbrgb1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 0xFF_FFFF).map(|v| {
                        self.sb_rgb1 = v as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cp_sbrgb1(&self) -> String {
                    (SETTINGS_CP_SBRGB1.get)(&self.network).to_string()
                }
                fn cvar_set_cp_sbrgb1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_SBRGB1.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_CP_SBRGB2 {
        runtime: CvarDef::new(
                "cp_sbRGB2",
                "",
                "0..16777215",
                "jaPRO custom blade colour for color2 6, packed like cp_sbRGB1.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cp_sbrgb2(app)),
                |app, value| App::cvar_set_cp_sbrgb2(app, APP_CP_SBRGB2.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cp_sbrgb2, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((316, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.sb_rgb2)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_CP_SBRGB2 = APP_CP_SBRGB2
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.sb_rgb2.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_cp_sbrgb2(
                            state,
                            APP_CP_SBRGB2.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_cp_sbrgb2, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_cp_sbrgb2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, 0xFF_FFFF).map(|v| {
                        self.sb_rgb2 = v as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_cp_sbrgb2(&self) -> String {
                    (SETTINGS_CP_SBRGB2.get)(&self.network).to_string()
                }
                fn cvar_set_cp_sbrgb2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_CP_SBRGB2.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_FORCEPOWERS {
        runtime: CvarDef::new(
                "forcepowers",
                "",
                "rank-side-levels",
                "Force power configuration.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_forcepowers(app)),
                |app, value| App::cvar_set_forcepowers(app, APP_FORCEPOWERS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_forcepowers, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((317, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.forcepowers)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_FORCEPOWERS = APP_FORCEPOWERS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.forcepowers.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_forcepowers(
                            state,
                            APP_FORCEPOWERS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_forcepowers, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_forcepowers(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.forcepowers = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_forcepowers(&self) -> String {
                    (SETTINGS_FORCEPOWERS.get)(&self.network).to_string()
                }
                fn cvar_set_forcepowers(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_FORCEPOWERS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_FS_GAME {
        runtime: CvarDef::new("fs_game", "", "directory", "Game directory mounted over base for local games (applied when a map loads); empty or base for none. A connected server's own fs_game overrides it.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_fs_game(app)), |app, value| App::cvar_set_fs_game(app, APP_FS_GAME.entry.name, &value.to_string())).with_text_setter(App::cvar_set_fs_game, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((325, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.fs_game)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_FS_GAME = APP_FS_GAME
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.fs_game.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_fs_game(state, APP_FS_GAME.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_fs_game, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_fs_game(&mut self, name: &str, value: &str) -> Result<(), String> {
                    ({
                        // A sibling directory name, as `resolve_fs_game_directory` accepts.
                        let dir = value.trim();
                        let valid = dir.len() <= 63
                            && dir != "."
                            && dir != ".."
                            && !dir.bytes().any(|b| b < 0x20 || b == 0x7f || matches!(b, b'/' | b'\\' | b':' | b';' | b'"'));
                        if valid {
                            self.fs_game = dir.to_owned();
                            Ok(false)
                        } else {
                            Err(format!("{name}: expected a game directory name like japro (empty or base for none)"))
                        }
                    }).map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_fs_game(&self) -> String {
                    if self.net.is_some() {
                        self.game
                            .as_ref()
                            .and_then(|dir| dir.file_name())
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    } else {
                        (SETTINGS_FS_GAME.get)(&self.network).to_string()
                    }
                }
                fn cvar_set_fs_game(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_FS_GAME.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_HANDICAP {
        runtime: CvarDef::new(
                "handicap",
                "",
                "1..100",
                "Handicap sent in userinfo; the server uses it as maximum health.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_handicap(app)),
                |app, value| App::cvar_set_handicap(app, APP_HANDICAP.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_handicap, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((322, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.handicap)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_HANDICAP = APP_HANDICAP
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.handicap.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_handicap(
                            state,
                            APP_HANDICAP.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_handicap, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_handicap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 1, 100).map(|v| {
                        self.handicap = v as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_handicap(&self) -> String {
                    (SETTINGS_HANDICAP.get)(&self.network).to_string()
                }
                fn cvar_set_handicap(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_HANDICAP.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_NAME {
        runtime: CvarDef::new(
                "name",
                "",
                "string",
                "Player name sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_name(app)),
                |app, value| App::cvar_set_name(app, APP_NAME.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_name, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((305, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.name)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_NAME = APP_NAME
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.name.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_name(state, APP_NAME.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_name, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_name(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.name = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_name(&self) -> String {
                    (SETTINGS_NAME.get)(&self.network).to_string()
                }
                fn cvar_set_name(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_NAME.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_NET_PORT {
        runtime: CvarDef::new("net_port", "", "0..65535", "Preferred local UDP port for the live game connection. Like TaystJK/OpenJK, the client tries this port and the next 9 if it is busy; 0 lets the OS choose.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_net_port(app)), |app, value| App::cvar_set_net_port(app, APP_NET_PORT.entry.name, &value.to_string())).with_text_setter(App::cvar_set_net_port, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_NET_PORT = APP_NET_PORT
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.net_port.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_net_port(
                            state,
                            APP_NET_PORT.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_net_port, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_net_port(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 0, u16::MAX as i64).map(|v| {
                        self.net_port = v as u16;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_net_port(&self) -> String {
                    (SETTINGS_NET_PORT.get)(&self.network).to_string()
                }
                fn cvar_set_net_port(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_NET_PORT.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_PASSWORD {
        runtime: CvarDef::new(
                "password",
                "",
                "string",
                "Server join password.",
                CvarType::String,
                CvarFlags::NONE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_password(app)),
                |app, value| App::cvar_set_password(app, APP_PASSWORD.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_password, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_PASSWORD = APP_PASSWORD
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.password.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_password(
                            state,
                            APP_PASSWORD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_password, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_password(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.password = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_password(&self) -> String {
                    (SETTINGS_PASSWORD.get)(&self.network).to_string()
                }
                fn cvar_set_password(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_PASSWORD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_RATE {
        runtime: CvarDef::new(
                "rate",
                "",
                "1000..90000",
                "Maximum bytes/second the server may send.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_rate(app)),
                |app, value| App::cvar_set_rate(app, APP_RATE.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_rate, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((306, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.rate)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_RATE = APP_RATE
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.rate.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_rate(state, APP_RATE.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_rate, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_rate(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 1000, 90000).map(|v| {
                        self.rate = v as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_rate(&self) -> String {
                    (SETTINGS_RATE.get)(&self.network).to_string()
                }
                fn cvar_set_rate(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_RATE.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_RCONADDRESS {
        runtime: CvarDef::new("rconAddress", "", "host[:port]", "Alternate server address for rcon when not connected (default port 29070). Not saved to the config.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_rconaddress(app)), |app, value| App::cvar_set_rconaddress(app, APP_RCONADDRESS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_rconaddress, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_RCONADDRESS = APP_RCONADDRESS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.rcon_address.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_rconaddress(
                            state,
                            APP_RCONADDRESS.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_rconaddress, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_rconaddress(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.rcon_address = value.trim().to_owned();
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_rconaddress(&self) -> String {
                    (SETTINGS_RCONADDRESS.get)(&self.network).to_string()
                }
                fn cvar_set_rconaddress(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_RCONADDRESS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_RCONPASSWORD {
        runtime: CvarDef::new(
                "rconPassword",
                "",
                "string",
                "Password for remote console access (see rcon). Not saved to the config.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_rconpassword(app)),
                |app, value| {
                    App::cvar_set_rconpassword(app, APP_RCONPASSWORD.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_rconpassword, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_RCONPASSWORD = APP_RCONPASSWORD
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.rcon_password.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_rconpassword(
                            state,
                            APP_RCONPASSWORD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_rconpassword, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_rconpassword(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    (Ok({
                        self.rcon_password = value.to_owned();
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_rconpassword(&self) -> String {
                    (SETTINGS_RCONPASSWORD.get)(&self.network).to_string()
                }
                fn cvar_set_rconpassword(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_RCONPASSWORD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_SABER1 {
        runtime: CvarDef::new(
                "saber1",
                "",
                "saber name",
                "Primary saber sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_saber1(app)),
                |app, value| App::cvar_set_saber1(app, APP_SABER1.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_saber1, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((311, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.saber1)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_SABER1 = APP_SABER1
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.saber1.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_saber1(state, APP_SABER1.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_saber1, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_saber1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.saber1 = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_saber1(&self) -> String {
                    (SETTINGS_SABER1.get)(&self.network).to_string()
                }
                fn cvar_set_saber1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_SABER1.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_SABER2 {
        runtime: CvarDef::new(
                "saber2",
                "",
                "saber name",
                "Secondary saber sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_saber2(app)),
                |app, value| App::cvar_set_saber2(app, APP_SABER2.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_saber2, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((312, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.saber2)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_SABER2 = APP_SABER2
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.saber2.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_saber2(state, APP_SABER2.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_saber2, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_saber2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.saber2 = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_saber2(&self) -> String {
                    (SETTINGS_SABER2.get)(&self.network).to_string()
                }
                fn cvar_set_saber2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_SABER2.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_SEX {
        runtime: CvarDef::new(
                "sex",
                "",
                "male|female",
                "Sound gender hint sent in userinfo.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sex(app)),
                |app, value| App::cvar_set_sex(app, APP_SEX.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sex, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((318, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.sex)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_SEX = APP_SEX
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.sex.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_sex(state, APP_SEX.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_sex, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_sex(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.sex = v;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_sex(&self) -> String {
                    (SETTINGS_SEX.get)(&self.network).to_string()
                }
                fn cvar_set_sex(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_SEX.set_text.unwrap())(&mut self.network, name, value)?;
                    self.sync_local_client_info();
                    self.profile_preview_key = None;
                    self.publish_ui();
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_SNAPS {
        runtime: CvarDef::new(
                "snaps",
                "",
                "1..125",
                "Snapshots per second requested from the server.",
                CvarType::String,
                CvarFlags::ARCHIVE.union(CvarFlags::USERINFO),
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_snaps(app)),
                |app, value| App::cvar_set_snaps(app, APP_SNAPS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_snaps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((307, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.snaps)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_SNAPS = APP_SNAPS
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.snaps.to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_snaps(state, APP_SNAPS.entry.name, &value.to_string())
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_snaps, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_snaps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_number(name, value, 1, 125).map(|v| {
                        self.snaps = v as u32;
                        true
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_snaps(&self) -> String {
                    (SETTINGS_SNAPS.get)(&self.network).to_string()
                }
                fn cvar_set_snaps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_SNAPS.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();
                    self.request_userinfo_send();

                    Ok(())
                }
            }
        ]
    }

    APP_TEAMOVERLAY {
        runtime: CvarDef::new(
                "teamoverlay",
                "",
                "read-only",
                "Server tinfo request bit mirrored automatically from cg_drawTeamOverlay (CVAR_USERINFO).",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_teamoverlay(app)),
                |app, value| App::cvar_set_teamoverlay(app, APP_TEAMOVERLAY.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_teamoverlay, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_TEAMOVERLAY = APP_TEAMOVERLAY
                .bind(
                    |state: &NetworkSettings| CvarValue::String(u8::from(state.team_overlay).to_string()),
                    |state, value| {
                        NetworkSettings::cvar_set_teamoverlay(
                            state,
                            APP_TEAMOVERLAY.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_teamoverlay, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_teamoverlay(&mut self, _name: &str, _value: &str) -> Result<(), String> {
                    (Err("teamoverlay is read-only; use cg_drawTeamOverlay".to_owned())).map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_teamoverlay(&self) -> String {
                    (SETTINGS_TEAMOVERLAY.get)(&self.network).to_string()
                }
                fn cvar_set_teamoverlay(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_TEAMOVERLAY.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_UI_PASSWORD {
        runtime: CvarDef::new(
                "ui_password",
                "",
                "string",
                "Legacy ui_password account field retained for compatibility. Saved Account rows use Windows Credential Manager.",
                CvarType::String,
                CvarFlags::NONE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_ui_password(app)),
                |app, value| App::cvar_set_ui_password(app, APP_UI_PASSWORD.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_ui_password, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_UI_PASSWORD = APP_UI_PASSWORD
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.ui_password.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_ui_password(
                            state,
                            APP_UI_PASSWORD.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_ui_password, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_ui_password(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.ui_password = v;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_ui_password(&self) -> String {
                    (SETTINGS_UI_PASSWORD.get)(&self.network).to_string()
                }
                fn cvar_set_ui_password(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_UI_PASSWORD.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }

    APP_UI_USERNAME {
        runtime: CvarDef::new(
                "ui_username",
                "",
                "string",
                "Legacy ui_username account field retained for compatibility with servers and scripts.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_ui_username(app)),
                |app, value| App::cvar_set_ui_username(app, APP_UI_USERNAME.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_ui_username, None),
        config: CvarBinding {
                load_presentation: Some(|settings, name, value| {
                    let _ = settings.network.set_cvar(name, value);
                }),
                archive: Some((326, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.network.ui_username)
                })),
                ..CvarBinding::NONE
            },
        settings: SETTINGS_UI_USERNAME = APP_UI_USERNAME
                .bind(
                    |state: &NetworkSettings| CvarValue::String(state.ui_username.clone()),
                    |state, value| {
                        NetworkSettings::cvar_set_ui_username(
                            state,
                            APP_UI_USERNAME.entry.name,
                            &value.to_string(),
                        )
                    },
                )
                .with_text_setter(NetworkSettings::cvar_set_ui_username, None),
        implementations: [
            impl NetworkSettings {
                fn cvar_set_ui_username(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (network_text(name, value).map(|v| {
                        self.ui_username = v;
                        false
                    }))
                    .map(|_: bool| ())
                }
            }
            impl App {
                fn cvar_get_ui_username(&self) -> String {
                    (SETTINGS_UI_USERNAME.get)(&self.network).to_string()
                }
                fn cvar_set_ui_username(&mut self, name: &str, value: &str) -> Result<(), String> {
                    (SETTINGS_UI_USERNAME.set_text.unwrap())(&mut self.network, name, value)?;
                    self.mark_config_dirty();

                    Ok(())
                }
            }
        ]
    }
}
