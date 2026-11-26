//! Session-local console placements and independent EFX fire and smoke promotion.
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

use jka_ninja::{FLAME_BODY_JOINTS, FlamePreset};
use crate::fx::fire_geometry::{FlameGeometry, SmokeMotion};

pub(crate) const MAX_EMITTERS: usize = 4;
/// Limit separately simulated EFX fields, selecting the nearest eligible sources.
const MAX_PROMOTED_EMITTERS: usize = 4;
const MAX_PROMOTED_SMOKE_EMITTERS: usize = 2;
pub(crate) const PROMOTED_SMOKE_PRESET: FlamePreset = FlamePreset::SmokingMan;
const PROMOTION_SOURCE_TIMEOUT_MS: i32 = 250;
/// Squared-distance factor for an already promoted source (0.75 in distance:
/// a challenger must be about a third closer to take its place).
const SELECTED_RANK_BONUS: f32 = 0.5625;
pub(crate) const CM_PER_MAP_UNIT: f32 = 2.54;
pub(crate) const PLAYER_FIRE_DRAIN_MS: i32 = 2000;

/// Render-axis, emitter-relative footprint of the authored flame sprites while
/// they are visible (see `fx::fire_geometry`). Calculated from parsed EFX
/// primitive ranges, not from screenshot dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlameFit {
    pub min: glam::Vec3,
    pub max: glam::Vec3,
    /// Smoke travel data, with the launch position converted to render axes.
    pub smoke: Option<SmokeMotion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmitterSource { Manual, Promoted, PromotedSmoke { emitting: bool } }

/// Which Fire_Smoke_Explosive pawn setup `ninjafireon` applies to the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PlayerEffect {
    /// Stage 13 Burning Man 2.
    #[default]
    BurningMan,
    /// Stage 11 Flaming Fist setup B: blue smoke from the left hand.
    SmokingFist,
    /// Stage 14 Burning Man 3 (demo key 3): flame field plus a volume smoke field.
    BurningMan3,
    /// Stage 15 Smoking Man (demo key 2): volume smoke only.
    SmokingMan,
    /// Smoking Man that also smokes while standing still (DinurdoJK option).
    SmokingManConstant,
    /// Burning Man 3 whose smoke also emits while standing still.
    BurningMan3Constant,
}

