//! Jedi Academy `.skin` parsing ported from OpenJK `codemp/rd-vanilla/tr_skin.cpp`.
//!
//! The renderer's skin grammar is deliberately *not* the normal COM parser grammar. OpenJK
//! uses its local `CommaParse` routine, including comment skipping and comma-delimited words.
//! Keep this module aligned with `RE_SplitSkins`, `RE_RegisterIndividualSkin`, and `CommaParse`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinSurface {
    pub name: String,
    pub shader: String,
}

/// OpenJK `RE_SplitSkins`.
///
/// JKA can pass one macro skin name which expands to head/torso/lower skin files, e.g.
/// `models/players/jedi_tf/|head01_skin1|torso01|lower01`.
pub fn split_skin_macro(name: &str) -> Option<[String; 3]> {
    let first = name.find('|')?;
    let base = &name[..first];
    let remainder = &name[first + 1..];
    let second = remainder.find('|')?;
    let head = &remainder[..second];
    let remainder = &remainder[second + 1..];
    let third = remainder.find('|')?;
    let torso = &remainder[..third];
    let lower = &remainder[third + 1..];

    Some([
        format!("{base}{head}.skin"),
        format!("{base}{torso}.skin"),
        format!("{base}{lower}.skin"),
    ])
}

/// Parse one physical `.skin` file as `RE_RegisterIndividualSkin` does.
pub fn parse_skin(bytes: &[u8]) -> Result<Vec<SkinSurface>, String> {
    // FS_ReadFile provides a NUL-terminated C buffer. Treat an embedded NUL as end-of-file and
    // preserve every other byte 1:1 for CommaParse; skin qpaths are byte-oriented in the engine.
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    let data = &bytes[..end];
    let mut cursor = 0usize;
    let mut surfaces = Vec::new();

    loop {
        let token = comma_parse(data, &mut cursor);
        if token.is_empty() {
            break;
        }

        let mut surface_name = ascii_lowercase(&token);

        if cursor < data.len() && data[cursor] == b',' {
            cursor += 1;
        }

        // OpenJK checks the original token here rather than the lowercased copy.
        if token.starts_with(b"tag_") {
            continue;
        }

        let shader = comma_parse(data, &mut cursor);

        if surface_name.ends_with("_off") {
            if shader == b"*off" {
                // OpenJK intentionally does not add duplicate explicit *_off/*off entries.
                continue;
            }
            surface_name.truncate(surface_name.len() - 4);
        }

        surfaces.push(SkinSurface {
            name: surface_name,
            shader: bytes_to_qpath(&shader)?,
        });
    }

    Ok(surfaces)
}

/// Merge the physical skin files represented by a normal skin qpath or JKA's multipart macro.
///
/// This mirrors the order in `RE_RegisterSkin`: head first, torso if distinct, then lower if it
/// differs from both. The caller supplies the asset lookup so this module stays independent of
/// filesystem/PK3 ownership.
pub fn load_skin<F>(name: &str, mut read: F) -> Result<Vec<SkinSurface>, String>
where
    F: FnMut(&str) -> Result<Option<Vec<u8>>, String>,
{
    let names: Vec<String> = if let Some([head, torso, lower]) = split_skin_macro(name) {
        let mut names = vec![head.clone()];
        if torso != head {
            names.push(torso.clone());
        }
        if lower != head && lower != torso {
            names.push(lower);
        }
        names
    } else {
        vec![name.to_owned()]
    };

    let mut surfaces = Vec::new();
    for qpath in names {
        let bytes = read(&qpath)?.ok_or_else(|| format!("RE_RegisterSkin: {qpath} failed to load"))?;
        surfaces.extend(parse_skin(&bytes)?);
    }

    // RE_RegisterIndividualSkin returns 0 when the accumulated skin has no surfaces.
    if surfaces.is_empty() {
        return Err(format!("RE_RegisterSkin: {name} has no surfaces"));
    }
    Ok(surfaces)
}

fn comma_parse(data: &[u8], cursor: &mut usize) -> Vec<u8> {
    let mut i = *cursor;

    loop {
        while i < data.len() && data[i] <= b' ' {
            i += 1;
        }
        if i >= data.len() {
            *cursor = i;
            return Vec::new();
        }

        if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'/' {
            i += 2;
            while i < data.len() && data[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        if data[i] == b'/' && i + 1 < data.len() && data[i + 1] == b'*' {
            i += 2;
            while i + 1 < data.len() && !(data[i] == b'*' && data[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < data.len() {
                i += 2;
            } else {
                i = data.len();
            }
            continue;
        }
        break;
    }

    if data[i] == b'"' {
        i += 1;
        let start = i;
        while i < data.len() && data[i] != b'"' {
            i += 1;
        }
        let token = data[start..i].to_vec();
        if i < data.len() {
            i += 1;
        }
        *cursor = i;
        return token;
    }

    let start = i;
    while i < data.len() && data[i] > b' ' && data[i] != b',' {
        i += 1;
    }
    let token = data[start..i].to_vec();
    *cursor = i;
    token
}

fn ascii_lowercase(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn bytes_to_qpath(bytes: &[u8]) -> Result<String, String> {
    if bytes.iter().any(|&byte| byte == 0) {
        return Err("skin token contains NUL".to_owned());
    }
    // JKA asset names are ASCII in stock data. Preserve high bytes losslessly enough for a
    // diagnostic path rather than silently replacing them.
    if !bytes.is_ascii() {
        return Err("skin token is not ASCII".to_owned());
    }
    Ok(String::from_utf8(bytes.to_vec()).expect("ASCII is UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn split_skin_macro_matches_openjk_example() {
        assert_eq!(
            split_skin_macro("models/players/jedi_tf/|head01_skin1|torso01|lower01"),
            Some([
                "models/players/jedi_tf/head01_skin1.skin".to_owned(),
                "models/players/jedi_tf/torso01.skin".to_owned(),
                "models/players/jedi_tf/lower01.skin".to_owned(),
            ])
        );
        assert_eq!(split_skin_macro("models/players/kyle/model_default.skin"), None);
    }

    #[test]
    fn comma_parser_and_off_rules_match_register_individual_skin() {
        let parsed = parse_skin(
            br#"
                // line comment
                Torso,models/players/kyle/torso
                tag_weapon,ignored/tag_shader
                head_off,*off
                /* block comment */
                arm_off,models/players/kyle/arm
                "quoted surface","textures/test/quoted shader"
            "#,
        )
        .unwrap();

        assert_eq!(
            parsed,
            vec![
                SkinSurface {
                    name: "torso".to_owned(),
                    shader: "models/players/kyle/torso".to_owned(),
                },
                SkinSurface {
                    name: "arm".to_owned(),
                    shader: "models/players/kyle/arm".to_owned(),
                },
                SkinSurface {
                    name: "quoted surface".to_owned(),
                    shader: "textures/test/quoted shader".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn multipart_skin_loads_unique_parts_in_openjk_order() {
        let files = BTreeMap::from([
            ("models/p/head.skin".to_owned(), b"head,shader/head\n".to_vec()),
            ("models/p/torso.skin".to_owned(), b"torso,shader/torso\n".to_vec()),
        ]);
        let skin = load_skin("models/p/|head|torso|head", |name| Ok(files.get(name).cloned())).unwrap();
        assert_eq!(skin.len(), 2);
        assert_eq!(skin[0].name, "head");
        assert_eq!(skin[1].name, "torso");
    }
}
