//! Built-in presentation edits for the main-menu background scene.
//!
//! These run inside map preparation (gated by `MapPrepareOptions::menu_look`)
//! rather than as a runtime `/remapShader`: the native surfaceSprites and Godot
//! grass emitters are baked from the BSP's *own* materials at prepare time, so a
//! post-hoc remap would swap the ground texture but never grow any blades. A
//! substitution at the point where each BSP shader is resolved gives the grass
//! shader's sprite stage exactly as if the map had been compiled with it.
//!
//! Props (`MapPrepareOptions::menu_props`) come from `mapmods/mp/duel9.ent`, the
//! file the in-game Map Mods editor exports, when it exists; otherwise from the
//! built-in layout below. `menudevmap` + `menuent` let that file be edited in
//! game.
use super::material_batches::append_material_batches;
use super::{CullMode, DrawBatch, GpuVertex, MapFxRunner, SurfaceMaterial};
use jka_assets::bsp::{Bsp, StaticModel, StaticModels};
use std::path::Path;

/// `mp/duel9` (Hoth Canyon): the only map with a menu look so far.
fn is_duel9(map: &str) -> bool {
    let map = map.trim().replace('\\', "/").to_ascii_lowercase();
    let map = map.strip_prefix("maps/").unwrap_or(&map);
    let map = map.strip_suffix(".bsp").unwrap_or(map);
    map == "mp/duel9"
}

/// Replacement for a BSP shader (already lower-cased), or `None` to keep it.
/// The Hoth materials become Yavin ones, each kept distinct: snow and ground
/// become grass ground (the only swaps that grow sprites), the cliff faces and
/// trims become different Yavin rocks, and the dirt/rock ground variant becomes
/// dug dirt. Lightmapped originals map to lightmapped targets (a bare image
/// name is an implicit `$lightmap * image` material) and vertex-lit originals
/// to vertex-lit targets, so each surface keeps the lighting it was baked for.
pub(in crate::scene) fn material_swap(map: &str, shader: &str) -> Option<&'static str> {
    if !is_duel9(map) {
        return None;
    }
    match shader {
        // Vertex-lit snow.
        "textures/hoth/snowbiga" | "textures/hoth/snow_01_clip" => {
            Some("textures/yavin/ground_grasssprite_phong_vertex")
        }
        // Lightmapped snowy ground: grass.
        "textures/mp/s_outground_phong" | "textures/mp/s_outground_nophong" => {
            Some("textures/yavin/ground_grasssprite")
        }
        // The second ground variant: dug dirt.
        "textures/mp/s_outground_3_phong" => Some("textures/yavin/dugdirt"),
        // Vertex-lit rock (clip-model ground).
        "textures/hoth/h_outground_3_clip" => Some("textures/yavin/stone_vertex"),
        // Cliff faces and their trim.
        "textures/mp/s_cliffwall1" | "textures/mp/s_cliffwall1_nophong" => {
            Some("textures/yavin/rock3")
        }
        "textures/mp/s_cliffwall1top" | "textures/mp/s_cliffwall1top_nophong" => {
            Some("textures/yavin/rockmossy")
        }
        _ => None,
    }
}

/// Fixed staging for the menu scene: where the camera sits and what the
/// foreground player is doing. All positions are native JKA coordinates (the
/// numbers `viewpos` prints) and angles are JKA degrees.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MenuStage {
    pub camera: [f32; 3],
    pub camera_yaw: f32,
    /// As printed by `viewpos` (positive looks down in JKA's sense of the
    /// word, i.e. `viewpos` shows the negated render pitch).
    pub camera_pitch: f32,
    /// Player entity origin at the feet.
    pub player_feet: [f32; 3],
    pub player_yaw: f32,
    /// `anims.h` name, e.g. `BOTH_STAND1`, or `BOTH_SIT1` for jaPRO's `/amsit`.
    pub player_anim: &'static str,
    /// Stage the foreground player at all.
    pub player: bool,
    /// The slow idle drift of the camera. Off where the main page pins
    /// screen-space elements to scene features (the ctf4 TV).
    pub camera_sway: bool,
}

const DEFAULT_STAGE: MenuStage = MenuStage {
    camera: [-225.0, -433.0, 261.0],
    camera_yaw: 112.0,
    camera_pitch: -3.0,
    // Near viewpos (-345 -369 266), with the standing view height (36)
    // subtracted for the entity origin. Nudge toward the pulled-back camera
    // and slightly farther left in its view; face east toward the camera.
    player_feet: [-317.0, -406.0, 230.0],
    player_yaw: 0.0,
    player_anim: "BOTH_STAND1",
    player: true,
    camera_sway: true,
};

