//! Live-editable client extensions (stage 2: read-only state + events).
//!
//! No code in this module touches wgpu, the prediction loop, or the renderer.
//! Asset lookup, Rhai parsing and executing extension entrypoints all happen
//! on a dedicated worker. Later API bridges will send immutable snapshots and
//! receive bounded HUD/world draw commands through this same worker.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError}},
    thread,
    time::{Duration, Instant},
};

use jka_assets::pk3::AssetSearchPath;
use rhai::{CallFnOptions, Dynamic, Engine, Map, Scope, AST};
use jka_protocol::{server::PlayerState, PLAYER_FIELDS, VEHICLE_ONLY_FIELDS};
use crate::cgame::{ClientGameState, PresentationEvent};
use jka_protocol::entity_event::EntityEvent;
use serde::{Deserialize, Serialize};

const MAX_EXTENSIONS: usize = 64;
/// Maximum number of distinct JKA weapons in an extension-defined scroll order.
const MAX_WEAPON_CYCLE: usize = 16;
const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
const MAX_HUD_USER_SHADERS: usize = 16;
const MAX_HUD_SHADER_BYTES: usize = 32 * 1024;
const SCAN_INTERVAL: Duration = Duration::from_millis(500);
const API_VERSION: u32 = 1;
const UPDATE_INTERVAL: Duration = Duration::from_millis(16); // <= ~62 Hz script tick
const MAX_EVENT_QUEUE: usize = 1024;
const WORKER_POLL: Duration = Duration::from_millis(10);

/// Plain data only. No references to mutable gameplay state or renderer types.
#[derive(Debug, Clone)]
pub struct GameStateSnapshot {
    /// Playerstate actually presented to the viewer (prediction/interpolation).
    pub presented: Option<PlayerState>,
    /// Snapshot playerstate only when that snapshot belongs to the viewer.
    pub authoritative: Option<PlayerState>,
    pub map_name: String,
    pub server_time: i32,
    pub viewer_client: Option<i32>,
    pub is_live: bool,
    pub is_demo: bool,
    /// Framebuffer size HUD widgets are drawn into, in pixels.
    pub screen_size: [u32; 2],
    /// Local pending JKA weapon selection; absent in demos and when following.
    pub selected_weapon: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct GameEvent {
    pub kind: &'static str,
    pub name: &'static str,
    pub event_id: i32,
    pub receive_sequence: u64,
    pub server_time: i32,
    pub entity_num: i32,
    pub parm: i32,
    pub position: [f32; 3],
    pub attacker: Option<i32>,
    pub victim: Option<i32>,
    pub attacker_name: Option<String>,
    pub victim_name: Option<String>,
    pub amount: Option<i32>,
}

impl GameEvent {
    /// Only events accepted and dispatched by ClientGameState enter this bridge.
    pub fn from_accepted(event: &PresentationEvent, game: &ClientGameState) -> Self {
        let kind = match event.event {
            EntityEvent::EV_OBITUARY => "player_killed",
            EntityEvent::EV_ITEM_PICKUP | EntityEvent::EV_GLOBAL_ITEM_PICKUP => "item_picked_up",
            EntityEvent::EV_SABER_HIT => "saber_hit",
            EntityEvent::EV_FIRE_WEAPON | EntityEvent::EV_ALT_FIRE => "weapon_fired",
            _ => "raw_event",
        };
        let (attacker, victim) = if event.event == EntityEvent::EV_OBITUARY {
            (event.state.field_i32("otherEntityNum2"), event.state.field_i32("otherEntityNum"))
        } else { (None, None) };
        let client_name = |num: Option<i32>| -> Option<String> {
            usize::try_from(num?).ok().filter(|&n| n < 32)
                .and_then(|n| game.client_name(n))
        };
        Self {
            kind, name: event.event.name(), event_id: event.event.as_i32(),
            receive_sequence: event.receive_sequence, server_time: event.server_time,
            entity_num: i32::from(event.entity_num), parm: event.parm,
            position: event.position, attacker, victim,
            attacker_name: client_name(attacker), victim_name: client_name(victim), amount: None,
        }
    }

    pub fn damage_taken(ps: &PlayerState, server_time: i32) -> Self {
        Self {
            kind: "damage_taken", name: "damage_taken", event_id: -1,
            receive_sequence: 0, server_time,
            entity_num: ps.field_i32("clientNum").unwrap_or(-1),
            parm: 0, position: [0.0; 3],
            attacker: Some(ps.persistant[6]), victim: ps.field_i32("clientNum"),
            attacker_name: None, victim_name: None,
            amount: ps.field_i32("damageCount"),
        }
    }
}

/// Called only for accepted committed damage transitions, not render prediction.
/// The damageEvent counter is a small cyclic netfield; time also bounds dedup.
pub fn enqueue_damage(
    queue: &mut Vec<GameEvent>, last: &mut Option<(i32, i32, i32)>,
    ps: &PlayerState, server_time: i32,
) {
    let client = ps.field_i32("clientNum").unwrap_or(-1);
    let marker = ps.field_i32("damageEvent").unwrap_or(-1);
    if last.is_some_and(|(c, id, t)| c == client && id == marker && server_time.saturating_sub(t) < 2_000) {
        return;
    }
    *last = Some((client, marker, server_time));
    if queue.len() < MAX_EVENT_QUEUE { queue.push(GameEvent::damage_taken(ps, server_time)); }
}

struct BridgeFrame {
    state: Option<GameStateSnapshot>,
    events: Vec<GameEvent>,
}

fn array_i32(values: &[i32]) -> Dynamic {
    Dynamic::from_array(values.iter().map(|&n| Dynamic::from_int(i64::from(n))).collect())
}

fn vec3(values: [f32; 3]) -> Dynamic {
    Dynamic::from_array(values.into_iter().map(|n| Dynamic::from_float(f64::from(n))).collect())
}

fn player_map(ps: &PlayerState) -> Map {
    let mut map = Map::new();
    let mut fields = Map::new();
    for (name, bits) in PLAYER_FIELDS {
        if let Some(raw) = ps.field_bits(name) {
            let value = if *bits == 0 { Dynamic::from_float(f64::from(f32::from_bits(raw))) }
                else { Dynamic::from_int(i64::from(raw as i32)) };
            fields.insert((*name).into(), value);
        }
    }
    for name in VEHICLE_ONLY_FIELDS {
        if let Some(raw) = ps.field_bits(name) {
            // The vehicle schema describes additional protocol fields; use
            // their raw signed bits until we have a typed schema for each.
            fields.insert((*name).into(), Dynamic::from_int(i64::from(raw as i32)));
        }
    }
    map.insert("fields".into(), Dynamic::from_map(fields));
    map.insert("stats".into(), array_i32(&ps.stats));
    map.insert("persistant".into(), array_i32(&ps.persistant));
    map.insert("ammo".into(), array_i32(&ps.ammo));
    map.insert("powerups".into(), array_i32(&ps.powerups));
    // playerState_t stats[] indexes: STAT_HEALTH 0, STAT_ARMOR 5, STAT_MAX_HEALTH 8.
    map.insert("health".into(), Dynamic::from_int(i64::from(ps.stats[0])));
    map.insert("armor".into(), Dynamic::from_int(i64::from(ps.stats[5])));
    map.insert("max_health".into(), Dynamic::from_int(i64::from(ps.stats[8].max(1))));
    // The values CG_DrawHUD reads, resolved the way the native HUD resolves them.
    let weapon = ps.field_i32("weapon").unwrap_or(0);
    let info = jka_movement::weapon_info(weapon);
    let ammo_index = info.map_or(0, |(index, _, _)| index);
    map.insert("force_power".into(), Dynamic::from_int(i64::from(ps.field_i32("fd.forcePower").unwrap_or(0))));
    map.insert("saber_style".into(), Dynamic::from_int(i64::from(ps.field_i32("fd.saberDrawAnimLevel").unwrap_or(0))));
    map.insert("saber_active_style".into(), Dynamic::from_int(i64::from(ps.field_i32("fd.saberAnimLevel").unwrap_or(0))));
    map.insert("ammo_index".into(), Dynamic::from_int(i64::from(ammo_index)));
    map.insert("ammo_current".into(), usize::try_from(ammo_index).ok().filter(|&i| i > 0)
        .and_then(|i| ps.ammo.get(i).copied()).map_or(Dynamic::UNIT, |n| Dynamic::from_int(i64::from(n))));
    // weaponData energyPerShot/altEnergyPerShot both 0 (saber, melee, stun baton).
    map.insert("ammo_infinite".into(), Dynamic::from_bool(info.is_some_and(|(_, e, a)| e == 0 && a == 0)));
    for name in ["origin", "velocity", "viewangles"] {
        let v: [f32; 3] = std::array::from_fn(|i| ps.field_f32(&format!("{name}[{i}]")).unwrap_or(0.0));
        map.insert(name.into(), vec3(v));
    }
    map.insert("client_num".into(), Dynamic::from_int(i64::from(ps.field_i32("clientNum").unwrap_or(-1))));
    map.insert("weapon".into(), Dynamic::from_int(i64::from(weapon)));
    map.insert("weapon_selected".into(), Dynamic::from_int(i64::from(weapon)));
    map.insert("weapon_owned_bits".into(), Dynamic::from_int(i64::from(ps.stats[4])));
    // Use the *same* ammo/ownership criteria as LiveInput weapon selection.
    let selectable_bits = (1..=MAX_WEAPON_CYCLE as i32)
        .filter(|&wp| crate::net::LiveInput::selectable(ps, wp))
        .fold(0_i64, |mask, wp| mask | (1_i64 << wp));
    map.insert("weapon_selectable_bits".into(), Dynamic::from_int(selectable_bits));
    let pm_type = ps.field_i32("pm_type").unwrap_or(0);
    let pm_flags = ps.field_i32("pm_flags").unwrap_or(0);
    map.insert("pm_type".into(), Dynamic::from_int(i64::from(pm_type)));
    // PM_SPECTATOR=4, PMF_FOLLOW=4096 (OpenJK). Follow spectators see a player's HUD.
    map.insert("is_free_spectator".into(), Dynamic::from_bool(pm_type == 4 && pm_flags & 4096 == 0));
    map.insert("duel_time".into(), Dynamic::from_int(i64::from(ps.field_i32("duelTime").unwrap_or(0))));
    map
}

/// Keep the network/equipped ps.weapon separate from the immediate local selection.
fn player_map_selected(ps: &PlayerState, selected: Option<u8>) -> Map {
    let mut map = player_map(ps);
    if let Some(selected) = selected {
        map.insert("weapon_selected".into(), Dynamic::from_int(i64::from(selected)));
    }
    map
}

fn session_map(state: &GameStateSnapshot) -> Map {
    let mut map = Map::new();
    map.insert("map".into(), Dynamic::from(state.map_name.clone()));
    map.insert("server_time".into(), Dynamic::from_int(i64::from(state.server_time)));
    map.insert("viewer_client".into(), Dynamic::from_int(i64::from(state.viewer_client.unwrap_or(-1))));
    map.insert("is_live".into(), Dynamic::from_bool(state.is_live));
    map.insert("is_demo".into(), Dynamic::from_bool(state.is_demo));
    map.insert("screen_width".into(), Dynamic::from_int(i64::from(state.screen_size[0])));
    map.insert("screen_height".into(), Dynamic::from_int(i64::from(state.screen_size[1])));
    map
}

fn event_map(event: &GameEvent) -> Map {
    let mut map = Map::new();
    map.insert("kind".into(), Dynamic::from(event.kind.to_owned()));
    map.insert("name".into(), Dynamic::from(event.name.to_owned()));
    map.insert("id".into(), Dynamic::from_int(i64::from(event.event_id)));
    map.insert("sequence".into(), Dynamic::from_int(event.receive_sequence.min(i64::MAX as u64) as i64));
    map.insert("server_time".into(), Dynamic::from_int(i64::from(event.server_time)));
    map.insert("entity_num".into(), Dynamic::from_int(i64::from(event.entity_num)));
    map.insert("parm".into(), Dynamic::from_int(i64::from(event.parm)));
    map.insert("position".into(), vec3(event.position));
    map.insert("attacker".into(), event.attacker.map_or(Dynamic::UNIT, |n| Dynamic::from_int(i64::from(n))));
    map.insert("victim".into(), event.victim.map_or(Dynamic::UNIT, |n| Dynamic::from_int(i64::from(n))));
    map.insert("attacker_name".into(), event.attacker_name.clone().map_or(Dynamic::UNIT, Dynamic::from));
    map.insert("victim_name".into(), event.victim_name.clone().map_or(Dynamic::UNIT, Dynamic::from));
    map.insert("amount".into(), event.amount.map_or(Dynamic::UNIT, |n| Dynamic::from_int(i64::from(n))));
    map
}


// Immutable presentation commands. Never expose UiVertex, wgpu or native pointers to Rhai.
const MAX_HUD_WIDGETS: usize = 16;
pub const MAX_HUD_COMMANDS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum HudCommand {
    Text { text: String, x: f32, y: f32, size: f32, color: [f32; 4] },
    Rect { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: [f32; 4] },
    Circle { x: f32, y: f32, radius: f32, color: [f32; 4] },
    /// A game image (`gfx/...`) stretched over the rect and tinted by `color`.
    Image { name: String, x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },
    /// A source subrectangle in original image pixels, using the ordinary UI
    /// image atlas. Negative destination sizes may mirror the sprite.
    ImageRegion { name: String, x: f32, y: f32, w: f32, h: f32,
        source: [f32; 4], color: [f32; 4] },
    /// The shader source lives inside this extension's shaders/ directory.
    /// The renderer runs the WGSL every present; Rhai provides only parameters.
    ShaderImage { name: String, shader: String, x: f32, y: f32, w: f32, h: f32,
        color: [f32; 4], params: [f32; 4] },
}

/// Screen corner a widget's placement offset is measured from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HudAnchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl HudAnchor {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "top-left" => Self::TopLeft,
            "top-right" => Self::TopRight,
            "bottom-left" => Self::BottomLeft,
            "bottom-right" => Self::BottomRight,
            _ => return None,
        })
    }

    fn right(self) -> bool { matches!(self, Self::TopRight | Self::BottomRight) }
    fn bottom(self) -> bool { matches!(self, Self::BottomLeft | Self::BottomRight) }
}