impl PlayerEffect {
    pub(crate) fn presets(self) -> &'static [FlamePreset] {
        match self {
            Self::BurningMan => &[FlamePreset::BurningMan],
            Self::SmokingFist => &[FlamePreset::SmokingFist],
            Self::BurningMan3 | Self::BurningMan3Constant => &[FlamePreset::BurningMan3, FlamePreset::BurningMan3Smoke],
            Self::SmokingMan | Self::SmokingManConstant => &[FlamePreset::SmokingMan],
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::BurningMan => "Burning Man",
            Self::SmokingFist => "Flaming Fist B (blue smoke)",
            Self::BurningMan3 => "Burning Man 3 (fire and smoke)",
            Self::SmokingMan => "Smoking Man",
            Self::SmokingManConstant => "Smoking Man (smokes at rest)",
            Self::BurningMan3Constant => "Burning + Smoking Man (smokes at rest)",
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::SmokingFist,
            2 => Self::BurningMan3,
            3 => Self::SmokingMan,
            4 => Self::SmokingManConstant,
            5 => Self::BurningMan3Constant,
            _ => Self::BurningMan,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Emitter {
    pub id: u64,
    pub origin: glam::Vec3,
    pub preset: FlamePreset,
    pub source: EmitterSource,
    pub fit: Option<FlameFit>,
}

#[derive(Clone, Copy)]
pub(crate) struct PlayerPose {
    pub id: u64,
    pub time_ms: i32,
    pub origin: glam::Vec3,
    pub forward: glam::Vec3,
    pub velocity: glam::Vec3,
    pub emitting: bool,
    pub suppress_wind: bool,
    pub joints: [Option<glam::Vec3>; FLAME_BODY_JOINTS],
    pub scale: f32,
}

#[derive(Clone, Default)]
pub(crate) struct FireScene {
    pub placed: Option<Arc<Vec<Emitter>>>,
    pub player: Option<PlayerPose>,
    pub player_effect: PlayerEffect,
    pub world_brushes: Option<Arc<[WorldBrush]>>,
    /// The same brushes for turret-ember collision, independent of the
    /// MeshField collision toggle.
    pub ember_brushes: Option<Arc<[WorldBrush]>>,
    /// (ET_FX entity number, stable simulation ID) for renderer ACK.
    pub promoted: Vec<(u16, u64)>,
    pub promoted_smoke: Vec<(u16, u64)>,
}

pub(crate) struct WorldBrush {
    pub mins: glam::Vec3,
    pub maxs: glam::Vec3,
    pub planes: Vec<(glam::Vec3, f32)>,
}

#[derive(Clone, Copy)]
struct PromotedRunner {
    id: u64,
    origin: glam::Vec3,
    preset: FlamePreset,
    fit: FlameFit,
    cull_range_sq: f32,
    /// Which effect definition and orientation `fit` was computed from.
    effect_hash: u64,
    axis: [[f32; 3]; 3],
    last_seen_ms: i32,
    /// In the set of simulated sources at the last selection.
    selected: bool,
}

struct PlacementState {
    map: String,
    emitters: Option<Arc<Vec<Emitter>>>,
    stopped_player: Option<PlayerPose>,
    next_id: u64,
    world_brushes: Option<(String, Arc<[WorldBrush]>)>,
    promoted: HashMap<u16, PromotedRunner>,
    promoted_smoke: HashMap<u16, PromotedRunner>,
}

static PROMOTE: AtomicBool = AtomicBool::new(false);
static PROMOTE_SMOKE: AtomicBool = AtomicBool::new(false);
/// Registered EFX geometry is produced by the FX worker at parse time. A
/// missing/rejected profile means stock sprites stay active until supported.
static FIRE_GEOMETRY: LazyLock<Mutex<HashMap<String, Arc<FlameGeometry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SMOKE_GEOMETRY: LazyLock<Mutex<HashMap<String, Arc<FlameGeometry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Renderer-published availability. Original sprites remain until a replacement
/// has successfully allocated and encoded at least once.
static READY_PROMOTED: LazyLock<Mutex<HashSet<u64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static READY_PROMOTED_SMOKE: LazyLock<Mutex<HashSet<u64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static COLLISION: AtomicBool = AtomicBool::new(true);
static FLICKER: AtomicBool = AtomicBool::new(true);
static AIRFLOW_RESPONSE: AtomicBool = AtomicBool::new(false);
static EMBERS: AtomicBool = AtomicBool::new(false);
static HEAT_HAZE: AtomicBool = AtomicBool::new(false);
static PLAYER_ID: AtomicU64 = AtomicU64::new(0);
static PLAYER_EFFECT: AtomicU8 = AtomicU8::new(0);

pub(crate) fn player_effect() -> PlayerEffect {
    PlayerEffect::from_index(PLAYER_EFFECT.load(Ordering::Relaxed))
}
/// f32 bits; 1.0 = the preset's authored smoke persistence.
static SMOKE_LIFETIME: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3F80_0000);

/// Density persistence multiplier for promoted EFX smoke.
pub(crate) fn smoke_lifetime() -> f32 {
    f32::from_bits(SMOKE_LIFETIME.load(Ordering::Relaxed))
}

pub(crate) fn set_smoke_lifetime(scale: f32) {
    if scale.is_finite() && scale > 0.0 {
        SMOKE_LIFETIME.store(scale.to_bits(), Ordering::Relaxed);
    }
}
static PLACEMENTS: LazyLock<Mutex<PlacementState>> = LazyLock::new(|| Mutex::new(PlacementState {
    map: String::new(),
    emitters: None,
    stopped_player: None,
    next_id: 0,
    world_brushes: None,
    promoted: HashMap::new(),
    promoted_smoke: HashMap::new(),
}));

/// Promotion is opt-in, independent of manually spawned NinjaFire emitters.
pub(crate) fn promote() -> bool {
    PROMOTE.load(Ordering::Relaxed)
}

pub(crate) fn set_promote(value: bool) {
    PROMOTE.store(value, Ordering::Relaxed);
    if !value {
        PLACEMENTS.lock().unwrap_or_else(|poison| poison.into_inner()).promoted.clear();
        publish_ready_promoted(std::iter::empty());
    }
}

pub(crate) fn promote_smoke() -> bool {
    PROMOTE_SMOKE.load(Ordering::Relaxed)
}

pub(crate) fn set_promote_smoke(value: bool) {
    PROMOTE_SMOKE.store(value, Ordering::Relaxed);
    if !value {
        PLACEMENTS.lock().unwrap_or_else(|p| p.into_inner()).promoted_smoke.clear();
        publish_ready_smoke(std::iter::empty());
    }
}

pub(crate) fn promotion_enabled() -> bool {
    promote() || promote_smoke()
}

/// This allowlist is about *effect semantics*, not calibration: only
/// continuous upright orange ET_FX sources are currently replaceable by the
/// available Ninja presets. Unknown/directional/colored effects stay stock.
fn fire_preset(path: &str) -> Option<FlamePreset> {
    match path {
        "env/fire" | "ships/fire" => Some(FlamePreset::Bonfire),
        "env/small_fire" | "env/small_fire_lod" => Some(FlamePreset::Torch),
        _ => None,
    }
}

fn canonical_effect_name(path: &str) -> String {
    let value = path.trim().replace('\\', "/").to_ascii_lowercase();
    let value = value.strip_prefix("effects/").unwrap_or(&value);
    value.strip_suffix(".efx").unwrap_or(value).to_owned()
}

pub(crate) fn promotion_candidate(name: &str) -> bool {
    fire_preset(&canonical_effect_name(name)).is_some()
}

/// Called once on the FX worker when its actual (possibly mod-overridden)
/// .efx definition is parsed. Unsupported definitions never get a fit.
pub(crate) fn register_effect_geometry(name: &str, geometry: Option<FlameGeometry>) {
    let mut profiles = FIRE_GEOMETRY.lock().unwrap_or_else(|p| p.into_inner());
    let key = canonical_effect_name(name);
    if let Some(geometry) = geometry { profiles.insert(key, Arc::new(geometry)); }
    else { profiles.remove(&key); }
}

pub(crate) fn register_smoke_geometry(name: &str, geometry: FlameGeometry) {
    SMOKE_GEOMETRY.lock().unwrap_or_else(|p| p.into_inner())
        .insert(canonical_effect_name(name), Arc::new(geometry));
}

pub(crate) fn clear_effect_geometry() {
    FIRE_GEOMETRY.lock().unwrap_or_else(|p| p.into_inner()).clear();
    SMOKE_GEOMETRY.lock().unwrap_or_else(|p| p.into_inner()).clear();
    // Never let an old GPU field suppress sprites from a new FX session.
    let mut state = PLACEMENTS.lock().unwrap_or_else(|p| p.into_inner());
    state.promoted.clear();
    state.promoted_smoke.clear();
    publish_ready_promoted(std::iter::empty());
    publish_ready_smoke(std::iter::empty());
}

/// Convert all 8 corners of a JKA-coordinate envelope into render axes. This
/// is exact for the engine's orthonormal coordinate transform and preserves
/// authored positions regardless of map FX orientation.
fn rendered_fit(bounds: crate::fx::fire_geometry::FlameBounds) -> FlameFit {
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                let pos = glam::Vec3::from_array(crate::scene::render_position([x, y, z]));
                min = min.min(pos);
                max = max.max(pos);
            }
        }
    }
    let smoke = bounds.smoke.map(|mut motion| {
        motion.origin = crate::scene::render_position(motion.origin);
        motion
    });
    FlameFit { min, max, smoke }
}