/// `mp/ctf4`: looking down the street at the wall screen on the right, which
/// the main page covers with its CRT. Camera = `viewpos` (3789 -2177 -240) 325, pitch -5.
const CTF4_STAGE: MenuStage = MenuStage {
    camera: [3789.0, -2177.0, -240.0],
    camera_yaw: 325.0,
    camera_pitch: -5.0,
    player_feet: [0.0; 3],
    player_yaw: 0.0,
    player_anim: "BOTH_STAND1",
    player: false,
    camera_sway: false,
};

fn map_key(map: &str) -> String {
    let map = map.trim().replace('\\', "/").to_ascii_lowercase();
    let map = map.strip_prefix("maps/").unwrap_or(&map);
    map.strip_suffix(".bsp").unwrap_or(map).to_owned()
}

pub(crate) fn menu_stage(map: &str) -> Option<MenuStage> {
    if is_duel9(map) {
        return Some(DEFAULT_STAGE);
    }
    (map_key(map) == "mp/ctf4").then_some(CTF4_STAGE)
}

const WATER_HEIGHT: f32 = 186.0;

/// Nothing is planted below the water (JKA z / render y), with a little margin
/// so the shoreline row of blades does not stand half in the surface.
pub(in crate::scene) fn planting_floor(map: &str) -> Option<f32> {
    is_duel9(map).then_some(WATER_HEIGHT + 4.0)
}

// ---------------------------------------------------------------------------
// Water
// ---------------------------------------------------------------------------

/// Stock water shader. It is an ordinary `surfaceparm water` material, so the
/// normal promotion path turns it into the user's selected ocean and, with the
/// ocean off, it is plain stock JKA water (an authored-ocean marker would draw
/// as a black sheet then).
pub(in crate::scene) const WATER_SHADER: &str = "textures/common/water_yavin2";

pub(in crate::scene) fn is_menu_map(map: &str) -> bool {
    is_duel9(map)
}

pub(in crate::scene) struct MenuScenery {
    pub water: SurfaceMaterial,
}

