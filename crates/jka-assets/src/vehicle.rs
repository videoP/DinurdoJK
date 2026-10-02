//! Vehicle presentation metadata from JKA `ext_data/vehicles/*.veh` files.
//!
//! Multiplayer does not send a vehicle's Ghoul2 qpath directly. The server can
//! put the vehicle definition name in `CS_MODELS`/`modelindex`; OpenJK then
//! resolves that definition's `model` (and optional `skin`) before registering
//! `models/players/<model>/model.glm`.

use std::collections::BTreeMap;

use crate::pk3::AssetSearchPath;

const VEHICLE_DIRECTORY: &str = "ext_data/vehicles/";
const VEHICLE_EXTENSION: &str = ".veh";
const WEAPON_DIRECTORY: &str = "ext_data/vehicles/weapons/";
const WEAPON_EXTENSION: &str = ".vwp";
const MAX_VEHICLE_FILE_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleCameraDefinition {
    pub override_enabled: bool,
    pub range: f32,
    pub vert_offset: f32,
    pub horz_offset: f32,
    pub pitch_offset: f32,
    pub fov: f32,
    pub alpha: f32,
    pub pitch_dependent_vert_offset: bool,
}

impl Default for VehicleCameraDefinition {
    fn default() -> Self {
        // BG_VehicleSetDefaults initializes these to zero and the authored
        // .veh values replace them when cameraOverride is enabled.
        Self {
            override_enabled: false,
            range: 0.0,
            vert_offset: 0.0,
            horz_offset: 0.0,
            pitch_offset: 0.0,
            fov: 0.0,
            alpha: 0.0,
            pitch_dependent_vert_offset: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VehicleDefinition {
    pub name: String,
    pub model: String,
    pub skin: Option<String>,
    pub vehicle_type: Option<String>,
    pub camera: VehicleCameraDefinition,
    /// `weapMuzzle1..12`: the vehicle weapon (a `.vwp` name) fired from each
    /// `*muzzleN` bolt. Turret-owned muzzles are not resolved.
    pub weap_muzzles: [Option<String>; MAX_VEHICLE_MUZZLES],
}

pub const MAX_VEHICLE_MUZZLES: usize = 12;

#[derive(Debug, Clone, Default)]
pub struct VehicleDefinitions {
    definitions: BTreeMap<String, VehicleDefinition>,
    /// Vehicle weapon name (lower case) -> `muzzleFX` effect from `.vwp` files.
    weapon_muzzle_fx: BTreeMap<String, String>,
}

impl VehicleDefinitions {
    pub fn get(&self, name: &str) -> Option<&VehicleDefinition> {
        self.definitions.get(&name.to_ascii_lowercase())
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// `g_vehWeaponInfo[weapon].iMuzzleFX` as an effect name.
    pub fn weapon_muzzle_fx(&self, weapon: &str) -> Option<&str> {
        self.weapon_muzzle_fx.get(&weapon.to_ascii_lowercase()).map(String::as_str)
    }
}

pub fn load_vehicle_definitions(assets: &mut AssetSearchPath) -> Result<VehicleDefinitions, String> {
    let names = assets
        .names()
        .filter(|name| name.starts_with(VEHICLE_DIRECTORY) && name.ends_with(VEHICLE_EXTENSION))
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let mut definitions = BTreeMap::new();
    for name in names {
        let Some(asset) = assets
            .read(&name, MAX_VEHICLE_FILE_SIZE)
            .map_err(|error| format!("{name}: {error}"))?
        else {
            continue;
        };
        parse_vehicle_file(&asset.bytes, &mut definitions)
            .map_err(|error| format!("{name}: {error}"))?;
    }
    let mut weapon_muzzle_fx = BTreeMap::new();
    let weapons = assets
        .names()
        .filter(|name| name.starts_with(WEAPON_DIRECTORY) && name.ends_with(WEAPON_EXTENSION))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for name in weapons {
        let Some(asset) = assets
            .read(&name, MAX_VEHICLE_FILE_SIZE)
            .map_err(|error| format!("{name}: {error}"))?
        else {
            continue;
        };
        // A malformed weapon file only loses its muzzle effects.
        let _ = parse_weapon_file(&asset.bytes, &mut weapon_muzzle_fx);
    }
    Ok(VehicleDefinitions { definitions, weapon_muzzle_fx })
}

/// `.vwp` blocks: `name { ... muzzleFX "effect" ... }`.
fn parse_weapon_file(bytes: &[u8], out: &mut BTreeMap<String, String>) -> Result<(), String> {
    let mut parser = ComParser::new(bytes);
    while let Some(name) = parser.token(true)? {
        if parser.token(true)?.as_deref() != Some("{") {
            return Err(format!("vehicle weapon {name:?} missing opening '{{'"));
        }
        let mut muzzle_fx = None;
        loop {
            let Some(key) = parser.token(true)? else { return Err("unexpected EOF in vehicle weapon".into()) };
            if key == "}" {
                break;
            }
            if key.eq_ignore_ascii_case("muzzleFX") {
                muzzle_fx = parser.token(false)?.filter(|value| !value.is_empty());
            } else {
                parser.skip_rest_of_line();
            }
        }
        if let Some(effect) = muzzle_fx {
            out.entry(name.to_ascii_lowercase()).or_insert(effect);
        }
    }
    Ok(())
}

fn parse_vehicle_file(
    bytes: &[u8],
    definitions: &mut BTreeMap<String, VehicleDefinition>,
) -> Result<(), String> {
    let mut parser = ComParser::new(bytes);
    while let Some(name) = parser.token(true)? {
        if name == "{" || name == "}" {
            return Err(format!("unexpected vehicle token {name:?}"));
        }
        let Some(open) = parser.token(true)? else {
            return Err(format!("unexpected EOF after vehicle {name:?}"));
        };
        if open != "{" {
            return Err(format!("vehicle {name:?} missing opening '{{'"));
        }

        let mut model = None;
        let mut skin = None;
        let mut vehicle_type = None;
        let mut camera = VehicleCameraDefinition::default();
        let mut weap_muzzles: [Option<String>; MAX_VEHICLE_MUZZLES] = Default::default();
        loop {
            let Some(key) = parser.token(true)? else {
                return Err(format!("unexpected EOF while parsing vehicle {name:?}"));
            };
            if key == "}" {
                break;
            }
            if key == "{" {
                // A nested block (turret1 { ... }): skip to its matching brace.
                let mut depth = 1;
                while depth > 0 {
                    match parser.token(true)?.as_deref() {
                        Some("{") => depth += 1,
                        Some("}") => depth -= 1,
                        Some(_) => {}
                        None => return Err(format!("unexpected EOF in nested block of vehicle {name:?}")),
                    }
                }
                continue;
            }
            let lower = key.to_ascii_lowercase();
            if let Some(slot) = lower
                .strip_prefix("weapmuzzle")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| (1..=MAX_VEHICLE_MUZZLES).contains(n))
            {
                weap_muzzles[slot - 1] = parser.token(false)?.filter(|weapon| !weapon.is_empty() && weapon != "0");
                continue;
            }
            match lower.as_str() {
                "model" => {
                    if let Some(value) = parser.token(false)? {
                        let value = value.replace('\\', "/");
                        if !value.is_empty() {
                            model = Some(value);
                        }
                    }
                }
                "skin" => {
                    if let Some(value) = parser.token(false)? {
                        let value = value.replace('\\', "/");
                        if !value.is_empty() && !value.eq_ignore_ascii_case("default") {
                            skin = Some(value);
                        }
                    }
                }
                "type" => {
                    vehicle_type = parser.token(false)?;
                }
                "cameraoverride" => {
                    if let Some(value) = parser.token(false)? {
                        camera.override_enabled = parse_vehicle_bool(&value);
                    }
                }
                "camerarange" => parse_camera_float(&mut parser, &mut camera.range)?,
                "cameravertoffset" => parse_camera_float(&mut parser, &mut camera.vert_offset)?,
                "camerahorzoffset" => parse_camera_float(&mut parser, &mut camera.horz_offset)?,
                "camerapitchoffset" => parse_camera_float(&mut parser, &mut camera.pitch_offset)?,
                "camerafov" => parse_camera_float(&mut parser, &mut camera.fov)?,
                "cameraalpha" => parse_camera_float(&mut parser, &mut camera.alpha)?,
                "camerapitchdependantvertoffset" => {
                    if let Some(value) = parser.token(false)? {
                        camera.pitch_dependent_vert_offset = parse_vehicle_bool(&value);
                    }
                }
                _ => parser.skip_rest_of_line(),
            }
        }

        // A malformed or abstract block with no model is not renderable, so it
        // should not shadow a later usable definition of the same name.
        if let Some(model) = model {
            definitions
                .entry(name.to_ascii_lowercase())
                .or_insert_with(|| VehicleDefinition {
                    name,
                    model,
                    skin,
                    vehicle_type,
                    camera,
                    weap_muzzles,
                });
        }
    }
    Ok(())
}

fn parse_vehicle_bool(value: &str) -> bool {
    // OpenJK VF_BOOL is atof(value) != 0. Keep friendly textual forms too.
    value
        .parse::<f32>()
        .map_or_else(
            |_| matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "on"),
            |number| number != 0.0,
        )
}

fn parse_camera_float(parser: &mut ComParser<'_>, target: &mut f32) -> Result<(), String> {
    let Some(value) = parser.token(false)? else {
        return Ok(());
    };
    if let Ok(parsed) = value.parse::<f32>() {
        if parsed.is_finite() {
            *target = parsed;
        }
    }
    Ok(())
}

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
                crossed_line |= self.data[self.cursor] == b'\n';
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
                    return Err("unterminated block comment in vehicle file".to_owned());
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
                return Err("unterminated quoted string in vehicle file".to_owned());
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
            return Err("empty vehicle token".to_owned());
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
        return Err("vehicle token is not ASCII".to_owned());
    }
    Ok(String::from_utf8(bytes.to_vec()).expect("ASCII is UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_muzzle_weapons_skipping_nested_turret_blocks() {
        let mut defs = BTreeMap::new();
        parse_vehicle_file(
            br#"
                atst {
                    model atst
                    turret1
                    {
                        weapon atst_laser
                        muzzle1 3
                    }
                    weapMuzzle2 atst_rocket
                    weapMuzzle3 0
                }
            "#,
            &mut defs,
        )
        .unwrap();
        let atst = &defs["atst"];
        assert_eq!(atst.weap_muzzles[1].as_deref(), Some("atst_rocket"));
        assert_eq!(atst.weap_muzzles[2], None);
        let mut fx = BTreeMap::new();
        parse_weapon_file(b"swoop_laser
{
name swoop_laser
muzzleFX \"ships/swoop_blastermuzzleflash\"
speed 2500
}
", &mut fx).unwrap();
        assert_eq!(fx["swoop_laser"], "ships/swoop_blastermuzzleflash");
    }

    #[test]
    fn parses_model_and_skin_from_vehicle_blocks() {
        let mut defs = BTreeMap::new();
        parse_vehicle_file(
            br#"
                // Stock-style vehicle definition.
                x-wing {
                    name "X-Wing"
                    type VH_FIGHTER
                    model x-wing
                    skin red
                    speedMax 900
                }
                swoop {
                    model "swoop"
                    skin default
                }
            "#,
            &mut defs,
        )
        .unwrap();

        let xwing = &defs["x-wing"];
        assert_eq!(xwing.model, "x-wing");
        assert_eq!(xwing.skin.as_deref(), Some("red"));
        assert_eq!(xwing.vehicle_type.as_deref(), Some("VH_FIGHTER"));
        let swoop = &defs["swoop"];
        assert_eq!(swoop.model, "swoop");
        assert_eq!(swoop.skin, None);
    }

    #[test]
    fn parses_openjk_vehicle_camera_overrides() {
        let mut defs = BTreeMap::new();
        parse_vehicle_file(
            br#"tie-fighter {
                type VH_FIGHTER
                model tie_fighter
                cameraOverride 1
                cameraRange 240
                cameraVertOffset 48
                cameraHorzOffset -8
                cameraPitchOffset 3
                cameraFOV 90
                cameraAlpha 0.5
                cameraPitchDependantVertOffset 1
            }"#,
            &mut defs,
        )
        .unwrap();
        let tie = &defs["tie-fighter"];
        assert!(tie.camera.override_enabled);
        assert_eq!(tie.camera.range, 240.0);
        assert_eq!(tie.camera.vert_offset, 48.0);
        assert_eq!(tie.camera.horz_offset, -8.0);
        assert_eq!(tie.camera.pitch_offset, 3.0);
        assert_eq!(tie.camera.fov, 90.0);
        assert_eq!(tie.camera.alpha, 0.5);
        assert!(tie.camera.pitch_dependent_vert_offset);
    }

    #[test]
    fn first_definition_wins_like_jka_parameter_loading() {
        let mut defs = BTreeMap::new();
        parse_vehicle_file(b"X-Wing { model first }\nx-wing { model second }\n", &mut defs).unwrap();
        assert_eq!(defs["x-wing"].model, "first");
    }
}