pub(crate) fn observe_runner(
    entity: u16, effect: &str, origin: [f32; 3],
    axis: [[f32; 3]; 3], time_ms: i32, continuous: bool,
) {
    if !jka_ninja::INCLUDED || !promotion_enabled() || !origin.iter().all(|p| p.is_finite()) {
        return;
    }
    let key = canonical_effect_name(effect);
    let position = glam::Vec3::from_array(crate::scene::render_position(origin));
    let mut state = PLACEMENTS.lock().unwrap_or_else(|p| p.into_inner());
    let PlacementState { promoted, promoted_smoke, next_id, .. } = &mut *state;
    if let Some(preset) = promote().then(|| fire_preset(&key)).flatten() {
        observe_source(promoted, next_id, &FIRE_GEOMETRY, entity, &key, preset, position, axis, time_ms);
    }
    if promote_smoke() {
        if continuous {
            observe_source(promoted_smoke, next_id, &SMOKE_GEOMETRY, entity, &key,
                PROMOTED_SMOKE_PRESET, position, axis, time_ms);
        } else {
            promoted_smoke.remove(&entity);
        }
    }
}

fn observe_source(
    runners: &mut HashMap<u16, PromotedRunner>, next_id: &mut u64,
    profiles: &Mutex<HashMap<String, Arc<FlameGeometry>>>, entity: u16, key: &str,
    preset: FlamePreset, position: glam::Vec3, axis: [[f32; 3]; 3], time_ms: i32,
) {
    let effect_hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    };
    // Runners are stationary: the footprint depends only on the effect
    // definition and the runner's axes, so it is evaluated once per runner
    // and kept while both are unchanged (this runs every frame per runner).
    // `clear_effect_geometry` drops every entry when definitions are reset.
    if let Some(entry) = runners.get_mut(&entity)
        .filter(|entry| entry.effect_hash == effect_hash && entry.axis == axis)
    {
        // The same definition and orientation can reuse the fitted field.
        entry.origin = position;
        entry.last_seen_ms = time_ms;
        return;
    }
    let geometry = profiles.lock().unwrap_or_else(|p| p.into_inner()).get(key).cloned();
    let Some((fit, cull_range_sq)) = geometry.and_then(|geometry| {
        Some((rendered_fit(geometry.visible_bounds(axis)?), geometry.cull_range_sq))
    }) else {
        runners.remove(&entity);
        return;
    };
    let id = match runners.get(&entity) {
        Some(entry) if preset != PROMOTED_SMOKE_PRESET => entry.id,
        _ => {
            *next_id = next_id.wrapping_add(1).max(1);
            *next_id
        }
    };
    runners.insert(entity, PromotedRunner {
        id, origin: position, preset, fit, cull_range_sq,
        effect_hash, axis, last_seen_ms: time_ms, selected: false,
    });
}

