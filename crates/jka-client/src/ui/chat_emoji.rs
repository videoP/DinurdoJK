//! TaystJK-compatible named chat emotes. The PK3 keeps individual PNGs; the
//! renderer supplies one packed texture and these UVs after its async upload.
//! Unlike Unicode font fallback, names are case-sensitive text replacements.
use std::collections::HashMap;

pub(in crate::ui) const CHAT_EMOJI_SOURCE: f32 = 14.0;

#[derive(Debug, Clone)]
pub(crate) struct ChatEmojiCell {
    pub(crate) trigger: String,
    pub(crate) uv: ([f32; 2], [f32; 2]),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ChatEmojiAtlas {
    by_first_byte: HashMap<u8, Vec<ChatEmojiCell>>,
    pub(crate) count: usize,
}

pub(in crate::ui) enum ChatPart<'a> {
    Text(&'a str),
    Image(&'a ChatEmojiCell),
}

impl ChatEmojiAtlas {
    pub(crate) fn from_cells(cells: impl IntoIterator<Item = ChatEmojiCell>) -> Self {
        let mut by_first_byte: HashMap<u8, Vec<ChatEmojiCell>> = HashMap::new();
        for cell in cells {
            if let Some(&first) = cell.trigger.as_bytes().first() {
                let bucket = by_first_byte.entry(first).or_default();
                if !bucket.iter().any(|existing| existing.trigger == cell.trigger) {
                    bucket.push(cell);
                }
            }
        }
        let mut count = 0;
        for bucket in by_first_byte.values_mut() {
            // Prefer the longest exact match (TaystJK's names may overlap).
            bucket.sort_by(|a, b| b.trigger.len().cmp(&a.trigger.len()).then(a.trigger.cmp(&b.trigger)));
            count += bucket.len();
        }
        Self { by_first_byte, count }
    }

    /// Same cells and atlas positions as the renderer, for the chat picker.
    pub(crate) fn sorted_cells(&self) -> Vec<ChatEmojiCell> {
        let mut cells: Vec<_> = self.by_first_byte.values().flatten().cloned().collect();
        cells.sort_by(|a, b| a.trigger.to_lowercase().cmp(&b.trigger.to_lowercase())
            .then(a.trigger.cmp(&b.trigger)));
        cells
    }

    /// Split at matched names only. Unknown spellings and native emoji stay
    /// verbatim, including their original colors and transmitted text.
    pub(in crate::ui) fn for_each_part<'a>(&'a self, value: &'a str, mut emit: impl FnMut(ChatPart<'a>)) {
        let mut plain_start = 0;
        let mut index = 0;
        while index < value.len() {
            if let Some((skip, _)) = super::text::color_escape(value.as_bytes(), index, 1.0) {
                index += skip;
                continue;
            }
            let found = self.by_first_byte.get(&value.as_bytes()[index]).and_then(|bucket| {
                bucket.iter().find(|cell| value[index..].starts_with(&cell.trigger))
            });
            if let Some(cell) = found {
                if index > plain_start { emit(ChatPart::Text(&value[plain_start..index])); }
                emit(ChatPart::Image(cell));
                index += cell.trigger.len();
                plain_start = index;
            } else {
                index += value[index..].chars().next().unwrap().len_utf8();
            }
        }
        if plain_start < value.len() { emit(ChatPart::Text(&value[plain_start..])); }
    }
}

/// TaystJK CG_LoadEmojis (cg_main.c): backtick encodes ':', tilde encodes
/// '>', and '!' capitalizes its following character. Do NOT add implicit
/// colons: TaystJK also ships triggers such as #:) and #>:D.
pub(crate) fn tayst_emoji_trigger(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".png")?;
    if stem.is_empty() || stem.len() > 80 || stem.contains(&['/', '\\'][..]) { return None; }
    let mut out = String::with_capacity(stem.len());
    let mut upper_next = false;
    for ch in stem.chars() {
        if ch == '!' {
            upper_next = true;
            continue;
        }
        let replacement = match ch { '`' => ':', '~' => '>', _ => ch };
        if upper_next {
            out.extend(replacement.to_uppercase());
            upper_next = false;
        } else {
            out.push(replacement);
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tayst_filename_escape_rules() {
        assert_eq!(tayst_emoji_trigger("`kappa`.png").as_deref(), Some(":kappa:"));
        assert_eq!(tayst_emoji_trigger("#`).png").as_deref(), Some("#:)"));
        assert_eq!(tayst_emoji_trigger("#~`!d.png").as_deref(), Some("#>:D"));
        assert_eq!(tayst_emoji_trigger("`!pog!champ`.png").as_deref(), Some(":PogChamp:"));
    }
    #[test]
    fn names_match_case_and_leave_unknown_codes_alone() {
        let atlas = ChatEmojiAtlas::from_cells([ChatEmojiCell {
            trigger: ":PogChamp:".into(), uv: ([0.0, 0.0], [1.0, 1.0]),
        }]);
        let mut parts = Vec::new();
        atlas.for_each_part("^2:PogChamp: :pogchamp: 😀", |part| parts.push(part));
        assert_eq!(parts.len(), 3);
        assert!(matches!(parts[1], ChatPart::Image(_)));
        assert!(matches!(parts[2], ChatPart::Text(" :pogchamp: 😀")));
    }
}