/// Distinct game images one frame of widgets may reference (atlas capacity).
/// Rhai extensions retain their original 32-image budget; the shared UI atlas
/// additionally accommodates the 31 built-in stock HUD pictures and Siege art.
pub const MAX_EXTENSION_HUD_IMAGES: usize = 32;
pub const MAX_HUD_IMAGES: usize = 64;

/// Plain HUD art names and authored `fx/hud/` materials only: no file
/// extensions, traversal or arbitrary filesystem path access.
fn valid_hud_image(name: &str) -> bool {
    name.len() <= 64 && (name.starts_with("gfx/") || name.starts_with("fx/hud/"))
        && !name.contains("//")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/'))
        && !name.ends_with('/')
}

/// Give every wanted image an atlas slot. Slots only ever grow (first come, first
/// served, capped at `MAX_EXTENSION_HUD_IMAGES`), so an image that leaves the screen for a
/// frame keeps its cell and the renderer never has to rebuild the atlas from the
/// game files mid-play. Returns true when a slot was added.
fn assign_image_slots<'a>(slots: &mut Vec<String>, wanted: impl IntoIterator<Item = &'a str>) -> bool {
    let before = slots.len();
    for name in wanted {
        if slots.len() >= MAX_EXTENSION_HUD_IMAGES { break; }
        if !slots.iter().any(|slot| slot == name) { slots.push(name.to_owned()); }
    }
    slots.len() != before
}

/// Top-left corner, in framebuffer pixels, of a widget whose placement is
/// `placement`. Right/bottom anchors measure the widget's own right/bottom edge
/// from the screen edge, so a scaled widget grows inward and stays on screen.
pub fn hud_widget_origin(
    anchor: HudAnchor, placement: HudPlacement, width: f32, height: f32, screen_w: f32, screen_h: f32,
) -> (f32, f32) {
    let (w, h) = (width * placement.scale, height * placement.scale);
    (
        if anchor.right() { screen_w + placement.x - w } else { placement.x },
        if anchor.bottom() { screen_h + placement.y - h } else { placement.y },
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct HudWidget {
    pub key: String, // extension_id/widget_id: globally unique and stable across reloads.
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub anchor: HudAnchor,
    pub commands: Vec<HudCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HudPlacement {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub visible: bool,
}

impl Default for HudPlacement {
    fn default() -> Self { Self::for_anchor(HudAnchor::TopLeft) }
}

impl HudPlacement {
    /// Where a widget sits until the user moves it: the historical spot for
    /// top-left widgets, flush with the corner for the others.
    pub fn for_anchor(anchor: HudAnchor) -> Self {
        match anchor {
            HudAnchor::TopLeft => Self { x: 24.0, y: 150.0, scale: 1.0, visible: true },
            _ => Self { x: 0.0, y: 0.0, scale: 1.0, visible: true },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedHudWidget {
    pub widget: HudWidget,
    pub placement: HudPlacement,
}

/// Native HUD elements an extension may replace. Only elements whose data a
/// script can already read through `playerstate()`/`session_info()` are listed;
/// chat, center print, scoreboard, vote and friends need a data bridge first.
/// Hiding is a runtime overlay on the user's settings: it never touches a cvar
/// or the saved config, and it ends when the extension stops running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeHud {
    Health,
    Shield,
    Ammo,
    Force,
    /// The speedometer readout, its jump counter and its speed graph.
    Speedometer,
}

impl NativeHud {
    pub const ALL: [Self; 5] =
        [Self::Health, Self::Shield, Self::Ammo, Self::Force, Self::Speedometer];

    pub fn name(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Shield => "shield",
            Self::Ammo => "ammo",
            Self::Force => "force",
            Self::Speedometer => "speedometer",
        }
    }

    fn bit(self) -> u32 { 1 << self as u32 }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeHudMask(u32);

impl NativeHudMask {
    pub fn contains(self, element: NativeHud) -> bool { self.0 & element.bit() != 0 }
    pub fn is_empty(self) -> bool { self.0 == 0 }
    pub fn union(self, other: Self) -> Self { Self(self.0 | other.0) }

    pub fn names(self) -> Vec<&'static str> {
        NativeHud::ALL.into_iter().filter(|e| self.contains(*e)).map(NativeHud::name).collect()
    }

    /// Parse the manifest's `hides_native_hud` list. Unknown names are an error
    /// so a typo cannot silently leave both HUDs on screen.
    pub fn from_names(names: &[String]) -> Result<Self, String> {
        if names.len() > NativeHud::ALL.len() * 2 {
            return Err("hides_native_hud has too many entries".into());
        }
        let mut mask = Self::default();
        for name in names {
            let element = NativeHud::ALL.into_iter().find(|e| e.name() == name.trim()).ok_or_else(|| {
                let known: Vec<_> = NativeHud::ALL.iter().map(|e| e.name()).collect();
                format!("hides_native_hud: unknown element \"{name}\" (known: {})", known.join(", "))
            })?;
            mask.0 |= element.bit();
        }
        Ok(mask)
    }
}

#[derive(Default)]
struct HudScriptState {
    widgets: BTreeMap<String, HudWidget>,
    /// Images the script will draw, declared up front by `hud_preload`.
    preload: BTreeSet<String>,
    /// Validated fixed order for weapnext/weapprev, authored by this extension.
    weapon_cycle: Vec<u8>,
    shader_paths: BTreeMap<String, String>,
    shader_sources: BTreeMap<String, String>,
}

/// Reject invalid, duplicated or out-of-range weapon IDs at the script boundary.
/// An empty array explicitly disables the extension's override.
fn weapon_cycle_order(values: rhai::Array) -> Option<Vec<u8>> {
    if values.len() > MAX_WEAPON_CYCLE { return None; }
    let mut seen = 0_u32;
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let Ok(weapon) = value.as_int() else { return None; };
        if !(1..=MAX_WEAPON_CYCLE as i64).contains(&weapon) { return None; }
        let bit = 1_u32 << weapon;
        if seen & bit != 0 { return None; }
        seen |= bit;
        out.push(weapon as u8);
    }
    Some(out)
}

fn bounded_color(values: rhai::Array) -> Option<[f32; 4]> {
    if values.len() != 4 { return None; }
    let mut color = [0.0; 4];
    for (i, v) in values.into_iter().enumerate() {
        let value = if let Ok(f) = v.as_float() { f }
            else if let Ok(n) = v.as_int() { n as f64 } else { return None; };
        if !value.is_finite() { return None; }
        color[i] = value.clamp(0.0, 1.0) as f32;
    }
    Some(color)
}

fn shader_params(values: rhai::Array) -> Option<[f32; 4]> {
    if values.len() != 4 { return None; }
    let mut out = [0.0_f32; 4];
    for (i, value) in values.into_iter().enumerate() {
        let f = if let Ok(f) = value.as_float() { f }
            else if let Ok(n) = value.as_int() { n as f64 } else { return None };
        if !f.is_finite() || f.abs() > 64.0 { return None; }
        out[i] = f as f32;
    }
    Some(out)
}

fn valid_shader_asset(path: &str) -> bool {
    path.len() <= 80 && path.starts_with("shaders/") && path.ends_with(".wgsl")
        && !path.contains("..") && !path.contains("//")
        && path.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'.'))
}

fn hud_number(value: rhai::FLOAT) -> Option<f32> {
    (value.is_finite() && value.abs() <= 8192.0).then_some(value as f32)
}

fn append_hud(state: &Arc<Mutex<HudScriptState>>, id: &str, cmd: HudCommand) {
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(widget) = state.widgets.get_mut(id) {
        if widget.commands.len() < MAX_HUD_COMMANDS { widget.commands.push(cmd); }
    }
}

#[derive(Debug, Clone)]
pub struct ExtensionInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub running: bool,
    pub revision: u64,
    pub error: Option<String>,
    pub source: String,
    pub editable: bool,
    /// Native HUD elements this extension replaces while it is running.
    pub hides: NativeHudMask,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    api_version: u32,
    name: String,
    #[serde(default = "default_entry")]
    entry: String,
    /// Native HUD elements this extension replaces, e.g. `["health", "shield"]`.
    #[serde(default)]
    hides_native_hud: Vec<String>,
}