/// The same ET_FX entity must be in the current promoted scene AND have a
/// successfully encoded GPU field before its *flame shaders only* are hidden.
pub(crate) fn ready_runner(entity: u16) -> bool {
    if !promote() { return false; }
    let id = PLACEMENTS.lock().unwrap_or_else(|poison| poison.into_inner())
        .promoted.get(&entity).map(|source| source.id);
    id.is_some_and(|id| READY_PROMOTED.lock()
        .unwrap_or_else(|poison| poison.into_inner()).contains(&id))
}

/// Drop acknowledgments for retired/replaced sources without also clearing
/// valid unchanged fields (which would cause repeated original-flame flashes).
pub(crate) fn retain_ready_promoted(scene_sources: &[(u16, u64)]) {
    READY_PROMOTED.lock().unwrap_or_else(|poison| poison.into_inner())
        .retain(|id| scene_sources.iter().any(|&(_, active)| *id == active));
}

pub(crate) fn publish_ready_promoted(ids: impl IntoIterator<Item = u64>) {
    let mut ready = READY_PROMOTED.lock().unwrap_or_else(|poison| poison.into_inner());
    ready.clear();
    if promote() { ready.extend(ids); }
}

pub(crate) fn retain_ready_smoke(scene_sources: &[(u16, u64)]) {
    READY_PROMOTED_SMOKE.lock().unwrap_or_else(|p| p.into_inner())
        .retain(|id| scene_sources.iter().any(|&(_, active)| *id == active));
}

pub(crate) fn publish_ready_smoke(ids: impl IntoIterator<Item = u64>) {
    let mut ready = READY_PROMOTED_SMOKE.lock().unwrap_or_else(|p| p.into_inner());
    ready.clear();
    if promote_smoke() { ready.extend(ids); }
}

pub(crate) fn suppresses_smoke(entity: u16, shader: &str) -> bool {
    if !promote_smoke() || !original_smoke_shader(shader) { return false; }
    let id = PLACEMENTS.lock().unwrap_or_else(|p| p.into_inner())
        .promoted_smoke.get(&entity).map(|source| source.id);
    id.is_some_and(|id| READY_PROMOTED_SMOKE.lock()
        .unwrap_or_else(|p| p.into_inner()).contains(&id))
}

pub(crate) fn original_smoke_shader(shader: &str) -> bool {
    [
        "gfx/effects/alpha_smoke", "gfx/effects/alpha_smoke2", "gfx/effects/smoke",
        "gfx/effects/jk_smoke5", "gfx/effects/bp_smoke01", "gfx/effects/bp_smoke02",
        "gfx/misc/black_smoke", "gfx/misc/black_smoke2",
        "gfx/misc/steam", "gfx/misc/steam2", "gfx/misc/steam3",
    ].iter().any(|name| shader.eq_ignore_ascii_case(name))
}

