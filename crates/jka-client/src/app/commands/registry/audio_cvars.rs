//! Audio cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

app_cvars! {
    APP_S_VARIOMETERINPUT {
        runtime: CvarDef::new("s_variometerInput", "0", "0..1", "Variometer input: 0 horizontal acceleration, 1 strafehelper angle error (high = overshooting, low = undershooting).", CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.audio.variometer_input)),
            |app, value| App::cvar_set_s_variometerinput(app, APP_S_VARIOMETERINPUT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometerinput, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| { settings.variometer_input = int_in(value, 1).unwrap_or(settings.variometer_input); }),
            archive: Some((27352, |state| state.audio.variometer_input.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometerinput(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer_input = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0) as u8;
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_VARIOMETERDEADZONE {
        runtime: CvarDef::new("s_variometerDeadZone", "0.5", "0..5", "Strafe-angle variometer dead zone: degrees either side of the strafehelper's optimal angle that stay silent.", CvarType::Float { min: 0.0, max: 5.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.audio.variometer_dead_zone),
            |app, value| App::cvar_set_s_variometerdeadzone(app, APP_S_VARIOMETERDEADZONE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometerdeadzone, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| {
                if let Some(degrees) = value.parse::<f32>().ok().filter(|n| n.is_finite()) {
                    settings.variometer_dead_zone = degrees.clamp(0.0, 5.0);
                }
            }),
            archive: Some((27353, |state| format!("{:.2}", state.audio.variometer_dead_zone))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometerdeadzone(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer_dead_zone = Self::parse_cvar_finite(name, value)?.clamp(0.0, 5.0);
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_VARIOMETERSCALE {
        runtime: CvarDef::new("s_variometerScale", "1", "0.25..8", "Strafe-angle variometer sensitivity multiplier. 1 reaches the full tone range 15 degrees past the dead zone; 4 reaches it in under 4.", CvarType::Float { min: 0.25, max: 8.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.audio.variometer_scale),
            |app, value| App::cvar_set_s_variometerscale(app, APP_S_VARIOMETERSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometerscale, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| {
                if let Some(scale) = value.parse::<f32>().ok().filter(|n| n.is_finite()) {
                    settings.variometer_scale = scale.clamp(0.25, 8.0);
                }
            }),
            archive: Some((27354, |state| format!("{:.2}", state.audio.variometer_scale))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometerscale(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer_scale = Self::parse_cvar_finite(name, value)?.clamp(0.25, 8.0);
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_VARIOMETER {
        runtime: CvarDef::new("s_variometer", "0", "0|1", "Enable the horizontal acceleration variometer.", CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.audio.variometer),
            |app, value| App::cvar_set_s_variometer(app, APP_S_VARIOMETER.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometer, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| { settings.variometer = parse_bool(value).unwrap_or(settings.variometer); }),
            archive: Some((27349, |state| u8::from(state.audio.variometer).to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometer(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer = Self::parse_cvar_bool(name, value)?;
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_VARIOMETERVOLUME {
        runtime: CvarDef::new("s_variometerVolume", "0.35", "0..1", "Horizontal variometer volume, scaled by s_volume.", CvarType::Float { min: 0.0, max: 1.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.audio.variometer_volume),
            |app, value| App::cvar_set_s_variometervolume(app, APP_S_VARIOMETERVOLUME.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometervolume, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| {
                if let Some(level) = value.parse::<f32>().ok().filter(|n| n.is_finite()) {
                    settings.variometer_volume = level.clamp(0.0, 1.0);
                }
            }),
            archive: Some((27350, |state| format!("{:.3}", state.audio.variometer_volume))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometervolume(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer_volume = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_VARIOMETERSTYLE {
        runtime: CvarDef::new("s_variometerStyle", "0", "0..2", "Variometer sound: 0 Classic, 1 BeatDrop-inspired note pairs, 2 Continuous pitch.", CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.audio.variometer_style)),
            |app, value| App::cvar_set_s_variometerstyle(app, APP_S_VARIOMETERSTYLE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_variometerstyle, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| { settings.variometer_style = int_in(value, 2).unwrap_or(settings.variometer_style); }),
            archive: Some((27351, |state| state.audio.variometer_style.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_variometerstyle(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.variometer_style = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0) as u8;
                self.update_variometer();
                self.mark_config_dirty();
                self.publish_ui();
                Ok(())
            }
        }]
    }
    APP_S_EMOTEMUSICVOLUME {
        runtime: CvarDef::new("s_emoteMusicVolume", "0.5", "0..1", "Emote music level, independent of game effects and map music.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{:.3}", app.audio.emote_music_volume)),
            |app, value| App::cvar_set_s_emotemusicvolume(app, APP_S_EMOTEMUSICVOLUME.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_s_emotemusicvolume, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| { if let Some(level)=value.parse::<f32>().ok().filter(|n|n.is_finite()){settings.emote_music_volume=level.clamp(0.,1.);} }),
            archive: Some((27348, |state| format!("{:.3}", state.audio.emote_music_volume))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_s_emotemusicvolume(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.audio.emote_music_volume=Self::parse_cvar_finite(name,value)?.clamp(0.,1.);
                self.apply_audio_mix();self.mark_config_dirty();self.publish_ui();Ok(())
            }
        }]
    }
    APP_CG_AMBIENTSOUNDS {
        runtime: CvarDef::new("cg_ambientSounds", "", "0..1", "Level ambience from the map's sound sets (worldspawn soundSet and local ambient entities).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_ambientsounds(app)), |app, value| App::cvar_set_cg_ambientsounds(app, APP_CG_AMBIENTSOUNDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_ambientsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.ambient = parse_bool(value).unwrap_or(settings.game.ambient)
                    }
                }),
                archive: Some((404, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.game.ambient))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_ambientsounds(&self) -> String {
                    bool_value(self.audio.game.ambient)
                }
                fn cvar_set_cg_ambientsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.ambient = Self::parse_cvar_bool(name, value)?;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_AUTOSWITCH {
        runtime: CvarDef::new("cg_autoSwitch", "", "0..2", "Weapon pickup/empty auto switch: 0 never, 1 to a better safe weapon (no rocket/thermal/mines), 2 to any better weapon.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_autoswitch(app)), |app, value| App::cvar_set_cg_autoswitch(app, APP_CG_AUTOSWITCH.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_autoswitch, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.auto_switch = int_in(value, 2).unwrap_or(settings.game.auto_switch)
                    }
                }),
                archive: Some((406, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.auto_switch)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_autoswitch(&self) -> String {
                    self.audio.game.auto_switch.to_string()
                }
                fn cvar_set_cg_autoswitch(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.auto_switch =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0) as u8;
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_BLOOD {
        runtime: CvarDef::new("cg_blood", "", "0..2", "jaPRO gibs: 0 none (death voice instead), 1 skull or brain, 2 full gibs (needs the jaPRO models/gibs assets).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_blood(app)), |app, value| App::cvar_set_cg_blood(app, APP_CG_BLOOD.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_blood, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.blood = int_in(value, 2).unwrap_or(settings.game.blood)
                }),
                archive: Some((405, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.blood)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_blood(&self) -> String {
                    self.audio.game.blood.to_string()
                }
                fn cvar_set_cg_blood(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.blood = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0) as u8;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_CHATSOUNDS {
        runtime: CvarDef::new("cg_chatSounds", "", "0..2", "Beep on incoming chat: 0 silent, 1 the legacy talk beep, 2 distinct beeps for private messages and team chat (jaPRO).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_chatsounds(app)), |app, value| App::cvar_set_cg_chatsounds(app, APP_CG_CHATSOUNDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_chatsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.chat_sounds = int_in(value, 2).unwrap_or(settings.game.chat_sounds)
                    }
                }),
                archive: Some((410, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.chat_sounds)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_chatsounds(&self) -> String {
                    self.audio.game.chat_sounds.to_string()
                }
                fn cvar_set_cg_chatsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.chat_sounds =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0) as u8;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DRAWREWARDS {
        runtime: CvarDef::new(
                "cg_drawRewards",
                "",
                "0..2",
                "TaystJK/JKA award medals: 0 off, 1 JKA voice/medals, 2 Quake 3 variants where available.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_drawrewards(app)),
                |app, value| {
                    App::cvar_set_cg_drawrewards(app, APP_CG_DRAWREWARDS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_drawrewards, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.draw_rewards = int_in(value, 2).unwrap_or(settings.game.draw_rewards)
                    }
                }),
                archive: Some((401, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.draw_rewards)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_drawrewards(&self) -> String {
                    self.audio.game.draw_rewards.to_string()
                }
                fn cvar_set_cg_drawrewards(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_drawrewards".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DUELMUSIC {
        runtime: CvarDef::new(
                "cg_duelMusic",
                "",
                "0..1",
                "jaPRO: play the duel music track while you are in a duel.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_duelmusic(app)),
                |app, value| {
                    App::cvar_set_cg_duelmusic(app, APP_CG_DUELMUSIC.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_duelmusic, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.duel_music = parse_bool(value).unwrap_or(settings.game.duel_music)
                    }
                }),
                archive: Some((403, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.game.duel_music))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_duelmusic(&self) -> String {
                    bool_value(self.audio.game.duel_music)
                }
                fn cvar_set_cg_duelmusic(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.duel_music = Self::parse_cvar_bool(name, value)?;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_DUELSOUNDS {
        runtime: CvarDef::new(
                "cg_duelSounds",
                "",
                "0..3",
                "jaPRO duel start: 0 off, 1 sound and text, 2 sound only, 3 text only.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_duelsounds(app)),
                |app, value| {
                    App::cvar_set_cg_duelsounds(app, APP_CG_DUELSOUNDS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_duelsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.duel = int_in(value, 3).unwrap_or(settings.game.duel)
                }),
                archive: Some((398, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.duel)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_duelsounds(&self) -> String {
                    self.audio.game.duel.to_string()
                }
                fn cvar_set_cg_duelsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_duelsounds".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_FOOTSTEPS {
        runtime: CvarDef::new("cg_footsteps", "", "0..4", "Player footsteps (jaPRO): 0 off, 1 sounds, 2 sounds and material effects (sand, mud, snow, gravel dust), 3+ same (footprints are controlled separately by r_footprints), 4 debugging.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_footsteps(app)), |app, value| App::cvar_set_cg_footsteps(app, APP_CG_FOOTSTEPS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_footsteps, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.footsteps = int_in(value, 4).unwrap_or(settings.game.footsteps)
                    }
                }),
                archive: Some((411, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.footsteps)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_footsteps(&self) -> String {
                    self.audio.game.footsteps.to_string()
                }
                fn cvar_set_cg_footsteps(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, 4.0) as u8;
                        self.audio.game.footsteps = level;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_GHOUL2MARKS {
        runtime: CvarDef::new("cg_ghoul2Marks", "", "0..64", "Burn marks kept on each player model by blaster/rocket hits (0 disables). Needs GPU Ghoul2 skinning.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_ghoul2marks(app)), |app, value| App::cvar_set_cg_ghoul2marks(app, APP_CG_GHOUL2MARKS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_ghoul2marks, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.g2_marks = int_in(value, 64).unwrap_or(settings.game.g2_marks)
                    }
                }),
                archive: Some((408, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.g2_marks)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_ghoul2marks(&self) -> String {
                    self.audio.game.g2_marks.to_string()
                }
                fn cvar_set_cg_ghoul2marks(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.g2_marks =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 64.0) as u8;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_HITSOUNDS {
        runtime: CvarDef::new("cg_hitsounds", "", "0..6", "jaPRO hit feedback: 1-4 pick a hit sound (needs the jaPRO sound/effects/hitsound assets), 5 plain saber hit, 6 any saber hit variant.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_hitsounds(app)), |app, value| App::cvar_set_cg_hitsounds(app, APP_CG_HITSOUNDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_hitsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.hit = int_in(value, 6).unwrap_or(settings.game.hit)
                }),
                archive: Some((402, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.hit)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_hitsounds(&self) -> String {
                    self.audio.game.hit.to_string()
                }
                fn cvar_set_cg_hitsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_hitsounds".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_JUMPSOUNDS {
        runtime: CvarDef::new("cg_jumpSounds", "", "0..3", "jaPRO jump voice: 0 off, 1 everyone, 2 other players only, 3 only you (jaPRO itself defaults to 0).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_jumpsounds(app)), |app, value| App::cvar_set_cg_jumpsounds(app, APP_CG_JUMPSOUNDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_jumpsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.jump = int_in(value, 3).unwrap_or(settings.game.jump)
                }),
                archive: Some((395, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.jump)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_jumpsounds(&self) -> String {
                    self.audio.game.jump.to_string()
                }
                fn cvar_set_cg_jumpsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_jumpsounds".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_KILLMESSAGE {
        runtime: CvarDef::new("cg_killMessage", "", "0..3", "TaystJK kill center-print: 0 off, 1 normal with FFA place/score, 2 kill only, 3 higher on screen.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_killmessage(app)), |app, value| App::cvar_set_cg_killmessage(app, APP_CG_KILLMESSAGE.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_killmessage, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.kill_message = int_in(value, 3).unwrap_or(settings.game.kill_message)
                    }
                }),
                archive: Some((400, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.kill_message)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_killmessage(&self) -> String {
                    self.audio.game.kill_message.to_string()
                }
                fn cvar_set_cg_killmessage(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_killmessage".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_KILLSOUNDS {
        runtime: CvarDef::new("cg_killSounds", "", "0..2", "jaPRO frag sound when you kill: 1 always, 2 with a mid-air variant (needs the jaPRO sound/frag assets).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None, |app| CvarValue::String(App::cvar_get_cg_killsounds(app)), |app, value| App::cvar_set_cg_killsounds(app, APP_CG_KILLSOUNDS.entry.name, &value.to_string())).with_text_setter(App::cvar_set_cg_killsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.kill = int_in(value, 2).unwrap_or(settings.game.kill)
                }),
                archive: Some((399, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.kill)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_killsounds(&self) -> String {
                    self.audio.game.kill.to_string()
                }
                fn cvar_set_cg_killsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_killsounds".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_NOTAUNT {
        runtime: CvarDef::new(
                "cg_noTaunt",
                "",
                "0..1",
                "jaPRO: silence taunt voice lines.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_notaunt(app)),
                |app, value| App::cvar_set_cg_notaunt(app, APP_CG_NOTAUNT.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_cg_notaunt, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.no_taunt = parse_bool(value).unwrap_or(settings.game.no_taunt)
                    }
                }),
                archive: Some((397, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.game.no_taunt))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_notaunt(&self) -> String {
                    bool_value(self.audio.game.no_taunt)
                }
                fn cvar_set_cg_notaunt(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.no_taunt = Self::parse_cvar_bool(name, value)?;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_RACESOUNDS {
        runtime: CvarDef::new(
                "cg_raceSounds",
                "",
                "0..255",
                "jaPRO race mode sound bit mask; bit 1 keeps the race start-trigger sound.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_racesounds(app)),
                |app, value| {
                    App::cvar_set_cg_racesounds(app, APP_CG_RACESOUNDS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_racesounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.race_sounds = int_in(value, 255).unwrap_or(settings.game.race_sounds)
                    }
                }),
                archive: Some((409, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.race_sounds)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_racesounds(&self) -> String {
                    self.audio.game.race_sounds.to_string()
                }
                fn cvar_set_cg_racesounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.race_sounds =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 255.0) as u8;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_ROLLSOUNDS {
        runtime: CvarDef::new(
                "cg_rollSounds",
                "",
                "0..3",
                "jaPRO roll voice: 0 off, 1 everyone, 2 other players only, 3 only you.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_rollsounds(app)),
                |app, value| {
                    App::cvar_set_cg_rollsounds(app, APP_CG_ROLLSOUNDS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_rollsounds, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.game.roll = int_in(value, 3).unwrap_or(settings.game.roll)
                }),
                archive: Some((396, |state| {
                    let audio = state.audio;
                    format!("{}", audio.game.roll)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_rollsounds(&self) -> String {
                    self.audio.game.roll.to_string()
                }
                fn cvar_set_cg_rollsounds(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "cg_rollsounds".to_owned();
                    {
                        let max = match lower.as_str() {
                            "cg_killsounds" | "cg_drawrewards" => 2.0,
                            "cg_hitsounds" => 6.0,
                            _ => 3.0,
                        };
                        let level = Self::parse_cvar_finite(name, value)?.clamp(0.0, max) as u8;
                        match lower.as_str() {
                            "cg_jumpsounds" => self.audio.game.jump = level,
                            "cg_rollsounds" => self.audio.game.roll = level,
                            "cg_duelsounds" => self.audio.game.duel = level,
                            "cg_killsounds" => self.audio.game.kill = level,
                            "cg_killmessage" => self.audio.game.kill_message = level,
                            "cg_drawrewards" => {
                                self.audio.game.draw_rewards = level;
                                if let Some(session) = &mut self.game_session {
                                    session.reward_active = None;
                                    session.reward_queue.clear();
                                }
                            }
                            _ => self.audio.game.hit = level,
                        }
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_CG_SCOREPLUMS {
        runtime: CvarDef::new(
                "cg_scorePlums",
                "",
                "0..1",
                "Floating score numbers where you score (kills, captures).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_cg_scoreplums(app)),
                |app, value| {
                    App::cvar_set_cg_scoreplums(app, APP_CG_SCOREPLUMS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_cg_scoreplums, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.game.score_plums = parse_bool(value).unwrap_or(settings.game.score_plums)
                    }
                }),
                archive: Some((407, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.game.score_plums))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_cg_scoreplums(&self) -> String {
                    bool_value(self.audio.game.score_plums)
                }
                fn cvar_set_cg_scoreplums(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.game.score_plums = Self::parse_cvar_bool(name, value)?;
                        self.apply_game_sounds();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_MUSICVOLUME {
        runtime: CvarDef::new(
                "s_musicvolume",
                "",
                "0..1",
                "OpenJK background music volume (level music and the duel track).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_musicvolume(app)),
                |app, value| {
                    App::cvar_set_s_musicvolume(app, APP_S_MUSICVOLUME.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_s_musicvolume, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.music_volume = finite().unwrap_or(settings.music_volume).clamp(0.0, 1.0)
                    }
                }),
                archive: Some((389, |state| {
                    let audio = state.audio;
                    format!("{:.3}", audio.music_volume)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_musicvolume(&self) -> String {
                    format!("{:.3}", self.audio.music_volume)
                }
                fn cvar_set_s_musicvolume(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.music_volume = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.apply_audio_mix();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_MUTEWHENUNFOCUSED {
        runtime: CvarDef::new(
                "s_muteWhenUnfocused",
                "",
                "0..1",
                "Mute game and voice audio while the game window is unfocused.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_mutewhenunfocused(app)),
                |app, value| {
                    App::cvar_set_s_mutewhenunfocused(
                        app,
                        APP_S_MUTEWHENUNFOCUSED.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_s_mutewhenunfocused, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.mute_when_unfocused =
                            parse_bool(value).unwrap_or(settings.mute_when_unfocused)
                    }
                }),
                archive: Some((391, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.mute_when_unfocused))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_mutewhenunfocused(&self) -> String {
                    bool_value(self.audio.mute_when_unfocused)
                }
                fn cvar_set_s_mutewhenunfocused(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.mute_when_unfocused = Self::parse_cvar_bool(name, value)?;
                        self.apply_audio_mix();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_SEPARATION {
        runtime: CvarDef::new(
                "s_separation",
                "",
                "0..1",
                "OpenJK stereo separation used by positional sound.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_separation(app)),
                |app, value| {
                    App::cvar_set_s_separation(app, APP_S_SEPARATION.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_s_separation, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.separation = finite().unwrap_or(settings.separation).clamp(0.0, 1.0)
                    }
                }),
                archive: Some((390, |state| {
                    let audio = state.audio;
                    format!("{:.3}", audio.separation)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_separation(&self) -> String {
                    format!("{:.3}", self.audio.separation)
                }
                fn cvar_set_s_separation(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.separation = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.apply_audio_mix();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_EAL {
        runtime: CvarDef::new(
                "s_eal", "", "0..1",
                "Use authored map .eal filtering, diffraction, room routing and reverb through OpenAL Soft. EAL-owned sources use one acoustic renderer.",
                CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_eal(app)),
                |app, value| App::cvar_set_s_eal(app, APP_S_EAL.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_s_eal, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| {
                settings.eal_enabled = parse_bool(value).unwrap_or(settings.eal_enabled);
            }),
            archive: Some((430, |state| format!("{}", u8::from(state.audio.eal_enabled)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_s_eal(&self) -> String { bool_value(self.audio.eal_enabled) }
                fn cvar_set_s_eal(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.audio.eal_enabled = enabled;
                    if let Some(sound) = self.game_session.as_mut()
                        .and_then(|session| session.sound_presenter.as_mut()) {
                        sound.set_eal_enabled(enabled);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_S_EALDEBUG {
        runtime: CvarDef::new(
                "s_ealDebug", "0", "0..9",
                "EAL transport: 0 normal EAX with callback PCM (legacy queue fallback if unsupported), 1 direct PCM (no OpenAL), 2 OpenAL dry, 3 cached EAX, 4 OpenAL non-spatial, 5 queue tone, 6 queue double preroll, 7 EAX callback, 8 callback tone, 9 original queue-based EAX (A/B only). Not archived.",
                CvarType::Int { min: 0, max: 9 }, CvarFlags::NONE, LatchScope::None,
                |app: &App| CvarValue::Int(i32::from(app.audio.eal_debug_mode)),
                |app, value| App::cvar_set_s_ealdebug(app, APP_S_EALDEBUG.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_s_ealdebug, None),
        config: CvarBinding::NONE,
        implementations: [
            impl App {
                fn cvar_set_s_ealdebug(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let mode = Self::parse_cvar_finite(name, value)?;
                    if mode.fract() != 0.0 || !(0.0..=9.0).contains(&mode) {
                        return Err(format!("{name} must be an integer from 0 through 9"));
                    }
                    self.audio.eal_debug_mode = mode as u8;
                    if let Some(sound) = self.game_session.as_mut()
                        .and_then(|session| session.sound_presenter.as_mut()) {
                        sound.set_eal_debug_mode(mode as u8);
                    }
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_S_EALHRTF {
        runtime: CvarDef::new(
                "s_ealHrtf", "0", "0..1",
                "OpenAL Soft headphone HRTF for map-authored EAL sources. Independent of Steam Audio HRTF; updates live.",
                CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_ealhrtf(app)),
                |app, value| App::cvar_set_s_ealhrtf(app, APP_S_EALHRTF.entry.name, &value.to_string()),
            ).with_text_setter(App::cvar_set_s_ealhrtf, None),
        config: CvarBinding {
            load_audio: Some(|settings, _name, value| {
                settings.eal_hrtf = parse_bool(value).unwrap_or(settings.eal_hrtf);
            }),
            archive: Some((431, |state| format!("{}", u8::from(state.audio.eal_hrtf)))),
            ..CvarBinding::NONE
        },
        implementations: [
            impl App {
                fn cvar_get_s_ealhrtf(&self) -> String { bool_value(self.audio.eal_hrtf) }
                fn cvar_set_s_ealhrtf(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let enabled = Self::parse_cvar_bool(name, value)?;
                    self.audio.eal_hrtf = enabled;
                    if let Some(sound) = self.game_session.as_mut()
                        .and_then(|session| session.sound_presenter.as_mut()) {
                        sound.set_eal_hrtf_enabled(enabled);
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_S_STEAMAUDIO {
        runtime: CvarDef::new(
                "s_steamAudio",
                "",
                "custom",
                "Controls s_steamAudio.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_steamaudio(app)),
                |app, value| {
                    App::cvar_set_s_steamaudio(app, APP_S_STEAMAUDIO.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_s_steamaudio, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.steam_audio = parse_bool(value).unwrap_or(settings.steam_audio)
                    }
                }),
                archive: Some((392, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.steam_audio))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_steamaudio(&self) -> String {
                    bool_value(self.audio.steam_audio)
                }
                fn cvar_set_s_steamaudio(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        self.audio.steam_audio = enabled;
                        if !enabled {
                            self.steam_audio_bake = None;
                            self.steam_audio_bake_progress = None;
                            self.steam_audio_bake_error = None;
                        }
                        let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
                        let bake = self.steam_audio_bake.clone();
                        if let Some(sound) = self
                            .game_session
                            .as_mut()
                            .and_then(|session| session.sound_presenter.as_mut())
                        {
                            sound.set_steam_audio_enabled(enabled);
                            sound.set_steam_audio_map(acoustic_mesh, bake);
                        }
                        println!(
                            "Steam Audio: {} ({})",
                            if enabled { "enabled" } else { "disabled" },
                            if enabled {
                                "signal DSP applies live; run snd_restart to prepare/attach current-map acoustics immediately, otherwise they will be prepared on the next map load"
                            } else {
                                "runtime use and future acoustic BSP/bake preparation gated off"
                            }
                        );
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_STEAMAUDIOBINAURAL {
        runtime: CvarDef::new(
                "s_steamAudioBinaural",
                "",
                "custom",
                "Controls s_steamAudioBinaural.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_steamaudiobinaural(app)),
                |app, value| {
                    App::cvar_set_s_steamaudiobinaural(
                        app,
                        APP_S_STEAMAUDIOBINAURAL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_s_steamaudiobinaural, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.steam_audio_binaural =
                            parse_bool(value).unwrap_or(settings.steam_audio_binaural)
                    }
                }),
                archive: Some((393, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.steam_audio_binaural))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_steamaudiobinaural(&self) -> String {
                    bool_value(self.audio.steam_audio_binaural)
                }
                fn cvar_set_s_steamaudiobinaural(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        self.audio.steam_audio_binaural = enabled;
                        if let Some(sound) = self
                            .game_session
                            .as_mut()
                            .and_then(|session| session.sound_presenter.as_mut())
                        {
                            sound.set_steam_audio_binaural_enabled(enabled);
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_STEAMAUDIOHRTFINTERPOLATION {
        runtime: CvarDef::new(
                "s_steamAudioHrtfInterpolation",
                "",
                "nearest|bilinear",
                "Steam Audio HRTF direction interpolation: nearest (fast default) or bilinear (smoother, more audio-thread CPU).",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_steamaudiohrtfinterpolation(app)),
                |app, value| {
                    App::cvar_set_s_steamaudiohrtfinterpolation(
                        app,
                        APP_S_STEAMAUDIOHRTFINTERPOLATION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_s_steamaudiohrtfinterpolation, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    settings.steam_audio_hrtf_interpolation =
                        match value.trim().to_ascii_lowercase().as_str() {
                            "1" | "bilinear" => 1,
                            "0" | "nearest" => 0,
                            _ => settings.steam_audio_hrtf_interpolation,
                        };
                }),
                archive: Some((394, |state| {
                    let audio = state.audio;
                    match audio.steam_audio_hrtf_interpolation {
                        1 => "bilinear".to_owned(),
                        _ => "nearest".to_owned(),
                    }
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_steamaudiohrtfinterpolation(&self) -> String {
                    match self.audio.steam_audio_hrtf_interpolation {
                        1 => "bilinear",
                        _ => "nearest",
                    }.to_owned()
                }
                fn cvar_set_s_steamaudiohrtfinterpolation(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    let interpolation = match value.trim().to_ascii_lowercase().as_str() {
                        "0" | "nearest" => 0,
                        "1" | "bilinear" => 1,
                        _ => return Err(format!("{name}: expected nearest|bilinear (or 0|1)")),
                    };
                    self.audio.steam_audio_hrtf_interpolation = interpolation;
                    if let Some(sound) = self
                        .game_session
                        .as_mut()
                        .and_then(|session| session.sound_presenter.as_mut())
                    {
                        sound.set_steam_audio_hrtf_interpolation(interpolation);
                    }
                    println!(
                        "Steam Audio HRTF interpolation: {} (live)",
                        if interpolation == 1 { "bilinear" } else { "nearest" },
                    );
                    self.mark_config_dirty();
                    self.publish_ui();
                    Ok(())
                }
            }
        ]
    }

    APP_S_STEAMAUDIOENVIRONMENTAL {
        runtime: CvarDef::new(
                "s_steamAudioEnvironmental",
                "",
                "custom",
                "Controls s_steamAudioEnvironmental.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_steamaudioenvironmental(app)),
                |app, value| {
                    App::cvar_set_s_steamaudioenvironmental(
                        app,
                        APP_S_STEAMAUDIOENVIRONMENTAL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_s_steamaudioenvironmental, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    {
                        settings.steam_audio_environmental =
                            parse_bool(value).unwrap_or(settings.steam_audio_environmental)
                    }
                }),
                archive: Some((394, |state| {
                    let audio = state.audio;
                    format!("{}", u8::from(audio.steam_audio_environmental))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_steamaudioenvironmental(&self) -> String {
                    bool_value(self.audio.steam_audio_environmental)
                }
                fn cvar_set_s_steamaudioenvironmental(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let enabled = Self::parse_cvar_bool(name, value)?;
                        self.audio.steam_audio_environmental = enabled;
                        if let Some(sound) = self
                            .game_session
                            .as_mut()
                            .and_then(|session| session.sound_presenter.as_mut())
                        {
                            sound.set_steam_audio_environmental_enabled(enabled);
                        }
                        println!(
                            "Steam Audio environmental acoustics: {} (live direct occlusion/transmission; no map reload or rebake required)",
                            if enabled { "enabled" } else { "disabled" },
                        );
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_VOLUME {
        runtime: CvarDef::new(
                "s_volume",
                "",
                "0..1",
                "OpenJK game/effects volume.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_volume(app)),
                |app, value| App::cvar_set_s_volume(app, APP_S_VOLUME.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_s_volume, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.effects_volume =
                            finite().unwrap_or(settings.effects_volume).clamp(0.0, 1.0)
                    }
                }),
                archive: Some((387, |state| {
                    let audio = state.audio;
                    format!("{:.3}", audio.effects_volume)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_volume(&self) -> String {
                    format!("{:.3}", self.audio.effects_volume)
                }
                fn cvar_set_s_volume(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.effects_volume = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.apply_audio_mix();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_S_VOLUMEVOICE {
        runtime: CvarDef::new(
                "s_volumeVoice",
                "",
                "0..1",
                "OpenJK voice-channel volume.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_s_volumevoice(app)),
                |app, value| {
                    App::cvar_set_s_volumevoice(app, APP_S_VOLUMEVOICE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_s_volumevoice, None),
        config: CvarBinding {
                load_audio: Some(|settings, _name, value| {
                    let finite = || {
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|number| number.is_finite())
                    };
                    {
                        settings.voice_volume = finite().unwrap_or(settings.voice_volume).clamp(0.0, 1.0)
                    }
                }),
                archive: Some((388, |state| {
                    let audio = state.audio;
                    format!("{:.3}", audio.voice_volume)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_s_volumevoice(&self) -> String {
                    format!("{:.3}", self.audio.voice_volume)
                }
                fn cvar_set_s_volumevoice(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.audio.voice_volume = Self::parse_cvar_finite(name, value)?.clamp(0.0, 1.0);
                        self.apply_audio_mix();
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }
}
