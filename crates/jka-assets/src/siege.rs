//! Jedi Academy Siege class model/skin metadata ported from OpenJK `codemp/game/bg_saga.c`.
//!
//! This intentionally implements only the fields consumed by `CG_NewClientInfo` for player
//! presentation: class `name`, forced `model`, and forced `skin`. Parsing follows the same
//! `BG_SiegeGetValueGroup` / `BG_SiegeGetPairedValue` grammar rather than substituting JSON or a
//! generic key/value parser.

use crate::pk3::AssetSearchPath;

const CLASS_DIRECTORY: &str = "ext_data/siege/classes/";
const CLASS_EXTENSION: &str = ".scl";
const MAX_CLASS_FILE: usize = 4095; // BG_SiegeParseClassFile rejects len >= 4096.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiegeClassVisual {
    pub name: String,
    pub forced_model: String,
    pub forced_skin: String,
}

pub fn load_siege_class_visuals(
    assets: &mut AssetSearchPath,
) -> Result<Vec<SiegeClassVisual>, String> {
    // FS_GetFileList returns files directly in ext_data/Siege/Classes. AssetSearchPath names are
    // normalized to lowercase, which is equivalent for JKA's case-insensitive qpath lookup.
    let names = assets
        .names()
        .filter(|name| {
            name.starts_with(CLASS_DIRECTORY)
                && name.ends_with(CLASS_EXTENSION)
                && !name[CLASS_DIRECTORY.len()..].contains('/')
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let mut classes = Vec::new();
    for name in names {
        let Some(asset) = assets
            .read(&name, MAX_CLASS_FILE)
            .map_err(|error| format!("{name}: {error}"))?
        else {
            continue;
        };
        if let Some(class) = parse_siege_class_visual(&asset.bytes)? {
            classes.push(class);
        }
    }
    Ok(classes)
}

pub fn parse_siege_class_visual(bytes: &[u8]) -> Result<Option<SiegeClassVisual>, String> {
    if bytes.len() >= 4096 {
        // OpenJK simply returns without adding a class in this case.
        return Ok(None);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "Siege class file is not valid UTF-8/ASCII".to_owned())?;
    let class_info = siege_value_group(text, "ClassInfo")?
        .ok_or_else(|| "Siege class missing ClassInfo group".to_owned())?;
    let name = siege_paired_value(&class_info, "name")?
        .ok_or_else(|| "Siege class without name entry".to_owned())?;
    let forced_model = siege_paired_value(&class_info, "model")?.unwrap_or_default();
    let forced_skin = siege_paired_value(&class_info, "skin")?.unwrap_or_default();
    Ok(Some(SiegeClassVisual {
        name,
        forced_model,
        forced_skin,
    }))
}

pub fn find_siege_class_visual<'a>(
    classes: &'a [SiegeClassVisual],
    name: &str,
) -> Option<&'a SiegeClassVisual> {
    classes
        .iter()
        .find(|class| class.name.eq_ignore_ascii_case(name))
}

/// Safe Rust equivalent of `BG_SiegeGetValueGroup` for the syntax used by stock class files.
fn siege_value_group(text: &str, group: &str) -> Result<Option<String>, String> {
    let data = text.as_bytes();
    let mut i = 0usize;
    while i < data.len() {
        skip_space_and_line_comments(data, &mut i);
        if i >= data.len() {
            break;
        }
        if data[i] == b'{' {
            skip_group(data, &mut i)?;
            continue;
        }
        if data[i] == b'}' {
            i += 1;
            continue;
        }

        let token_start = i;
        while i < data.len()
            && !matches!(data[i], b' ' | b'\n' | b'\r' | b'\t' | b'{')
        {
            if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'/' {
                break;
            }
            i += 1;
        }
        let token = &text[token_start..i];

        if i + 1 < data.len() && data[i] == b'/' && data[i + 1] == b'/' {
            skip_line_comment(data, &mut i);
        }
        while i < data.len() && matches!(data[i], b' ' | b'\n' | b'\r' | b'\t') {
            i += 1;
        }

        if i < data.len() && data[i] == b'{' {
            if token.eq_ignore_ascii_case(group) {
                return extract_group_contents(text, i).map(Some);
            }
            skip_group(data, &mut i)?;
        } else {
            while i < data.len() && !matches!(data[i], b'\n' | b'\r') {
                i += 1;
            }
        }
    }
    Ok(None)
}

