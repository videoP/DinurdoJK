//! StringEd (`strings/<language>/*.str`) lookup and the `@@@REFERENCE`
//! substitution servers use in print/centre-print/disconnect text.
//! References: OpenJK codemp/qcommon/stringed_ingame.cpp (SE_GetString) and
//! cgame/cg_servercmds.c CG_CheckSVStringEdRef.

use std::collections::HashMap;

use jka_assets::pk3::AssetSearchPath;

const MAX_STRING_FILE_BYTES: usize = 1024 * 1024;
/// MAX_STRINGED_SV_STRING.
const MAX_SV_STRING: usize = 1024;

#[derive(Debug, Default, Clone)]
pub struct StringEd {
    /// "FILE_REFERENCE" (upper case) -> text.
    strings: HashMap<String, Vec<u8>>,
}

impl StringEd {
    /// Loads every `strings/english/*.str` file (the stock SE language).
    pub fn load(assets: &mut AssetSearchPath) -> Self {
        let mut table = Self::default();
        let files: Vec<String> = assets
            .names()
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                lower.starts_with("strings/english/") && lower.ends_with(".str")
            })
            .map(str::to_owned)
            .collect();
        for name in files {
            let Ok(Some(asset)) = assets.read(&name, MAX_STRING_FILE_BYTES) else { continue };
            let file = name
                .rsplit('/')
                .next()
                .and_then(|base| base.rsplit_once('.').map(|(stem, _)| stem))
                .unwrap_or_default()
                .to_ascii_uppercase();
            table.parse_file(&file, &asset.bytes);
        }
        table
    }

    /// The subset of the .str format SE_GetString needs: REFERENCE names
    /// followed by their LANG_ENGLISH text.
    fn parse_file(&mut self, file: &str, bytes: &[u8]) {
        let mut reference: Option<String> = None;
        for line in bytes.split(|&b| b == b'\n') {
            let line = trim(line);
            if let Some(rest) = line.strip_prefix(b"REFERENCE") {
                reference = Some(String::from_utf8_lossy(trim(rest)).to_ascii_uppercase());
            } else if let Some(rest) = line.strip_prefix(b"LANG_ENGLISH") {
                if let Some(reference) = reference.take() {
                    self.strings.insert(format!("{file}_{reference}"), unquote(trim(rest)));
                }
            }
        }
    }

    /// SE_GetString("FILE_REFERENCE"); unknown references come back empty
    /// like the stock lookup's "" result.
    pub fn get(&self, key: &str) -> &[u8] {
        self.strings.get(&key.to_ascii_uppercase()).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// CG_CheckSVStringEdRef: replace each `@@@REF` (ended by space, ':',
    /// '.' or newline) with SE_GetString("MP_SVGAME", REF).
    pub fn translate_server_text(&self, text: &[u8]) -> Vec<u8> {
        if text.is_empty() || text.len() >= MAX_SV_STRING {
            return text.to_vec();
        }
        let mut out = Vec::with_capacity(text.len() + 32);
        let mut i = 0;
        while i < text.len() {
            if text[i..].starts_with(b"@@@") && i + 3 < text.len() {
                while i < text.len() && text[i] == b'@' {
                    i += 1;
                }
                let start = i;
                while i < text.len() && !matches!(text[i], b' ' | b':' | b'.' | b'\n') {
                    i += 1;
                }
                let reference = String::from_utf8_lossy(&text[start..i]);
                out.extend_from_slice(self.get(&format!("MP_SVGAME_{reference}")));
            }
            if i < text.len() {
                out.push(text[i]);
            }
            i += 1;
        }
        out
    }
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |p| p + 1);
    &bytes[start..end]
}

fn unquote(bytes: &[u8]) -> Vec<u8> {
    let inner = bytes.strip_prefix(b"\"").unwrap_or(bytes);
    let inner = inner.strip_suffix(b"\"").unwrap_or(inner);
    // StringEd escapes newlines as \n inside the quoted text.
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' && inner.get(i + 1) == Some(&b'n') {
            out.push(b'\n');
            i += 2;
        } else {
            out.push(inner[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> StringEd {
        let mut table = StringEd::default();
        table.parse_file(
            "MP_SVGAME",
            b"VERSION \"1\"\nREFERENCE           PLENTER\nNOTES \"x\"\nLANG_ENGLISH        \"entered the game\"\n\
REFERENCE JOINEDTHEBATTLE\r\nLANG_ENGLISH \"joined the battle.\"\r\n",
        );
        table
    }

    #[test]
    fn substitutes_server_references_like_cg_check_sv_stringed_ref() {
        let table = table();
        assert_eq!(table.translate_server_text(b"Padawan^7 @@@PLENTER\n"), b"Padawan^7 entered the game\n");
        assert_eq!(
            table.translate_server_text(b"x @@@JOINEDTHEBATTLE\n"),
            b"x joined the battle.\n"
        );
        // Unknown references disappear (SE_GetString returns "").
        assert_eq!(table.translate_server_text(b"a @@@NOPE b"), b"a  b");
        assert_eq!(table.translate_server_text(b"no refs"), b"no refs");
    }
}