fn default_entry() -> String { "main.rhai".into() }

#[derive(Debug)]
enum Command {
    Refresh,
    SetGame(Option<PathBuf>),
    Reload(String),
    SetEnabled(String, bool),
    SaveSource(String, String),
    Create(String),
    SaveLayouts(BTreeMap<String, HudPlacement>),
}

enum Response {
    Snapshot(Vec<ExtensionInfo>),
    Saved(String),
    Diagnostic(String),
    /// HUD + validated weapon cycle order, both sent by the extension worker.
    Hud(Vec<HudWidget>, NativeHudMask, Vec<String>, Vec<u8>, Vec<(String, String)>),
}

/// Owned by App on the main thread; neither the engine nor AST crosses this boundary.
pub struct Manager {
    root: PathBuf,
    tx: Sender<Command>,
    rx: Receiver<Response>,
    frame_tx: SyncSender<BridgeFrame>,
    last_frame: Option<Instant>,
    pending_events: VecDeque<GameEvent>,
    pub entries: Vec<ExtensionInfo>,
    pub selected: Option<String>,
    pub editor: String,
    pub editor_dirty: bool,
    pub new_id: String,
    pub diagnostic: String,
    pub hud_widgets: Vec<HudWidget>,
    /// Cached on the main thread. Wheel input never runs Rhai or locks a worker.
    pub weapon_cycle: Vec<u8>,
    /// Fully qualified shader ID -> validated WGSL body. Loaded only on activation.
    pub hud_shaders: Vec<(String, String)>,
    /// Union of every running extension's `hides_native_hud`.
    pub native_hidden: NativeHudMask,
    /// Atlas slot per extension image (grow-only while any extension is loaded).
    pub hud_images: Vec<String>,
    hud_preload: Vec<String>,
    pub hud_layouts: BTreeMap<String, HudPlacement>,
}

impl Manager {
    pub fn spawn(base: PathBuf, game: Option<PathBuf>) -> Result<Self, String> {
        let root = base.join("extensions");
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::sync_channel(2);
        thread::Builder::new()
            .name("rhai-extensions".into())
            .spawn(move || Worker::new(base, game, response_tx).run(command_rx, frame_rx))
            .map_err(|err| format!("Could not start extension worker: {err}"))?;
        let hud_layouts = fs::read_to_string(root.join("hud-layout.toml"))
            .ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        Ok(Self {
            root,
            tx: command_tx,
            rx: response_rx,
            frame_tx, last_frame: None, pending_events: VecDeque::new(),
            entries: Vec::new(),
            selected: None,
            editor: String::new(),
            editor_dirty: false,
            new_id: String::new(),
            diagnostic: String::new(),
            hud_widgets: Vec::new(),
            weapon_cycle: Vec::new(),
            hud_shaders: Vec::new(),
            native_hidden: NativeHudMask::default(),
            hud_images: Vec::new(),
            hud_preload: Vec::new(),
            hud_layouts,
        })
    }

    pub fn root(&self) -> &Path { &self.root }

    /// Cheap on the main thread; never runs a script or blocks on the worker.
    pub fn queue_events(&mut self, events: Vec<GameEvent>) {
        if !self.entries.iter().any(|e| e.running) { self.pending_events.clear(); return; }
        for event in events {
            if self.pending_events.len() == MAX_EVENT_QUEUE { self.pending_events.pop_front(); }
            self.pending_events.push_back(event);
        }
    }

    pub fn wants_frame(&self, now: Instant) -> bool {
        self.entries.iter().any(|e| e.running)
            && self.last_frame.is_none_or(|last| now.saturating_duration_since(last) >= UPDATE_INTERVAL)
    }

    /// A bounded transport: never block the game loop when script execution is slow.
    pub fn submit_frame(&mut self, now: Instant, state: Option<GameStateSnapshot>) {
        let frame = BridgeFrame { state, events: self.pending_events.drain(..).collect() };
        match self.frame_tx.try_send(frame) {
            Ok(()) => self.last_frame = Some(now),
            Err(TrySendError::Full(frame)) => self.pending_events.extend(frame.events),
            Err(TrySendError::Disconnected(_)) => self.diagnostic = "Extension worker stopped".into(),
        }
    }

