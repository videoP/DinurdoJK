//! Language codes, the target-language list, and local language detection.
//!
//! Codes are ISO 639-1, which MyMemory and LibreTranslate accept directly;
//! DeepL wants upper case plus a regional variant for a few targets.

/// Writing system, used to tell "obviously not the target language" apart
/// from same-script guesses that need a confident detector result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Latin,
    Cyrillic,
    Greek,
    Han,
    Japanese,
    Hangul,
    Other,
}

pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
    pub script: Script,
    /// Offered as a translation target. Right-to-left and complex-shaping
    /// scripts are source-only: the console cannot lay them out correctly.
    pub target: bool,
}

const fn lang(code: &'static str, name: &'static str, script: Script, target: bool) -> Language {
    Language { code, name, script, target }
}

pub const LANGUAGES: &[Language] = &[
    lang("en", "English", Script::Latin, true),
    lang("es", "Spanish", Script::Latin, true),
    lang("pt", "Portuguese", Script::Latin, true),
    lang("fr", "French", Script::Latin, true),
    lang("de", "German", Script::Latin, true),
    lang("it", "Italian", Script::Latin, true),
    lang("nl", "Dutch", Script::Latin, true),
    lang("pl", "Polish", Script::Latin, true),
    lang("cs", "Czech", Script::Latin, true),
    lang("sk", "Slovak", Script::Latin, true),
    lang("sl", "Slovenian", Script::Latin, true),
    lang("hr", "Croatian", Script::Latin, true),
    lang("ro", "Romanian", Script::Latin, true),
    lang("hu", "Hungarian", Script::Latin, true),
    lang("tr", "Turkish", Script::Latin, true),
    lang("sv", "Swedish", Script::Latin, true),
    lang("da", "Danish", Script::Latin, true),
    lang("nb", "Norwegian", Script::Latin, true),
    lang("fi", "Finnish", Script::Latin, true),
    lang("et", "Estonian", Script::Latin, true),
    lang("lv", "Latvian", Script::Latin, true),
    lang("lt", "Lithuanian", Script::Latin, true),
    lang("id", "Indonesian", Script::Latin, true),
    lang("vi", "Vietnamese", Script::Latin, true),
    lang("ru", "Russian", Script::Cyrillic, true),
    lang("uk", "Ukrainian", Script::Cyrillic, true),
    lang("bg", "Bulgarian", Script::Cyrillic, true),
    lang("sr", "Serbian", Script::Cyrillic, true),
    lang("el", "Greek", Script::Greek, true),
    lang("zh", "Chinese", Script::Han, true),
    lang("ja", "Japanese", Script::Japanese, true),
    lang("ko", "Korean", Script::Hangul, true),
    lang("be", "Belarusian", Script::Cyrillic, false),
    lang("mk", "Macedonian", Script::Cyrillic, false),
    lang("ca", "Catalan", Script::Latin, false),
    lang("af", "Afrikaans", Script::Latin, false),
    lang("az", "Azerbaijani", Script::Latin, false),
    lang("uz", "Uzbek", Script::Latin, false),
    lang("tl", "Tagalog", Script::Latin, false),
    lang("cy", "Welsh", Script::Latin, false),
    lang("ar", "Arabic", Script::Other, false),
    lang("fa", "Persian", Script::Other, false),
    lang("ur", "Urdu", Script::Other, false),
    lang("he", "Hebrew", Script::Other, false),
    lang("hi", "Hindi", Script::Other, false),
    lang("bn", "Bengali", Script::Other, false),
    lang("th", "Thai", Script::Other, false),
    lang("ka", "Georgian", Script::Other, false),
    lang("hy", "Armenian", Script::Other, false),
];

pub fn find(code: &str) -> Option<&'static Language> {
    let code = code.trim();
    LANGUAGES.iter().find(|language| language.code.eq_ignore_ascii_case(code))
}

pub fn targets() -> impl Iterator<Item = &'static Language> {
    LANGUAGES.iter().filter(|language| language.target)
}

pub fn name(code: &str) -> &str {
    find(code).map_or(code, |language| language.name)
}

/// `cg_chatTranslateTarget ""` follows the Windows display language, and
/// falls back to English when that is not a supported target.
pub fn resolve_target(setting: &str) -> &'static str {
    if let Some(language) = find(setting).filter(|language| language.target) {
        return language.code;
    }
    system_language()
        .and_then(|code| find(&code))
        .filter(|language| language.target)
        .map_or("en", |language| language.code)
}

