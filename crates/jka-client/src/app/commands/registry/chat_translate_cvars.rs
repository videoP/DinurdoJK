//! DinurdoJK chat translation cvars (`cg_chatTranslate*`), shown under
//! Setup -> Interface -> Translation. Service API keys are not cvars: they
//! live in Windows Credential Manager so they never reach the cfg or console.
use super::cvar_prelude::*;
use crate::chat_translation as ct;

app_cvars! {
    APP_CG_CHATTRANSLATE {
        runtime: CvarDef::new("cg_chatTranslate", "0", "0..2",
            "Translate incoming chat: 0 off (nothing is sent anywhere), 1 only messages you pick in the console, 2 every message detected as another language.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.chat_translation.settings.mode.index()),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_translation.settings.mode = ct::Mode::from_index(v);
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_translate.mode = ct::Mode::from_index(v); }
            }),
            archive: Some((28140, |state| state.presentation.chat_translate.mode.index().to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATETARGET {
        runtime: CvarDef::new("cg_chatTranslateTarget", "", "language code",
            "Language to translate chat into, as a two-letter code (en, ru, de, ...). Empty follows the Windows display language.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_translation.settings.target.clone()),
            |app, value| {
                let value = value.to_string().trim().to_ascii_lowercase();
                if !value.is_empty() && !ct::lang::find(&value).is_some_and(|language| language.target) {
                    let codes: Vec<_> = ct::lang::targets().map(|language| language.code).collect();
                    return Err(format!("cg_chatTranslateTarget expects one of: {}", codes.join(" ")));
                }
                app.chat_translation.settings.target = value;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                let value = value.trim().to_ascii_lowercase();
                if value.is_empty() || ct::lang::find(&value).is_some_and(|language| language.target) {
                    settings.chat_translate.target = value;
                }
            }),
            archive: Some((28141, |state| state.presentation.chat_translate.target.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEDISPLAY {
        runtime: CvarDef::new("cg_chatTranslateDisplay", "0", "0..2",
            "How translated chat is shown: 0 translation replaces the message, 1 original with the translation under it, 2 original only (translations on request in the console).",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.chat_translation.settings.display.index()),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_translation.settings.display = ct::Display::from_index(v);
                app.chat_translation_display_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_translate.display = ct::Display::from_index(v); }
            }),
            archive: Some((28142, |state| state.presentation.chat_translate.display.index().to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEPROVIDER {
        runtime: CvarDef::new("cg_chatTranslateProvider", "mymemory", "mymemory|deepl|libretranslate|local",
            "Translation service. MyMemory is free without a key; DeepL needs an API key; LibreTranslate uses cg_chatTranslateUrl. Message text is sent to the chosen service. local runs cg_chatTranslateModel on this computer instead and sends nothing.",
            CvarType::Enum(&["mymemory", "deepl", "libretranslate", "local"]), CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_translation.settings.provider.name().to_owned()),
            |app, value| {
                let provider = ct::ProviderKind::from_name(&value.to_string())
                    .ok_or_else(|| "cg_chatTranslateProvider expects mymemory, deepl, libretranslate or local".to_owned())?;
                app.chat_translation.settings.provider = provider;
                app.chat_translation.key_note = None;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(provider) = ct::ProviderKind::from_name(value) { settings.chat_translate.provider = provider; }
            }),
            archive: Some((28143, |state| state.presentation.chat_translate.provider.name().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEURL {
        runtime: CvarDef::new("cg_chatTranslateUrl", "", "http(s) URL",
            "LibreTranslate server address, e.g. http://localhost:5000 for a self-hosted instance.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_translation.settings.libre_url.clone()),
            |app, value| {
                let value = value.to_string().trim().to_owned();
                if !value.is_empty() && !(value.starts_with("http://") || value.starts_with("https://")) {
                    return Err("cg_chatTranslateUrl must start with http:// or https://".to_owned());
                }
                app.chat_translation.settings.libre_url = value;
                app.chat_translation.url_input = None;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                settings.chat_translate.libre_url = value.trim().to_owned();
            }),
            archive: Some((28144, |state| state.presentation.chat_translate.libre_url.clone())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATETEAM {
        runtime: CvarDef::new("cg_chatTranslateTeam", "0", "0|1",
            "Also translate team chat automatically. Off by default: team chat is sent to the service only when you pick a message.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_translation.settings.team),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_translation.settings.team = v;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_translate.team = v; }
            }),
            archive: Some((28145, |state| u8::from(state.presentation.chat_translate.team).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEDEMOS {
        runtime: CvarDef::new("cg_chatTranslateDemos", "0", "0|1",
            "Translate chat automatically during demo playback too.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.chat_translation.settings.demos),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!("validated cvar type") };
                app.chat_translation.settings.demos = v;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(v) = parse_bool(value) { settings.chat_translate.demos = v; }
            }),
            archive: Some((28146, |state| u8::from(state.presentation.chat_translate.demos).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATECODEPAGE {
        runtime: CvarDef::new("cg_chatTranslateCodepage", "auto", "auto|utf8|1252|1251",
            "How to read chat that is not UTF-8: auto tells Cyrillic (Windows-1251) from Western (Windows-1252) text; or force one encoding.",
            CvarType::Enum(&["auto", "utf8", "1252", "1251"]), CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.chat_translation.settings.codepage.name().to_owned()),
            |app, value| {
                let codepage = ct::Codepage::from_name(&value.to_string())
                    .ok_or_else(|| "cg_chatTranslateCodepage expects auto, utf8, 1252 or 1251".to_owned())?;
                app.chat_translation.settings.codepage = codepage;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Some(codepage) = ct::Codepage::from_name(value) { settings.chat_translate.codepage = codepage; }
            }),
            archive: Some((28147, |state| state.presentation.chat_translate.codepage.name().to_owned())),
            ..CvarBinding::NONE
        },
        implementations: []
    }

    APP_CG_CHATTRANSLATEMAXCHARS {
        runtime: CvarDef::new("cg_chatTranslateMaxChars", "300", "20..500",
            "Longest message text sent for translation; longer messages are cut.",
            CvarType::Int { min: 20, max: 500 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.chat_translation.settings.max_chars)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!("validated cvar type") };
                app.chat_translation.settings.max_chars = v.clamp(20, 500) as u16;
                app.chat_translation_settings_changed();
                Ok(())
            }),
        config: CvarBinding {
            load_presentation: Some(|settings, _name, value| {
                if let Ok(v) = value.trim().parse::<i32>() { settings.chat_translate.max_chars = v.clamp(20, 500) as u16; }
            }),
            archive: Some((28148, |state| state.presentation.chat_translate.max_chars.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
}