    /// Called from App::tick; never reads files, compiles scripts or blocks.
    pub fn poll(&mut self) -> bool {
        let mut hud_changed = false;
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Response::Snapshot(entries) => {
                    self.entries = entries;
                    if !self.selected.as_ref().is_some_and(|id| self.entries.iter().any(|e| &e.id == id)) {
                        self.selected = self.entries.first().map(|e| e.id.clone());
                        self.editor_dirty = false;
                    }
                    if !self.editor_dirty {
                        self.editor = self.selected_entry()
                            .map_or_else(String::new, |entry| entry.source.clone());
                    }
                }
                Response::Saved(id) => {
                    if self.selected.as_deref() == Some(&id) { self.editor_dirty = false; }
                }
                Response::Hud(widgets, hidden, preload, cycle, shaders) => {
                    self.weapon_cycle = cycle;
                    if self.hud_shaders != shaders {
                        self.hud_shaders = shaders;
                        hud_changed = true;
                    }
                    if self.hud_widgets != widgets || self.native_hidden != hidden {
                        self.hud_widgets = widgets;
                        self.native_hidden = hidden;
                        hud_changed = true;
                    }
                    self.hud_preload = preload;
                    hud_changed |= self.refresh_image_slots();
                }
                Response::Diagnostic(message) => {
                    println!("[EXT] {message}");
                    self.diagnostic = message;
                }
            }
        }
        hud_changed
    }

    /// Keep the atlas slots in step with what extensions draw or preload.
    /// With nothing loaded the slots are released; otherwise they only grow.
    fn refresh_image_slots(&mut self) -> bool {
        if self.hud_widgets.is_empty() && self.hud_preload.is_empty() {
            return !std::mem::take(&mut self.hud_images).is_empty();
        }
        let drawn = self.hud_widgets.iter().flat_map(|w| w.commands.iter())
            .filter_map(|c| match c {
                HudCommand::Image { name, .. } | HudCommand::ImageRegion { name, .. }
                    | HudCommand::ShaderImage { name, .. } => Some(name.as_str()),
                _ => None,
            });
        let wanted = self.hud_preload.iter().map(String::as_str).chain(drawn);
        assign_image_slots(&mut self.hud_images, wanted)
    }

    pub fn placed_widgets(&self) -> Vec<PlacedHudWidget> {
        self.hud_widgets.iter().map(|widget| PlacedHudWidget {
            widget: widget.clone(),
            placement: self.widget_placement(&widget.key),
        }).collect()
    }

    pub fn widget_anchor(&self, key: &str) -> HudAnchor {
        self.hud_widgets.iter().find(|w| w.key == key).map_or_else(HudAnchor::default, |w| w.anchor)
    }

    pub fn widget_placement(&self, key: &str) -> HudPlacement {
        self.hud_layouts.get(key).copied()
            .unwrap_or_else(|| HudPlacement::for_anchor(self.widget_anchor(key)))
    }

    pub fn set_widget_placement(&mut self, key: &str, placement: HudPlacement) {
        if !self.hud_widgets.iter().any(|w| w.key == key) { return; }
        let placement = HudPlacement {
            x: placement.x.clamp(-8192.0, 8192.0), y: placement.y.clamp(-8192.0, 8192.0),
            scale: placement.scale.clamp(0.5, 2.0), visible: placement.visible,
        };
        if self.widget_placement(key) == placement { return; }
        self.hud_layouts.insert(key.to_owned(), placement);
    }

    pub fn save_widget_layouts(&self) {
        let _ = self.tx.send(Command::SaveLayouts(self.hud_layouts.clone()));
    }

    pub fn selected_entry(&self) -> Option<&ExtensionInfo> {
        self.entries.iter().find(|entry| Some(&entry.id) == self.selected.as_ref())
    }

    pub fn select(&mut self, id: &str) {
        if self.editor_dirty && self.selected.as_deref() != Some(id) {
            self.diagnostic = "Save or discard edits before switching extensions".into();
            return;
        }
        if let Some(entry) = self.entries.iter().find(|entry| entry.id == id) {
            self.selected = Some(id.to_owned());
            self.editor = entry.source.clone();
            self.editor_dirty = false;
        }
    }

    pub fn set_game(&self, game: Option<PathBuf>) { let _ = self.tx.send(Command::SetGame(game)); }

    pub fn refresh(&self) { let _ = self.tx.send(Command::Refresh); }
    pub fn reload_selected(&self) {
        if let Some(id) = &self.selected { let _ = self.tx.send(Command::Reload(id.clone())); }
    }
    pub fn enable(&self, id: &str, enabled: bool) {
        let _ = self.tx.send(Command::SetEnabled(id.to_owned(), enabled));
    }
    pub fn save_selected(&mut self) {
        if let Some(id) = &self.selected {
            if self.editor.len() > MAX_SOURCE_BYTES {
                self.diagnostic = "Script exceeds the 256 KiB source limit".into();
                return;
            }
            if self.tx.send(Command::SaveSource(id.clone(), self.editor.clone())).is_err() {
                self.diagnostic = "Extension worker is not running".into();
            }
        }
    }
    pub fn create(&mut self) {
        let id = self.new_id.trim().to_owned();
        if !valid_id(&id) {
            self.diagnostic = "Extension ID must be 1–48 characters: letters, digits, _ or -".into();
            return;
        }
        let _ = self.tx.send(Command::Create(id));
        self.new_id.clear();
    }
}

struct Active {
    // Kept alive for future state/event callbacks without recompiling the AST.
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    state: Arc<Mutex<Option<GameStateSnapshot>>>,
    failed_hooks: BTreeSet<String>,
    hud: Arc<Mutex<HudScriptState>>,
    hides: NativeHudMask,
}

impl Active {
    /// What this extension currently suppresses. A replacement whose `on_update`
    /// has been disabled by a runtime error is no longer drawing anything, so it
    /// gives the native elements back instead of leaving a hole in the HUD.
    fn effective_hides(&self) -> NativeHudMask {
        if self.failed_hooks.contains("on_update") { NativeHudMask::default() } else { self.hides }
    }

    fn callback(&mut self, name: &str, arg: Option<Map>) -> Result<(), String> {
        let arity = usize::from(arg.is_some());
        if self.failed_hooks.contains(name) || !self.ast.iter_functions()
            .any(|fun| fun.name == name && fun.params.len() == arity) {
            return Ok(());
        }
        // Stage 1 already evaluated the top level once. The default call_fn
        // would evaluate it AGAIN for every event/update, repeating side effects.
        let opts = CallFnOptions::new().eval_ast(false);
        let result = match arg {
            Some(arg) => self.engine.call_fn_with_options::<()>(
                opts, &mut self.scope, &self.ast, name, (arg,)),
            None => self.engine.call_fn_with_options::<()>(
                opts, &mut self.scope, &self.ast, name, ()),
        };
        if result.is_err() { self.failed_hooks.insert(name.to_owned()); }
        result.map_err(|e| e.to_string())
    }
}

struct Slot {
    fingerprint: (String, String, bool),
    info: ExtensionInfo,
    active: Option<Active>,
    source_path: Option<PathBuf>,
}

struct Worker {
    root: PathBuf,
    base: PathBuf,
    game: Option<PathBuf>,
    response: Sender<Response>,
    slots: BTreeMap<String, Slot>,
    revision: u64,
    force_reload: BTreeSet<String>,
    last_hud: Vec<HudWidget>,
    last_hidden: NativeHudMask,
    last_preload: Vec<String>,
    last_cycle: Vec<u8>,
    last_shaders: Vec<(String, String)>,
}

impl Worker {
    fn new(base: PathBuf, game: Option<PathBuf>, response: Sender<Response>) -> Self {
        Self {
            root: base.join("extensions"), base, game, response, slots: BTreeMap::new(), revision: 0, force_reload: BTreeSet::new(),
            last_hud: Vec::new(), last_hidden: NativeHudMask::default(), last_preload: Vec::new(), last_cycle: Vec::new(), last_shaders: Vec::new(),
        }
    }

    fn native_hidden(&self) -> NativeHudMask {
        self.slots.values().filter_map(|slot| slot.active.as_ref())
            .fold(NativeHudMask::default(), |mask, active| mask.union(active.effective_hides()))
    }

    /// Only one enabled and healthy extension may own weapon scroll. Ambiguous
    /// overrides fall back to native ordering rather than depending on load order.
    fn active_weapon_cycle(&self) -> Vec<u8> {
        let mut chosen = None;
        for active in self.slots.values().filter_map(|slot| slot.active.as_ref()) {
            if active.failed_hooks.contains("on_update") { continue; }
            let hud = active.hud.lock().unwrap_or_else(|e| e.into_inner());
            if hud.weapon_cycle.is_empty() { continue; }
            if chosen.is_some() { return Vec::new(); }
            chosen = Some(hud.weapon_cycle.clone());
        }
        chosen.unwrap_or_default()
    }

