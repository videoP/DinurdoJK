//! Client cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_UI_GAMELAUNCHSCREEN {
        runtime: CvarDef::new("ui_gameLaunchScreen", "main", "main|play|solo|servers|demo|profile|controls|setup|chatlogs|screenshots|devtools|assets|maps", "Menu to open on the next game launch without a map.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(app.game_launch_screen.page_name().to_owned()),
            |app, value| {
                app.game_launch_screen = crate::config::GameLaunchScreen::parse(&value.to_string())
                    .ok_or_else(|| "ui_gameLaunchScreen: unknown launch screen".to_owned())?;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(screen) = crate::config::GameLaunchScreen::parse(value) {
                    settings.game_launch_screen = screen;
                }
            }),
            archive: Some((27000, |state| state.presentation.game_launch_screen.page_name().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CL_STREAMERMODE {
        runtime: CvarDef::new("cl_streamerMode", "0", "0|1", "Use a separately filtered stream presentation snapshot; local game is unaffected.", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::Bool(app.streamer.enabled),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!("validated cvar type") };
                app.streamer.enabled = enabled;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(flag) = parse_bool(value) { settings.streamer.enabled = flag; }
            }),
            archive: Some((360, |state| format!("{}", u8::from(state.presentation.streamer.enabled)))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CL_STREAMERHIDEPRIVATE {
        runtime: CvarDef::new("cl_streamerHidePrivate", "1", "0|1", "Mask identified private messages on the stream-only chat and console UI.", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::Bool(app.streamer.hide_private_messages),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!("validated cvar type") };
                app.streamer.hide_private_messages = enabled;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(flag) = parse_bool(value) { settings.streamer.hide_private_messages = flag; }
            }),
            archive: Some((361, |state| format!("{}", u8::from(state.presentation.streamer.hide_private_messages)))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CL_STREAMERHIDECONSOLE {
        runtime: CvarDef::new("cl_streamerHideConsole", "1", "0|1", "Mask all console text on the stream-only UI, including input and notifications.", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::Bool(app.streamer.hide_console),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!("validated cvar type") };
                app.streamer.hide_console = enabled;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(flag) = parse_bool(value) { settings.streamer.hide_console = flag; }
            }),
            archive: Some((362, |state| format!("{}", u8::from(state.presentation.streamer.hide_console)))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATBOXCOMPLETION {
        runtime: CvarDef::new("cg_chatboxCompletion", "", "0..1", "JAPP: Tab in the chat input completes a player name (colours ignored); repeated Tab cycles ranked matches, preferring exact and prefix matches.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_chatboxcompletion(app)), |app, value| App::cvar_set_cg_chatboxcompletion(app, APP_CG_CHATBOXCOMPLETION.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_chatboxcompletion, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.chatbox_completion =
                            parse_bool(value).unwrap_or(settings.chatbox_completion)
                    }
                }),
                archive: Some((289, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.chatbox_completion))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_chatboxcompletion(&self) -> String {
                    bool_value(self.chatbox_completion)
                }
                fn cvar_set_cg_chatboxcompletion(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.chatbox_completion = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CLEANCHATBOX {
        runtime: CvarDef::new("cg_cleanChatbox", "", "0|1|2", "TaystJK: 0 off, 1 filter spam/duplicates and strip message-body colors, 2 filter spam/duplicates and recolor an initial message-body color green.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_cleanchatbox(app)), |app, value| App::cvar_set_cg_cleanchatbox(app, "cg_cleanChatbox", &value.to_string())).with_text_setter(App::cvar_set_cg_cleanchatbox, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    if mode <= 2 { settings.clean_chatbox = mode; }
                }
            }),
            archive: Some((290, |state| state.presentation.clean_chatbox.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_cg_cleanchatbox(&self) -> String {
                    self.clean_chatbox.to_string()
                }
                fn cvar_set_cg_cleanchatbox(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>().map_err(|_| "cg_cleanChatbox expects 0, 1, or 2".to_owned())?;
                    if mode > 2 { return Err("cg_cleanChatbox expects 0, 1, or 2".to_owned()); }
                    self.clean_chatbox = mode;
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DISMEMBER {
        runtime: CvarDef::new(
                "cg_dismember",
                "",
                "0|1|2",
                "OpenJK dismemberment: 0 off, 1 limbs only, 2 full including head/waist.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_dismember(app)),
                |app, value| {
                    App::cvar_set_cg_dismember(app, APP_CG_DISMEMBER.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_dismember, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(value) = value.parse::<u8>() {
                        settings.dismemberment = value.min(2);
                    }
                }),
                archive: Some((201, |state| {
                    let settings = state.video;
                    format!("{}", settings.dismemberment)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_dismember(&self) -> String {
                    self.video.dismemberment.to_string()
                }
                fn cvar_set_cg_dismember(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| "cg_dismember expects 0, 1, or 2".to_owned())?;
                        self.video.dismemberment = requested.min(2);
                        self.physics_menu_changed();
                        return Ok(());
                    }
                }
            }
        ]
    }

    APP_CG_DYNAMICCROSSHAIR {
        runtime: CvarDef::new("cg_dynamicCrosshair", "", "0|1|2", "TaystJK dynamic crosshair: 0 static screen-center trace, 1 always trace from the weapon/player muzzle, 2 use the dynamic trace except for the original melee/saber/racemode/strafehelper static overrides.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_dynamiccrosshair(app)), |app, value| App::cvar_set_cg_dynamiccrosshair(app, APP_CG_DYNAMICCROSSHAIR.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_dynamiccrosshair, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if let Ok(mode) = value.trim().parse::<u8>() {
                        settings.crosshair.dynamic = mode.min(2);
                    }
                }),
                archive: Some((234, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.crosshair.dynamic)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_dynamiccrosshair(&self) -> String {
                    self.crosshair.dynamic.to_string()
                }
                fn cvar_set_cg_dynamiccrosshair(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = value
                            .trim()
                            .parse::<u8>()
                            .map_err(|_| "cg_dynamicCrosshair: expected 0, 1, or 2".to_owned())?;
                        if mode > 2 {
                            return Err("cg_dynamicCrosshair: expected 0, 1, or 2".to_owned());
                        }
                        self.crosshair.dynamic = mode;
                        self.crosshair_scan_at = Instant::now() - Duration::from_secs(1);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FKDURATION {
        runtime: CvarDef::new(
                "cg_fkDuration",
                "",
                "integer",
                "TaystJK flipkick sequence frame duration.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_fkduration(app)),
                |app, value| {
                    App::cvar_set_cg_fkduration(app, APP_CG_FKDURATION.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_fkduration, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.fk_duration = value.trim().parse::<i32>().unwrap_or(settings.fk_duration)
                    }
                }),
                archive: Some((295, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.fk_duration)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fkduration(&self) -> String {
                    self.japro_fk_duration.to_string()
                }
                fn cvar_set_cg_fkduration(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.japro_fk_duration = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected an integer"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FKFIRSTJUMPDURATION {
        runtime: CvarDef::new(
                "cg_fkFirstJumpDuration",
                "",
                "integer",
                "TaystJK flipkick first jump duration.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_fkfirstjumpduration(app)),
                |app, value| {
                    App::cvar_set_cg_fkfirstjumpduration(
                        app,
                        APP_CG_FKFIRSTJUMPDURATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_fkfirstjumpduration, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.fk_first_jump_duration = value
                            .trim()
                            .parse::<i32>()
                            .unwrap_or(settings.fk_first_jump_duration)
                    }
                }),
                archive: Some((296, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.fk_first_jump_duration)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fkfirstjumpduration(&self) -> String {
                    self.japro_fk_first_jump_duration.to_string()
                }
                fn cvar_set_cg_fkfirstjumpduration(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.japro_fk_first_jump_duration = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected an integer"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FKSECONDJUMPDELAY {
        runtime: CvarDef::new(
                "cg_fkSecondJumpDelay",
                "",
                "integer",
                "TaystJK flipkick second jump delay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_fksecondjumpdelay(app)),
                |app, value| {
                    App::cvar_set_cg_fksecondjumpdelay(
                        app,
                        APP_CG_FKSECONDJUMPDELAY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_fksecondjumpdelay, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.fk_second_jump_delay = value
                            .trim()
                            .parse::<i32>()
                            .unwrap_or(settings.fk_second_jump_delay)
                    }
                }),
                archive: Some((297, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.fk_second_jump_delay)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fksecondjumpdelay(&self) -> String {
                    self.japro_fk_second_jump_delay.to_string()
                }
                fn cvar_set_cg_fksecondjumpdelay(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.japro_fk_second_jump_delay = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected an integer"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FORCEMODEL {
        runtime: CvarDef::new("cg_forceModel", "", "0 | model[/skin] | allyModel,enemyModel", "Client-side forced player models: off, one model for all other players, or separate ally/enemy models.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_forcemodel(app)), |app, value| App::cvar_set_cg_forcemodel(app, APP_CG_FORCEMODEL.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_forcemodel, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.force_model = value.trim().to_owned()
                }),
                archive: Some((228, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.force_model)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_forcemodel(&self) -> String {
                    self.force_model.clone()
                }
                fn cvar_set_cg_forcemodel(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let parsed = ForcedPlayerModels::parse(value)?;
                        self.force_model = parsed
                            .as_ref()
                            .map(ForcedPlayerModels::serialize)
                            .unwrap_or_else(|| "0".to_owned());
                        self.forced_player_models = parsed;
                        if let Some(session) = self.game_session.as_mut() {
                            session.forced_player_models = self.forced_player_models.clone();
                        }
                        self.mark_config_dirty();
                        // PlayerPresenter compares the requested model/skin with each
                        // entity runtime every frame, so the visual swap is immediate
                        // and does not require a reconnect or a client-info rebuild.
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FOV {
        runtime: CvarDef::new(
                "cg_fov",
                "",
                "1..140",
                "TaystJK horizontal FOV on the legacy 4:3 baseline.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_fov(app)),
                |app, value| App::cvar_set_cg_fov(app, APP_CG_FOV.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_fov, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.fov = finite()
                            .unwrap_or(settings.fov)
                            .clamp(MIN_CG_FOV, MAX_CG_FOV)
                    }
                }),
                archive: Some((298, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.fov)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fov(&self) -> String {
                    format!("{:.3}", self.camera.cg_fov())
                }
                fn cvar_set_cg_fov(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let fov = Self::parse_cvar_finite(name, value)?;
                        if !(crate::camera::MIN_CG_FOV..=crate::camera::MAX_CG_FOV).contains(&fov) {
                            return Err(format!(
                                "{name}: expected {}..{} degrees",
                                crate::camera::MIN_CG_FOV,
                                crate::camera::MAX_CG_FOV
                            ));
                        }
                        self.camera.set_cg_fov(fov);
                        self.mark_config_dirty();
                        self.publish_snapshot();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FPLS {
        runtime: CvarDef::new(
                "cg_fpls",
                "",
                "0..1",
                "Allow first-person saber/melee when cg_thirdPerson is disabled.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_fpls(app)),
                |app, value| App::cvar_set_cg_fpls(app, APP_CG_FPLS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_fpls, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.first_person_lightsaber =
                            parse_bool(value).unwrap_or(settings.first_person_lightsaber)
                    }
                }),
                archive: Some((366, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.first_person_lightsaber))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_fpls(&self) -> String {
                    bool_value(self.first_person_lightsaber)
                }
                fn cvar_set_cg_fpls(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.first_person_lightsaber = Self::parse_cvar_bool(name, value)?;
                        self.third_person_camera.reset();
                        self.mark_config_dirty();
                        self.update_solo_player_view_and_presentation();
                        self.publish_snapshot();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SABERSTAFFMULTICOLOR {
        runtime: CvarDef::new("cg_saberStaffMultiColor", "", "0..1", "TaystJK staff-saber option: use the secondary saber color for primary-saber blades after blade 0.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_saberstaffmulticolor(app)), |app, value| App::cvar_set_cg_saberstaffmulticolor(app, APP_CG_SABERSTAFFMULTICOLOR.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_saberstaffmulticolor, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.saber_staff_multi_color =
                            parse_bool(value).unwrap_or(settings.saber_staff_multi_color)
                    }
                }),
                archive: Some((369, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.saber_staff_multi_color))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_saberstaffmulticolor(&self) -> String {
                    bool_value(self.saber_staff_multi_color)
                }
                fn cvar_set_cg_saberstaffmulticolor(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.saber_staff_multi_color = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session
                                .player_presenter
                                .set_saber_staff_multi_color(self.saber_staff_multi_color);
                        }
                        self.mark_config_dirty();
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SABERTEAMCOLORS {
        runtime: CvarDef::new("cg_saberTeamColors", "", "0..1", "TaystJK team saber colors: force red-team sabers red and blue-team sabers blue outside Siege/Jedi-vs-Merc.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_saberteamcolors(app)), |app, value| App::cvar_set_cg_saberteamcolors(app, APP_CG_SABERTEAMCOLORS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_saberteamcolors, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.saber_team_colors = parse_bool(value).unwrap_or(settings.saber_team_colors)
                    }
                }),
                archive: Some((368, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.saber_team_colors))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_saberteamcolors(&self) -> String {
                    bool_value(self.saber_team_colors)
                }
                fn cvar_set_cg_saberteamcolors(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.saber_team_colors = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session
                                .player_presenter
                                .set_saber_team_colors(self.saber_team_colors);
                        }
                        self.mark_config_dirty();
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SABERTRAIL {
        runtime: CvarDef::new("cg_saberTrail", "", "0|1|2", "OpenJK saber swing trail: 0 off, 1 normal authored saberBlur trail, 2 legacy special/high-frequency mode.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_sabertrail(app)), |app, value| App::cvar_set_cg_sabertrail(app, APP_CG_SABERTRAIL.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_sabertrail, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.saber_trail = value
                            .trim()
                            .parse::<i32>()
                            .ok()
                            .map(|value| value.clamp(0, 2))
                            .unwrap_or(settings.saber_trail)
                    }
                }),
                archive: Some((367, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.saber_trail)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_sabertrail(&self) -> String {
                    self.saber_trail.to_string()
                }
                fn cvar_set_cg_sabertrail(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.saber_trail = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_saberTrail expects 0, 1, or 2".to_owned())?
                            .clamp(0, 2);
                        self.mark_config_dirty();
                        self.push_console_line(format!("cg_saberTrail = {}", self.saber_trail));
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SCOREDEATHS {
        runtime: CvarDef::new(
                "cg_scoreDeaths",
                "",
                "0..3",
                "TaystJK scoreboard deaths: 0 off, 1 server data, 2 local fallback, 3 local count.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_scoredeaths(app)),
                |app, value| {
                    App::cvar_set_cg_scoredeaths(app, APP_CG_SCOREDEATHS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_scoredeaths, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.score_deaths = value
                            .trim()
                            .parse::<i32>()
                            .ok()
                            .map(|v| v.clamp(0, 3))
                            .unwrap_or(settings.score_deaths)
                    }
                }),
                archive: Some((377, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.score_deaths)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_scoredeaths(&self) -> String {
                    self.score_deaths.to_string()
                }
                fn cvar_set_cg_scoredeaths(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        self.score_deaths = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| "cg_scoreDeaths expects 0..3".to_owned())?
                            .clamp(0, 3);
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SCORESPECTATORGUESS {
        runtime: CvarDef::new(
                "cg_scoreSpectatorGuess",
                "",
                "0..1",
                "Heuristically label a spectator as SPECTATING YOU after they mirror three consecutive distinct changes of your scoreboard ping. Cosmetic guess only.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_scorespectatorguess(app)),
                |app, value| {
                    App::cvar_set_cg_scorespectatorguess(app, APP_CG_SCORESPECTATORGUESS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_scorespectatorguess, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.score_spectator_guess =
                        parse_bool(value).unwrap_or(settings.score_spectator_guess);
                }),
                archive: Some((417, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.score_spectator_guess))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_scorespectatorguess(&self) -> String {
                    bool_value(self.score_spectator_guess)
                }
                fn cvar_set_cg_scorespectatorguess(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    self.score_spectator_guess = Self::parse_cvar_bool(name, value)?;
                    for guess in &mut self.spectator_ping_guesses {
                        guess.reset();
                    }
                    if let Some(board) = self.scoreboard.as_mut() {
                        for entry in &mut board.entries {
                            entry.spectating_you = false;
                        }
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SCREENSHAKE {
        runtime: CvarDef::new("cg_screenShake", "", "0..2", "Camera shake: 1 effect-driven (explosions, creature efx), 2 also the kick of firing rockets, repeater alt, flechette, bryar/demp2 alt and bowcaster. Server-triggered shake events are not affected, as in OpenJK.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_screenshake(app)), |app, value| App::cvar_set_cg_screenshake(app, APP_CG_SCREENSHAKE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_screenshake, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.screen_shake = int_in(value, 2).unwrap_or(settings.screen_shake)
                    }
                }),
                archive: Some((293, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.screen_shake)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_screenshake(&self) -> String {
                    self.screen_shake.to_string()
                }
                fn cvar_set_cg_screenshake(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.screen_shake = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0) as u8;
                        if let Some(session) = self.game_session.as_mut() {
                            session.screen_shake_level = self.screen_shake;
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HEADLOOK {
        runtime: CvarDef::new(
            "cg_headLook", "", "0|1|2",
            "Client-only automatic player head look targets: 0 off, 1 original OpenJK/TaystJK tracking, 2 gradual tracking with frame-rate-independent yaw smoothing. Does not change server target selection or player aim.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app| CvarValue::String(App::cvar_get_cg_headlook(app)),
            |app, value| App::cvar_set_cg_headlook(app, APP_CG_HEADLOOK.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_headlook, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    if mode <= 2 { settings.head_look = mode; }
                }
            }),
            archive: Some((499, |state| state.presentation.head_look.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_cg_headlook(&self) -> String {
                    self.head_look.to_string()
                }
                fn cvar_set_cg_headlook(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let mode = value.trim().parse::<u8>()
                        .map_err(|_| "cg_headLook expects 0, 1, or 2".to_owned())?;
                    if mode > 2 { return Err("cg_headLook expects 0, 1, or 2".to_owned()); }
                    self.head_look = mode;
                    if let Some(session) = self.game_session.as_mut() {
                        session.player_presenter.set_head_look_mode(mode);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SWIMTILT {
        runtime: CvarDef::new(
            "cg_swimTilt", "0.9", "0..1",
            "Client-only direction-based swim body tilt (0 stock OpenJK, 1 full; fractional strengths allowed). Keeps the stock swim animations and never alters collision, movement, or server state.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.swim_tilt.to_string()),
            |app, value| App::cvar_set_cg_swimtilt(app, APP_CG_SWIMTILT.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_swimtilt, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(strength) = value.trim().parse::<f32>() {
                    if strength.is_finite() { settings.swim_tilt = strength.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((27001, |state| state.presentation.swim_tilt.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_cg_swimtilt(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let strength = value.trim().parse::<f32>()
                        .map_err(|_| "cg_swimTilt expects a number between 0 and 1".to_owned())?;
                    if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
                        return Err("cg_swimTilt expects a number between 0 and 1".to_owned());
                    }
                    self.swim_tilt = strength;
                    if let Some(session) = self.game_session.as_mut() {
                        session.player_presenter.set_swim_tilt(strength);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SWIMTURNBANK {
        runtime: CvarDef::new(
            "cg_swimTurnBank", "1", "0..1",
            "Client-only swim banking up to 45 degrees in response to changes in horizontal travel direction (0 off, 1 full). cg_swimTilt 0 disables all swim orientation changes; does not affect physics or camera.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.swim_turn_bank.to_string()),
            |app, value| App::cvar_set_cg_swimturnbank(app, APP_CG_SWIMTURNBANK.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_swimturnbank, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(strength) = value.trim().parse::<f32>() {
                    if strength.is_finite() { settings.swim_turn_bank = strength.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((27002, |state| state.presentation.swim_turn_bank.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_cg_swimturnbank(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let strength = value.trim().parse::<f32>()
                        .map_err(|_| "cg_swimTurnBank expects a number between 0 and 1".to_owned())?;
                    if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
                        return Err("cg_swimTurnBank expects a number between 0 and 1".to_owned());
                    }
                    self.swim_turn_bank = strength;
                    if let Some(session) = self.game_session.as_mut() {
                        session.player_presenter.set_swim_turn_bank(strength);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_MOVELEAN {
        runtime: CvarDef::new(
            "cg_moveLean", "0.75", "0..1",
            "PM_NORMAL airborne strafejump lean (0..1). No effect through 500 UPS; smoothly full by 800 UPS. No noclip or grounded lean.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.move_lean.to_string()),
            |app, value| App::cvar_set_cg_movelean(app, APP_CG_MOVELEAN.entry.name, &value.to_string()),
        ).with_text_setter(App::cvar_set_cg_movelean, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(strength) = value.trim().parse::<f32>() {
                    if strength.is_finite() { settings.move_lean = strength.clamp(0.0, 1.0); }
                }
            }),
            archive: Some((27003, |state| state.presentation.move_lean.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_set_cg_movelean(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    let strength = value.trim().parse::<f32>()
                        .map_err(|_| "cg_moveLean expects a number between 0 and 1".to_owned())?;
                    if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
                        return Err("cg_moveLean expects a number between 0 and 1".to_owned());
                    }
                    self.move_lean = strength;
                    if let Some(session) = self.game_session.as_mut() {
                        session.player_presenter.set_move_lean(strength);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPECCAMERAMOTION {
        runtime: CvarDef::new("cg_specCameraMotion", "", "0|1", "Third-person spectator camera faces the current presented direction of motion (TaystJK cg_thirdPersonAngle -1 behavior).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_speccameramotion(app)), |app, value| App::cvar_set_cg_speccameramotion(app, APP_CG_SPECCAMERAMOTION.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_speccameramotion, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.spectator_camera.motion_direction =
                            parse_bool(value).unwrap_or(settings.spectator_camera.motion_direction)
                    }
                }),
                archive: Some((364, |state| {
                    let presentation = state.presentation;
                    format!(
                        "{}",
                        u8::from(presentation.spectator_camera.motion_direction)
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_speccameramotion(&self) -> String {
                    bool_value(self.spectator_camera.motion_direction)
                }
                fn cvar_set_cg_speccameramotion(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.spectator_camera.motion_direction = Self::parse_cvar_bool(name, value)?;
                        self.third_person_camera.reset();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SPECORBITRANGE {
        runtime: CvarDef::new(
                "cg_specOrbitRange",
                "",
                "24..1200",
                "Spectator orbit-camera distance; the mouse wheel changes it while orbiting.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_specorbitrange(app)),
                |app, value| {
                    App::cvar_set_cg_specorbitrange(
                        app,
                        APP_CG_SPECORBITRANGE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_specorbitrange, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.spectator_camera.orbit_range = finite()
                            .unwrap_or(settings.spectator_camera.orbit_range)
                            .clamp(MIN_SPECTATOR_ORBIT_RANGE, MAX_SPECTATOR_ORBIT_RANGE)
                    }
                }),
                archive: Some((365, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.spectator_camera.orbit_range)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_specorbitrange(&self) -> String {
                    format!("{:.3}", self.spectator_camera.orbit_range)
                }
                fn cvar_set_cg_specorbitrange(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.spectator_camera.orbit_range = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?
                            .clamp(MIN_SPECTATOR_ORBIT_RANGE, MAX_SPECTATOR_ORBIT_RANGE);
                        self.third_person_camera.reset();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSON {
        runtime: CvarDef::new("cg_thirdPerson", "", "0|1|2", "Camera mode: 0 first person, 1 third person, 2 spectator orbit (treated as 1 while playing).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_thirdperson(app)), |app, value| App::cvar_set_cg_thirdperson(app, APP_CG_THIRDPERSON.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_thirdperson, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let mode = value
                        .trim()
                        .parse::<i32>()
                        .ok()
                        .map(|value| value.clamp(0, 2))
                        .unwrap_or(settings.spectator_camera.mode.as_i32());
                    settings.third_person.enabled = mode != 0;
                    settings.spectator_camera.mode = SpectatorCameraMode::from_i32(mode);
                }),
                archive: Some((363, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.spectator_camera.mode.as_i32())
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdperson(&self) -> String {
                    self.spectator_camera.mode.as_i32().to_string()
                }
                fn cvar_set_cg_thirdperson(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let mode = value.trim().parse::<i32>().map_err(|_| {
                            "cg_thirdPerson expects 0 (first), 1 (third), or 2 (spectator orbit)".to_owned()
                        })?;
                        if !(0..=2).contains(&mode) {
                            return Err(
                                "cg_thirdPerson expects 0 (first), 1 (third), or 2 (spectator orbit)"
                                    .to_owned(),
                            );
                        }
                        // One camera-mode cvar. Mode 2 is still ordinary third person
                        // for an active player; only spectator/demo follow interprets
                        // it as free orbit.
                        self.third_person.enabled = mode != 0;
                        self.spectator_camera.mode = SpectatorCameraMode::from_i32(mode);
                        self.spectator_camera_state.reset();
                        self.third_person_camera.reset();
                        self.sync_demo_camera_capture();
                        self.mark_config_dirty();
                        self.update_solo_player_view_and_presentation();
                        self.publish_snapshot();
                        self.publish_ui();
                    }
                    self.mark_japro_camera_userinfo_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONALPHA {
        runtime: CvarDef::new(
                "cg_thirdPersonAlpha",
                "",
                "float",
                "OpenJK third-person player alpha.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonalpha(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonalpha(
                        app,
                        APP_CG_THIRDPERSONALPHA.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonalpha, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.alpha = finite().unwrap_or(settings.third_person.alpha)
                    }
                }),
                archive: Some((379, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.alpha)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonalpha(&self) -> String {
                    format!("{:.3}", self.third_person.alpha)
                }
                fn cvar_set_cg_thirdpersonalpha(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.third_person.alpha = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONANGLE {
        runtime: CvarDef::new(
                "cg_thirdPersonAngle",
                "",
                "degrees",
                "OpenJK third-person orbit angle.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonangle(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonangle(
                        app,
                        APP_CG_THIRDPERSONANGLE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonangle, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.angle = finite().unwrap_or(settings.third_person.angle)
                    }
                }),
                archive: Some((380, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.angle)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonangle(&self) -> String {
                    format!("{:.3}", self.third_person.angle)
                }
                fn cvar_set_cg_thirdpersonangle(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.third_person.angle = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONCAMERADAMP {
        runtime: CvarDef::new(
                "cg_thirdPersonCameraDamp",
                "",
                "float",
                "Third-person camera damping (DinurdoJK default 1).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersoncameradamp(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersoncameradamp(
                        app,
                        APP_CG_THIRDPERSONCAMERADAMP.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersoncameradamp, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.camera_damp =
                            finite().unwrap_or(settings.third_person.camera_damp)
                    }
                }),
                archive: Some((381, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.camera_damp)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersoncameradamp(&self) -> String {
                    format!("{:.3}", self.third_person.camera_damp)
                }
                fn cvar_set_cg_thirdpersoncameradamp(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.camera_damp = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONHORZOFFSET {
        runtime: CvarDef::new(
                "cg_thirdPersonHorzOffset",
                "",
                "units",
                "OpenJK third-person horizontal camera offset.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonhorzoffset(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonhorzoffset(
                        app,
                        APP_CG_THIRDPERSONHORZOFFSET.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonhorzoffset, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.horz_offset =
                            finite().unwrap_or(settings.third_person.horz_offset)
                    }
                }),
                archive: Some((382, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.horz_offset)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonhorzoffset(&self) -> String {
                    format!("{:.3}", self.third_person.horz_offset)
                }
                fn cvar_set_cg_thirdpersonhorzoffset(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.horz_offset = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONPITCHOFFSET {
        runtime: CvarDef::new(
                "cg_thirdPersonPitchOffset",
                "",
                "degrees",
                "OpenJK third-person pitch offset.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonpitchoffset(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonpitchoffset(
                        app,
                        APP_CG_THIRDPERSONPITCHOFFSET.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonpitchoffset, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.pitch_offset =
                            finite().unwrap_or(settings.third_person.pitch_offset)
                    }
                }),
                archive: Some((383, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.pitch_offset)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonpitchoffset(&self) -> String {
                    format!("{:.3}", self.third_person.pitch_offset)
                }
                fn cvar_set_cg_thirdpersonpitchoffset(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.pitch_offset = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONRANGE {
        runtime: CvarDef::new(
                "cg_thirdPersonRange",
                "",
                "units",
                "TaystJK third-person camera range.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonrange(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonrange(
                        app,
                        APP_CG_THIRDPERSONRANGE.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonrange, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.range = finite().unwrap_or(settings.third_person.range)
                    }
                }),
                archive: Some((384, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.range)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonrange(&self) -> String {
                    format!("{:.3}", self.third_person.range)
                }
                fn cvar_set_cg_thirdpersonrange(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.third_person.range = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    self.mark_japro_camera_userinfo_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONSPECIALCAM {
        runtime: CvarDef::new("cg_thirdPersonSpecialCam", "", "0..1", "TaystJK action camera during saber special moves; Rust action-camera branch is not ported yet.", CvarType::String, CvarFlags::NONE, LatchScope::None,
         |app| CvarValue::String(App::cvar_get_cg_thirdpersonspecialcam(app)),
         |app, value| App::cvar_set_cg_thirdpersonspecialcam(app, APP_CG_THIRDPERSONSPECIALCAM.entry.name, &value.to_string())
        ).with_text_setter(App::cvar_set_cg_thirdpersonspecialcam, None),
        config: CvarBinding {
                default: Some(|state| {
                    bool_value(state.presentation.third_person.special_cam)
                 }),
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.third_person.special_cam =
                            parse_bool(value).unwrap_or(settings.third_person.special_cam)
                    }
                }),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonspecialcam(&self) -> String {
                    bool_value(self.third_person.special_cam)
                }
                fn cvar_set_cg_thirdpersonspecialcam(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.special_cam = Self::parse_cvar_bool(name, value)?;
                        // CVAR_NONE in TaystJK/OpenJK: runtime only, not archived.
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONTARGETDAMP {
        runtime: CvarDef::new(
                "cg_thirdPersonTargetDamp",
                "",
                "float",
                "Third-person target damping (DinurdoJK default 1).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersontargetdamp(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersontargetdamp(
                        app,
                        APP_CG_THIRDPERSONTARGETDAMP.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersontargetdamp, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.target_damp =
                            finite().unwrap_or(settings.third_person.target_damp)
                    }
                }),
                archive: Some((385, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.target_damp)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersontargetdamp(&self) -> String {
                    format!("{:.3}", self.third_person.target_damp)
                }
                fn cvar_set_cg_thirdpersontargetdamp(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.target_damp = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_THIRDPERSONVERTOFFSET {
        runtime: CvarDef::new(
                "cg_thirdPersonVertOffset",
                "",
                "units",
                "OpenJK third-person vertical target offset.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_thirdpersonvertoffset(app)),
                |app, value| {
                    App::cvar_set_cg_thirdpersonvertoffset(
                        app,
                        APP_CG_THIRDPERSONVERTOFFSET.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_cg_thirdpersonvertoffset, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.third_person.vert_offset =
                            finite().unwrap_or(settings.third_person.vert_offset)
                    }
                }),
                archive: Some((386, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.third_person.vert_offset)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_thirdpersonvertoffset(&self) -> String {
                    format!("{:.3}", self.third_person.vert_offset)
                }
                fn cvar_set_cg_thirdpersonvertoffset(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.third_person.vert_offset = value
                            .trim()
                            .parse::<f32>()
                            .map_err(|_| format!("{name}: expected a number"))?;
                        self.mark_config_dirty();
                    }
                    self.mark_japro_camera_userinfo_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CG_ZOOMFOV {
        runtime: CvarDef::new(
                "cg_zoomFov",
                "",
                "number",
                "TaystJK held +zoom target field of view.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_zoomfov(app)),
                |app, value| App::cvar_set_cg_zoomfov(app, APP_CG_ZOOMFOV.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_zoomfov, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    settings.zoom_fov = finite().unwrap_or(settings.zoom_fov)
                }),
                archive: Some((294, |state| {
                    let presentation = state.presentation;
                    format!("{:.3}", presentation.zoom_fov)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_zoomfov(&self) -> String {
                    format!("{:.3}", self.japro_zoom_fov)
                }
                fn cvar_set_cg_zoomfov(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.japro_zoom_fov = Self::parse_cvar_finite(name, value)?;
                        self.mark_config_dirty();
                        self.publish_snapshot();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CL_CHATLOG {
        runtime: CvarDef::new("cl_chatLog", "", "0..1", "Save live chat as one HTML log per day per server in the active fs_game/chatlogs directory. File I/O runs on a dedicated worker thread.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_chatlog(app)), |app, value| App::cvar_set_cl_chatlog(app, APP_CL_CHATLOG.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_chatlog, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.chat_log = parse_bool(value).unwrap_or(settings.chat_log)
                }),
                archive: Some((290, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.chat_log))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cl_chatlog(&self) -> String {
                    bool_value(self.chat_log_enabled)
                }
                fn cvar_set_cl_chatlog(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        if self.chat_log_enabled != enabled {
                            self.chat_log_enabled = enabled;
                            if enabled {
                                self.begin_chat_log_for_current_connection();
                                if let Some(map_name) = self
                                    .game_session
                                    .as_ref()
                                    .filter(|session| session.live && !session.local)
                                    .and_then(|session| session.map_name.clone())
                                {
                                    self.update_chat_log_live_metadata(&map_name);
                                }
                            } else {
                                self.chat_log.end("Chat logging disabled");
                            }
                            self.mark_config_dirty();
                            self.publish_ui();
                        }
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CL_INPUT_LATELATCH {
        runtime: CvarDef::new("cl_input_latelatch", "", "0..1", "Experimental: resample the newest subframe view orientation on the render thread at the latest coherent camera point.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_input_latelatch(app)), |app, value| App::cvar_set_cl_input_latelatch(app, APP_CL_INPUT_LATELATCH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_input_latelatch, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.input_latelatch = parse_bool(value).unwrap_or(settings.input_latelatch);
                }),
                archive: Some((35, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.input_latelatch))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cl_input_latelatch(&self) -> String {
                    bool_value(self.video.input_latelatch)
                }
                fn cvar_set_cl_input_latelatch(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        self.set_input_latelatch(enabled);
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CL_MOUSEACCEL {
        runtime: CvarDef::new(
                "cl_mouseAccel",
                "",
                "float",
                "TaystJK legacy style-0 mouse acceleration; 0 disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cl_mouseaccel(app)),
                |app, value| {
                    App::cvar_set_cl_mouseaccel(app, APP_CL_MOUSEACCEL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cl_mouseaccel, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    settings.mouse.accel = finite().unwrap_or(settings.mouse.accel)
                }),
                archive: Some((362, |state| {
                    let presentation = state.presentation;
                    format!("{:.6}", presentation.mouse.accel)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cl_mouseaccel(&self) -> String {
                    format!("{:.6}", self.mouse_input.accel)
                }
                fn cvar_set_cl_mouseaccel(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.mouse_input.accel = Self::parse_cvar_finite(name, value)?;
                        if let Some(player) = &mut self.local_server {
                            player.set_mouse_input_settings(self.mouse_input);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CL_TIMERRESOLUTION1MS {
        runtime: CvarDef::new("cl_timerResolution1ms", "", "0|1", "Windows-only A/B toggle for timeBeginPeriod(1). Affects timeout/sleep granularity, not raw mouse-event wakeup.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cl_timerresolution1ms(app)), |app, value| App::cvar_set_cl_timerresolution1ms(app, APP_CL_TIMERRESOLUTION1MS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cl_timerresolution1ms, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.timer_resolution_1ms =
                        parse_bool(value).unwrap_or(settings.timer_resolution_1ms);
                }),
                archive: Some((34, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.timer_resolution_1ms))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cl_timerresolution1ms(&self) -> String {
                    bool_value(self.video.timer_resolution_1ms)
                }
                fn cvar_set_cl_timerresolution1ms(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        self.set_timer_resolution_1ms(enabled)?;
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_COM_MAXFPS {
        runtime: CvarDef::new(
                "com_maxfps",
                "",
                "0 or 1..10000",
                "Render FPS cap; 0 resolves to the highest rate the present path can reach.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_com_maxfps(app)),
                |app, value| App::cvar_set_com_maxfps(app, APP_COM_MAXFPS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_com_maxfps, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.fps_cap = normalize_fps_cap(value, settings.fps_cap);
                }),
                archive: Some((31, |state| {
                    let settings = state.video;
                    format!("{}", settings.fps_cap)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_com_maxfps(&self) -> String {
                    self.effective_fps_cap().to_string()
                }
                fn cvar_set_com_maxfps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let cap = config::normalize_fps_cap(value, self.video.fps_cap);
                        if value.trim().parse::<u32>().is_err() {
                            return Err(format!("{name}: expected an integer FPS cap"));
                        }
                        self.set_fps_cap(cap);
                    }
                    self.mark_japro_net_userinfo_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYLINES {
        runtime: CvarDef::new(
                "con_notifylines",
                "",
                "integer",
                "TaystJK/OpenJK: number of recent console rows considered by the top-left notify overlay.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_con_notifylines(app)),
                |app, value| App::cvar_set_con_notifylines(app, "con_notifylines", &value.to_string()),
            )
            .with_text_setter(App::cvar_set_con_notifylines, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.console_notify_lines = value
                        .trim()
                        .parse::<i32>()
                        .unwrap_or(settings.console_notify_lines);
                }),
                archive: Some((9999, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.console_notify_lines)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_con_notifylines(&self) -> String {
                    self.console_notify_lines.to_string()
                }
                fn cvar_set_con_notifylines(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_lines = value
                        .trim()
                        .parse::<i32>()
                        .map_err(|_| format!("{name}: expected an integer"))?;
                    self.mark_config_dirty();
                    self.publish_transient_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYWORDS {
        runtime: CvarDef::new("con_notifywords", "0", "0 | -1 | word [word ...]", "TaystJK: desktop/highlight notification words for chat. Up to 8 space-separated terms; -1 alerts on every chat.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_notifywords(app)), |app, value| App::cvar_set_con_notifywords(app, "con_notifywords", &value.to_string())).with_text_setter(App::cvar_set_con_notifywords, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                let value = value.trim();
                settings.console_notify_words = if value.is_empty() { "0".to_owned() } else { value.to_owned() };
            }),
            archive: Some((412, |state| {
                let presentation = state.presentation;
                presentation.console_notify_words.clone()
            })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_con_notifywords(&self) -> String {
                    self.console_notify_words.clone()
                }
                fn cvar_set_con_notifywords(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_words = value.trim().to_owned();
                    if self.console_notify_words.is_empty() {
                        self.console_notify_words = "0".to_owned();
                    }
                    self.console_notify_terms = crate::app::console_state::parse_console_notify_terms(&self.console_notify_words);
                    self.mark_config_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYCONNECT {
        runtime: CvarDef::new("con_notifyconnect", "0", "0..1", "TaystJK: request desktop attention for connect/disconnect server messages.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_notifyconnect(app)), |app, value| App::cvar_set_con_notifyconnect(app, "con_notifyconnect", &value.to_string())).with_text_setter(App::cvar_set_con_notifyconnect, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.console_notify_connect = parse_bool(value).unwrap_or(settings.console_notify_connect);
            }),
            archive: Some((413, |state| {
                let presentation = state.presentation;
                format!("{}", u8::from(presentation.console_notify_connect))
            })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_con_notifyconnect(&self) -> String { bool_value(self.console_notify_connect) }
                fn cvar_set_con_notifyconnect(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_connect = Self::parse_cvar_bool(name, value)?;
                    self.mark_config_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYVOTE {
        runtime: CvarDef::new("con_notifyvote", "1", "0..1", "TaystJK: request desktop attention when someone calls a vote.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_notifyvote(app)), |app, value| App::cvar_set_con_notifyvote(app, "con_notifyvote", &value.to_string())).with_text_setter(App::cvar_set_con_notifyvote, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.console_notify_vote = parse_bool(value).unwrap_or(settings.console_notify_vote);
            }),
            archive: Some((414, |state| {
                let presentation = state.presentation;
                format!("{}", u8::from(presentation.console_notify_vote))
            })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_con_notifyvote(&self) -> String { bool_value(self.console_notify_vote) }
                fn cvar_set_con_notifyvote(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_vote = Self::parse_cvar_bool(name, value)?;
                    self.mark_config_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYBEEP {
        runtime: CvarDef::new("con_notifybeep", "1", "0..1", "Play the standard JKA chat beep when a desktop/highlight notification condition fires.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_notifybeep(app)), |app, value| App::cvar_set_con_notifybeep(app, "con_notifybeep", &value.to_string())).with_text_setter(App::cvar_set_con_notifybeep, None),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.console_notify_beep = parse_bool(value).unwrap_or(settings.console_notify_beep);
            }),
            archive: Some((415, |state| {
                let presentation = state.presentation;
                format!("{}", u8::from(presentation.console_notify_beep))
            })),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_con_notifybeep(&self) -> String { bool_value(self.console_notify_beep) }
                fn cvar_set_con_notifybeep(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_beep = Self::parse_cvar_bool(name, value)?;
                    self.mark_config_dirty();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_NOTIFYTIME {
        runtime: CvarDef::new("con_notifytime", "3", "float", "TaystJK/OpenJK Con_DrawNotify lifetime in seconds. Negative values suppress notify drawing while console printing continues.", CvarType::String, CvarFlags::NONE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_notifytime(app)), |app, value| App::cvar_set_con_notifytime(app, "con_notifytime", &value.to_string())).with_text_setter(App::cvar_set_con_notifytime, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_get_con_notifytime(&self) -> String {
                    format!("{:.3}", self.console_notify_time)
                }
                fn cvar_set_con_notifytime(&mut self, name: &str, value: &str) -> Result<(), String> {
                    self.console_notify_time = value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|number| number.is_finite())
                        .ok_or_else(|| format!("{name}: expected a finite number"))?;
                    self.publish_transient_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_CON_SUGGEST {
        runtime: CvarDef::new("con_suggest", "", "0..1", "Live command/cvar filter while typing in the console: Up/Down pick, Tab completes, Esc dismisses.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_con_suggest(app)), |app, value| App::cvar_set_con_suggest(app, APP_CON_SUGGEST.entry.name, &value.to_string())).with_text_setter(App::cvar_set_con_suggest, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.console_suggest = parse_bool(value).unwrap_or(settings.console_suggest)
                    }
                }),
                archive: Some((288, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.console_suggest))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_con_suggest(&self) -> String {
                    bool_value(self.console_suggest)
                }
                fn cvar_set_con_suggest(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.console_suggest = Self::parse_cvar_bool(name, value)?;
                        self.console_suggest_index = 0;
                        self.console_suggest_picked = false;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CON_TIMESTAMPS {
        runtime: CvarDef::new(
                "con_timestamps",
                "",
                "0..1",
                "Prefix console lines with local HH:MM:SS timestamps.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_con_timestamps(app)),
                |app, value| {
                    App::cvar_set_con_timestamps(app, APP_CON_TIMESTAMPS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_con_timestamps, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    {
                        settings.console_timestamps =
                            parse_bool(value).unwrap_or(settings.console_timestamps)
                    }
                }),
                archive: Some((287, |state| {
                    let presentation = state.presentation;
                    format!("{}", u8::from(presentation.console_timestamps))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_con_timestamps(&self) -> String {
                    bool_value(self.console_timestamps)
                }
                fn cvar_set_con_timestamps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.console_timestamps = Self::parse_cvar_bool(name, value)?;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_M_PITCH {
        runtime: CvarDef::new(
                "m_pitch",
                "",
                "float",
                "TaystJK vertical mouse scale.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_m_pitch(app)),
                |app, value| App::cvar_set_m_pitch(app, APP_M_PITCH.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_m_pitch, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    settings.mouse.pitch = finite().unwrap_or(settings.mouse.pitch)
                }),
                archive: Some((361, |state| {
                    let presentation = state.presentation;
                    format!("{:.6}", presentation.mouse.pitch)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_m_pitch(&self) -> String {
                    format!("{:.6}", self.mouse_input.pitch)
                }
                fn cvar_set_m_pitch(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.mouse_input.pitch = Self::parse_cvar_finite(name, value)?;
                        if let Some(player) = &mut self.local_server {
                            player.set_mouse_input_settings(self.mouse_input);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_M_YAW {
        runtime: CvarDef::new(
                "m_yaw",
                "",
                "float",
                "TaystJK horizontal mouse scale.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_m_yaw(app)),
                |app, value| App::cvar_set_m_yaw(app, APP_M_YAW.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_m_yaw, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    settings.mouse.yaw = finite().unwrap_or(settings.mouse.yaw)
                }),
                archive: Some((360, |state| {
                    let presentation = state.presentation;
                    format!("{:.6}", presentation.mouse.yaw)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_m_yaw(&self) -> String {
                    format!("{:.6}", self.mouse_input.yaw)
                }
                fn cvar_set_m_yaw(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.mouse_input.yaw = Self::parse_cvar_finite(name, value)?;
                        if let Some(player) = &mut self.local_server {
                            player.set_mouse_input_settings(self.mouse_input);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_MODEL {
        runtime: CvarDef::new(
                "model",
                "",
                "model[/skin]",
                "Local player model and skin.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_model(app)),
                |app, value| App::cvar_set_model(app, APP_MODEL.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_model, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    if !value.trim().is_empty() {
                        settings.model = value.trim().to_owned();
                    }
                }),
                archive: Some((227, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.model)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_model(&self) -> String {
                    self.solo_client_info.model_cvar()
                }
                fn cvar_set_model(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let trimmed = value.trim();
                        if trimmed.is_empty() {
                            return Err("model: expected model[/skin]".to_owned());
                        }
                        self.solo_client_info = ClientInfo::solo_model(trimmed);
                        self.sync_local_client_info();
                        self.mark_config_dirty();
                        self.update_solo_player_view_and_presentation();
                        self.publish_snapshot();
                        self.publish_ui();
                    }
                    self.request_userinfo_send();
                    Ok(())
                }
            }
        ]
    }

    APP_SENSITIVITY {
        runtime: CvarDef::new(
                "sensitivity",
                "",
                "float",
                "TaystJK mouse sensitivity multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sensitivity(app)),
                |app, value| App::cvar_set_sensitivity(app, APP_SENSITIVITY.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sensitivity, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.mouse.sensitivity = finite().unwrap_or(settings.mouse.sensitivity)
                    }
                }),
                archive: Some((359, |state| {
                    let presentation = state.presentation;
                    format!("{:.6}", presentation.mouse.sensitivity)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sensitivity(&self) -> String {
                    format!("{:.6}", self.mouse_input.sensitivity)
                }
                fn cvar_set_sensitivity(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.mouse_input.sensitivity = Self::parse_cvar_finite(name, value)?;
                        if let Some(player) = &mut self.local_server {
                            player.set_mouse_input_settings(self.mouse_input);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CL_SERVERFILTERS {
        runtime: CvarDef::new(
                "cl_serverFilters",
                "",
                "flags|maxping|mods",
                "Server browser filters (hide flags, max ping, mod list); saved between launches.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app: &App| CvarValue::String(app.server_browser.filters_string()),
                |app, value| App::cvar_set_cl_serverfilters(app, APP_CL_SERVERFILTERS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cl_serverfilters, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.server_filters = value.to_owned()
                }),
                archive: Some((26036, |state| state.presentation.server_filters.clone())),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_set_cl_serverfilters(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    self.server_browser.apply_filters_string(value);
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_SV_MASTER1 {
        runtime: CvarDef::new(
                "sv_master1",
                "",
                "host[:port]",
                "TaystJK master slot 1 (Raven); empty disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sv_master1(app)),
                |app, value| App::cvar_set_sv_master1(app, APP_SV_MASTER1.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sv_master1, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.master_servers[0] = value.trim().to_owned()
                }),
                archive: Some((299, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.master_servers[0])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sv_master1(&self) -> String {
                    self.server_browser.master_servers[0].clone()
                }
                fn cvar_set_sv_master1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "sv_master1".to_owned();
                    {
                        let slot = lower
                            .strip_prefix("sv_master")
                            .and_then(|number| number.parse::<usize>().ok())
                            .and_then(|number| number.checked_sub(1))
                            .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                            .ok_or_else(|| format!("{name}: invalid master slot"))?;
                        let master = value.trim();
                        if master.len() > 255 {
                            return Err(format!("{name}: master address is too long"));
                        }
                        self.server_browser.master_servers[slot] = master.to_owned();
                        if !master.is_empty() {
                            self.server_browser.master_drafts[slot] = master.to_owned();
                        }
                        self.server_browser.status_text =
                            "Master server settings changed â€” press Refresh to query them".to_owned();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_SV_MASTER2 {
        runtime: CvarDef::new(
                "sv_master2",
                "",
                "host[:port]",
                "TaystJK master slot 2 (JKHub); empty disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sv_master2(app)),
                |app, value| App::cvar_set_sv_master2(app, APP_SV_MASTER2.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sv_master2, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.master_servers[1] = value.trim().to_owned()
                }),
                archive: Some((300, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.master_servers[1])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sv_master2(&self) -> String {
                    self.server_browser.master_servers[1].clone()
                }
                fn cvar_set_sv_master2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "sv_master2".to_owned();
                    {
                        let slot = lower
                            .strip_prefix("sv_master")
                            .and_then(|number| number.parse::<usize>().ok())
                            .and_then(|number| number.checked_sub(1))
                            .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                            .ok_or_else(|| format!("{name}: invalid master slot"))?;
                        let master = value.trim();
                        if master.len() > 255 {
                            return Err(format!("{name}: master address is too long"));
                        }
                        self.server_browser.master_servers[slot] = master.to_owned();
                        if !master.is_empty() {
                            self.server_browser.master_drafts[slot] = master.to_owned();
                        }
                        self.server_browser.status_text =
                            "Master server settings changed â€” press Refresh to query them".to_owned();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_SV_MASTER3 {
        runtime: CvarDef::new(
                "sv_master3",
                "",
                "host[:port]",
                "TaystJK master slot 3 (Ouned); empty disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sv_master3(app)),
                |app, value| App::cvar_set_sv_master3(app, APP_SV_MASTER3.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sv_master3, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.master_servers[2] = value.trim().to_owned()
                }),
                archive: Some((301, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.master_servers[2])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sv_master3(&self) -> String {
                    self.server_browser.master_servers[2].clone()
                }
                fn cvar_set_sv_master3(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "sv_master3".to_owned();
                    {
                        let slot = lower
                            .strip_prefix("sv_master")
                            .and_then(|number| number.parse::<usize>().ok())
                            .and_then(|number| number.checked_sub(1))
                            .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                            .ok_or_else(|| format!("{name}: invalid master slot"))?;
                        let master = value.trim();
                        if master.len() > 255 {
                            return Err(format!("{name}: master address is too long"));
                        }
                        self.server_browser.master_servers[slot] = master.to_owned();
                        if !master.is_empty() {
                            self.server_browser.master_drafts[slot] = master.to_owned();
                        }
                        self.server_browser.status_text =
                            "Master server settings changed â€” press Refresh to query them".to_owned();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_SV_MASTER4 {
        runtime: CvarDef::new(
                "sv_master4",
                "",
                "host[:port]",
                "TaystJK custom master slot 4; empty disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sv_master4(app)),
                |app, value| App::cvar_set_sv_master4(app, APP_SV_MASTER4.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sv_master4, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.master_servers[3] = value.trim().to_owned()
                }),
                archive: Some((302, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.master_servers[3])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sv_master4(&self) -> String {
                    self.server_browser.master_servers[3].clone()
                }
                fn cvar_set_sv_master4(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "sv_master4".to_owned();
                    {
                        let slot = lower
                            .strip_prefix("sv_master")
                            .and_then(|number| number.parse::<usize>().ok())
                            .and_then(|number| number.checked_sub(1))
                            .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                            .ok_or_else(|| format!("{name}: invalid master slot"))?;
                        let master = value.trim();
                        if master.len() > 255 {
                            return Err(format!("{name}: master address is too long"));
                        }
                        self.server_browser.master_servers[slot] = master.to_owned();
                        if !master.is_empty() {
                            self.server_browser.master_drafts[slot] = master.to_owned();
                        }
                        self.server_browser.status_text =
                            "Master server settings changed â€” press Refresh to query them".to_owned();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_SV_MASTER5 {
        runtime: CvarDef::new(
                "sv_master5",
                "",
                "host[:port]",
                "TaystJK custom master slot 5; empty disables it.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_sv_master5(app)),
                |app, value| App::cvar_set_sv_master5(app, APP_SV_MASTER5.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_sv_master5, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.master_servers[4] = value.trim().to_owned()
                }),
                archive: Some((303, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.master_servers[4])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_sv_master5(&self) -> String {
                    self.server_browser.master_servers[4].clone()
                }
                fn cvar_set_sv_master5(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "sv_master5".to_owned();
                    {
                        let slot = lower
                            .strip_prefix("sv_master")
                            .and_then(|number| number.parse::<usize>().ok())
                            .and_then(|number| number.checked_sub(1))
                            .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                            .ok_or_else(|| format!("{name}: invalid master slot"))?;
                        let master = value.trim();
                        if master.len() > 255 {
                            return Err(format!("{name}: master address is too long"));
                        }
                        self.server_browser.master_servers[slot] = master.to_owned();
                        if !master.is_empty() {
                            self.server_browser.master_drafts[slot] = master.to_owned();
                        }
                        self.server_browser.status_text =
                            "Master server settings changed â€” press Refresh to query them".to_owned();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_UI_VGS {
        runtime: CvarDef::new(
                "ui_vgs",
                "",
                "0=off, nonzero=on",
                "TaystJK: use the jaPRO VGS canned-voice menu.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_ui_vgs(app)),
                |app, value| App::cvar_set_ui_vgs(app, APP_UI_VGS.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_ui_vgs, None),
        config: CvarBinding {
                load_presentation: Some(|settings, _name, value| {
                    settings.ui_vgs = value.trim().parse::<i32>().unwrap_or(settings.ui_vgs)
                }),
                archive: Some((291, |state| {
                    let presentation = state.presentation;
                    format!("{}", presentation.ui_vgs)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_ui_vgs(&self) -> String {
                    self.ui_vgs.to_string()
                }
                fn cvar_set_ui_vgs(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.ui_vgs = value
                            .trim()
                            .parse::<i32>()
                            .map_err(|_| format!("{name}: expected an integer"))?;
                        self.mark_config_dirty();
                        if self.ui_vgs == 0 && self.overlay == OverlayMode::Vgs {
                            self.vgs_menu = crate::vgs::Menu::Main;
                            self.set_overlay(OverlayMode::None);
                        } else {
                            self.publish_ui();
                        }
                    }
                    Ok(())
                }
            }
        ]
    }
}