/// Exact authored flame sprite materials; smoke, sparks, sound and other
/// primitives in the same source EFX continue through the original renderer.
pub(crate) fn original_flame_shader(shader: &str) -> bool {
    matches!(shader.to_ascii_lowercase().as_str(),
        "gfx/effects/fire2" | "gfx/effects/fire3" | "gfx/effects/fire4")
}

pub(crate) fn collision() -> bool {
    COLLISION.load(Ordering::Relaxed)
}

pub(crate) fn set_collision(enabled: bool) {
    COLLISION.store(enabled, Ordering::Relaxed);
}

pub(crate) fn flicker() -> bool {
    FLICKER.load(Ordering::Relaxed)
}

pub(crate) fn set_flicker(enabled: bool) {
    FLICKER.store(enabled, Ordering::Relaxed);
}

pub(crate) fn airflow_response() -> bool {
    AIRFLOW_RESPONSE.load(Ordering::Relaxed)
}

pub(crate) fn set_airflow_response(enabled: bool) {
    AIRFLOW_RESPONSE.store(enabled, Ordering::Relaxed);
}

/// Fire_Smoke_Explosive turret embers on every Ninja fire source.
pub(crate) fn embers() -> bool {
    EMBERS.load(Ordering::Relaxed)
}

pub(crate) fn set_embers(enabled: bool) {
    EMBERS.store(enabled, Ordering::Relaxed);
}

/// Screen-space heat haze behind Ninja surface flames.
pub(crate) fn heat_haze() -> bool {
    HEAT_HAZE.load(Ordering::Relaxed)
}

pub(crate) fn set_heat_haze(enabled: bool) {
    HEAT_HAZE.store(enabled, Ordering::Relaxed);
}

/// Ember collision or MeshField collision needs the map's solid brushes.
pub(crate) fn brushes_wanted() -> bool {
    collision() || embers()
}

pub(crate) fn world_brushes_loaded(map: &str) -> bool {
    let state = PLACEMENTS.lock().unwrap_or_else(|poison| poison.into_inner());
    state.world_brushes.as_ref().is_some_and(|(loaded, _)| loaded == map)
}

pub(crate) fn set_world_brushes(map: &str, brushes: Vec<jka_assets::bsp::river::SolidBrush>) {
    let brushes = brushes.into_iter().map(|brush| {
        let min = glam::Vec3::from_array(crate::scene::render_position(brush.mins)) * CM_PER_MAP_UNIT;
        let max = glam::Vec3::from_array(crate::scene::render_position(brush.maxs)) * CM_PER_MAP_UNIT;
        WorldBrush {
            mins: min.min(max),
            maxs: min.max(max),
            planes: brush.planes.into_iter().map(|(normal, distance)| {
                (glam::Vec3::from_array(crate::scene::render_position(normal)), distance * CM_PER_MAP_UNIT)
            }).collect(),
        }
    }).collect();
    let mut state = PLACEMENTS.lock().unwrap_or_else(|poison| poison.into_inner());
    state.world_brushes = Some((map.to_owned(), brushes));
}

pub(crate) fn spawn(map: &str, origin: glam::Vec3, preset: FlamePreset) -> Option<usize> {
    let mut state = PLACEMENTS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if state.map != map {
        state.map = map.to_owned();
        state.emitters = None;
        state.stopped_player = None;
        state.promoted.clear();
        state.promoted_smoke.clear();
        publish_ready_promoted(std::iter::empty());
        publish_ready_smoke(std::iter::empty());
        PLAYER_ID.store(0, Ordering::Relaxed);
    }
    if state
        .emitters
        .as_ref()
        .is_some_and(|items| items.len() >= MAX_EMITTERS)
    {
        return None;
    }
    state.next_id = state.next_id.wrapping_add(1);
    let emitter = Emitter {
        id: state.next_id,
        origin,
        preset,
        source: EmitterSource::Manual,
        fit: None,
    };
    let emitters = Arc::make_mut(state.emitters.get_or_insert_with(|| Arc::new(Vec::new())));
    emitters.push(emitter);
    Some(emitters.len())
}

pub(crate) fn clear() -> usize {
    let mut state = PLACEMENTS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let active = PLAYER_ID.swap(0, Ordering::Relaxed) != 0;
    let stopped = state.stopped_player.take().is_some();
    state.promoted.clear();
    state.promoted_smoke.clear();
    publish_ready_promoted(std::iter::empty());
    publish_ready_smoke(std::iter::empty());
    state.emitters.take().map_or(0, |items| items.len()) + usize::from(active || stopped)
}

