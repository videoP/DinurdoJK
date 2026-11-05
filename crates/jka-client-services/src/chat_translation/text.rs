//! Chat text for translation: legacy decoding of the wire body, the
//! normalized provider input, skip rules, and the HUD-safe projection of a
//! Unicode result. The stored wire text is never modified.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codepage {
    /// Valid UTF-8 as UTF-8; otherwise Windows-1251 when high bytes make up
    /// whole words (Cyrillic), else Windows-1252.
    #[default]
    Auto,
    Utf8,
    Windows1252,
    Windows1251,
}

impl Codepage {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Utf8, Self::Windows1252, Self::Windows1251];

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Utf8 => "utf8",
            Self::Windows1252 => "1252",
            Self::Windows1251 => "1251",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "Western (Windows-1252)",
            Self::Windows1251 => "Cyrillic (Windows-1251)",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|codepage| codepage.name().eq_ignore_ascii_case(value.trim()))
    }
}

/// JKA `Q_IsColorString`: `^` followed by a digit.
pub fn strip_color_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'^' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            i += 2;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn looks_cyrillic_1251(bytes: &[u8]) -> bool {
    // In CP1251 Cyrillic letters are 0xC0..=0xFF (plus Ё/ё and Ukrainian
    // letters below that); a Russian word is almost entirely high bytes. In
    // CP1252 those bytes are accented Latin letters inside mostly-ASCII words.
    let (mut high, mut ascii) = (0usize, 0usize);
    for word in bytes.split(|byte| byte.is_ascii_whitespace() || byte.is_ascii_punctuation()) {
        if !word.iter().any(|&byte| byte >= 0xC0) {
            continue;
        }
        high += word.iter().filter(|&&byte| byte >= 0x80).count();
        ascii += word.iter().filter(|byte| byte.is_ascii_alphabetic()).count();
    }
    high >= 2 && high >= ascii * 2
}

pub fn decode(bytes: &[u8], codepage: Codepage) -> String {
    let encoding = match codepage {
        Codepage::Utf8 => return String::from_utf8_lossy(bytes).into_owned(),
        Codepage::Windows1252 => encoding_rs::WINDOWS_1252,
        Codepage::Windows1251 => encoding_rs::WINDOWS_1251,
        Codepage::Auto => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                return text.to_owned();
            }
            if looks_cyrillic_1251(bytes) {
                encoding_rs::WINDOWS_1251
            } else {
                encoding_rs::WINDOWS_1252
            }
        }
    };
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// Provider input: colour codes removed, controls turned into spaces,
/// whitespace collapsed, and capped at `max_chars` characters.
pub fn provider_text(body: &[u8], codepage: Codepage, max_chars: usize) -> String {
    let decoded = decode(&strip_color_bytes(body), codepage);
    let mut out = String::with_capacity(decoded.len());
    for word in decoded.split(|ch: char| ch.is_whitespace() || ch.is_control()) {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    match out.char_indices().nth(max_chars) {
        Some((cut, _)) => out[..cut].trim_end().to_owned(),
        None => out,
    }
}

/// Chat shorthand that every language uses; never worth a request.
const SHORTHAND: &[&str] = &[
    "gg", "ggs", "wp", "gl", "hf", "glhf", "lol", "lmao", "lmfao", "rofl", "xd", "xdd", "ty", "thx", "tnx",
    "np", "brb", "afk", "ok", "okay", "k", "kk", "omg", "wtf", "gj", "nj", "ez", "rip", "noob", "lag", "lagg",
    "haha", "hahaha", "hehe", "hah", "ha", "ah", "oh", "ops", "oops", "yes", "no", "yeah", "nah", "yep",
    "nope", "hi", "hey", "yo", "sup", "bye", "cya", "pls", "plz", "sry", "sorry", "nice", "n1", "gz", "gratz",
    "re", "ff", "bb", "u", "ur", "r",
];

/// Why an automatic request is not worth making; `None` means translate.
pub fn auto_skip_reason(text: &str) -> Option<&'static str> {
    let letters = text.chars().filter(|ch| ch.is_alphabetic()).count();
    if letters < 3 {
        return Some("too short");
    }
    let mut words = text
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .peekable();
    if words.peek().is_none() {
        return Some("no words");
    }
    let mut all_shorthand = true;
    let mut all_links = true;
    for word in words {
        let lower = word.to_lowercase();
        all_links &= lower.starts_with("http") || lower.starts_with("www.") || lower.contains(".com");
        all_shorthand &= SHORTHAND.contains(&lower.as_str()) || lower.chars().all(|ch| !ch.is_alphabetic());
    }
    if all_links {
        return Some("link");
    }
    all_shorthand.then_some("chat shorthand")
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = entity.strip_prefix('#')?;
            let value = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(value)
        }
    }
}