/// JKA position/direction -> render space (x, jka z up, -jka y).
fn render(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

/// The water sheet over the whole world as ordinary draw batches (coarse and
/// FULL lists, no PVS restriction). Wherever terrain rises above the water it
/// hides it, so the flooded area follows the map's own contours. Culling is off
/// so no winding convention can hide the sheet.
pub(in crate::scene) fn append_scenery(
    scenery: &MenuScenery,
    vertices: &mut Vec<GpuVertex>,
    batches: &mut Vec<DrawBatch>,
    pvs_batches: &mut Vec<DrawBatch>,
) {
    let (wx, wy) = ([-3136.0, 5440.0], [-2112.0, 5184.0]);
    let corners = [
        [wx[0], wy[0], WATER_HEIGHT],
        [wx[1], wy[0], WATER_HEIGHT],
        [wx[1], wy[1], WATER_HEIGHT],
        [wx[0], wy[1], WATER_HEIGHT],
    ];
    let first = vertices.len() as u32;
    for corner in [0, 1, 2, 0, 2, 3] {
        let p = corners[corner];
        vertices.push(GpuVertex {
            position: render(p),
            // Stock water tiles its texture (tcMod scale on top of this).
            uv: [p[0] / 256.0, p[1] / 256.0],
            lightmap_uv: [0.0; 2],
            normal: render([0.0, 0.0, 1.0]),
            color: [1.0; 4],
            alpha_cutoff: 1.0,
        });
    }
    let range = first..vertices.len() as u32;
    for list in [batches, pvs_batches] {
        let start = list.len();
        append_material_batches(
            list, &scenery.water,
            // No BSP shader entry; u32::MAX is the existing "absent" value.
            usize::MAX, None,
            true, range.clone(), None,
            &[], [0; 4], [0.0; 4], false,
        );
        for batch in &mut list[start..] {
            batch.pipeline.cull = CullMode::None;
        }
    }
}

// ---------------------------------------------------------------------------
// Props and fx
// ---------------------------------------------------------------------------

/// One prop placement (JKA coordinates; angles are pitch/yaw/roll).
#[derive(Debug, Clone)]
pub(crate) struct MenuProp {
    pub model: &'static str,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub scale: f32,
}

/// One effect placement, as an `fx_runner` (effect path as in the map's
/// `fxFile`, direction as JKA angles).
#[derive(Debug, Clone)]
pub(crate) struct MenuFx {
    pub effect: &'static str,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub delay_ms: i32,
}

const TREE05: &str = "models/map_objects/yavin/tree05_b.md3";
const TREE02: &str = "models/map_objects/yavin/tree02_b.md3";
const GRASS: &str = "models/map_objects/yavin/grass_b.md3";
const PLANT: &str = "models/map_objects/yavin/plant_b.md3";
const FERN: &str = "models/map_objects/yavin/fern3_b.md3";
const ROCK: &str = "models/map_objects/desert/rock.md3";
const LOG: &str = "models/map_objects/yavin/trunk_log.md3";

/// The curated menu layout (exported from the in-game Map Mods editor): yavin2
/// trees, grass, plants, ferns and rocks, and the logs around the campfire.
/// Used when `mapmods/mp/duel9.ent` does not exist, and by `menuent`.
const BUILTIN_PROPS: &[(&str, [f32; 3], [f32; 3], f32)] = &[
    (LOG, [-326.0, -324.0, 192.0], [0.0, 70.0, 0.0], 0.3),
    (FERN, [-977.4757, 188.248, 517.8253], [0.0, 114.8386, 0.0], 1.0822),
    (GRASS, [-1296.9946, 1662.4944, 429.5367], [0.0, 214.4203, 0.0], 1.1989),
    (ROCK, [-1858.4043, 1690.8975, 624.418], [0.0, 295.9254, 0.0], 0.5177),
    (GRASS, [-389.3021, -1657.5757, 728.1432], [0.0, 287.7609, 0.0], 0.9974),
    (GRASS, [698.2642, -333.7266, 320.0], [0.0, 145.5082, 0.0], 0.9547),
    (PLANT, [-1455.7396, 1520.7114, 656.0591], [0.0, 89.3455, 0.0], 0.9752),
    (TREE02, [-1398.9655, 1271.3411, 974.8271], [0.0, 350.9404, 0.0], 0.7689),
    (GRASS, [2070.126, 1708.5488, 1466.0177], [0.0, 256.1651, 0.0], 1.1061),
    (ROCK, [-1418.2179, 443.7532, 771.8023], [0.0, 328.4879, 0.0], 0.8133),
    (GRASS, [1356.5132, 1420.8594, 500.349], [0.0, 5.8705, 0.0], 1.2264),
    (TREE05, [-1392.4959, 1748.2214, 664.9713], [0.0, 103.5568, 0.0], 0.7303),
    (GRASS, [-1331.6407, 921.3281, 831.4039], [0.0, 129.2951, 0.0], 1.2115),
    (FERN, [1334.9556, 1356.6047, 477.9048], [0.0, 334.8186, 0.0], 1.0631),
    (GRASS, [-1489.4121, 561.5637, 778.9774], [0.0, 321.4156, 0.0], 0.8978),
    (PLANT, [-1292.2502, 1563.3943, 462.4465], [0.0, 242.7356, 0.0], 0.7468),
    (GRASS, [-1448.1729, 1725.1018, 491.8295], [0.0, 49.4592, 0.0], 0.873),
    (FERN, [-2026.8932, -203.5435, 1226.3326], [0.0, 178.8236, 0.0], 1.105),
    (GRASS, [-1548.7134, 1215.6377, 786.5456], [0.0, 95.722, 0.0], 0.9565),
    (ROCK, [515.5386, -51.3989, 330.6946], [0.0, 158.1238, 0.0], 0.4278),
    (FERN, [-1528.9916, -4.4292, 728.5536], [0.0, 322.1726, 0.0], 1.0091),
    (FERN, [-2016.6914, -558.6979, 1042.6167], [0.0, 154.6395, 0.0], 0.8792),
    (FERN, [-1121.4844, 829.032, 803.0378], [0.0, 306.7481, 0.0], 1.021),
    (PLANT, [977.3755, -282.6024, 929.1266], [0.0, 287.8957, 0.0], 0.7524),
    (GRASS, [-975.4498, 407.3726, 521.0124], [0.0, 240.4462, 0.0], 0.8108),
    (ROCK, [-1130.4618, 1307.3557, 401.5047], [0.0, 325.5838, 0.0], 0.4968),
    (FERN, [481.1958, -10.9543, 320.0], [0.0, 348.7163, 0.0], 0.9125),
    (GRASS, [365.1243, 103.8861, 320.0], [0.0, 69.8448, 0.0], 0.9057),
    (GRASS, [-1451.1068, 361.3618, 741.1702], [0.0, 219.5177, 0.0], 1.1897),
    (ROCK, [-1406.9749, 1071.6841, 838.1708], [0.0, 270.4824, 0.0], 0.7434),
    (PLANT, [-977.2565, 268.9504, 525.063], [0.0, 285.4269, 0.0], 0.7311),
    (GRASS, [-985.0923, -51.6274, 524.5461], [0.0, 301.0617, 0.0], 1.1711),
    (GRASS, [-1058.6061, -5.2412, 561.303], [0.0, 277.8042, 0.0], 0.8005),
    (PLANT, [135.325, -319.1486, 213.0123], [0.0, 123.344, 0.0], 0.7937),
    (FERN, [708.1873, -125.6235, 320.0], [0.0, 47.0497, 0.0], 0.8997),
    (GRASS, [-1640.3873, 1831.6135, 781.7138], [0.0, 111.0354, 0.0], 1.1646),
    (ROCK, [-244.7354, -1804.1167, 684.3788], [0.0, 230.9198, 0.0], 0.5324),
    (GRASS, [-359.2214, -303.5624, 192.0], [0.0, 254.1127, 0.0], 0.9909),
    (FERN, [792.8831, 466.3, 832.0], [0.0, 171.4394, 0.0], 0.85),
    (PLANT, [-1720.0631, 882.4788, 1118.3257], [0.0, 25.097, 0.0], 0.7739),
    (GRASS, [1124.9338, 857.8635, 832.0], [0.0, 349.6065, 0.0], 0.9112),
    (GRASS, [-2174.1436, 213.9326, 1441.7601], [0.0, 162.8971, 0.0], 1.2151),
    (FERN, [-1569.6111, 1155.1543, 815.1176], [0.0, 302.2366, 0.0], 0.9936),
    (GRASS, [-367.2722, -1843.3765, 780.3352], [0.0, 46.951, 0.0], 1.2345),
    (TREE05, [-789.1145, -219.0386, 372.0429], [0.0, 33.8115, 0.0], 0.7116),
    (PLANT, [-1511.9989, -109.3864, 756.0475], [0.0, 271.4509, 0.0], 0.8701),
    (PLANT, [-1593.5281, 527.875, 771.4772], [0.0, 263.4354, 0.0], 0.9493),
    (LOG, [-227.0234, -257.0819, 192.125], [0.0, 90.2, 23.2001], 0.1321),
    (LOG, [-233.9116, -261.325, 192.125], [0.0, 139.5992, 335.999], 0.1438),
    (LOG, [-234.9481, -269.8996, 192.125], [0.0, 48.4004, 30.2], 0.1321),
    (LOG, [-248.6525, -261.4027, 192.125], [13.6, 1.4, 328.5995], 0.1438),
    (TREE05, [504.6921, 62.5616, 518.2314], [0.0, 80.0721, 0.0], 0.7835),
];

/// Campfire and waterfall (yavin2's `env/small_fire` torch and its
/// `world/waterfall3.efx`, the latter flowing +x on the duel9 canyon wall).
const BUILTIN_FX: &[(&str, [f32; 3], [f32; 3], i32)] = &[
    ("env/small_fire", [-231.3878, -263.5522, 192.125], [-90.0, 0.0, 0.0], 200),
    ("world/waterfall3.efx", [-1030.9039, 2794.5796, 827.9846], [0.0, 0.0, 0.0], 50),
];

/// The built-in props (used by `menuent`).
pub(crate) fn default_prop_placements() -> Vec<MenuProp> {
    BUILTIN_PROPS
        .iter()
        .map(|&(model, origin, angles, scale)| MenuProp { model, origin, angles, scale })
        .collect()
}

/// The built-in effects (used by `menuent`).
pub(crate) fn default_fx_placements() -> Vec<MenuFx> {
    BUILTIN_FX
        .iter()
        .map(|&(effect, origin, angles, delay_ms)| MenuFx { effect, origin, angles, delay_ms })
        .collect()
}

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let n: Vec<f32> = value.split_whitespace().filter_map(|word| word.parse().ok()).collect();
    (n.len() >= 3).then(|| [n[0], n[1], n[2]])
}