    fn run(mut self, rx: Receiver<Command>, frames: Receiver<BridgeFrame>) {
        self.scan(false);
        let mut last_scan = Instant::now();
        loop {
            // The render thread never waits for this worker, even if scripts stall.
            while let Ok(frame) = frames.try_recv() { self.run_frame(frame); }
            if last_scan.elapsed() >= SCAN_INTERVAL {
                self.scan(false);
                last_scan = Instant::now();
            }
            match rx.recv_timeout(WORKER_POLL) {
                Ok(command) => {
                    self.handle(command);
                    // Drain bursts (e.g. multiple toggles) before another filesystem scan.
                    while let Ok(command) = rx.try_recv() { self.handle(command); }
                    self.scan(false);
                    last_scan = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => {},
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn log(&self, message: impl Into<String>) {
        let _ = self.response.send(Response::Diagnostic(message.into()));
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Refresh => self.scan(false),
            Command::SetGame(game) => {
                if self.game == game { return; }
                self.game = game;
                self.slots.clear();
                self.force_reload.clear();
                self.send_snapshot();
                self.send_hud();
            }
            Command::Reload(id) => {
                if !valid_id(&id) { return; }
                self.force_reload.insert(id);
            }
            Command::SetEnabled(id, enabled) => {
                if !valid_id(&id) || !self.slots.contains_key(&id) { return; }
                if let Err(err) = fs::create_dir_all(self.root.join(&id)) {
                    self.log(format!("{id}: cannot update enabled state: {err}")); return;
                }
                let marker = self.root.join(&id).join(".disabled");
                let result = if enabled {
                    match fs::remove_file(&marker) {
                        Ok(()) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e),
                    }
                } else {
                    fs::write(marker, b"Disabled by DinurdoJK\n")
                };
                if let Err(err) = result { self.log(format!("{id}: cannot update enabled state: {err}")); }
            }
            Command::SaveSource(id, source) => {
                if !valid_id(&id) || source.len() > MAX_SOURCE_BYTES { return; }
                let Some(slot) = self.slots.get(&id) else { return; };
                let manifest_text = &slot.fingerprint.0;
                let Ok(manifest) = toml::from_str::<Manifest>(manifest_text) else { return; };
                if !valid_entry(&manifest.entry) { return; }
                let Some(path) = slot.source_path.as_ref() else {
                    self.log(format!("{id}: packaged scripts are read-only; edit the source package")); return;
                };
                if path.is_symlink() { self.log(format!("{id}: script symlink refused")); return; }
                match fs::write(path, source) {
                    Ok(()) => {
                        let _ = self.response.send(Response::Saved(id.clone()));
                        self.log(format!("{id}: source saved; compiling..."));
                    },
                    Err(err) => self.log(format!("{id}: cannot save script: {err}")),
                }
            }
            Command::SaveLayouts(layouts) => {
                if let Ok(text) = toml::to_string(&layouts) {
                    // The worker owns all extension filesystem writes.
                    if let Err(e) = fs::create_dir_all(&self.root)
                        .and_then(|()| fs::write(self.root.join("hud-layout.toml"), text)) {
                        self.log(format!("HUD layout save failed: {e}"));
                    }
                }
            }
            Command::Create(id) => {
                if !valid_id(&id) { return; }
                let path = self.root.join(&id);
                if path.exists() || self.slots.contains_key(&id) { self.log(format!("{id}: extension already exists")); return; }
                if let Err(err) = fs::create_dir_all(&path) {
                    self.log(format!("{id}: cannot create extension: {err}"));
                    return;
                }
                let manifest = format!("api_version = 1\nname = \"{id}\"\nentry = \"main.rhai\"\n");
                let sample = "// Stage 2: game state and accepted gameplay events.\nfn on_update() {\n    let ps = playerstate();\n    if ps != () { /* ps.health, ps.velocity, ps.fields */ }\n}\nfn on_player_killed(e) { print(`Kill: ${e.victim_name}`); }\nprint(\"Hello from Rhai!\");\n";
                if let Err(err) = fs::write(path.join("extension.toml"), manifest)
                    .and_then(|()| fs::write(path.join("main.rhai"), sample)) {
                    self.log(format!("{id}: cannot write extension: {err}"));
                } else {
                    self.log(format!("{id}: created"));
                }
            }
        }
    }

    fn scan(&mut self, force: bool) {
        let mut assets = match AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => assets,
            Err(err) => { self.log(format!("Extension asset search: {err}")); return; }
        };
        let ids: BTreeSet<String> = assets.names().filter_map(|name| {
            let id = name.strip_prefix("extensions/")?.strip_suffix("/extension.toml")?;
            valid_id(id).then(|| id.to_owned())
        }).take(MAX_EXTENSIONS).collect();
        let mut seen = BTreeSet::new();
        let mut changed = false;
        for id in ids {
            seen.insert(id.clone());
            let qroot = format!("extensions/{id}");
            let manifest_str = read_asset_text(&mut assets, &format!("{qroot}/extension.toml"), MAX_MANIFEST_BYTES)
                .unwrap_or_else(|err| format!("# Manifest read error: {err}"));
            let parsed = toml::from_str::<Manifest>(&manifest_str);
            let script_result = parsed.as_ref()
                .map_err(|e| e.to_string())
                .and_then(|m| {
                    if !valid_entry(&m.entry) { return Err("Invalid entry path (only a .rhai filename is allowed)".into()); }
                    read_asset_text(&mut assets, &format!("{qroot}/{}", m.entry), MAX_SOURCE_BYTES)
                });
            let source = script_result.as_ref().cloned().unwrap_or_default();
            // Enable state is a local preference, including for packaged extensions.
            let disabled = self.root.join(&id).join(".disabled").exists();
            let source_path = parsed.as_ref().ok().filter(|m| valid_entry(&m.entry))
                .and_then(|m| assets.loose_file_path(&format!("{qroot}/{}", m.entry)));
            let fingerprint_source = script_result.as_ref().map_or_else(
                |err| format!("<read error: {err}>"), Clone::clone);
            let fingerprint = (manifest_str, fingerprint_source, disabled);
            if !force && !self.force_reload.remove(&id) && self.slots.get(&id).is_some_and(|slot| slot.fingerprint == fingerprint && slot.source_path == source_path) {
                continue;
            }
            let hides = parsed.as_ref().map_err(|e| e.to_string())
                .and_then(|m| NativeHudMask::from_names(&m.hides_native_hud));
            let prior = self.slots.remove(&id);
            let prior_revision = prior.as_ref().map_or(0, |s| s.info.revision);
            let mut active = prior.and_then(|s| s.active);
            let mut info = ExtensionInfo {
                id: id.clone(), name: id.clone(), enabled: !disabled,
                running: active.is_some(), revision: prior_revision,
                error: None, source, editable: source_path.is_some(), hides: NativeHudMask::default(),
            };
            let result = match parsed {
                Ok(manifest) => {
                    info.name = manifest.name.clone();
                    if manifest.api_version != API_VERSION {
                        Err(format!("Unsupported API version {} (expected {})", manifest.api_version, API_VERSION))
                    } else if manifest.name.trim().is_empty() || manifest.name.len() > 80 {
                        Err("Manifest name must be 1–80 characters".into())
                    } else if let Err(err) = &hides {
                        Err(err.clone())
                    } else { script_result }
                }
                Err(err) => Err(format!("Manifest parse error: {err}")),
            };
            if disabled {
                active = None;
                info.running = false;
            } else {
                match result.and_then(|script| self.compile(&id, &script)) {
                    Ok(mut new_active) => {
                        new_active.hides = hides.unwrap_or_default();
                        active = Some(new_active);
                        self.revision += 1;
                        info.revision = self.revision;
                        info.running = true;
                        self.log(format!("{id}: activated revision {}", info.revision));
                    }
                    Err(err) => {
                        info.error = Some(err.clone());
                        info.running = active.is_some();
                        self.log(format!("{id}: {err}{}", if info.running { " (previous revision remains active)" } else { "" }));
                    }
                }
            }
            info.hides = active.as_ref().map_or_else(NativeHudMask::default, |a| a.hides);
            self.slots.insert(id, Slot { fingerprint, info, active, source_path });
            changed = true;
        }
        let old_len = self.slots.len();
        self.slots.retain(|id, _| seen.contains(id));
        if changed || self.slots.len() != old_len || force {
            self.send_snapshot();
            self.send_hud();
        }
    }

    fn run_frame(&mut self, frame: BridgeFrame) {
        let mut hud_changed = false;
        for (id, slot) in &mut self.slots {
            let Some(active) = slot.active.as_mut() else { continue; };
            *active.state.lock().unwrap_or_else(|e| e.into_inner()) = frame.state.clone();
            {
                let mut hud = active.hud.lock().unwrap_or_else(|e| e.into_inner());
                for widget in hud.widgets.values_mut() { widget.commands.clear(); }
            }
            for event in &frame.events {
                let map = event_map(event);
                if let Err(err) = active.callback("on_event", Some(map.clone())) {
                    let _ = self.response.send(Response::Diagnostic(format!("{id}: on_event: {err}")));
                }
                if event.kind != "raw_event" {
                    let callback = format!("on_{}", event.kind);
                    if let Err(err) = active.callback(&callback, Some(map)) {
                        let _ = self.response.send(Response::Diagnostic(format!("{id}: {callback}: {err}")));
                    }
                }
            }
            if let Err(err) = active.callback("on_update", None) {
                let _ = self.response.send(Response::Diagnostic(format!("{id}: on_update: {err}")));
            }
            if !active.hud.lock().unwrap_or_else(|e| e.into_inner()).widgets.is_empty() {
                hud_changed = true;
            }
        }
        // A hook failure releases its native elements, so re-check the mask too.
        if hud_changed || self.native_hidden() != self.last_hidden
            || self.active_weapon_cycle() != self.last_cycle { self.send_hud(); }
    }

    fn send_hud(&mut self) {
        let widgets: Vec<HudWidget> = self.slots.iter().flat_map(|(id, slot)| {
            slot.active.as_ref().into_iter().flat_map(move |active| {
                let hud = active.hud.lock().unwrap_or_else(|e| e.into_inner());
                hud.widgets.values().cloned().map(|mut widget| {
                    widget.key = format!("{id}/{}", widget.key);
                    // Shader IDs are scoped to the extension; one package cannot
                    // accidentally use another package's shader or parameters.
                    for command in &mut widget.commands {
                        if let HudCommand::ShaderImage { shader, .. } = command {
                            *shader = format!("{id}/{shader}");
                        }
                    }
                    widget
                }).collect::<Vec<_>>()
            })
        }).take(64).collect();
        let hidden = self.native_hidden();
        let preload: Vec<String> = self.slots.values().filter_map(|slot| slot.active.as_ref())
            .flat_map(|active| active.hud.lock().unwrap_or_else(|e| e.into_inner()).preload.iter().cloned().collect::<Vec<_>>())
            .collect::<BTreeSet<_>>().into_iter().take(MAX_EXTENSION_HUD_IMAGES).collect();
        let cycle = self.active_weapon_cycle();
        let shaders: Vec<(String, String)> = self.slots.iter()
            .filter_map(|(id, slot)| slot.active.as_ref().map(|active| (id, active)))
            .flat_map(|(id, active)| {
                let state = active.hud.lock().unwrap_or_else(|e| e.into_inner());
                state.shader_sources.iter().map(|(name, source)|
                    (format!("{id}/{name}"), source.clone())).collect::<Vec<_>>()
            }).take(MAX_HUD_USER_SHADERS).collect();
        if widgets != self.last_hud || hidden != self.last_hidden
            || preload != self.last_preload || cycle != self.last_cycle || shaders != self.last_shaders {
            self.last_hud = widgets.clone();
            self.last_hidden = hidden;
            self.last_preload = preload.clone();
            self.last_cycle = cycle.clone();
            self.last_shaders = shaders.clone();
            let _ = self.response.send(Response::Hud(widgets, hidden, preload, cycle, shaders));
        }
    }

    fn compile(&self, id: &str, source: &str) -> Result<Active, String> {
        let mut engine = Engine::new();
        engine.set_max_operations(200_000)
            .set_max_call_levels(32)
            .set_max_expr_depths(64, 32)
            .set_max_string_size(64 * 1024)
            .set_max_array_size(4_096)
            .set_max_map_size(1_024);
        let tx = self.response.clone();
        let diagnostic_id = id.to_owned();
        engine.on_print(move |message| {
            let _ = tx.send(Response::Diagnostic(format!("{diagnostic_id}: {message}")));
        });
        let state: Arc<Mutex<Option<GameStateSnapshot>>> = Arc::new(Mutex::new(None));
        let presented = Arc::clone(&state);
        engine.register_fn("playerstate", move || -> Dynamic {
            presented.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                .and_then(|s| s.presented.as_ref().map(|ps| player_map_selected(ps, s.selected_weapon)))
                .map_or(Dynamic::UNIT, Dynamic::from_map)
        });
        let authoritative = Arc::clone(&state);
        engine.register_fn("snapshot_playerstate", move || -> Dynamic {
            authoritative.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                .and_then(|s| s.authoritative.as_ref()).map(player_map)
                .map_or(Dynamic::UNIT, Dynamic::from_map)
        });
        let session = Arc::clone(&state);
        engine.register_fn("session_info", move || -> Dynamic {
            session.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                .map(session_map).map_or(Dynamic::UNIT, Dynamic::from_map)
        });
        let hud = Arc::new(Mutex::new(HudScriptState::default()));
        let registration = Arc::clone(&hud);
        let register = move |widget_id: &str, name: &str, width: rhai::FLOAT, height: rhai::FLOAT,
            anchor: HudAnchor| -> bool {
            let Some(w) = hud_number(width) else { return false; };
            let Some(h) = hud_number(height) else { return false; };
            if w < 16.0 || h < 12.0 || w > 2048.0 || h > 2048.0
                || !valid_id(&widget_id) || name.len() > 80 || name.is_empty() { return false; }
            let mut state = registration.lock().unwrap_or_else(|p| p.into_inner());
            if !state.widgets.contains_key(widget_id) && state.widgets.len() >= MAX_HUD_WIDGETS {
                return false;
            }
            state.widgets.insert(widget_id.to_string(), HudWidget {
                key: widget_id.to_string(), name: name.to_string(), width: w, height: h,
                anchor, commands: Vec::new(),
            });
            true
        };
        let register_plain = register.clone();
        engine.register_fn("hud_register", move |id: rhai::ImmutableString, name: rhai::ImmutableString,
            width: rhai::FLOAT, height: rhai::FLOAT| -> bool {
            register_plain(&id, &name, width, height, HudAnchor::TopLeft)
        });
        // Anchor is "top-left", "top-right", "bottom-left" or "bottom-right".
        engine.register_fn("hud_register", move |id: rhai::ImmutableString, name: rhai::ImmutableString,
            width: rhai::FLOAT, height: rhai::FLOAT, anchor: rhai::ImmutableString| -> bool {
            HudAnchor::parse(&anchor).is_some_and(|anchor| register(&id, &name, width, height, anchor))
        });
        // Declaration runs on the extension worker, not from input handlers.
        // Scripts can choose their on-screen fixed-slot order, including weapons
        // normally omitted from the native twelve-slot bar. Invalid declarations
        // do not replace the last valid order.
        let weapon_hud = Arc::clone(&hud);
        engine.register_fn("hud_weapon_cycle", move |values: rhai::Array| -> bool {
            let Some(order) = weapon_cycle_order(values) else { return false; };
            weapon_hud.lock().unwrap_or_else(|p| p.into_inner()).weapon_cycle = order;
            true
        });
        // Declare images the script will draw so they are loaded when it starts,
        // not the first time they appear on screen.
        // Declare packaged WGSL modules at top level. There is no runtime
        // source string evaluation, arbitrary path access or per-frame compile.
        let shader_hud = Arc::clone(&hud);
        engine.register_fn("hud_shader", move |name: rhai::ImmutableString, path: rhai::ImmutableString| -> bool {
            if !valid_id(&name) || !valid_shader_asset(&path) { return false; }
            let mut state = shader_hud.lock().unwrap_or_else(|e| e.into_inner());
            if !state.shader_paths.contains_key(name.as_str())
                && state.shader_paths.len() >= MAX_HUD_USER_SHADERS { return false; }
            state.shader_paths.insert(name.to_string(), path.to_string());
            true
        });
        let preload_hud = Arc::clone(&hud);
        engine.register_fn("hud_preload", move |name: rhai::ImmutableString| {
            if !valid_hud_image(&name) { return; }
            let mut state = preload_hud.lock().unwrap_or_else(|p| p.into_inner());
            if state.preload.len() < MAX_EXTENSION_HUD_IMAGES { state.preload.insert(name.to_string()); }
        });
        let image_hud = Arc::clone(&hud);
        engine.register_fn("hud_image", move |id: rhai::ImmutableString, name: rhai::ImmutableString,
            x: rhai::FLOAT, y: rhai::FLOAT, w: rhai::FLOAT, h: rhai::FLOAT, color: rhai::Array| {
            if !valid_hud_image(&name) { return; }
            if let (Some(x), Some(y), Some(w), Some(h), Some(color)) =
                (hud_number(x), hud_number(y), hud_number(w), hud_number(h), bounded_color(color)) {
                if w != 0.0 && h != 0.0 {
                    append_hud(&image_hud, &id, HudCommand::Image { name: name.to_string(), x, y, w, h, color });
                }
            }
        });
        let region_hud = Arc::clone(&hud);
        engine.register_fn("hud_image_region", move |id: rhai::ImmutableString,
            name: rhai::ImmutableString, x: rhai::FLOAT, y: rhai::FLOAT,
            w: rhai::FLOAT, h: rhai::FLOAT, sx: rhai::FLOAT, sy: rhai::FLOAT,
            sw: rhai::FLOAT, sh: rhai::FLOAT, color: rhai::Array| {
            if !valid_hud_image(&name) { return; }
            if let (Some(x), Some(y), Some(w), Some(h), Some(sx), Some(sy), Some(sw), Some(sh), Some(color)) =
                (hud_number(x), hud_number(y), hud_number(w), hud_number(h), hud_number(sx), hud_number(sy),
                 hud_number(sw), hud_number(sh), bounded_color(color)) {
                if w == 0.0 || h == 0.0 || sx < 0.0 || sy < 0.0 || sw <= 0.0 || sh <= 0.0 { return; }
                append_hud(&region_hud, &id, HudCommand::ImageRegion {
                    name: name.to_string(), x, y, w, h, source: [sx, sy, sw, sh], color,
                });
            }
        });
        let shaded_hud = Arc::clone(&hud);
        engine.register_fn("hud_image_shader", move |id: rhai::ImmutableString,
            image: rhai::ImmutableString, shader: rhai::ImmutableString,
            x: rhai::FLOAT, y: rhai::FLOAT, w: rhai::FLOAT, h: rhai::FLOAT,
            color: rhai::Array, values: rhai::Array| {
            if !valid_hud_image(&image) || !valid_id(&shader) { return; }
            if let (Some(x), Some(y), Some(w), Some(h), Some(color), Some(params)) =
                (hud_number(x), hud_number(y), hud_number(w), hud_number(h),
                 bounded_color(color), shader_params(values)) {
                if w == 0.0 || h == 0.0 { return; }
                // Only registered, validated shaders can be drawn.
                let valid = shaded_hud.lock().unwrap_or_else(|e| e.into_inner())
                    .shader_sources.contains_key(shader.as_str());
                if valid {
                    append_hud(&shaded_hud, &id, HudCommand::ShaderImage {
                        name: image.to_string(), shader: shader.to_string(),
                        x, y, w, h, color, params,
                    });
                }
            }
        });
        let text_hud = Arc::clone(&hud);
        engine.register_fn("hud_text", move |id: rhai::ImmutableString, text: rhai::ImmutableString,
            x: rhai::FLOAT, y: rhai::FLOAT, size: rhai::FLOAT, color: rhai::Array| {
            if text.len() > 256 { return; }
            if let (Some(x), Some(y), Some(size), Some(color)) =
                (hud_number(x), hud_number(y), hud_number(size), bounded_color(color)) {
                if (0.2..=8.0).contains(&size) {
                    append_hud(&text_hud, &id, HudCommand::Text { text: text.to_string(), x, y, size, color });
                }
            }
        });
        let rect_hud = Arc::clone(&hud);
        engine.register_fn("hud_rect", move |id: rhai::ImmutableString,
            x: rhai::FLOAT, y: rhai::FLOAT, w: rhai::FLOAT, h: rhai::FLOAT, color: rhai::Array| {
            if let (Some(x), Some(y), Some(w), Some(h), Some(color)) =
                (hud_number(x), hud_number(y), hud_number(w), hud_number(h), bounded_color(color)) {
                if w > 0.0 && h > 0.0 {
                    append_hud(&rect_hud, &id, HudCommand::Rect { x, y, w, h, color });
                }
            }
        });
        let line_hud = Arc::clone(&hud);
        engine.register_fn("hud_line", move |id: rhai::ImmutableString,
            x1: rhai::FLOAT, y1: rhai::FLOAT, x2: rhai::FLOAT, y2: rhai::FLOAT,
            thickness: rhai::FLOAT, color: rhai::Array| {
            if let (Some(x1), Some(y1), Some(x2), Some(y2), Some(thickness), Some(color)) =
                (hud_number(x1), hud_number(y1), hud_number(x2), hud_number(y2), hud_number(thickness), bounded_color(color)) {
                if (0.5..=24.0).contains(&thickness) {
                    append_hud(&line_hud, &id, HudCommand::Line { x1, y1, x2, y2, thickness, color });
                }
            }
        });
        let circle_hud = Arc::clone(&hud);
        engine.register_fn("hud_circle", move |id: rhai::ImmutableString,
            x: rhai::FLOAT, y: rhai::FLOAT, radius: rhai::FLOAT, color: rhai::Array| {
            if let (Some(x), Some(y), Some(radius), Some(color)) =
                (hud_number(x), hud_number(y), hud_number(radius), bounded_color(color)) {
                if (0.5..=512.0).contains(&radius) {
                    append_hud(&circle_hud, &id, HudCommand::Circle { x, y, radius, color });
                }
            }
        });
        let ast = engine.compile(source).map_err(|e| format!("Compile error: {e}"))?;
        let mut scope = Scope::new();
        engine.run_ast_with_scope(&mut scope, &ast)
            .map_err(|e| format!("Runtime error: {e}"))?;
        // Source loading and Naga validation are off the input and render
        // threads. The only valid source root is this extension package.
        let declared = hud.lock().unwrap_or_else(|e| e.into_inner()).shader_paths.clone();
        if !declared.is_empty() {
            let mut assets = AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|e| format!("HUD shader asset search: {e}"))?;
            let mut resolved = BTreeMap::new();
            for (name, path) in declared {
                let full = format!("extensions/{id}/{path}");
                let source = read_asset_text(&mut assets, &full, MAX_HUD_SHADER_BYTES)
                    .map_err(|e| format!("{full}: {e}"))?;
                if source.is_empty() || source.len() >= MAX_HUD_SHADER_BYTES {
                    return Err(format!("{full}: shader must be 1..{} bytes", MAX_HUD_SHADER_BYTES-1));
                }
                let combined = format!("{}\n{}\n{}", include_str!("hud_extension_shader_header.wgsl"),
                    source, include_str!("hud_extension_shader_footer.wgsl"));
                let module = naga::front::wgsl::parse_str(&combined)
                    .map_err(|e| format!("{full}: WGSL: {}", e.emit_to_string(&combined)))?;
                naga::valid::Validator::new(naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all()).validate(&module)
                    .map_err(|e| format!("{full}: shader validation: {e}"))?;
                resolved.insert(name, source);
            }
            hud.lock().unwrap_or_else(|e| e.into_inner()).shader_sources = resolved;
        }
        Ok(Active { engine, ast, scope, state, failed_hooks: BTreeSet::new(), hud, hides: NativeHudMask::default() })
    }