/// Provider output made safe for JKA text: HTML entities decoded, controls
/// and `^` removed (it would start a colour code), whitespace collapsed.
pub fn sanitize_result(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        decoded.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let entity = rest[1..].find(';').filter(|&end| end <= 10).and_then(|end| {
            decode_entity(&rest[1..=end]).map(|ch| (ch, end + 2))
        });
        match entity {
            Some((ch, len)) => {
                decoded.push(ch);
                rest = &rest[len..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    let mut out = String::with_capacity(decoded.len());
    for word in decoded.split(|ch: char| ch.is_whitespace() || ch.is_control() || ch == '^') {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Latin Extended-A (U+0100..=U+017F) folded to its base letter.
const LATIN_EXTENDED_A: &[u8; 128] = b"AaAaAaCcCcCcCcDdDdEeEeEeEeEeGgGgGgGgHhHhIiIiIiIiIiIiJjKkkLlLlLlLlLlNnNnNnnNnOoOoOoOoRrRrRrSsSsSsSsTtTtTtUuUuUuUuUuUuWwYyYZzZzZzs";

/// HUD fonts are byte indexed with Latin-1 glyphs. Returns the translation in
/// the lossless one-char-per-byte form, folding common punctuation and
/// accented letters, or `None` if it needs glyphs the HUD does not have.
pub fn hud_text(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        match code {
            0x20..=0x7E | 0xA0..=0xFF => out.push(ch),
            0x100..=0x17F => out.push(char::from(LATIN_EXTENDED_A[(code - 0x100) as usize])),
            0x218 | 0x219 => out.push(if code == 0x218 { 'S' } else { 's' }),
            0x21A | 0x21B => out.push(if code == 0x21A { 'T' } else { 't' }),
            0x2018..=0x201B | 0x2032 => out.push('\''),
            0x201C..=0x201F | 0x2033 => out.push('"'),
            0x2010..=0x2015 | 0x2212 => out.push('-'),
            0x2026 => out.push_str("..."),
            0x2022 | 0x00B7 => out.push('*'),
            0x20AC => out.push_str("EUR"),
            0x2122 => out.push_str("TM"),
            0x200B..=0x200F | 0xFEFF => {}
            // Vietnamese and other combining marks: keep the base letter.
            0x300..=0x36F => {}
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp1251_russian_is_detected_but_western_accents_are_not() {
        let (russian, _, _) = encoding_rs::WINDOWS_1251.encode("Привет всем, как дела");
        assert_eq!(decode(&russian, Codepage::Auto), "Привет всем, как дела");
        let (french, _, _) = encoding_rs::WINDOWS_1252.encode("très bien, à bientôt");
        assert_eq!(decode(&french, Codepage::Auto), "très bien, à bientôt");
        assert_eq!(decode("Привет".as_bytes(), Codepage::Auto), "Привет");
    }

    #[test]
    fn provider_text_strips_colours_and_caps_length() {
        assert_eq!(provider_text(b"^1hello   ^7world\x01!", Codepage::Auto, 300), "hello world !");
        assert_eq!(provider_text(b"abcdef", Codepage::Auto, 3), "abc");
    }

    #[test]
    fn shorthand_and_links_are_skipped() {
        assert!(auto_skip_reason("gg").is_some());
        assert!(auto_skip_reason("GG WP lol").is_some());
        assert!(auto_skip_reason("https://example.com").is_some());
        assert!(auto_skip_reason("123 456").is_some());
        assert!(auto_skip_reason("Привет всем").is_none());
        assert!(auto_skip_reason("donde esta la bandera").is_none());
    }

    #[test]
    fn results_are_sanitized_and_folded_for_the_hud() {
        assert_eq!(sanitize_result("it&#39;s ^1red &amp; fine\n"), "it's 1red & fine");
        assert_eq!(hud_text("Zażółć “quote” – ok…").unwrap(), "Zazólc \"quote\" - ok...");
        assert_eq!(hud_text("très").unwrap(), "très");
        assert!(hud_text("Привет").is_none());
    }
}
