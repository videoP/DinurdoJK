//! Saber metadata ported from OpenJK's saber parameter loader.
//!
//! OpenJK concatenates every visible `ext_data/sabers/*.sab` file into one
//! COM-parsed token stream and applies `WP_SaberSetDefaults` before reading a
//! named block.  This module keeps the subset needed by the Rust client:
//! animation speed plus authored hilt/blade and BG/Pmove gameplay fields.

use std::collections::BTreeMap;

use crate::{animation::animation_index, pk3::AssetSearchPath};

const SABER_DIRECTORY: &str = "ext_data/sabers/";
const SABER_EXTENSION: &str = ".sab";
const MAX_SABER_DATA_SIZE: usize = 0x80000;
const MAX_SABER_BLADES: usize = 8;
const DEFAULT_SABER_MODEL: &str = "models/weapons2/saber_reborn/saber_w.glm";
/// OpenJK DEFAULT_SABER (bg_public.h).
pub const DEFAULT_SABER: &str = "Kyle";

// OpenJK saber_styles_t values.
const SS_NONE: i32 = 0;
const SS_FAST: i32 = 1;
const SS_MEDIUM: i32 = 2;
const SS_STRONG: i32 = 3;
const SS_DESANN: i32 = 4;
const SS_TAVION: i32 = 5;
const SS_DUAL: i32 = 6;
const SS_STAFF: i32 = 7;
const SS_NUM_SABER_STYLES: i32 = 8;

// OpenJK SFL_* bits from bg_public.h. Keep the numeric values here because
// .sab files are parsed by jka-assets while the native movement crate owns C.
pub const SFL_NOT_LOCKABLE: i32 = 1 << 0;
pub const SFL_NOT_THROWABLE: i32 = 1 << 1;
pub const SFL_NOT_DISARMABLE: i32 = 1 << 2;
pub const SFL_NOT_ACTIVE_BLOCKING: i32 = 1 << 3;
pub const SFL_TWO_HANDED: i32 = 1 << 4;
pub const SFL_SINGLE_BLADE_THROWABLE: i32 = 1 << 5;
pub const SFL_RETURN_DAMAGE: i32 = 1 << 6;
pub const SFL_BOUNCE_ON_WALLS: i32 = 1 << 8;
pub const SFL_BOLT_TO_WRIST: i32 = 1 << 9;
pub const SFL_NO_PULL_ATTACK: i32 = 1 << 10;
pub const SFL_NO_BACK_ATTACK: i32 = 1 << 11;
pub const SFL_NO_STABDOWN: i32 = 1 << 12;
pub const SFL_NO_WALL_RUNS: i32 = 1 << 13;
pub const SFL_NO_WALL_FLIPS: i32 = 1 << 14;
pub const SFL_NO_WALL_GRAB: i32 = 1 << 15;
pub const SFL_NO_ROLLS: i32 = 1 << 16;
pub const SFL_NO_FLIPS: i32 = 1 << 17;
pub const SFL_NO_CARTWHEELS: i32 = 1 << 18;
pub const SFL_NO_KICKS: i32 = 1 << 19;
pub const SFL_NO_MIRROR_ATTACKS: i32 = 1 << 20;
pub const SFL_NO_ROLL_STAB: i32 = 1 << 21;

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