    fn send_snapshot(&self) {
        let _ = self.response.send(Response::Snapshot(
            self.slots.values().map(|slot| slot.info.clone()).collect(),
        ));
    }
}

fn read_asset_text(assets: &mut AssetSearchPath, name: &str, limit: usize) -> Result<String, String> {
    let asset = assets.read(name, limit).map_err(|err| err.to_string())?
        .ok_or_else(|| format!("Asset not found: {name}"))?;
    String::from_utf8(asset.bytes).map_err(|err| err.to_string())
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 &&
        s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_entry(s: &str) -> bool {
    s.len() <= 128 && s.ends_with(".rhai") &&
        Path::new(s).file_name().and_then(|n| n.to_str()) == Some(s) &&
        s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') &&
        s != ".rhai"
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ExtensionFixture(PathBuf);

    impl ExtensionFixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("djk-extensions-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn loose(dir: &Path, name: &str, source: &str) {
            let root = dir.join("extensions/test");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("extension.toml"), format!("api_version = 1\nname = '{name}'\n")).unwrap();
            fs::write(root.join("main.rhai"), source).unwrap();
        }

        fn package(dir: &Path, name: &str, source: &str) {
            use std::io::Write;
            fs::create_dir_all(dir).unwrap();
            let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("extensions.pk3")).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("extensions/test/extension.toml", options).unwrap();
            write!(zip, "api_version = 1\nname = '{name}'\n").unwrap();
            zip.start_file("extensions/test/main.rhai", options).unwrap();
            zip.write_all(source.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
    }

    impl Drop for ExtensionFixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn discovery_uses_pk3_and_mod_precedence_and_local_enable_state() {
        let fixture = ExtensionFixture::new();
        let base = fixture.0.join("base");
        let game = fixture.0.join("mod");
        ExtensionFixture::loose(&base, "Base loose", "let base_loose = 1;");
        ExtensionFixture::package(&base, "Base package", "let base_package = 1;");
        let (tx, _rx) = mpsc::channel();
        let mut worker = Worker::new(base.clone(), None, tx);
        worker.scan(false);
        assert_eq!(worker.slots.len(), 1);
        assert_eq!(worker.slots["test"].info.name, "Base package");
        assert!(worker.slots["test"].info.running);
        assert!(!worker.slots["test"].info.editable);
        worker.handle(Command::SaveSource("test".into(), "changed".into()));
        assert_eq!(fs::read_to_string(base.join("extensions/test/main.rhai")).unwrap(), "let base_loose = 1;");
        worker.handle(Command::SetEnabled("test".into(), false));
        worker.scan(false);
        assert!(!worker.slots["test"].info.enabled);
        assert!(!worker.slots["test"].info.running);
        worker.handle(Command::SetEnabled("test".into(), true));
        worker.scan(false);
        assert!(worker.slots["test"].info.running);

        ExtensionFixture::loose(&game, "Mod loose", "let mod_loose = 1;");
        worker.handle(Command::SetGame(Some(game.clone())));
        worker.scan(false);
        assert_eq!(worker.slots["test"].info.name, "Mod loose");
        assert!(worker.slots["test"].info.editable);
        worker.handle(Command::SaveSource("test".into(), "let edited = 2;".into()));
        worker.scan(false);
        assert_eq!(worker.slots["test"].info.source, "let edited = 2;");
        assert_eq!(fs::read_to_string(game.join("extensions/test/main.rhai")).unwrap(), "let edited = 2;");

        ExtensionFixture::package(&game, "Mod package", "let mod_package = 1;");
        worker.scan(false);
        assert_eq!(worker.slots["test"].info.name, "Mod package");
        assert!(!worker.slots["test"].info.editable);
        worker.handle(Command::SetGame(None));
        worker.scan(false);
        assert_eq!(worker.slots["test"].info.name, "Base package");
    }

    #[test]
    fn packaged_extensions_work_without_a_loose_extensions_directory() {
        let fixture = ExtensionFixture::new();
        ExtensionFixture::package(&fixture.0, "Packaged", "let value = 1;");
        let (tx, _rx) = mpsc::channel();
        let mut worker = Worker::new(fixture.0.clone(), None, tx);
        worker.scan(false);
        assert!(worker.slots["test"].info.running);
        assert!(!fixture.0.join("extensions").exists());
        worker.handle(Command::SetEnabled("test".into(), false));
        worker.scan(false);
        assert!(!worker.slots["test"].info.running);
        fs::remove_file(fixture.0.join("extensions.pk3")).unwrap();
        worker.scan(false);
        assert!(worker.slots.is_empty());
    }

    #[test]
    fn safe_paths() {
        assert!(valid_id("speed-hud_1"));
        assert!(!valid_id("../outside"));
        assert!(valid_entry("main.rhai"));
        assert!(!valid_entry("../main.rhai"));
        assert!(!valid_entry("C:\\temp.rhai"));
    }
    #[test]
    fn valid_manifest() {
        let m: Manifest = toml::from_str("api_version = 1\nname = 'Example'\n").unwrap();
        assert_eq!(m.entry, "main.rhai");
    }
    #[test]
    fn state_bridge_uses_jka_float_bits_and_raw_arrays() {
        let mut ps = PlayerState::default();
        ps.stats[0] = 75;
        ps.stats[5] = 42;
        ps.set_field_bits("velocity[0]", 123.5_f32.to_bits());
        ps.set_field_bits("clientNum", 4);
        let state = player_map(&ps);
        assert_eq!(state.get("health").unwrap().as_int().unwrap(), 75);
        assert_eq!(state.get("armor").unwrap().as_int().unwrap(), 42);
        let velocity = state.get("velocity").unwrap().clone().cast::<rhai::Array>();
        assert_eq!(velocity[0].as_float().unwrap(), 123.5);
        assert_eq!(state.get("client_num").unwrap().as_int().unwrap(), 4);
    }
    #[test]
    fn weapon_cycle_declaration_is_strict_and_cached_on_worker() {
        let integers = |numbers: &[i64]| -> rhai::Array {
            numbers.iter().copied().map(Dynamic::from_int).collect()
        };
        assert_eq!(weapon_cycle_order(integers(&[3, 4, 15, 16])), Some(vec![3, 4, 15, 16]));
        assert_eq!(weapon_cycle_order(integers(&[])), Some(Vec::new()));
        assert!(weapon_cycle_order(integers(&[3, 3])).is_none());
        assert!(weapon_cycle_order(integers(&[0, 3])).is_none());
        assert!(weapon_cycle_order(integers(&[17])).is_none());
        assert!(weapon_cycle_order(integers(&(1..=17).collect::<Vec<_>>())).is_none());
        assert!(weapon_cycle_order(vec![Dynamic::from("weapon")]).is_none());

        let (tx, _rx) = mpsc::channel();
        let mut worker = Worker::new(PathBuf::from("unused"), None, tx);
        let active = worker.compile("custom", r#"
            hud_weapon_cycle([3, 4, 5, 15, 16]);
            hud_register("bar", "Bar", 100.0, 40.0);
        "#).unwrap();
        assert_eq!(active.hud.lock().unwrap().weapon_cycle, [3, 4, 5, 15, 16]);
        assert!(worker.active_weapon_cycle().is_empty());
        let mut info = ExtensionInfo {
            id: "custom".into(), name: "Custom".into(), enabled: true,
            running: true, revision: 1, error: None, source: String::new(),
            editable: false, hides: NativeHudMask::default(),
        };
        worker.slots.insert("custom".into(), Slot {
            fingerprint: (String::new(), String::new(), false),
            info: info.clone(), active: Some(active), source_path: None,
        });
        assert_eq!(worker.active_weapon_cycle(), [3, 4, 5, 15, 16]);
        info.id = "conflict".into();
        let conflict = worker.compile("conflict", "hud_weapon_cycle([6, 7]);").unwrap();
        worker.slots.insert("conflict".into(), Slot {
            fingerprint: (String::new(), String::new(), false),
            info, active: Some(conflict), source_path: None,
        });
        assert!(worker.active_weapon_cycle().is_empty(), "multiple owners must not override input");
        worker.slots.remove("conflict");
        assert_eq!(worker.active_weapon_cycle(), [3, 4, 5, 15, 16]);
        worker.slots.remove("custom");
        assert!(worker.active_weapon_cycle().is_empty(), "disable/unload restores native cycling");
    }

    #[test]
    fn weapon_state_map_distinguishes_equipped_and_pending_selection() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("weapon", 5);
        ps.stats[4] = (1 << 3) | (1 << 5) | (1 << 15);
        for ammo in &mut ps.ammo { *ammo = 999; }
        let state = player_map_selected(&ps, Some(15));
        assert_eq!(state["weapon"].as_int().unwrap(), 5);
        assert_eq!(state["weapon_selected"].as_int().unwrap(), 15);
        assert_eq!(state["weapon_owned_bits"].as_int().unwrap(), ps.stats[4] as i64);
        let selectable = state["weapon_selectable_bits"].as_int().unwrap();
        assert_ne!(selectable & (1 << 15), 0);
        assert_eq!(selectable & (1 << 16), 0);
    }

    #[test]
    fn hud_registration_and_update_emit_bounded_draw_commands() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        let script = r#"
            hud_register("speed", "Speed", 170.0, 45.0);
            fn on_update() {
                hud_rect("speed", 0.0, 0.0, 170.0, 45.0, [0.1, 0.1, 0.1, 0.8]);
                hud_text("speed", "UPS", 8.0, 9.0, 2.0, [1.0, 1.0, 1.0, 1.0]);
            }
        "#;
        let mut active = worker.compile("test", script).unwrap();
        assert!(active.hud.lock().unwrap().widgets.contains_key("speed"));
        active.callback("on_update", None).unwrap();
        let hud = active.hud.lock().unwrap();
        let widget = hud.widgets.get("speed").unwrap();
        assert_eq!(widget.commands.len(), 2);
        assert!(matches!(widget.commands[0], HudCommand::Rect { .. }));
    }

    #[test]
    fn manifest_hides_native_hud_by_name() {
        let m: Manifest = toml::from_str(
            "api_version = 1
name = 'x'
hides_native_hud = ['health', 'speedometer']
").unwrap();
        let mask = NativeHudMask::from_names(&m.hides_native_hud).unwrap();
        assert!(mask.contains(NativeHud::Health) && mask.contains(NativeHud::Speedometer));
        assert!(!mask.contains(NativeHud::Ammo));
        assert_eq!(mask.names(), ["health", "speedometer"]);
        assert!(NativeHudMask::from_names(&["helth".to_owned()]).is_err());
        assert!(NativeHudMask::default().is_empty());
    }

    #[test]
    fn failed_update_hook_gives_native_elements_back() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        let mut active = worker.compile("test", "fn on_update() { let x = 1 / 0; throw \"boom\"; }").unwrap();
        active.hides = NativeHudMask::from_names(&["ammo".to_owned()]).unwrap();
        assert!(active.effective_hides().contains(NativeHud::Ammo));
        assert!(active.callback("on_update", None).is_err());
        assert!(active.effective_hides().is_empty());
    }

    #[test]
    fn top_level_variables_persist_between_callbacks() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        let mut active = worker.compile("test", r#"
            hud_register("a", "A", 20.0, 20.0);
            let n = 0;
            fn on_update() { n += 1; hud_text("a", `${n}`, 0.0, 0.0, 1.0, [1.0, 1.0, 1.0, 1.0]); }
        "#).unwrap();
        active.callback("on_update", None).unwrap();
        active.hud.lock().unwrap().widgets.get_mut("a").unwrap().commands.clear();
        active.callback("on_update", None).unwrap();
        let hud = active.hud.lock().unwrap();
        let HudCommand::Text { text, .. } = &hud.widgets["a"].commands[0] else { panic!() };
        assert_eq!(text, "2");
    }

    #[test]
    fn bundled_ups_speedometer_runs_and_draws() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        let manifest: Manifest = toml::from_str(include_str!("../extensions/ups-hud/extension.toml")).unwrap();
        assert!(NativeHudMask::from_names(&manifest.hides_native_hud).unwrap().contains(NativeHud::Speedometer));
        let mut active = worker.compile("ups-hud", include_str!("../extensions/ups-hud/main.rhai")).unwrap();
        active.callback("on_update", None).unwrap(); // no player yet
        let mut ps = PlayerState::default();
        ps.set_field_bits("velocity[0]", 600.0_f32.to_bits());
        ps.set_field_bits("velocity[1]", 800.0_f32.to_bits());
        *active.state.lock().unwrap() = Some(GameStateSnapshot {
            presented: Some(ps), authoritative: None, map_name: "t".into(), server_time: 1000,
            viewer_client: Some(0), is_live: true, is_demo: false, screen_size: [1920, 1080],
            selected_weapon: None,
        });
        active.hud.lock().unwrap().widgets.get_mut("ups").unwrap().commands.clear();
        active.callback("on_update", None).unwrap();
        let hud = active.hud.lock().unwrap();
        let texts: Vec<_> = hud.widgets["ups"].commands.iter().filter_map(|c| match c {
            HudCommand::Text { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(texts, ["1000", "peak 1000"]);
    }

    #[test]
    fn shader_paths_are_extension_local_and_strictly_bounded() {
        assert!(valid_shader_asset("shaders/liquid.wgsl"));
        assert!(!valid_shader_asset("../shader.wgsl"));
        assert!(!valid_shader_asset("shaders/../../bad.wgsl"));
        assert!(!valid_shader_asset("gfx/material.wgsl"));
        assert!(!valid_shader_asset("shaders/other.wgsl?foo"));
        assert!(shader_params(vec![0.5_f64.into(), 1_i64.into(), 0_i64.into(), 0_i64.into()]).is_some());
        assert!(shader_params(vec![f64::NAN.into(), 0_i64.into(), 0_i64.into(), 0_i64.into()]).is_none());
    }

    #[test]
    fn hud_image_names_are_validated_and_ordered() {
        assert!(valid_hud_image("gfx/hud/health_tic_1"));
        assert!(valid_hud_image("fx/hud/background"));
        assert!(!valid_hud_image("fx/arbitrary_effect"));
        assert!(!valid_hud_image("../gfx/x") && !valid_hud_image("models/a") && !valid_hud_image("gfx/a.tga"));
        assert!(!valid_hud_image("gfx//a") && !valid_hud_image(&format!("gfx/{}", "a".repeat(80))));
        let widget = |names: &[&str]| PlacedHudWidget {
            widget: HudWidget { key: "k".into(), name: "n".into(), width: 10.0, height: 10.0,
                anchor: HudAnchor::TopLeft,
                commands: names.iter().map(|n| HudCommand::Image { name: (*n).into(), x: 0.0, y: 0.0,
                    w: 1.0, h: 1.0, color: [1.0; 4] }).collect() },
            placement: HudPlacement::default(),
        };
        let _ = widget;
        let mut slots = Vec::new();
        assert!(assign_image_slots(&mut slots, ["gfx/b", "gfx/a", "gfx/b"]));
        assert_eq!(slots, ["gfx/b", "gfx/a"]);
        // An image leaving the screen keeps its slot; new ones are appended.
        assert!(!assign_image_slots(&mut slots, ["gfx/a"]));
        assert!(assign_image_slots(&mut slots, ["gfx/c"]));
        assert_eq!(slots, ["gfx/b", "gfx/a", "gfx/c"]);
        let many: Vec<String> = (0..64).map(|i| format!("gfx/i{i}")).collect();
        assign_image_slots(&mut slots, many.iter().map(String::as_str));
        assert_eq!(slots.len(), MAX_EXTENSION_HUD_IMAGES);
    }

    #[test]
    fn right_anchored_widgets_hug_the_corner_and_grow_inward() {
        let place = HudPlacement::for_anchor(HudAnchor::BottomRight);
        assert_eq!(hud_widget_origin(HudAnchor::BottomRight, place, 100.0, 50.0, 1920.0, 1080.0), (1820.0, 1030.0));
        let scaled = HudPlacement { scale: 2.0, ..place };
        assert_eq!(hud_widget_origin(HudAnchor::BottomRight, scaled, 100.0, 50.0, 1920.0, 1080.0), (1720.0, 980.0));
        assert_eq!(hud_widget_origin(HudAnchor::TopLeft, HudPlacement::default(), 100.0, 50.0, 1920.0, 1080.0), (24.0, 150.0));
    }

    #[test]
    fn hud_layout_roundtrip() {
        let mut map = BTreeMap::new();
        map.insert("a/speed".to_owned(), HudPlacement { x: 32.0, y: 123.0,
            scale: 1.5, visible: false });
        let serialized = toml::to_string(&map).unwrap();
        let parsed: BTreeMap<String, HudPlacement> = toml::from_str(&serialized).unwrap();
        assert_eq!(map, parsed);
    }

    #[test]
    fn callbacks_do_not_rerun_script_top_level() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        let mut active = worker.compile("test", "print(\"loaded\"); fn on_update() { } ").unwrap();
        assert!(rx.try_recv().is_ok());
        active.callback("on_update", None).unwrap();
        active.callback("on_update", None).unwrap();
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn compiling_is_a_separate_stage_from_activation() {
        let (tx, _rx) = mpsc::channel();
        let worker = Worker::new(PathBuf::from("unused"), None, tx);
        assert!(worker.compile("test", "let value = 42;").is_ok());
        assert!(worker.compile("test", "let value = ;").is_err());
        assert!(worker.compile("test", "while true { }").is_err());
    }
}
