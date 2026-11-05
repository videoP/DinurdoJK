//! Translation services. Each takes plain message text (no names, colours,
//! server or player details) and returns the translation plus the language
//! the service detected, if it reports one.

use super::http;
use serde_json::{json, Value};

const TIMEOUT_MS: i32 = 8_000;
const MAX_RESPONSE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProviderKind {
    /// Free and keyless, with a daily character quota per IP address.
    #[default]
    MyMemory,
    DeepL,
    /// Self-hosted or public LibreTranslate at `cg_chatTranslateUrl`.
    Libre,
}

impl ProviderKind {
    pub const ALL: [Self; 3] = [Self::MyMemory, Self::DeepL, Self::Libre];

    pub fn name(self) -> &'static str {
        match self {
            Self::MyMemory => "mymemory",
            Self::DeepL => "deepl",
            Self::Libre => "libretranslate",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MyMemory => "MyMemory",
            Self::DeepL => "DeepL",
            Self::Libre => "LibreTranslate",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mymemory" | "0" => Some(Self::MyMemory),
            "deepl" | "1" => Some(Self::DeepL),
            "libretranslate" | "libre" | "2" => Some(Self::Libre),
            _ => None,
        }
    }

    /// Windows Credential Manager entry holding this service's API key.
    pub fn secret_name(self) -> Option<&'static str> {
        match self {
            Self::MyMemory => None,
            Self::DeepL => Some("chat-translate/deepl"),
            Self::Libre => Some("chat-translate/libretranslate"),
        }
    }

    /// Lowest spacing between requests, shared by all workers.
    pub fn min_interval_ms(self) -> u64 {
        match self {
            Self::MyMemory => 400,
            Self::DeepL | Self::Libre => 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub libre_url: String,
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Request {
    /// `None` asks the service to detect the language itself.
    pub source: Option<&'static str>,
    pub target: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translated {
    pub text: String,
    pub detected: Option<String>,
}

pub fn translate(config: &ProviderConfig, request: &Request) -> Result<Translated, String> {
    let result = match config.kind {
        ProviderKind::MyMemory => mymemory(request),
        ProviderKind::DeepL => deepl(config, request),
        ProviderKind::Libre => libre(config, request),
    }?;
    let text = super::text::sanitize_result(&result.text);
    if text.is_empty() {
        return Err(format!("{} returned an empty translation", config.kind.label()));
    }
    Ok(Translated { text, detected: result.detected.map(|code| code.to_ascii_lowercase()) })
}

fn parse_json(response: &http::Response, service: &str) -> Result<Value, String> {
    serde_json::from_slice(&response.body)
        .map_err(|_| format!("{service} answered HTTP {} with an unreadable body", response.status))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn mymemory_code(code: &str) -> &str {
    match code {
        "zh" => "zh-CN",
        "nb" => "no",
        other => other,
    }
}

fn mymemory(request: &Request) -> Result<Translated, String> {
    // MyMemory rejects queries over 500 bytes.
    let mut text = request.text.as_str();
    while text.len() > 500 {
        let mut cut = 500;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text = &text[..cut];
    }
    let url = format!(
        "https://api.mymemory.translated.net/get?q={}&langpair={}%7C{}",
        percent_encode(text),
        request.source.map_or("autodetect", mymemory_code),
        mymemory_code(request.target),
    );
    let response = http::request("GET", &url, &[("Accept", "application/json")], &[], TIMEOUT_MS, MAX_RESPONSE)?;
    let value = parse_json(&response, "MyMemory")?;
    // responseStatus is a number on success and sometimes a string on errors.
    let status = match &value["responseStatus"] {
        Value::Number(number) => number.as_u64().unwrap_or(0),
        Value::String(text) => text.parse().unwrap_or(0),
        _ => u64::from(response.status),
    };
    if value["quotaFinished"].as_bool() == Some(true) || status == 429 {
        return Err("MyMemory's free daily quota is used up".to_owned());
    }
    if status != 200 {
        let details = value["responseDetails"].as_str().unwrap_or("").trim();
        return Err(if details.is_empty() {
            format!("MyMemory error {status}")
        } else {
            format!("MyMemory: {}", details.chars().take(120).collect::<String>())
        });
    }
    let data = &value["responseData"];
    let translated = data["translatedText"].as_str().ok_or("MyMemory sent no translation")?;
    Ok(Translated {
        text: translated.to_owned(),
        detected: data["detectedLanguage"].as_str().map(|code| code.split('-').next().unwrap_or(code).to_owned()),
    })
}

/// DeepL source languages; others are left for DeepL to detect.
const DEEPL_SOURCES: &[&str] = &[
    "ar", "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "he", "hu", "id", "it", "ja", "ko", "lt",
    "lv", "nb", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "th", "tr", "uk", "vi", "zh",
];

fn deepl_target(code: &str) -> String {
    match code {
        "en" => "EN-US".to_owned(),
        "pt" => "PT-BR".to_owned(),
        "zh" => "ZH-HANS".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

fn deepl(config: &ProviderConfig, request: &Request) -> Result<Translated, String> {
    let key = config.key.as_deref().filter(|key| !key.is_empty()).ok_or("DeepL needs an API key (Setup > Interface > Translation)")?;
    // Free-plan keys end in ":fx" and use their own host.
    let url = if key.ends_with(":fx") {
        "https://api-free.deepl.com/v2/translate"
    } else {
        "https://api.deepl.com/v2/translate"
    };
    let mut body = json!({ "text": [request.text], "target_lang": deepl_target(request.target) });
    if let Some(source) = request.source.filter(|source| DEEPL_SOURCES.contains(source)) {
        body["source_lang"] = json!(source.to_ascii_uppercase());
    }
    let auth = format!("DeepL-Auth-Key {key}");
    let response = http::request(
        "POST",
        url,
        &[("Authorization", auth.as_str()), ("Content-Type", "application/json")],
        body.to_string().as_bytes(),
        TIMEOUT_MS,
        MAX_RESPONSE,
    )?;
    match response.status {
        200 => {}
        403 => return Err("DeepL rejected the API key".to_owned()),
        429 => return Err("DeepL: too many requests".to_owned()),
        456 => return Err("DeepL quota exceeded".to_owned()),
        status => return Err(format!("DeepL error {status}")),
    }
    let value = parse_json(&response, "DeepL")?;
    let first = &value["translations"][0];
    let text = first["text"].as_str().ok_or("DeepL sent no translation")?;
    Ok(Translated { text: text.to_owned(), detected: first["detected_source_language"].as_str().map(str::to_owned) })
}

fn libre(config: &ProviderConfig, request: &Request) -> Result<Translated, String> {
    let base = config.libre_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("set the LibreTranslate address (cg_chatTranslateUrl)".to_owned());
    }
    let mut body = json!({
        "q": request.text,
        "source": request.source.unwrap_or("auto"),
        "target": request.target,
        "format": "text",
    });
    if let Some(key) = config.key.as_deref().filter(|key| !key.is_empty()) {
        body["api_key"] = json!(key);
    }
    let response = http::request(
        "POST",
        &format!("{base}/translate"),
        &[("Content-Type", "application/json"), ("Accept", "application/json")],
        body.to_string().as_bytes(),
        TIMEOUT_MS,
        MAX_RESPONSE,
    )?;
    let value = parse_json(&response, "LibreTranslate")?;
    if response.status != 200 {
        let error = value["error"].as_str().unwrap_or("").chars().take(120).collect::<String>();
        return Err(format!("LibreTranslate error {}{}{error}", response.status, if error.is_empty() { "" } else { ": " }));
    }
    let text = value["translatedText"].as_str().ok_or("LibreTranslate sent no translation")?;
    Ok(Translated { text: text.to_owned(), detected: value["detectedLanguage"]["language"].as_str().map(str::to_owned) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_round_trip() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(ProviderKind::from_name("LIBRE"), Some(ProviderKind::Libre));
    }

    #[test]
    fn query_text_is_percent_encoded() {
        assert_eq!(percent_encode("a b|ж"), "a%20b%7C%D0%B6");
    }
}
