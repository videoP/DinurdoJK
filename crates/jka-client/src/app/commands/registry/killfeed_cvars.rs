//! DinurdoJK interface cvars: kill feed (`cg_killfeed*`), chat box (`cg_chat*`)
//! and scoreboard (`cg_scoreboard*`), shown under Setup -> Interface.
use super::cvar_prelude::*;

app_cvars! {
    APP_CG_KILLFEED {
        runtime: CvarDef::new("cg_killfeed", "1", "0|1",
            "Show the DinurdoJK kill feed in the top-right corner (move and scale it in HUD Edit).",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.killfeed_settings.enabled),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.killfeed_settings.enabled = v;
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.killfeed.enabled = v; }
            }),
            archive: Some((28110, |state| u8::from(state.presentation.killfeed.enabled).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDSTYLE {
        runtime: CvarDef::new("cg_killfeedStyle", "0", "0..4",
            "Kill feed look: 0 Call of Duty, 1 Quake, 2 Counter-Strike, 3 Mordhau, 4 Chivalry.",
            CvarType::Int { min: 0, max: 4 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.killfeed_settings.style.index()),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.killfeed_settings.style = crate::ui::KillfeedStyle::from_index(v);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.killfeed.style = crate::ui::KillfeedStyle::from_index(v); }
            }),
            archive: Some((28111, |state| state.presentation.killfeed.style.index().to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDHIGHLIGHTOWN {
        runtime: CvarDef::new("cg_killfeedHighlightOwn", "1", "0|1",
            "Mark kill feed lines where you are the killer or the victim.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.killfeed_settings.highlight_own),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.killfeed_settings.highlight_own = v;
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.killfeed.highlight_own = v; }
            }),
            archive: Some((28112, |state| u8::from(state.presentation.killfeed.highlight_own).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDOWNKILLTIME {
        runtime: CvarDef::new("cg_killfeedOwnKillTime", "8", "0.5..30",
            "Seconds your own kills stay in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.own_kill_time)),
            |app, value| {
                app.killfeed_settings.own_kill_time = App::parse_cvar_finite("cg_killfeedOwnKillTime", &value.to_string())?.clamp(0.5, 30.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.own_kill_time = v.clamp(0.5, 30.0); } }
            }),
            archive: Some((28113, |state| format!("{}", state.presentation.killfeed.own_kill_time))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDOWNDEATHTIME {
        runtime: CvarDef::new("cg_killfeedOwnDeathTime", "8", "0.5..30",
            "Seconds your own deaths stay in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.own_death_time)),
            |app, value| {
                app.killfeed_settings.own_death_time = App::parse_cvar_finite("cg_killfeedOwnDeathTime", &value.to_string())?.clamp(0.5, 30.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.own_death_time = v.clamp(0.5, 30.0); } }
            }),
            archive: Some((28114, |state| format!("{}", state.presentation.killfeed.own_death_time))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDTIME {
        runtime: CvarDef::new("cg_killfeedTime", "5", "0.5..30",
            "Seconds other players' kills stay in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.other_time)),
            |app, value| {
                app.killfeed_settings.other_time = App::parse_cvar_finite("cg_killfeedTime", &value.to_string())?.clamp(0.5, 30.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.other_time = v.clamp(0.5, 30.0); } }
            }),
            archive: Some((28115, |state| format!("{}", state.presentation.killfeed.other_time))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDOWNKILLALPHA {
        runtime: CvarDef::new("cg_killfeedOwnKillAlpha", "1", "0..1",
            "Opacity of your own kills in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.own_kill_alpha)),
            |app, value| {
                app.killfeed_settings.own_kill_alpha = App::parse_cvar_finite("cg_killfeedOwnKillAlpha", &value.to_string())?.clamp(0.0, 1.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.own_kill_alpha = v.clamp(0.0, 1.0); } }
            }),
            archive: Some((28116, |state| format!("{}", state.presentation.killfeed.own_kill_alpha))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDOWNDEATHALPHA {
        runtime: CvarDef::new("cg_killfeedOwnDeathAlpha", "1", "0..1",
            "Opacity of your own deaths in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.own_death_alpha)),
            |app, value| {
                app.killfeed_settings.own_death_alpha = App::parse_cvar_finite("cg_killfeedOwnDeathAlpha", &value.to_string())?.clamp(0.0, 1.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.own_death_alpha = v.clamp(0.0, 1.0); } }
            }),
            archive: Some((28117, |state| format!("{}", state.presentation.killfeed.own_death_alpha))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDALPHA {
        runtime: CvarDef::new("cg_killfeedAlpha", "0.85", "0..1",
            "Opacity of other players' kills in the kill feed.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.killfeed_settings.other_alpha)),
            |app, value| {
                app.killfeed_settings.other_alpha = App::parse_cvar_finite("cg_killfeedAlpha", &value.to_string())?.clamp(0.0, 1.0);
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.killfeed.other_alpha = v.clamp(0.0, 1.0); } }
            }),
            archive: Some((28118, |state| format!("{}", state.presentation.killfeed.other_alpha))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_KILLFEEDLINES {
        runtime: CvarDef::new("cg_killfeedLines", "6", "1..12",
            "Most kill feed lines shown at once.",
            CvarType::Int { min: 1, max: 12 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.killfeed_settings.max_lines)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.killfeed_settings.max_lines = v.clamp(1, 12) as u8;
                app.mark_config_dirty();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.killfeed.max_lines = v.clamp(1, 12) as u8; }
            }),
            archive: Some((28119, |state| state.presentation.killfeed.max_lines.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }


    APP_CG_CHATSTYLE {
        runtime: CvarDef::new("cg_chatStyle", "1", "0|1",
            "Chat box style: 1 docked (typing field under the feed, with recent history), 0 classic top bar.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_settings.docked),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_settings.docked = v;
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat.docked = v; }
            }),
            archive: Some((28120, |state| u8::from(state.presentation.chat.docked).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATHISTORYLINES {
        runtime: CvarDef::new("cg_chatHistoryLines", "8", "1..20",
            "Messages shown at full strength while you type (docked chat).",
            CvarType::Int { min: 1, max: 20 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.chat_settings.history_lines)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_settings.history_lines = v.clamp(1, 20) as u8;
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat.history_lines = v.clamp(1, 20) as u8; }
            }),
            archive: Some((28121, |state| state.presentation.chat.history_lines.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATFADETIME {
        runtime: CvarDef::new("cg_chatFadeTime", "10", "1..60",
            "Seconds a chat message stays on screen before fading.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.chat_settings.fade_time)),
            |app, value| {
                app.chat_settings.fade_time = App::parse_cvar_finite("cg_chatFadeTime", &value.to_string())?.clamp(1.0, 60.0);
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.chat.fade_time = v.clamp(1.0, 60.0); } }
            }),
            archive: Some((28122, |state| format!("{}", state.presentation.chat.fade_time))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATHISTORYTIME {
        runtime: CvarDef::new("cg_chatHistoryTime", "120", "10..600",
            "Seconds chat messages are kept for the typing view.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.chat_settings.history_time)),
            |app, value| {
                app.chat_settings.history_time = App::parse_cvar_finite("cg_chatHistoryTime", &value.to_string())?.clamp(10.0, 600.0);
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.chat.history_time = v.clamp(10.0, 600.0); } }
            }),
            archive: Some((28123, |state| format!("{}", state.presentation.chat.history_time))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATMUTEAFTERATTACK {
        runtime: CvarDef::new("cg_chatMuteAfterAttack", "2", "0..10",
            "Silence chat sounds for this many seconds after you attack; 0 never silences them.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(format!("{}", app.chat_settings.mute_after_attack)),
            |app, value| {
                app.chat_settings.mute_after_attack = App::parse_cvar_finite("cg_chatMuteAfterAttack", &value.to_string())?.clamp(0.0, 10.0);
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<f32>() { if v.is_finite() { settings.chat.mute_after_attack = v.clamp(0.0, 10.0); } }
            }),
            archive: Some((28124, |state| format!("{}", state.presentation.chat.mute_after_attack))),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_SCOREBOARDAVATARS {
        runtime: CvarDef::new("cg_scoreboardAvatars", "1", "0|1",
            "Show player avatars beside the scoreboard's centre divider.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.scoreboard_settings.avatars),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.scoreboard_settings.avatars = v;
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.scoreboard.avatars = v; }
            }),
            archive: Some((28125, |state| u8::from(state.presentation.scoreboard.avatars).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_SCOREBOARDHEADTOHEAD {
        runtime: CvarDef::new("cg_scoreboardHeadToHead", "1", "0|1",
            "Show your kills/deaths against each player by the scoreboard's centre divider.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.scoreboard_settings.head_to_head),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.scoreboard_settings.head_to_head = v;
                app.mark_config_dirty();
                app.publish_ui();
                app.publish_transient_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.scoreboard.head_to_head = v; }
            }),
            archive: Some((28126, |state| u8::from(state.presentation.scoreboard.head_to_head).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
}