/// The scene as edited in game: `misc_model_static` and `fx_runner` entities
/// from the Map Mods export for this map (`mapmods/mp/duel9.ent`, in the game
/// dir or base), or `None` if there is no usable file.
fn scene_from_ent(
    root: &Path,
    game: Option<&Path>,
    bsp: &Bsp,
) -> Option<(Vec<StaticModel>, Vec<MapFxRunner>)> {
    let path = game.unwrap_or(root).join("mapmods").join("mp").join("duel9.ent");
    let text = std::fs::read_to_string(&path).ok()?;
    let document = match jka_assets::map::parse(&text) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("MENU SCENE: {} is not a valid entity file ({error}); using the built-in scene", path.display());
            return None;
        }
    };
    let mut placements = Vec::new();
    let mut runners = Vec::new();
    for entity in &document.entities {
        let key = |name: &str| {
            entity
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        let Some(class) = key("classname") else { continue };
        let origin = key("origin").and_then(parse_vec3).unwrap_or([0.0; 3]);
        if class.eq_ignore_ascii_case("misc_model_static") {
            let Some(model) = key("model").map(str::trim).filter(|m| !m.is_empty()) else { continue };
            let angles = key("angles").and_then(parse_vec3).unwrap_or_else(|| {
                [0.0, key("angle").and_then(|v| v.trim().parse().ok()).unwrap_or(0.0), 0.0]
            });
            let scale = key("modelscale_vec").and_then(parse_vec3).unwrap_or_else(|| {
                [key("modelscale").and_then(|v| v.trim().parse().ok()).unwrap_or(1.0); 3]
            });
            let zoffset = key("zoffset").and_then(|v| v.trim().parse().ok()).unwrap_or(0.0);
            if !origin.iter().chain(&angles).chain(&scale).all(|value| value.is_finite()) {
                continue;
            }
            let cull_origin = [origin[0], origin[1], origin[2] + 1.0 + zoffset];
            placements.push(StaticModel {
                model: model.to_owned(),
                origin,
                angles,
                scale,
                zoffset,
                cull_cluster: bsp.visibility.as_ref().and_then(|vis| vis.cluster_at(cull_origin)),
            });
        } else if class.eq_ignore_ascii_case("fx_runner") {
            let Some(effect) = key("fxfile").map(str::trim).filter(|e| !e.is_empty()) else { continue };
            // No aim means straight up, as OpenJK's SP_fx_runner.
            let angles = key("angles").and_then(parse_vec3).unwrap_or_else(|| {
                match key("angle").and_then(|v| v.trim().parse::<f32>().ok()) {
                    Some(yaw) => [0.0, yaw, 0.0],
                    None => [-90.0, 0.0, 0.0],
                }
            });
            runners.push(MapFxRunner {
                bake_static: false,
                source_entity_index: usize::MAX,
                effect: effect.replace('\\', "/"),
                origin,
                angles,
                delay_ms: key("delay").and_then(|v| v.trim().parse().ok()).unwrap_or(200),
                random_ms: key("random").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0) as i32,
                spawnflags: 0,
            });
        }
    }
    println!(
        "MENU SCENE: {} prop(s), {} fx from {}",
        placements.len(),
        runners.len(),
        path.display()
    );
    Some((placements, runners))
}