/// Safe Rust equivalent of `BG_SiegeGetPairedValue`. It deliberately does not recurse into
/// subgroups, and quoted values may contain spaces exactly as in OpenJK.
fn siege_paired_value(text: &str, key: &str) -> Result<Option<String>, String> {
    let data = text.as_bytes();
    let mut i = 0usize;
    while i < data.len() {
        while i < data.len() && matches!(data[i], b' ' | b'{' | b'}' | b'\n' | b'\r' | b'\t') {
            if data[i] == b'{' {
                skip_group(data, &mut i)?;
            } else {
                i += 1;
            }
        }
        if i >= data.len() {
            break;
        }
        if i + 1 < data.len() && data[i] == b'/' && data[i + 1] == b'/' {
            skip_line_comment(data, &mut i);
            continue;
        }

        let key_start = i;
        while i < data.len() && !matches!(data[i], b' ' | b'\n' | b'\r' | b'\t') {
            if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'/' {
                break;
            }
            i += 1;
        }
        let candidate = &text[key_start..i];

        let mut look = i;
        while look < data.len() && matches!(data[look], b' ' | b'\n' | b'\r') {
            look += 1;
        }
        if look < data.len() && data[look] == b'{' {
            i = look;
            skip_group(data, &mut i)?;
            continue;
        }

        if i + 1 < data.len() && data[i] == b'/' && data[i + 1] == b'/' {
            return Err(format!("found comment, expected value for '{key}'"));
        }

        if candidate.eq_ignore_ascii_case(key) {
            while i < data.len() && matches!(data[i], b' ' | b'\n' | b'\r' | b'\t') {
                i += 1;
            }
            if i >= data.len() {
                return Err(format!("unexpected EOF while looking for value '{key}'"));
            }
            if data[i] == b'"' {
                i += 1;
                let start = i;
                while i < data.len() && data[i] != b'"' {
                    if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'/' {
                        break;
                    }
                    i += 1;
                }
                if i >= data.len() {
                    return Err(format!("unexpected EOF while looking for endquote for '{key}'"));
                }
                return Ok(Some(text[start..i].to_owned()));
            }
            let start = i;
            while i < data.len() && !matches!(data[i], b' ' | b'\n' | b'\r') {
                if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'/' {
                    break;
                }
                i += 1;
            }
            return Ok(Some(text[start..i].to_owned()));
        }

        // OpenJK skips the rest of the line after a non-matching key so a value cannot be
        // mistaken for another key.
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
    }
    Ok(None)
}

fn extract_group_contents(text: &str, opening: usize) -> Result<String, String> {
    let data = text.as_bytes();
    let mut depth = 0i32;
    let mut i = opening;
    let content_start = opening + 1;
    while i < data.len() {
        match data[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(text[content_start..i].replace('\t', " "));
                }
                if depth < 0 {
                    return Err("unexpected closing bracket in Siege group".to_owned());
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err("Siege group is missing a closing bracket".to_owned())
}

fn skip_group(data: &[u8], i: &mut usize) -> Result<(), String> {
    if *i >= data.len() || data[*i] != b'{' {
        return Ok(());
    }
    let mut depth = 0i32;
    while *i < data.len() {
        match data[*i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                *i += 1;
                if depth == 0 {
                    return Ok(());
                }
                if depth < 0 {
                    return Err("unexpected closing bracket in Siege file".to_owned());
                }
                continue;
            }
            _ => {}
        }
        *i += 1;
    }
    Err("opening bracket without matching closing bracket in Siege file".to_owned())
}

fn skip_space_and_line_comments(data: &[u8], i: &mut usize) {
    loop {
        while *i < data.len() && matches!(data[*i], b' ' | b'\n' | b'\r' | b'\t') {
            *i += 1;
        }
        if *i + 1 < data.len() && data[*i] == b'/' && data[*i + 1] == b'/' {
            skip_line_comment(data, i);
            continue;
        }
        break;
    }
}

fn skip_line_comment(data: &[u8], i: &mut usize) {
    while *i < data.len() && !matches!(data[*i], b'\n' | b'\r') {
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_visual_fields_from_classinfo_only() {
        let file = br#"
            description "A class outside the group"
            Other { model wrong }
            ClassInfo
            {
                name "Jedi Guardian"
                model jedi_hm
                skin |head_a1|torso_a1|lower_a1
                weapons WP_SABER
                Nested { model ignored }
            }
        "#;
        assert_eq!(
            parse_siege_class_visual(file).unwrap(),
            Some(SiegeClassVisual {
                name: "Jedi Guardian".to_owned(),
                forced_model: "jedi_hm".to_owned(),
                forced_skin: "|head_a1|torso_a1|lower_a1".to_owned(),
            })
        );
    }

    #[test]
    fn class_name_lookup_is_case_insensitive() {
        let classes = vec![SiegeClassVisual {
            name: "Heavy Weapons".to_owned(),
            forced_model: "stormtrooper".to_owned(),
            forced_skin: "default".to_owned(),
        }];
        assert_eq!(
            find_siege_class_visual(&classes, "heavy weapons")
                .unwrap()
                .forced_model,
            "stormtrooper"
        );
    }
}
