//! Saber metadata ported from OpenJK's saber parameter loader.
//!
//! OpenJK concatenates every visible `ext_data/sabers/*.sab` file into one
//! COM-parsed token stream and applies `WP_SaberSetDefaults` before reading a
//! named block.  This module keeps the subset needed by the Rust client:
//! animation speed plus the authored hilt/blade presentation fields.

use std::collections::BTreeMap;

use crate::pk3::AssetSearchPath;

const SABER_DIRECTORY: &str = "ext_data/sabers/";
const SABER_EXTENSION: &str = ".sab";
const MAX_SABER_DATA_SIZE: usize = 0x80000;
const MAX_SABER_BLADES: usize = 8;
const DEFAULT_SABER_MODEL: &str = "models/weapons2/saber_reborn/saber_w.glm";

#[derive(Debug, Clone, Default)]
pub struct SaberAnimationScales {
    scales: BTreeMap<String, f32>,
}

impl SaberAnimationScales {
    /// OpenJK `WP_SaberSetDefaults` initializes `animSpeedScale` to 1.0.
    pub fn get(&self, saber_name: &str) -> f32 {
        self.scales
            .get(&saber_name.to_ascii_lowercase())
            .copied()
            .unwrap_or(1.0)
    }
}

/// OpenJK saber_colors_t.
pub const SABER_RED: i32 = 0;
pub const SABER_ORANGE: i32 = 1;
pub const SABER_BLUE: i32 = 4;
pub const SABER_PURPLE: i32 = 5;

#[derive(Debug, Clone, Copy)]
pub struct SaberBladeDefinition {
    pub length: f32,
    pub radius: f32,
    /// Authored blade color (saber_colors_t). Players override it with their
    /// `c1`/`c2` userinfo; NPCs without `boltToPlayer` colors draw it as-is.
    pub color: i32,
}

/// OpenJK TranslateSaberColor (vanilla names; unknown names are blue).
/// `random` is Q_irand(SABER_ORANGE, SABER_PURPLE) at load time in OpenJK;
/// the definition keeps it deterministic so every client draws one color.
pub fn translate_saber_color(name: &str) -> i32 {
    match name.to_ascii_lowercase().as_str() {
        "red" => SABER_RED,
        "orange" => SABER_ORANGE,
        "yellow" => 2,
        "green" => 3,
        "blue" => SABER_BLUE,
        "purple" => SABER_PURPLE,
        "random" => SABER_ORANGE,
        _ => SABER_BLUE,
    }
}