/// Adds the menu scene's props and effects (`props` is the menu itself; the
/// `menudevmap` session leaves them out so they can be staged as editable
/// placements instead). They come from the Map Mods `.ent` when it exists
/// (effects fall back to the built-in ones if the file has none), else from the
/// built-in layout.
pub(in crate::scene) fn apply_scene(
    root: &Path,
    game: Option<&Path>,
    map: &str,
    props: bool,
    bsp: &Bsp,
    models: &mut StaticModels,
    fx_runners: &mut Vec<MapFxRunner>,
) {
    if !is_duel9(map) || !props {
        return;
    }
    let from_file = scene_from_ent(root, game, bsp);
    let (placements, runners) = match from_file {
        Some((placements, runners)) => (Some(placements), runners),
        None => (None, Vec::new()),
    };
    if let Some(placements) = placements {
        models.placements.extend(placements);
    } else {
        for prop in default_prop_placements() {
            let cull_origin = [prop.origin[0], prop.origin[1], prop.origin[2] + 1.0];
            models.placements.push(StaticModel {
                model: prop.model.to_owned(),
                origin: prop.origin,
                angles: prop.angles,
                scale: [prop.scale; 3],
                zoffset: 0.0,
                cull_cluster: bsp.visibility.as_ref().and_then(|vis| vis.cluster_at(cull_origin)),
            });
        }
    }
    if runners.is_empty() {
        fx_runners.extend(default_fx_placements().into_iter().map(|fx| MapFxRunner {
                bake_static: false,
            source_entity_index: usize::MAX,
            effect: fx.effect.to_owned(),
            origin: fx.origin,
            angles: fx.angles,
            delay_ms: fx.delay_ms,
            random_ms: 0,
            spawnflags: 0,
        }));
    } else {
        fx_runners.extend(runners);
    }
}