pub(crate) fn player_id() -> Option<u64> {
    let id = PLAYER_ID.load(Ordering::Relaxed);
    (id != 0).then_some(id)
}

pub(crate) fn start_player(map: &str, effect: PlayerEffect) {
    let mut state = PLACEMENTS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if state.map != map {
        state.map = map.to_owned();
        state.emitters = None;
    }
    state.stopped_player = None;
    PLAYER_EFFECT.store(effect as u8, Ordering::Relaxed);
    state.next_id = state.next_id.wrapping_add(1).max(1);
    PLAYER_ID.store(state.next_id, Ordering::Relaxed);
}

pub(crate) fn stop_player(pose: Option<PlayerPose>) {
    let mut state = PLACEMENTS.lock().unwrap_or_else(|poison| poison.into_inner());
    let id = PLAYER_ID.swap(0, Ordering::Relaxed);
    if id != 0 {
        state.stopped_player = pose.filter(|pose| pose.id == id).map(|mut pose| {
            // Leave the existing field at the last position after cutting its fuel.
            pose.emitting = false;
            pose.velocity = glam::Vec3::ZERO;
            pose
        });
    }
}

pub(crate) fn scene(
    map: Option<&str>, player: Option<PlayerPose>, time_ms: Option<i32>, camera: glam::Vec3,
    fx_lod: u32, fx_lod_scale: f32,
) -> Option<FireScene> {
    if !jka_ninja::INCLUDED {
        return None;
    }
    let mut state = PLACEMENTS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if map != Some(state.map.as_str()) {
        // Map transitions invalidate stable ET_FX IDs as well as placed flames.
        state.map = map.unwrap_or_default().to_owned();
        state.promoted.clear();
        state.promoted_smoke.clear();
        state.emitters = None;
        state.stopped_player = None;
        PLAYER_ID.store(0, Ordering::Relaxed);
        publish_ready_promoted(std::iter::empty());
        publish_ready_smoke(std::iter::empty());
    }
    let mut promoted = Vec::new();
    let mut promoted_smoke = Vec::new();
    let mut placed = state.emitters.clone();
    if promote() {
        if let Some(now) = time_ms {
            state.promoted.retain(|_, source| {
                (0..=PROMOTION_SOURCE_TIMEOUT_MS).contains(&now.wrapping_sub(source.last_seen_ms))
            });
            // Copy just the small source records, allowing mutation of the
            // separately owned manual-emitter Arc below without a borrow clash.
            let mut candidates: Vec<_> = state.promoted.iter()
                .map(|(&entity, &source)| (entity, source)).collect();
            // A source that is already promoted keeps its field unless another
            // is clearly closer (hysteresis): swapping at the boundary would
            // discard a running field, replay it and flash stock sprites back.
            let rank = |source: &PromotedRunner| {
                source.origin.distance_squared(camera)
                    * if source.selected { SELECTED_RANK_BONUS } else { 1.0 }
            };
            candidates.sort_unstable_by(|a, b| {
                rank(&a.1).total_cmp(&rank(&b.1)).then_with(|| a.0.cmp(&b.0))
            });
            // Do not run expensive fields on sources far outside a useful
            // presentation distance; those retain stock EFX instead.
            candidates.retain(|(_, source)| source.origin.distance_squared(camera) <= 1536.0_f32.powi(2));
            candidates.truncate(MAX_PROMOTED_EMITTERS);
            let chosen: Vec<u16> = candidates.iter().map(|(entity, _)| *entity).collect();
            for (entity, source) in state.promoted.iter_mut() {
                source.selected = chosen.contains(entity);
            }
            // Stable order prevents needless preset pipeline resets as the
            // nearest pair changes position without changing membership.
            candidates.sort_unstable_by_key(|(entity, _)| *entity);
            if !candidates.is_empty() {
                let emitters = Arc::make_mut(placed.get_or_insert_with(|| Arc::new(Vec::new())));
                for (entity, source) in candidates {
                    emitters.push(Emitter {
                        id: source.id, origin: source.origin, preset: source.preset,
                        source: EmitterSource::Promoted, fit: Some(source.fit),
                    });
                    promoted.push((entity, source.id));
                }
            }
        }
    }
    if let Some(now) = time_ms.filter(|_| promote_smoke() && crate::fx::render_enabled()) {
        let drain_ms = (1000.0 * smoke_lifetime()) as i32;
        state.promoted_smoke.retain(|_, source| {
            let elapsed = now.wrapping_sub(source.last_seen_ms);
            let timeout = PROMOTION_SOURCE_TIMEOUT_MS + if source.selected { drain_ms } else { 0 };
            (0..=timeout).contains(&elapsed)
        });
        let candidates = &mut state.promoted_smoke;
        let mut nearest: Vec<_> = candidates.iter().filter_map(|(&entity, source)| {
            let distance_sq = source.origin.distance_squared(camera);
            let authored_range_sq = if fx_lod >= crate::fx::FX_LOD_AUTHORED {
                source.cull_range_sq * fx_lod_scale * fx_lod_scale
            } else {
                f32::INFINITY
            };
            let rank = distance_sq * if source.selected { SELECTED_RANK_BONUS } else { 1.0 };
            let emitting = now.wrapping_sub(source.last_seen_ms) <= PROMOTION_SOURCE_TIMEOUT_MS;
            (distance_sq <= authored_range_sq.min(1536.0_f32.powi(2)))
                .then_some((entity, rank, emitting))
        }).collect();
        nearest.sort_unstable_by(|a, b| {
            b.2.cmp(&a.2).then_with(|| a.1.total_cmp(&b.1)).then_with(|| a.0.cmp(&b.0))
        });
        nearest.truncate(MAX_PROMOTED_SMOKE_EMITTERS);
        for (entity, source) in candidates.iter_mut() {
            source.selected = nearest.iter().any(|(chosen, _, _)| chosen == entity);
        }
        nearest.sort_unstable_by_key(|entry| entry.0);
        for (entity, _, emitting) in nearest {
            let source = &candidates[&entity];
            Arc::make_mut(placed.get_or_insert_with(|| Arc::new(Vec::new()))).push(Emitter {
                id: source.id, origin: source.origin, preset: source.preset,
                source: EmitterSource::PromotedSmoke { emitting }, fit: Some(source.fit),
            });
            promoted_smoke.push((entity, source.id));
        }
    }
    let active_player = player_id();
    let draining_player = if let (Some(mut pose), Some(now)) = (state.stopped_player, time_ms) {
        if (0..PLAYER_FIRE_DRAIN_MS).contains(&now.wrapping_sub(pose.time_ms)) {
            pose.time_ms = now;
            Some(pose)
        } else {
            state.stopped_player = None;
            None
        }
    } else {
        None
    };
    let player = player.filter(|pose| Some(pose.id) == active_player).or(draining_player);
    let map_brushes = state.world_brushes.as_ref()
        .filter(|(map, brushes)| map == &state.map && !brushes.is_empty())
        .map(|(_, brushes)| brushes);
    let world_brushes = map_brushes.filter(|_| collision()).map(Arc::clone);
    let ember_brushes = map_brushes.filter(|_| embers()).map(Arc::clone);
    // Keep the output format stable through frames where CGame omits the pose.
    (placed.is_some() || player.is_some()).then_some(FireScene {
        placed, player, player_effect: player_effect(), world_brushes, ember_brushes, promoted,
        promoted_smoke,
    })
}

#[cfg(test)]
mod fire_promotion_tests {
    use super::*;

    #[test]
    fn only_upright_vanilla_fire_roots_are_eligible() {
        assert_eq!(fire_preset(&canonical_effect_name("effects/env/fire.efx")), Some(FlamePreset::Bonfire));
        assert_eq!(fire_preset(&canonical_effect_name("env/small_fire_LOD")), Some(FlamePreset::Torch));
        for path in ["env/flame_jet", "env/fire_wall", "env/small_fire_blue",
            "env/small_fire_green", "env/small_fire_red", "boba/jet",
            "rocket/shot", "atst/side_alt_shot", "env/mini_flamejet"] {
            assert!(fire_preset(path).is_none(), "{path}");
        }
    }

    #[test]
    fn only_real_fire_layers_can_be_replaced() {
        for shader in ["gfx/effects/fire2", "gfx/effects/fire3", "gfx/effects/fire4"] {
            assert!(original_flame_shader(shader));
        }
        for shader in ["gfx/effects/whiteFlare", "gfx/misc/steam",
            "gfx/effects/alpha_smoke", "gfx/effects/flamejet"] {
            assert!(!original_flame_shader(shader));
        }
    }
}