#[cfg(windows)]
pub fn system_language() -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, len: i32) -> i32;
    }
    let mut buffer = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if len <= 1 {
        return None;
    }
    let locale = String::from_utf16_lossy(&buffer[..len as usize - 1]);
    // "en-US", "pt-BR", "zh-Hans-CN": the primary subtag is the language.
    let primary = locale.split('-').next()?.to_ascii_lowercase();
    // Windows reports Norwegian Bokmål as "nb"; older systems used "no".
    Some(if primary == "no" { "nb".to_owned() } else { primary })
}

#[cfg(not(windows))]
pub fn system_language() -> Option<String> {
    let value = std::env::var("LANG").ok()?;
    Some(value.split(['_', '.', '-']).next()?.to_ascii_lowercase())
}

/// whatlang reports ISO 639-3; only languages in [`LANGUAGES`] are used.
/// Latin and Esperanto are left out: whatlang picks them for short slang.
fn from_whatlang(lang: whatlang::Lang) -> Option<&'static str> {
    Some(match lang.code() {
        "eng" => "en", "spa" => "es", "por" => "pt", "fra" => "fr", "deu" => "de",
        "ita" => "it", "nld" => "nl", "pol" => "pl", "ces" => "cs", "slk" => "sk",
        "slv" => "sl", "hrv" => "hr", "ron" => "ro", "hun" => "hu", "tur" => "tr",
        "swe" => "sv", "dan" => "da", "nob" => "nb", "fin" => "fi", "est" => "et",
        "lav" => "lv", "lit" => "lt", "ind" => "id", "vie" => "vi", "rus" => "ru",
        "ukr" => "uk", "bul" => "bg", "srp" => "sr", "bel" => "be", "mkd" => "mk",
        "ell" => "el", "cmn" => "zh", "jpn" => "ja", "kor" => "ko", "cat" => "ca",
        "afr" => "af", "aze" => "az", "uzb" => "uz", "tgl" => "tl", "cym" => "cy",
        "ara" => "ar", "pes" => "fa", "urd" => "ur", "heb" => "he", "hin" => "hi",
        "ben" => "bn", "tha" => "th", "kat" => "ka", "hye" => "hy",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detection {
    pub script: Script,
    /// Best guess at the language, when the detector names a supported one.
    pub code: Option<&'static str>,
    pub confidence: f64,
}

pub fn detect(text: &str) -> Option<Detection> {
    use whatlang::Script as W;
    let script = whatlang::detect_script(text)?;
    let info = whatlang::detect(text);
    let guess = info.as_ref().and_then(|info| from_whatlang(info.lang()));
    let confidence = info.as_ref().map_or(0.0, |info| info.confidence());
    // A script used by one language identifies it regardless of whatlang's
    // trigram confidence, which is low for the short lines typical of chat.
    let (script, sure) = match script {
        W::Latin => (Script::Latin, None),
        W::Cyrillic => (Script::Cyrillic, None),
        W::Greek => (Script::Greek, Some("el")),
        W::Hangul => (Script::Hangul, Some("ko")),
        W::Hiragana | W::Katakana => (Script::Japanese, Some("ja")),
        W::Mandarin => (Script::Han, if guess == Some("ja") { Some("ja") } else { Some("zh") }),
        W::Hebrew => (Script::Other, Some("he")),
        W::Thai => (Script::Other, Some("th")),
        W::Georgian => (Script::Other, Some("ka")),
        W::Armenian => (Script::Other, Some("hy")),
        W::Bengali => (Script::Other, Some("bn")),
        _ => (Script::Other, None),
    };
    Some(match sure {
        Some(code) => Detection { script, code: Some(code), confidence: 1.0 },
        None => Detection { script, code: guess, confidence },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_identify_obvious_languages() {
        assert_eq!(detect("안녕하세요 여러분").unwrap().code, Some("ko"));
        assert_eq!(detect("こんにちは、みんな").unwrap().code, Some("ja"));
        let russian = detect("Привет всем, как дела у вас сегодня?").unwrap();
        assert_eq!(russian.script, Script::Cyrillic);
        assert_eq!(russian.code, Some("ru"));
    }

    #[test]
    fn unknown_or_source_only_targets_fall_back() {
        assert_eq!(resolve_target("ru"), "ru");
        assert_ne!(resolve_target("ar"), "ar");
        assert!(find(resolve_target("")).is_some_and(|language| language.target));
    }
}
