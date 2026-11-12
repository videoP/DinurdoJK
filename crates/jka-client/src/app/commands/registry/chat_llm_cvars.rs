//! DinurdoJK local chat model cvars, shown under Setup -> Interface ->
//! Translation and Chat Filter. Models are GGUF files downloaded once from
//! the given address and run on this computer; chat text never leaves it.
//! Prompts are stored on one line: `\n` is a line break, `\\` a backslash.
use super::cvar_prelude::*;
use crate::chat_llm as cl;

/// An empty value restores the default model.
fn model_address(name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    jka_llm::download::parse_url(value).map(|model| model.url).map_err(|error| format!("{name}: {error}"))
}

/// A double quote cannot be written inside a quoted cfg value.
fn prompt_value(value: &str) -> String {
    value.trim().replace('"', "'")
}

app_cvars! {
    APP_CG_CHATTRANSLATEMODEL {
        runtime: CvarDef::new("cg_chatTranslateModel", "", "https URL of a .gguf file",
            "Model for cg_chatTranslateProvider local. Empty uses Tencent Hy-MT2 1.8B (about 1.1 GB, 33 languages). Any GGUF chat model works; Hugging Face page links are accepted.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_llm.settings.translate_url.clone()),
            |app, value| {
                app.chat_llm.settings.translate_url = model_address("cg_chatTranslateModel", &value.to_string())?;
                app.chat_llm.edits.translate_url = None;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(url) = model_address("", value) { settings.chat_llm.translate_url = url; }
            }),
            archive: Some((28150, |state| state.presentation.chat_llm.translate_url.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEPROMPT {
        runtime: CvarDef::new("cg_chatTranslatePrompt", "", "text",
            "Instruction given to the local translation model. {target}, {source} and {text} are filled in. Empty uses the built-in prompt.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_llm.settings.translate_prompt.clone()),
            |app, value| {
                app.chat_llm.settings.translate_prompt = prompt_value(&value.to_string());
                app.chat_llm.edits.translate_prompt = None;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.chat_llm.translate_prompt = prompt_value(value);
            }),
            archive: Some((28151, |state| state.presentation.chat_llm.translate_prompt.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEIGNORESELF {
        runtime: CvarDef::new("cg_chatTranslateIgnoreSelf", "1", "0|1",
            "Never translate your own chat messages automatically, with any translation service. 0 treats them like everyone else's. A message can still be translated by hand from the console.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_llm.settings.translate_ignore_self),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.translate_ignore_self = v;
                app.mark_config_dirty();
                app.publish_ui();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_llm.translate_ignore_self = v; }
            }),
            archive: Some((28161, |state| u8::from(state.presentation.chat_llm.translate_ignore_self).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSOR {
        runtime: CvarDef::new("cg_chatCensor", "0", "0|1",
            "Chat filter: a local model masks profanity and hostility in incoming chat, including disguised spellings, using the kill feed and recent chat as context. Downloads cg_chatLlmModel once.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_llm.settings.censor),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.censor = v;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_llm.censor = v; }
            }),
            archive: Some((28152, |state| u8::from(state.presentation.chat_llm.censor).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSORHOLD {
        runtime: CvarDef::new("cg_chatCensorHold", "1", "0|1",
            "Keep a message's text off the chat box and console until the filter has checked it (shown as ... meanwhile). Off shows it at once and replaces it if the filter objects.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_llm.settings.censor_hold),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.censor_hold = v;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_llm.censor_hold = v; }
            }),
            archive: Some((28153, |state| u8::from(state.presentation.chat_llm.censor_hold).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSORSEVERITY {
        runtime: CvarDef::new("cg_chatCensorSeverity", "3", "1..5",
            "How far the chat filter reads between the lines: 1 only explicit words, 3 also clear harassment, 5 treats anything that could be a jab at you as hostile, connecting kills and earlier chat.",
            CvarType::Int { min: 1, max: 5 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.chat_llm.settings.censor_severity)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.censor_severity = v.clamp(1, 5) as u8;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_llm.censor_severity = v.clamp(1, 5) as u8; }
            }),
            archive: Some((28154, |state| state.presentation.chat_llm.censor_severity.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSORACTION {
        runtime: CvarDef::new("cg_chatCensorAction", "0", "0|1",
            "What the chat filter does with a message it objects to: 0 shows it with the offending words masked, 1 does not show it and leaves a marked placeholder line.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.chat_llm.settings.censor_action.index()),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.censor_action = cl::CensorAction::from_index(v);
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_llm.censor_action = cl::CensorAction::from_index(v); }
            }),
            archive: Some((28155, |state| state.presentation.chat_llm.censor_action.index().to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSORRAGEBAIT {
        runtime: CvarDef::new("cg_chatCensorRagebait", "0", "0|1",
            "Chat filter also catches ragebait: messages meant to provoke (sarcastic praise after a kill, passive-aggressive politeness, goading someone who is upset), judged from the kill feed and the chat just before. Works at any cg_chatCensorSeverity. Needs the 4B filter model to be reliable.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_llm.settings.censor_ragebait),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.censor_ragebait = v;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_llm.censor_ragebait = v; }
            }),
            archive: Some((28160, |state| u8::from(state.presentation.chat_llm.censor_ragebait).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATLLMMODEL {
        runtime: CvarDef::new("cg_chatLlmModel", "", "https URL of a .gguf file",
            "General model for the chat filter and quickRundown. Empty uses Qwen3.5 4B (about 2.7 GB, about a second per message); Setup offers the lighter 2B model (1.3 GB), which misjudges banter. Set it to the same address as cg_chatTranslateModel to keep one model in memory for everything.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_llm.settings.assist_url.clone()),
            |app, value| {
                app.chat_llm.settings.assist_url = model_address("cg_chatLlmModel", &value.to_string())?;
                app.chat_llm.edits.assist_url = None;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(url) = model_address("", value) { settings.chat_llm.assist_url = url; }
            }),
            archive: Some((28156, |state| state.presentation.chat_llm.assist_url.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATCENSORPROMPT {
        runtime: CvarDef::new("cg_chatCensorPrompt", "", "text",
            "Instruction given to the chat filter model. It must keep the answer format (SAFE, or TOXIC: the offending words). Empty uses the built-in prompt; the strictness rule and match context are added to it.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_llm.settings.censor_prompt.clone()),
            |app, value| {
                app.chat_llm.settings.censor_prompt = prompt_value(&value.to_string());
                app.chat_llm.edits.censor_prompt = None;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.chat_llm.censor_prompt = prompt_value(value);
            }),
            archive: Some((28157, |state| state.presentation.chat_llm.censor_prompt.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_QUICKRUNDOWNPROMPT {
        runtime: CvarDef::new("cg_quickRundownPrompt", "", "text",
            "Instruction given to the model for quickRundown. {player} and {language} are filled in. Empty uses the built-in prompt.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_llm.settings.summary_prompt.clone()),
            |app, value| {
                app.chat_llm.settings.summary_prompt = prompt_value(&value.to_string());
                app.chat_llm.edits.summary_prompt = None;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.chat_llm.summary_prompt = prompt_value(value);
            }),
            archive: Some((28158, |state| state.presentation.chat_llm.summary_prompt.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATLLMTHREADS {
        runtime: CvarDef::new("cg_chatLlmThreads", "0", "0..32",
            "CPU threads the local chat models may use. 0 picks about a third of the cores so the game keeps its own.",
            CvarType::Int { min: 0, max: 32 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.chat_llm.settings.threads)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_llm.settings.threads = v.clamp(0, 32) as u8;
                app.chat_llm_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_llm.threads = v.clamp(0, 32) as u8; }
            }),
            archive: Some((28159, |state| state.presentation.chat_llm.threads.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
}