fn translate_saber_style(name: &str) -> i32 {
    match name.to_ascii_lowercase().as_str() {
        "fast" => SS_FAST,
        "medium" => SS_MEDIUM,
        "strong" => SS_STRONG,
        "desann" => SS_DESANN,
        "tavion" => SS_TAVION,
        "dual" => SS_DUAL,
        "staff" => SS_STAFF,
        _ => SS_NONE,
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
    /// Saber block identifier used by saber1/saber2.
    pub name: String,
    /// Authored menu/display name (`name` inside the .sab block).
    pub proper_name: String,
    /// Authored saberType used by the stock UI to separate single/staff modes.
    pub saber_type: String,
    pub model: String,
    pub custom_skin: Option<String>,
    pub num_blades: usize,
    pub blades: Vec<SaberBladeDefinition>,
    /// First blade using the saber definition's secondary authored style.
    /// This selects per-blade secondary properties; runtime blade activation
    /// (for example half-holstered staff) is controlled by saberHolstered.
    pub blade_style2_start: usize,
    /// OpenJK saber trail style: 0 = normal saber blur, 1 = sword/motion trail,
    /// >1 = no trail. Secondary style follows bladeStyle2Start.
    pub trail_style: i32,
    pub trail_style2: i32,
    /// OpenJK SFL2_NO_WALL_MARKS / SFL2_NO_WALL_MARKS2 equivalents. These
    /// suppress wall sparks/marks for the primary/secondary blade style.
    pub no_wall_marks: bool,
    pub no_wall_marks2: bool,
    /// OpenJK SFL2_NO_DLIGHT equivalent (`noDlight` in .sab files).
    pub no_dlight: bool,
    /// OpenJK saberInfo_t gameplay fields consumed by shared BG/Pmove.
    pub move_speed_scale: f32,
    pub anim_speed_scale: f32,
    pub styles_learned: i32,
    pub styles_forbidden: i32,
    pub saber_flags: i32,
    pub ready_anim: i32,
    pub draw_anim: i32,
    pub putaway_anim: i32,
    /// `singleBladeStyle`: the stance used while only the first blade is lit
    /// (`SS_NONE` when the saber does not override it).
    pub single_blade_style: i32,
    /// SFL2_NO_MANUAL_DEACTIVATE / SFL2_NO_MANUAL_DEACTIVATE2: the primary or
    /// secondary blade set cannot be toggled off with saberAttackCycle.
    pub no_manual_deactivate: bool,
    pub no_manual_deactivate2: bool,
    /// SFL_RETURN_DAMAGE: retain angular motion while the saber returns.
    pub return_damage: bool,
    /// `notInMP`: WP_SaberValidForPlayerInMP substitutes DEFAULT_SABER for a
    /// multiplayer client that selects this definition.
    pub not_in_mp: bool,
    /// `soundLoop`: the hum CGame adds as a looping sound while a blade is lit.
    pub sound_loop: String,
    /// `soundOn` / `soundOff`: played by EV_SABER_UNHOLSTER (on) when the blade lights.
    pub sound_on: String,
    pub sound_off: String,
    /// `swingSound1..3`: optional authored replacements used by EV_SABER_ATTACK.
    /// OpenJK falls back to saberhup1..8 unless swingSound1 is present.
    pub swing_sounds: [Option<String>; 3],
    /// OpenJK saber impact overrides consumed by EV_SABER_HIT / EV_SABER_BLOCK.
    pub hit_sounds: [Option<String>; 3],
    pub hit2_sounds: [Option<String>; 3],
    pub block_sounds: [Option<String>; 3],
    pub block2_sounds: [Option<String>; 3],
    pub block_effect: Option<String>,
    pub block_effect2: Option<String>,
    pub hit_person_effect: Option<String>,
    pub hit_person_effect2: Option<String>,
    pub hit_other_effect: Option<String>,
    pub hit_other_effect2: Option<String>,
    /// SFL2_NO_CLASH_FLARE / SFL2_NO_CLASH_FLARE2.  MP CG_EntityEvent
    /// intentionally checks the primary flag for EV_SABER_BLOCK.
    pub no_clash_flare: bool,
    pub no_clash_flare2: bool,
}

impl SaberDefinition {
    pub fn openjk_default(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            proper_name: name.clone(),
            saber_type: "SABER_SINGLE".to_owned(),
            name,
            model: DEFAULT_SABER_MODEL.to_owned(),
            custom_skin: None,
            num_blades: 1,
            blades: vec![SaberBladeDefinition::default()],
            blade_style2_start: 0,
            trail_style: 0,
            trail_style2: 0,
            no_wall_marks: false,
            no_wall_marks2: false,
            no_dlight: false,
            move_speed_scale: 1.0,
            anim_speed_scale: 1.0,
            styles_learned: 0,
            styles_forbidden: 0,
            saber_flags: 0,
            ready_anim: -1,
            draw_anim: -1,
            putaway_anim: -1,
            single_blade_style: SS_NONE,
            no_manual_deactivate: false,
            no_manual_deactivate2: false,
            return_damage: false,
            not_in_mp: false,
            // OpenJK MP WP_SaberSetDefaults.
            sound_loop: "sound/weapons/saber/saberhum3.wav".to_owned(),
            sound_on: "sound/weapons/saber/enemy_saber_on.wav".to_owned(),
            sound_off: "sound/weapons/saber/enemy_saber_off.wav".to_owned(),
            swing_sounds: [None, None, None],
            hit_sounds: [None, None, None],
            hit2_sounds: [None, None, None],
            block_sounds: [None, None, None],
            block2_sounds: [None, None, None],
            block_effect: None,
            block_effect2: None,
            hit_person_effect: None,
            hit_person_effect2: None,
            hit_other_effect: None,
            hit_other_effect2: None,
            no_clash_flare: false,
            no_clash_flare2: false,
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

    /// WP_SaberValidForPlayerInMP: a definition is valid unless it authors a
    /// non-zero `notInMP`.
    fn valid_for_player_in_mp(&self, saber_name: &str) -> bool {
        self.get(saber_name).is_none_or(|definition| !definition.not_in_mp)
    }

    /// The definition WP_SetSaber actually parses for `saber_name`. Player
    /// clients (entNum < MAX_CLIENTS) fall back to DEFAULT_SABER when the
    /// selection is not allowed in multiplayer.
    fn parsed_definition(&self, saber_name: &str, player_client: bool) -> SaberDefinition {
        if player_client && !self.valid_for_player_in_mp(saber_name) {
            self.definition_or_default(DEFAULT_SABER)
        } else {
            self.definition_or_default(saber_name)
        }
    }

    /// Which of the two saber slots exist after OpenJK `WP_SetSaber`:
    /// saber 0 can never be removed; saber 1 is dropped for "none"/"remove",
    /// when it is itself two-handed, or when saber 0 is two-handed.
    pub fn equipped_slots(&self, saber_names: [&str; 2], player_client: bool) -> [bool; 2] {
        let removed = |name: &str| {
            name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("remove")
        };
        let two_handed = |name: &str| {
            let name = if player_client && !self.valid_for_player_in_mp(name) {
                DEFAULT_SABER
            } else {
                name
            };
            self.get(name)
                .is_some_and(|definition| definition.saber_flags & SFL_TWO_HANDED != 0)
        };
        let primary = !removed(saber_names[0]);
        let mut secondary = !saber_names[1].is_empty() && !removed(saber_names[1]);
        if secondary
            && (two_handed(saber_names[1]) || (primary && two_handed(saber_names[0])))
        {
            secondary = false;
        }
        [primary, secondary]
    }

    /// The `clientInfo_t::saber[]` pair OpenJK ends up with for the two
    /// configured saber names (see [`Self::equipped_slots`]).
    pub fn equip(
        &self,
        saber_names: [&str; 2],
        player_client: bool,
    ) -> [Option<SaberDefinition>; 2] {
        let slots = self.equipped_slots(saber_names, player_client);
        [0, 1].map(|slot| {
            slots[slot].then(|| self.parsed_definition(saber_names[slot], player_client))
        })
    }

    /// Visible saber definitions in the active VFS, in deterministic key order.
    /// Profile/menu code uses the parsed definitions rather than re-parsing
    /// ext_data/sabers so preview and gameplay always resolve the same hilt.
    pub fn iter(&self) -> impl Iterator<Item = &SaberDefinition> {
        self.definitions.values()
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
                                definition.anim_speed_scale = parsed;
                            }
                        }
                    }
                }
                "movespeedscale" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<f32>() {
                            if parsed.is_finite() {
                                definition.move_speed_scale = parsed;
                            }
                        }
                    }
                }
                "readyanim" | "drawanim" | "putawayanim" => {
                    if let Some(value) = parser.token(false)? {
                        if let Some(index) = animation_index(&value) {
                            let index = index as i32;
                            match lower.as_str() {
                                "readyanim" => definition.ready_anim = index,
                                "drawanim" => definition.draw_anim = index,
                                "putawayanim" => definition.putaway_anim = index,
                                _ => unreachable!(),
                            }
                        }
                    }
                }
                "saberstyle" => {
                    if let Some(value) = parser.token(false)? {
                        let style = translate_saber_style(&value);
                        definition.styles_learned = 1 << style;
                        definition.styles_forbidden = 0;
                        for other in (SS_NONE + 1)..SS_NUM_SABER_STYLES {
                            if other != style {
                                definition.styles_forbidden |= 1 << other;
                            }
                        }
                    }
                }
                "saberstylelearned" => {
                    if let Some(value) = parser.token(false)? {
                        definition.styles_learned |= 1 << translate_saber_style(&value);
                    }
                }
                "saberstyleforbidden" => {
                    if let Some(value) = parser.token(false)? {
                        definition.styles_forbidden |= 1 << translate_saber_style(&value);
                    }
                }
                "singlebladestyle" => {
                    if let Some(value) = parser.token(false)? {
                        definition.single_blade_style = translate_saber_style(&value);
                    }
                }
                "nomanualdeactivate" | "nomanualdeactivate2" => {
                    if let Some(value) = parser.token(false)? {
                        let enabled = matches!(value.parse::<i32>(), Ok(value) if value != 0);
                        if lower == "nomanualdeactivate2" {
                            definition.no_manual_deactivate2 = enabled;
                        } else {
                            definition.no_manual_deactivate = enabled;
                        }
                    }
                }
                "lockable" | "throwable" | "disarmable" | "blocking" => {
                    if let Some(value) = parser.token(false)? {
                        if value.parse::<i32>().ok() == Some(0) {
                            definition.saber_flags |= match lower.as_str() {
                                "lockable" => SFL_NOT_LOCKABLE,
                                "throwable" => SFL_NOT_THROWABLE,
                                "disarmable" => SFL_NOT_DISARMABLE,
                                "blocking" => SFL_NOT_ACTIVE_BLOCKING,
                                _ => unreachable!(),
                            };
                        }
                    }
                }
                "twohanded" | "singlebladethrowable" | "bounceonwalls" | "bolttowrist"
                | "norollstab" | "nopullattack" | "nobackattack" | "nostabdown"
                | "nowallruns" | "nowallflips" | "nowallgrab" | "norolls"
                | "noflips" | "nocartwheels" | "nokicks" | "nomirrorattacks" => {
                    if let Some(value) = parser.token(false)? {
                        if matches!(value.parse::<i32>(), Ok(value) if value != 0) {
                            definition.saber_flags |= match lower.as_str() {
                                "twohanded" => SFL_TWO_HANDED,
                                "singlebladethrowable" => SFL_SINGLE_BLADE_THROWABLE,
                                "bounceonwalls" => SFL_BOUNCE_ON_WALLS,
                                "bolttowrist" => SFL_BOLT_TO_WRIST,
                                "norollstab" => SFL_NO_ROLL_STAB,
                                "nopullattack" => SFL_NO_PULL_ATTACK,
                                "nobackattack" => SFL_NO_BACK_ATTACK,
                                "nostabdown" => SFL_NO_STABDOWN,
                                "nowallruns" => SFL_NO_WALL_RUNS,
                                "nowallflips" => SFL_NO_WALL_FLIPS,
                                "nowallgrab" => SFL_NO_WALL_GRAB,
                                "norolls" => SFL_NO_ROLLS,
                                "noflips" => SFL_NO_FLIPS,
                                "nocartwheels" => SFL_NO_CARTWHEELS,
                                "nokicks" => SFL_NO_KICKS,
                                "nomirrorattacks" => SFL_NO_MIRROR_ATTACKS,
                                _ => unreachable!(),
                            };
                        }
                    }
                }
                "name" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            definition.proper_name = value;
                        }
                    }
                }
                "sabertype" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            definition.saber_type = value;
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
                "trailstyle" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            definition.trail_style = parsed.max(0);
                        }
                    }
                }
                "trailstyle2" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            definition.trail_style2 = parsed.max(0);
                        }
                    }
                }
                "nowallmarks" | "nowallmarks2" => {
                    if let Some(value) = parser.token(false)? {
                        let enabled = matches!(value.parse::<i32>(), Ok(value) if value != 0);
                        if lower == "nowallmarks2" {
                            definition.no_wall_marks2 = enabled;
                        } else {
                            definition.no_wall_marks = enabled;
                        }
                    }
                }
                "nodlight" => {
                    if let Some(value) = parser.token(false)? {
                        definition.no_dlight = matches!(value.parse::<i32>(), Ok(value) if value != 0);
                    }
                }
                "soundon" | "soundoff" => {
                    if let Some(value) = parser.token(false)? {
                        if !value.is_empty() {
                            let value = value.replace('\\', "/");
                            if lower == "soundon" {
                                definition.sound_on = value;
                            } else {
                                definition.sound_off = value;
                            }
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
                _ if lower.starts_with("hitsound") => {
                    if let Ok(index) = lower["hitsound".len()..].parse::<usize>() {
                        if (1..=3).contains(&index) {
                            definition.hit_sounds[index - 1] = parse_optional_qpath(&mut parser)?;
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ if lower.starts_with("hit2sound") => {
                    if let Ok(index) = lower["hit2sound".len()..].parse::<usize>() {
                        if (1..=3).contains(&index) {
                            definition.hit2_sounds[index - 1] = parse_optional_qpath(&mut parser)?;
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ if lower.starts_with("blocksound") => {
                    if let Ok(index) = lower["blocksound".len()..].parse::<usize>() {
                        if (1..=3).contains(&index) {
                            definition.block_sounds[index - 1] = parse_optional_qpath(&mut parser)?;
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                _ if lower.starts_with("block2sound") => {
                    if let Ok(index) = lower["block2sound".len()..].parse::<usize>() {
                        if (1..=3).contains(&index) {
                            definition.block2_sounds[index - 1] = parse_optional_qpath(&mut parser)?;
                            continue;
                        }
                    }
                    parser.skip_rest_of_line();
                }
                "blockeffect" => definition.block_effect = parse_optional_qpath(&mut parser)?,
                "blockeffect2" => definition.block_effect2 = parse_optional_qpath(&mut parser)?,
                "hitpersoneffect" => definition.hit_person_effect = parse_optional_qpath(&mut parser)?,
                "hitpersoneffect2" => definition.hit_person_effect2 = parse_optional_qpath(&mut parser)?,
                "hitothereffect" => definition.hit_other_effect = parse_optional_qpath(&mut parser)?,
                "hitothereffect2" => definition.hit_other_effect2 = parse_optional_qpath(&mut parser)?,
                "noclashflare" | "noclashflare2" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(value) = value.parse::<i32>() {
                            if lower == "noclashflare2" {
                                definition.no_clash_flare2 = value != 0;
                            } else {
                                definition.no_clash_flare = value != 0;
                            }
                        }
                    }
                }
                "notinmp" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            definition.not_in_mp = parsed != 0;
                        }
                    }
                }
                "returndamage" => {
                    if let Some(value) = parser.token(false)? {
                        if let Ok(parsed) = value.parse::<i32>() {
                            // OpenJK sets this flag when nonzero; zero does not clear it.
                            definition.return_damage |= parsed != 0;
                            if parsed != 0 {
                                definition.saber_flags |= SFL_RETURN_DAMAGE;
                            }
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

fn parse_optional_qpath(parser: &mut ComParser<'_>) -> Result<Option<String>, String> {
    Ok(parser.token(false)?.and_then(|value| {
        if value.is_empty() || value.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(value.replace('\\', "/"))
        }
    }))
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
    fn blade_toggle_gameplay_keys_are_parsed() {
        let mut scales = BTreeMap::new();
        let mut defs = BTreeMap::new();
        parse_saber_file(
            br#"
                staff_1 {
                    numBlades 2
                    singleBladeStyle medium
                    noManualDeactivate 1
                    noManualDeactivate2 0
                }
                plain_1 {
                }
            "#,
            &mut scales,
            &mut defs,
        )
        .unwrap();
        let staff = &defs["staff_1"];
        assert_eq!(staff.single_blade_style, SS_MEDIUM);
        assert!(staff.no_manual_deactivate);
        assert!(!staff.no_manual_deactivate2);
        let plain = &defs["plain_1"];
        assert_eq!(plain.single_blade_style, SS_NONE);
        assert!(!plain.no_manual_deactivate && !plain.no_manual_deactivate2);
    }

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
                    moveSpeedScale 0.85
                    readyAnim BOTH_SABERFAST_STANCE
                    drawAnim BOTH_STAND2
                    putawayAnim BOTH_STAND2
                    saberStyleLearned tavion
                    saberStyleForbidden strong
                    noCartwheels 1
                    noRolls 1
                    saberModel models/weapons2/saber/saber_w.glm
                    numBlades 2
                    saberLength 40
                    saberLength2 28
                    saberRadius 3.5
                    bladeStyle2Start 1
                    trailStyle 1
                    trailStyle2 2
                    noDlight 1
                    returnDamage 1
                    returnDamage 0
                    soundLoop "sound\weapons\saber\saberhum4.wav"
                    swingSound1 "sound\weapons\custom\swing1.wav"
                    swingSound2 sound/weapons/custom/swing2.wav
                    swingSound3 none
                    hitSound1 sound/weapons/custom/hit1.wav
                    hit2Sound2 sound/weapons/custom/hit2_2.wav
                    blockSound3 sound/weapons/custom/block3.wav
                    block2Sound1 sound/weapons/custom/block2_1.wav
                    blockEffect saber/custom_block.efx
                    blockEffect2 saber/custom_block2.efx
                    hitPersonEffect saber/custom_person.efx
                    hitPersonEffect2 saber/custom_person2.efx
                    hitOtherEffect saber/custom_other.efx
                    hitOtherEffect2 saber/custom_other2.efx
                    noClashFlare 1
                    noClashFlare2 1
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
        assert_eq!(def.anim_speed_scale, 1.25);
        assert_eq!(def.move_speed_scale, 0.85);
        assert_eq!(def.ready_anim, animation_index("BOTH_SABERFAST_STANCE").unwrap() as i32);
        assert_eq!(def.draw_anim, animation_index("BOTH_STAND2").unwrap() as i32);
        assert_eq!(def.putaway_anim, animation_index("BOTH_STAND2").unwrap() as i32);
        assert_ne!(def.styles_learned & (1 << SS_TAVION), 0);
        assert_ne!(def.styles_forbidden & (1 << SS_STRONG), 0);
        assert_ne!(def.saber_flags & SFL_NO_CARTWHEELS, 0);
        assert_ne!(def.saber_flags & SFL_NO_ROLLS, 0);
        assert_ne!(def.saber_flags & SFL_RETURN_DAMAGE, 0);
        assert_eq!(def.num_blades, 2);
        assert_eq!(def.blade(0).length, 40.0);
        assert_eq!(def.blade(1).length, 28.0);
        assert_eq!(def.blade(0).radius, 3.5);
        assert_eq!(def.blade_style2_start, 1);
        assert_eq!((def.trail_style, def.trail_style2), (1, 2));
        assert!(def.no_dlight);
        assert!(def.return_damage);
        assert!(!defs.get("defaultish").unwrap().return_damage);
        assert!(!defs.get("defaultish").unwrap().no_dlight);
        assert_eq!(def.sound_loop, "sound/weapons/saber/saberhum4.wav");
        assert_eq!(def.swing_sounds[0].as_deref(), Some("sound/weapons/custom/swing1.wav"));
        assert_eq!(def.swing_sounds[1].as_deref(), Some("sound/weapons/custom/swing2.wav"));
        assert!(def.swing_sounds[2].is_none());
        assert_eq!(def.hit_sounds[0].as_deref(), Some("sound/weapons/custom/hit1.wav"));
        assert_eq!(def.hit2_sounds[1].as_deref(), Some("sound/weapons/custom/hit2_2.wav"));
        assert_eq!(def.block_sounds[2].as_deref(), Some("sound/weapons/custom/block3.wav"));
        assert_eq!(def.block2_sounds[0].as_deref(), Some("sound/weapons/custom/block2_1.wav"));
        assert_eq!(def.block_effect.as_deref(), Some("saber/custom_block.efx"));
        assert_eq!(def.block_effect2.as_deref(), Some("saber/custom_block2.efx"));
        assert_eq!(def.hit_person_effect.as_deref(), Some("saber/custom_person.efx"));
        assert_eq!(def.hit_person_effect2.as_deref(), Some("saber/custom_person2.efx"));
        assert_eq!(def.hit_other_effect.as_deref(), Some("saber/custom_other.efx"));
        assert_eq!(def.hit_other_effect2.as_deref(), Some("saber/custom_other2.efx"));
        assert!(def.no_clash_flare);
        assert!(def.no_clash_flare2);
        // File order: saberColor after saberColor2 resets it, the later one wins.
        assert_eq!((def.blade(0).color, def.blade(1).color), (SABER_PURPLE, 2));
        assert_eq!(defs.get("defaultish").unwrap().blade(0).color, SABER_RED);
        assert_eq!(defs.get("defaultish").unwrap().sound_loop, "sound/weapons/saber/saberhum3.wav");
    }

    fn equip_fixture() -> SaberDefinitions {
        let mut scales = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        parse_saber_file(
            br#"
                Kyle { saberModel models/weapons2/saber/saber_w.glm }
                single_a { saberModel models/weapons2/saber_a/saber_w.glm }
                staff_a { saberModel models/weapons2/staff/saber_w.glm numBlades 2 twoHanded 1 }
                staff_free { saberModel models/weapons2/staff/saber_w.glm numBlades 2 }
                secret { saberModel models/weapons2/secret/saber_w.glm notInMP 1 }
            "#,
            &mut scales,
            &mut definitions,
        )
        .unwrap();
        SaberDefinitions { definitions }
    }

    #[test]
    fn equip_follows_wp_setsaber_slot_rules() {
        let defs = equip_fixture();
        // Plain dual sabers.
        assert_eq!(defs.equipped_slots(["single_a", "single_a"], true), [true, true]);
        // "none"/"remove"/empty never produce a second saber.
        for second in ["none", "REMOVE", ""] {
            assert_eq!(defs.equipped_slots(["single_a", second], true), [true, false]);
        }
        // A two-handed saber cannot be the second saber, nor coexist with one.
        assert_eq!(defs.equipped_slots(["single_a", "staff_a"], true), [true, false]);
        assert_eq!(defs.equipped_slots(["staff_a", "single_a"], true), [true, false]);
        assert_eq!(defs.equipped_slots(["staff_a", "none"], true), [true, false]);
        // A staff that is not flagged two-handed may be paired.
        assert_eq!(defs.equipped_slots(["staff_free", "single_a"], true), [true, true]);
    }

    #[test]
    fn equip_substitutes_default_saber_for_not_in_mp_selections() {
        let defs = equip_fixture();
        let [player, _] = defs.equip(["secret", "none"], true);
        assert_eq!(player.unwrap().model, "models/weapons2/saber/saber_w.glm");
        // NPCs (entNum >= MAX_CLIENTS) keep the authored definition.
        let [npc, _] = defs.equip(["secret", "none"], false);
        assert_eq!(npc.unwrap().model, "models/weapons2/secret/saber_w.glm");
    }
}