impl Default for SaberBladeDefinition {
    fn default() -> Self {
        // OpenJK gameplay defaults are 32/3.  Individual JA .sab definitions
        // commonly override the length (the UI fallback is 40).
        Self {
            length: 32.0,
            radius: 3.0,
            // WP_SaberSetDefaults
            color: SABER_RED,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SaberDefinition {
    pub name: String,
    pub model: String,
    pub custom_skin: Option<String>,
    pub num_blades: usize,
    pub blades: Vec<SaberBladeDefinition>,
    /// First blade using the saber definition's secondary authored style.
    /// This selects per-blade secondary properties; runtime blade activation
    /// (for example half-holstered staff) is controlled by saberHolstered.
    pub blade_style2_start: usize,
    /// SFL_RETURN_DAMAGE: retain angular motion while the saber returns.
    pub return_damage: bool,
    /// `soundLoop`: the hum CGame adds as a looping sound while a blade is lit.
    pub sound_loop: String,
    /// `swingSound1..3`: optional authored replacements used by EV_SABER_ATTACK.
    /// OpenJK falls back to saberhup1..8 unless swingSound1 is present.
    pub swing_sounds: [Option<String>; 3],
}

impl SaberDefinition {
    pub fn openjk_default(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            name,
            model: DEFAULT_SABER_MODEL.to_owned(),
            custom_skin: None,
            num_blades: 1,
            blades: vec![SaberBladeDefinition::default()],
            blade_style2_start: 0,
            return_damage: false,
            // OpenJK MP WP_SaberSetDefaults.
            sound_loop: "sound/weapons/saber/saberhum3.wav".to_owned(),
            swing_sounds: [None, None, None],
        }
    }

    pub fn blade(&self, index: usize) -> SaberBladeDefinition {
        self.blades
            .get(index)
            .copied()
            .unwrap_or_else(SaberBladeDefinition::default)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SaberDefinitions {
    definitions: BTreeMap<String, SaberDefinition>,
}

impl SaberDefinitions {
    pub fn get(&self, saber_name: &str) -> Option<&SaberDefinition> {
        self.definitions.get(&saber_name.to_ascii_lowercase())
    }

    pub fn definition_or_default(&self, saber_name: &str) -> SaberDefinition {
        self.get(saber_name)
            .cloned()
            .unwrap_or_else(|| SaberDefinition::openjk_default(saber_name))
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }
}

/// Load animation scales only.  Kept as a small compatibility API for the
/// movement/animation code that does not need rendering metadata.
pub fn load_saber_animation_scales(
    assets: &mut AssetSearchPath,
) -> Result<SaberAnimationScales, String> {
    let parsed = load_saber_metadata(assets)?;
    Ok(parsed.scales)
}

/// Load hilt/blade metadata used by player presentation.
pub fn load_saber_definitions(assets: &mut AssetSearchPath) -> Result<SaberDefinitions, String> {
    let parsed = load_saber_metadata(assets)?;
    Ok(parsed.definitions)
}

struct ParsedSabers {
    scales: SaberAnimationScales,
    definitions: SaberDefinitions,
}

fn load_saber_metadata(assets: &mut AssetSearchPath) -> Result<ParsedSabers, String> {
    let names = assets
        .names()
        .filter(|name| {
            name.starts_with(SABER_DIRECTORY)
                && name.ends_with(SABER_EXTENSION)
                && !name[SABER_DIRECTORY.len()..].contains('/')
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let mut total = 0usize;
    let mut scales = SaberAnimationScales::default();
    let mut definitions = SaberDefinitions::default();
    for name in names {
        let Some(asset) = assets
            .read(&name, MAX_SABER_DATA_SIZE)
            .map_err(|error| format!("{name}: {error}"))?
        else {
            continue;
        };
        total = total
            .checked_add(asset.bytes.len())
            .ok_or_else(|| "saber parameter size overflow".to_owned())?;
        if total >= MAX_SABER_DATA_SIZE {
            return Err(format!(
                "WP_SaberLoadParms: ran out of space before reading {name}"
            ));
        }
        parse_saber_file(&asset.bytes, &mut scales.scales, &mut definitions.definitions)?;
    }
    Ok(ParsedSabers { scales, definitions })
}

fn parse_saber_file(
    bytes: &[u8],
    scales: &mut BTreeMap<String, f32>,
    definitions: &mut BTreeMap<String, SaberDefinition>,
) -> Result<(), String> {
    let mut parser = ComParser::new(bytes);
    while let Some(name) = parser.token(true)? {
        if name == "{" || name == "}" {
            return Err(format!("unexpected saber token {name:?}"));
        }
        let Some(open) = parser.token(true)? else {
            return Err(format!("unexpected EOF after saber {name:?}"));
        };
        if open != "{" {
            return Err(format!("saber {name:?} missing opening '{{'"));
        }

        let mut anim_speed_scale = 1.0f32;
        let mut definition = SaberDefinition::openjk_default(name.clone());
        let mut common_length: Option<f32> = None;
        let mut common_radius: Option<f32> = None;
        let mut indexed_lengths: [Option<f32>; MAX_SABER_BLADES] = [None; MAX_SABER_BLADES];
        let mut indexed_radii: [Option<f32>; MAX_SABER_BLADES] = [None; MAX_SABER_BLADES];
        // Saber_ParseSaberColor* apply in file order: the plain key sets every
        // blade, saberColorN one blade.
        let mut colors = [SABER_RED; MAX_SABER_BLADES];

        loop {
            let Some(key) = parser.token(true)? else {
                return Err(format!("unexpected EOF while parsing saber {name:?}"));
            };
            if key == "}" {
                break;
            }
            let lower = key.to_ascii_lowercase();
            match lower.as_str() {
                "animspeedscale" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<f32>() {
                            if parsed.is_finite() {
                                anim_speed_scale = parsed;
                            }
                        }
                    }
                }
                "sabermodel" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            definition.model = value.replace('\\', "/");
                        }
                    }
                }
                "customskin" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            definition.custom_skin = Some(value.replace('\\', "/"));
                        }
                    }
                }
                "numblades" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            definition.num_blades = parsed.clamp(1, MAX_SABER_BLADES as i32) as usize;
                        }
                    }
                }
                "bladestyle2start" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            definition.blade_style2_start = parsed.max(0) as usize;
                        }
                    }
                }
                "soundloop" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            definition.sound_loop = value.replace('\\', "/");
                        }
                    }
                }
                _ if lower.starts_with("swingsound") => {
                    if let Ok(index) = lower["swingsound".len()..].parse::<usize>() {
                        if (1..=3).contains(&index) {
                            if let Some(value) = parser.token(false)? {
                                if !value.is_empty() && !value.eq_ignore_ascii_case("none") {
                                    definition.swing_sounds[index - 1] = Some(value.replace('\\', "/"));
                                }
                            }
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                "returndamage" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            // OpenJK sets this flag when nonzero; zero does not clear it.
                            definition.return_damage |= parsed != 0;
                        }
                    }
                }
                "saberlength" => {
                    common_length = parser
                        .token(false)?
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite())
                        .map(|value| value.max(4.0));
                }
                "saberradius" => {
                    common_radius = parser
                        .token(false)?
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite())
                        .map(|value| value.max(0.25));
                }
                "sabercolor" => {
                    if let Some(value) = parser.token(false)? {
                        colors = [translate_saber_color(&value); MAX_SABER_BLADES];
                    }
                }
                _ if lower.starts_with("sabercolor") => {
                    if let Ok(index) = lower["sabercolor".len()..].parse::<usize>() {
                        if (2..=MAX_SABER_BLADES).contains(&index) {
                            if let Some(value) = parser.token(false)? {
                                colors[index - 1] = translate_saber_color(&value);
                            }
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ if lower.starts_with("saberlength") => {
                    if let Ok(index) = lower["saberlength".len()..].parse::<usize>() {
                        if (1..=MAX_SABER_BLADES).contains(&index) {
                            indexed_lengths[index - 1] = parser
                                .token(false)?
                                .and_then(|value| value.parse::<f32>().ok())
                                .filter(|value| value.is_finite())
                                .map(|value| value.max(4.0));
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ if lower.starts_with("saberradius") => {
                    if let Ok(index) = lower["saberradius".len()..].parse::<usize>() {
                        if (1..=MAX_SABER_BLADES).contains(&index) {
                            indexed_radii[index - 1] = parser
                                .token(false)?
                                .and_then(|value| value.parse::<f32>().ok())
                                .filter(|value| value.is_finite())
                                .map(|value| value.max(0.25));
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ => parser.skip_rest_of_line(),
            }
        }

        definition.blades = (0..definition.num_blades)
            .map(|index| SaberBladeDefinition {
                length: indexed_lengths[index]
                    .or(common_length)
                    .unwrap_or_else(|| SaberBladeDefinition::default().length),
                radius: indexed_radii[index]
                    .or(common_radius)
                    .unwrap_or_else(|| SaberBladeDefinition::default().radius),
                color: colors[index],
            })
            .collect();
        definition.blade_style2_start = definition.blade_style2_start.min(definition.num_blades);

        let key = name.to_ascii_lowercase();
        scales.entry(key.clone()).or_insert(anim_speed_scale);
        definitions.entry(key).or_insert(definition);
    }
    Ok(())
}

/// Small safe equivalent of the COM_ParseExt behavior needed by `.sab` files.
struct ComParser<'a> {
    data: &'a [u8],
    cursor: usize,
}

impl<'a> ComParser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }

    fn token(&mut self, allow_line_breaks: bool) -> Result<Option<String>, String> {
        loop {
            let mut crossed_line = false;
            while self.cursor < self.data.len() && self.data[self.cursor] <= b' ' {
                if self.data[self.cursor] == b'\n' {
                    crossed_line = true;
                }
                self.cursor += 1;
            }
            if crossed_line && !allow_line_breaks {
                return Ok(None);
            }
            if self.cursor >= self.data.len() || self.data[self.cursor] == 0 {
                return Ok(None);
            }

            if self.starts_with(b"//") {
                self.skip_rest_of_line();
                if !allow_line_breaks {
                    return Ok(None);
                }
                continue;
            }
            if self.starts_with(b"/*") {
                self.cursor += 2;
                while self.cursor + 1 < self.data.len() && !self.starts_with(b"*/") {
                    self.cursor += 1;
                }
                if self.cursor + 1 >= self.data.len() {
                    return Err("unterminated block comment in saber file".to_owned());
                }
                self.cursor += 2;
                continue;
            }
            break;
        }

        if self.data[self.cursor] == b'"' {
            self.cursor += 1;
            let start = self.cursor;
            while self.cursor < self.data.len()
                && self.data[self.cursor] != b'"'
                && self.data[self.cursor] != 0
            {
                self.cursor += 1;
            }
            if self.cursor >= self.data.len() || self.data[self.cursor] != b'"' {
                return Err("unterminated quoted string in saber file".to_owned());
            }
            let token = ascii_token(&self.data[start..self.cursor])?;
            self.cursor += 1;
            return Ok(Some(token));
        }

        if matches!(self.data[self.cursor], b'{' | b'}') {
            let token = (self.data[self.cursor] as char).to_string();
            self.cursor += 1;
            return Ok(Some(token));
        }

        let start = self.cursor;
        while self.cursor < self.data.len()
            && self.data[self.cursor] > b' '
            && !matches!(self.data[self.cursor], b'{' | b'}')
        {
            if self.starts_with(b"//") || self.starts_with(b"/*") {
                break;
            }
            self.cursor += 1;
        }
        if self.cursor == start {
            return Err("empty saber token".to_owned());
        }
        Ok(Some(ascii_token(&self.data[start..self.cursor])?))
    }

    fn skip_rest_of_line(&mut self) {
        while self.cursor < self.data.len()
            && self.data[self.cursor] != b'\n'
            && self.data[self.cursor] != 0
        {
            self.cursor += 1;
        }
    }

    fn starts_with(&self, needle: &[u8]) -> bool {
        self.data.get(self.cursor..self.cursor + needle.len()) == Some(needle)
    }
}

fn ascii_token(bytes: &[u8]) -> Result<String, String> {
    if !bytes.is_ascii() {
        return Err("saber token is not ASCII".to_owned());
    }
    Ok(String::from_utf8(bytes.to_vec()).expect("ASCII is UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_render_fields_match_openjk_subset() {
        let mut scales = BTreeMap::new();
        let mut defs = BTreeMap::new();
        parse_saber_file(
            br#"
                // first definition wins
                single_1 {
                    name "Training Saber"
                    animSpeedScale 1.25
                    saberModel models/weapons2/saber/saber_w.glm
                    numBlades 2
                    saberLength 40
                    saberLength2 28
                    saberRadius 3.5
                    bladeStyle2Start 1
                    returnDamage 1
                    returnDamage 0
                    soundLoop "sound\weapons\saber\saberhum4.wav"
                    swingSound1 "sound\weapons\custom\swing1.wav"
                    swingSound2 sound/weapons/custom/swing2.wav
                    swingSound3 none
                    saberColor2 green
                    saberColor purple
                    saberColor2 Yellow
                }
                defaultish { }
                single_1 { animSpeedScale 9.0 }
            "#,
            &mut scales,
            &mut defs,
        )
        .unwrap();
        let set = SaberAnimationScales { scales };
        assert_eq!(set.get("single_1"), 1.25);
        assert_eq!(set.get("DEFAULTISH"), 1.0);
        assert_eq!(set.get("missing"), 1.0);
        let def = defs.get("single_1").unwrap();
        assert_eq!(def.num_blades, 2);
        assert_eq!(def.blade(0).length, 40.0);
        assert_eq!(def.blade(1).length, 28.0);
        assert_eq!(def.blade(0).radius, 3.5);
        assert_eq!(def.blade_style2_start, 1);
        assert!(def.return_damage);
        assert!(!defs.get("defaultish").unwrap().return_damage);
        assert_eq!(def.sound_loop, "sound/weapons/saber/saberhum4.wav");
        assert_eq!(def.swing_sounds[0].as_deref(), Some("sound/weapons/custom/swing1.wav"));
        assert_eq!(def.swing_sounds[1].as_deref(), Some("sound/weapons/custom/swing2.wav"));
        assert!(def.swing_sounds[2].is_none());
        // File order: saberColor after saberColor2 resets it, the later one wins.
        assert_eq!((def.blade(0).color, def.blade(1).color), (SABER_PURPLE, 2));
        assert_eq!(defs.get("defaultish").unwrap().blade(0).color, SABER_RED);
        assert_eq!(defs.get("defaultish").unwrap().sound_loop, "sound/weapons/saber/saberhum3.wav");
    }
}
