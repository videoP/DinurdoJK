//! ui module facade. Implementations are grouped by responsibility.
// Retain the existing facade API, including entry points used by external callers or tests.
#[allow(unused_imports)]
pub use console::{
    console_help_rect, console_suggest_chip_rect, console_suggest_geometry, ConsoleSuggestGeometry,
};
use hud::draw::{HUD_INK_BOTTOM, HUD_INK_TOP, HUD_VALUE};
#[allow(unused_imports)]
pub use hud::layout::HudAnchor;
#[allow(unused_imports)]
pub use hud::movement::FORCE_ICON_NAMES;
#[allow(unused_imports)]
pub use text::ProportionalGlyph;
mod chat;
mod compose;
mod console;
mod demo;
mod diagnostics;
mod geometry;
mod hud;
mod inspector;
mod loading;
mod scoreboard;
mod settings;
mod text;
use chat::{build_chat_history, build_chat_input, KEY_ATLAS_ROWS, KEY_TEXTURE_SOURCE};
pub use chat::{KEY_ART_SIZE, KEY_ATLAS_COLUMNS, MOVEMENT_KEY_ART};
use compose::gameplay_hud_visible;
pub use compose::{
    build_dynamic_static_vertices, build_movement_vertices, build_transient_vertices,
    build_vertices,
};
use console::{build_console, rect_gradient, with_alpha, CONSOLE_CHAR_HEIGHT, CONSOLE_CHAR_WIDTH};
pub use console::{
    console_help_hit, console_suggest_chip_hit, console_suggest_detail_line_count,
    console_suggest_hit, console_text_hit, console_visible_line_capacity,
};
use demo::build_demo_timeline;
use diagnostics::{build_fps_simple, build_perf, build_reflection_debug_legend, perf_panel_rect};
use geometry::{disc, glyph_quad, glyph_quad_sized, rect, rect_outline, textured_rect_with_source};
#[cfg(test)]
use hud::crosshair::build_crosshair;
pub use hud::crosshair::build_crosshair_vertices;
use hud::crosshair::{build_crosshair_name, build_force_select, hud_line};
use hud::draw::{
    build_hud, lighten, CHATBOX_CUTOFF, CHATBOX_FONT_HEIGHT, CHATBOX_FONT_SCALE, CHATBOX_Y,
};
use hud::layout::draw_placed;
pub use hud::layout::{
    hud_element_rect, HudElementId, HudElementLayout, HudLayout, HudRect, HudRectContext,
};
pub(crate) use hud::movement::visible_jka_chars;
use hud::movement::{
    build_lagometer, build_movement_keys, build_race_timer, build_speedometer, build_strafe_helper,
    center_print_lines, icon_cell_uv, movement_keys_origin, ICON_CROSSHAIR_BASE, ICON_FORCE_BASE,
    ICON_TEXTURE_SOURCE, TEAM_ICON_TEXTURE_SOURCE,
};
pub use hud::movement::{ICON_CELL, ICON_NAMES, TEAM_ICON_CELL, TEAM_ICON_COLUMNS};
use hud::notices::{build_center_print, build_follow_indicator, build_vote};
use hud::scores::{build_game_timer, build_mini_scores};
use hud::team::{build_prediction_debug, build_team_overlay, player_icon_atlas_cell};
use inspector::surface_inspector_frame;
use loading::{build_background_progress, build_loading_screen, build_splash_background};
use scoreboard::build_scoreboard;
pub use settings::environment::{
    CloudRenderResolution, CloudType, FogMode, FootprintMode, PuddleQuality, RainIntensity,
    SaberMarkMode, CLOUD_HEIGHT_MAX, CLOUD_HEIGHT_MIN, CLOUD_ROW_AERIAL, CLOUD_ROW_BASE_VARIATION,
    CLOUD_ROW_EMPTY_SKIP, CLOUD_ROW_HISTORY_BLEND, CLOUD_ROW_HISTORY_DEPTH_REJECT,
    CLOUD_ROW_MOTION_REJECT, CLOUD_ROW_SHAPE_EVOLUTION, CLOUD_ROW_SHEAR, CLOUD_ROW_SIZE,
    CLOUD_ROW_SKY_AMBIENT, CLOUD_ROW_TEMPORAL_DEPTH_FIX, CLOUD_ROW_TERRAIN_INTERACTION,
    CLOUD_ROW_THICKNESS_VARIATION, CLOUD_THICKNESS_MAX, CLOUD_THICKNESS_MIN, ENV_ROW_CLOUDS,
    ENV_ROW_CLOUD_COVERAGE, ENV_ROW_CLOUD_HEIGHT, ENV_ROW_CLOUD_QUALITY,
    ENV_ROW_CLOUD_RENDER_RESOLUTION, ENV_ROW_CLOUD_SHADOWS, ENV_ROW_CLOUD_TEMPORAL,
    ENV_ROW_CLOUD_THICKNESS, ENV_ROW_CLOUD_TUNING, ENV_ROW_CLOUD_TYPE, ENV_ROW_FOG_MODE,
    ENV_ROW_FOG_STRENGTH, ENV_ROW_FOOTPRINTS, ENV_ROW_PUDDLE_SCATTER, ENV_ROW_PUDDLE_WATER,
    ENV_ROW_RAIN, ENV_ROW_RAIN_GRADE, ENV_ROW_RAIN_INTENSITY, MAX_FOG_STRENGTH,
    VIDEO_ROW_FILM_GRAIN,
};
pub use settings::rendering::{
    ColorLutPreset, CullDebugMode, DetailTextureMode, DofQuality, DynamicLightsMode,
    DynamicShadowsMode, EntityAmbientLightingMode, EntityShadowLight, FullscreenMode,
    FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode, PlanarReflectionDebugMode,
    PlanarReflectionMode, PvsMode, ReflectionQuality, RendererBackend, SunVisibilityMode,
    TextureFilter, VsyncMode, ENV_ROW_ENTITY_SUN_LIGHTING, ENV_ROW_GRASS, ENV_ROW_OCEAN,
    ENV_ROW_OCEAN_SETTINGS, ENV_ROW_SUN_COLOR, ENV_ROW_SUN_INTENSITY, ENV_ROW_SUN_PITCH,
    ENV_ROW_SUN_SOURCE, ENV_ROW_SUN_VISIBILITY, ENV_ROW_SUN_YAW, ENV_ROW_VID_RESTART,
    ENV_ROW_WEATHER_DIRECTION_VARIATION, ENV_ROW_WEATHER_GUST_STRENGTH,
    ENV_ROW_WEATHER_WIND_DIRECTION, ENV_ROW_WEATHER_WIND_SPEED, MAX_DISTANCE_CULL_SCALE,
    SUN_INTENSITY_MAX, VIDEO_ROW_AMBIENT_OCCLUSION, VIDEO_ROW_ANTI_ALIASING,
    VIDEO_ROW_ASSET_OVERRIDES, VIDEO_ROW_AUTO_EXPOSURE, VIDEO_ROW_BAKED_AO_CURRENT_CELL,
    VIDEO_ROW_BAKED_AO_RANGE, VIDEO_ROW_BAKED_AO_RESOLUTION, VIDEO_ROW_BAKED_AO_SAMPLES,
    VIDEO_ROW_BAKED_AO_STRENGTH, VIDEO_ROW_BLOOM, VIDEO_ROW_BRIGHTNESS,
    VIDEO_ROW_CHROMATIC_ABERRATION, VIDEO_ROW_COLOR_LUT, VIDEO_ROW_CONTACT_SHADOWS,
    VIDEO_ROW_CULL_DEBUG, VIDEO_ROW_DELUXE_MAPPING, VIDEO_ROW_DELUXE_SPECULAR,
    VIDEO_ROW_DEPTH_OF_FIELD, VIDEO_ROW_DETAIL_TEXTURES, VIDEO_ROW_DEVELOPER_TOOLS,
    VIDEO_ROW_DISTANCE_CULL, VIDEO_ROW_DLIGHT_BRIGHTNESS, VIDEO_ROW_DOF_AUTOFOCUS,
    VIDEO_ROW_DOF_QUALITY, VIDEO_ROW_DRAW_CLIP_BRUSHES, VIDEO_ROW_DRAW_ENTITIES,
    VIDEO_ROW_DRAW_FPS, VIDEO_ROW_DRAW_MAP_MODELS, VIDEO_ROW_DRAW_TRIGGERS,
    VIDEO_ROW_DYNAMIC_LIGHTS, VIDEO_ROW_DYNAMIC_SHADOWS, VIDEO_ROW_EMISSIVE_AREA_LIGHTS,
    VIDEO_ROW_ENTITY_AMBIENT_LIGHTING, VIDEO_ROW_ENTITY_SHADOW_LIGHT, VIDEO_ROW_FLARES,
    VIDEO_ROW_FLOAT_LIGHTMAP, VIDEO_ROW_FPS_CAP, VIDEO_ROW_FULLSCREEN, VIDEO_ROW_FX_FPS,
    VIDEO_ROW_FX_FPS_SCOPE, VIDEO_ROW_FX_GEOMETRY, VIDEO_ROW_FX_LOD, VIDEO_ROW_FX_PHYSICS,
    VIDEO_ROW_FX_ZERO_ALPHA_DISCARD, VIDEO_ROW_GEN_NORMAL_MAPS, VIDEO_ROW_GHOUL2_BATCH_DRAWS,
    VIDEO_ROW_GHOUL2_EARLY_CULL, VIDEO_ROW_GHOUL2_LOD_BIAS, VIDEO_ROW_GHOUL2_SKINNING,
    VIDEO_ROW_GPU_DRIVEN, VIDEO_ROW_GPU_TIMINGS, VIDEO_ROW_HALATION, VIDEO_ROW_HDR, VIDEO_ROW_HIZ,
    VIDEO_ROW_LIGHTMAP_ONLY, VIDEO_ROW_LOCAL_LIGHT_SHADOWS, VIDEO_ROW_LUT_STRENGTH,
    VIDEO_ROW_MAP_LIGHT_SIMULATION, VIDEO_ROW_MAX_FRAME_LATENCY, VIDEO_ROW_MODEL_BRIGHTNESS,
    VIDEO_ROW_MODERN_SABERS, VIDEO_ROW_MOTION_BLUR, VIDEO_ROW_PBR, VIDEO_ROW_PERF_TRACE,
    VIDEO_ROW_PHYSICS_FPS, VIDEO_ROW_PICMIP, VIDEO_ROW_PLANAR_REFLECTIONS,
    VIDEO_ROW_PLANAR_REFLECTION_DEBUG, VIDEO_ROW_PVS, VIDEO_ROW_QUALITY_PRESET,
    VIDEO_ROW_RENDERER_VERBOSE, VIDEO_ROW_RENDER_BACKEND, VIDEO_ROW_RESOLUTION,
    VIDEO_ROW_SABER_IMPACT_FX, VIDEO_ROW_SABER_MARKS, VIDEO_ROW_SSR, VIDEO_ROW_TEXTURE_FILTER,
    VIDEO_ROW_TONE_MAPPING, VIDEO_ROW_VERTEX_LIGHTING, VIDEO_ROW_VID_RESTART, VIDEO_ROW_VIGNETTE,
    VIDEO_ROW_VOXEL_PROBE_GI, VIDEO_ROW_VSYNC, VIDEO_ROW_WIREFRAME, VIDEO_ROW_WORLD_LIGHTING,
};
pub use settings::video::VideoSettings;
pub use text::{
    build_world_player_label_vertices, fallback_font_rgba, parse_fontdat, ProportionalFont,
    UiVertex,
};
use text::{
    color_code, console_text, fixed_charset_text, proportional_text, proportional_text_width, text,
    truncate_jka_text, wrap_proportional_text,
};

use bytemuck::{Pod, Zeroable};

/// Bitmask selecting which renderer submission families participate in r_showtris.
/// A zero mask disables wireframe rendering.
pub mod wireframe {
    pub const MAP: u32 = 1 << 0;
    pub const PLAYERS: u32 = 1 << 1;
    pub const ENTITIES: u32 = 1 << 2;
    pub const EFFECTS: u32 = 1 << 3;
    pub const GRASS: u32 = 1 << 4;
    pub const OCEAN: u32 = 1 << 5;
    pub const DEFORMATION: u32 = 1 << 6;
    pub const ALL: u32 = MAP | PLAYERS | ENTITIES | EFFECTS | GRASS | OCEAN | DEFORMATION;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    None,
    Chat,
    Console,
    Video,
    Game,
    Vgs,
    HudEdit,
    /// The in-game third-person camera editor (Setup -> Camera -> Adjust).
    CameraEdit,
    MapEdit,
    EntityGraph,
    /// The `/trace` results menu: a clickable list of everything traced this
    /// session, with a full-info inspector and solo-server actions.
    Trace,
    /// Loaded/live jaPRO strafe trails and asynchronous trail-file browser.
    StrafeTrails,
    /// Remote/local race demo browser and synchronized visual ghosts.
    RaceGhosts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChatMode {
    #[default]
    Global,
    Team,
}

#[derive(Debug, Clone)]
pub struct UiChatLine {
    pub text: String,
    pub alpha: f32,
}

#[derive(Debug, Clone)]
pub struct UiCenterPrint {
    pub text: String,
    pub alpha: f32,
    /// Legacy CGame virtual-screen Y as a 0..1 screen-height fraction.
    pub y_fraction: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiScoreEntry {
    pub client: i32,
    pub name: String,
    pub score: i32,
    pub deaths: Option<i32>,
    pub ping: i32,
    pub time: i32,
    pub team: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiScoreboard {
    pub team_scores: [i32; 2],
    pub team_game: bool,
    /// Duel and power duel keep a win/loss score for the spectators waiting in line;
    /// every other mode shows only ping and time for them, like jaPRO's scoreboard.
    pub spectator_scores: bool,
    /// Whether this scoreboard mode has a usable deaths value to display.
    pub show_deaths: bool,
    pub entries: Vec<UiScoreEntry>,
}

#[derive(Debug, Clone, Copy)]
pub struct DemoKillMarkerUi {
    /// Normalized 0..1 position on the complete demo timeline.
    pub fraction: f32,
    /// TEAM_RED=1 / TEAM_BLUE=2 when known, otherwise neutral.
    pub attacker_team: i32,
    pub followed_kill: bool,
    pub followed_death: bool,
}

#[derive(Debug, Clone)]
pub struct DemoTimelineUi {
    pub elapsed_ms: f64,
    pub duration_ms: i32,
    /// Selected playback speed. When paused this is the resume speed.
    pub playback_rate: f64,
    pub paused: bool,
    /// While dragging, render the thumb/time at this pending seek position.
    pub scrub_fraction: Option<f32>,
    pub kill_markers: Vec<DemoKillMarkerUi>,
    pub camera_label: String,
    pub outside_authoritative_view: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DemoTimelineLayout {
    pub play: [f32; 4],
    pub track: [f32; 4],
    pub speed: [f32; 4],
}

pub fn demo_timeline_layout(width: u32, height: u32) -> Option<DemoTimelineLayout> {
    if width < 420 || height < 120 {
        return None;
    }
    let w = width as f32;
    let h = height as f32;
    let panel_y = h - 46.0;
    let play = [16.0, panel_y + 7.0, 30.0, 26.0];
    let speed = [w - 82.0, panel_y + 7.0, 66.0, 26.0];
    // Leave fixed text gutters for current and total time.
    let track_x = 116.0;
    let track_w = (w - track_x - 176.0).max(80.0);
    let track = [track_x, panel_y + 10.0, track_w, 20.0];
    Some(DemoTimelineLayout { play, track, speed })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TeamOverlaySettings {
    /// TaystJK cg_drawTeamOverlay: 0 off; 1/2 classic, 3/4 cards, 5/6 bars.
    /// Even modes omit the local player.
    pub mode: i32,
    pub x: i32,
    pub y: i32,
    pub weapons: bool,
    pub scale: f32,
    pub max_hp: f32,
    pub force: bool,
}

impl Default for TeamOverlaySettings {
    fn default() -> Self {
        Self {
            mode: 0,
            x: 640,
            y: 0,
            weapons: false,
            scale: 1.0,
            max_hp: 150.0,
            force: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeamOverlayEntry {
    pub client: u16,
    pub name: String,
    pub location: String,
    pub health: i32,
    pub armor: i32,
    pub force: i32,
    pub model_icon: Option<u16>,
    pub weapon_icon: Option<u16>,
    pub powerup_icons: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeamOverlayUi {
    pub settings: TeamOverlaySettings,
    pub team: i32,
    /// TaystJK scans every CS_LOCATIONS string, not only teammates' current locations.
    pub location_width: usize,
    pub has_locations: bool,
    /// Dynamic atlas cells. Each string contains one or more qpath candidates
    /// separated by `\n`; the renderer registers the first asset that exists.
    /// This mirrors CG_LoadClientInfo's model-icon fallback without doing VFS I/O
    /// on the main/app thread every snapshot.
    pub icon_paths: Vec<String>,
    pub entries: Vec<TeamOverlayEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HudState {
    pub health: i32,
    pub max_health: i32,
    pub armor: i32,
    /// None when a reconstructed demo POV has no recorded force value.
    pub force_power: Option<i32>,
    pub force_power_max: i32,
    pub ammo: Option<i32>,
    /// Selected weapon (`weapon_t`); WP_SABER swaps the ammo panel for the saber style.
    pub weapon: i32,
    /// `fd.saberDrawAnimLevel` (`saber_styles_t`: 1 fast .. 7 staff), 0 when unknown.
    pub saber_style: i32,
    /// The force bar is in the red half of its "no force / out of ammo" flash.
    pub force_flash: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrosshairSettings {
    /// 0 disables the crosshair. Non-zero values select a local HUD shape
    /// (`CROSSHAIR_STYLE_LINE` is jaPRO's strafehelper line crosshair).
    pub style: u8,
    /// 0 draws the `style` shape; 1..=`CROSSHAIR_IMAGE_COUNT` draws the stock
    /// `gfx/2d/crosshair{a..j}` image instead (while `style` is not 0).
    pub image: u8,
    /// JKA-compatible cg_crosshairSize value. 24 preserves the stock default.
    pub size: f32,
    /// RGBA, matching TaystJK's cg_crosshairColor 0..255 convention.
    pub color: [u8; 4],
    /// Visibility, 0..=2, 1 = as authored. Below 1 fades the crosshair; above 1
    /// strengthens the faint stock images by layering them (shapes are already opaque).
    pub strength: f32,
    /// TaystJK `cg_dynamicCrosshair`: 0 static, 1 always dynamic, 2 dynamic with
    /// the reference melee/racemode/strafehelper static overrides.
    pub dynamic: u8,
    /// jaPRO `cg_crosshairIdentifyTarget`: colour the crosshair by what it is on.
    pub identify_target: bool,
    /// jaPRO `cg_drawCrosshairNames`: 0 off, > 0 seconds a name lingers after
    /// aiming away, < 0 only while aimed at.
    pub names: f32,
    /// jaPRO `cg_drawCrosshairNamesColours`: 1 keeps the name's own colour codes
    /// (white base), 0 strips them and colours by friend/foe.
    pub names_colours: bool,
    /// jaPRO `cg_drawCrosshairNamesOpacity`, 0..=1.
    pub names_opacity: f32,
}

/// Highest `cg_drawCrosshair` shape: the jaPRO strafehelper line (`SHELPER_CROSSHAIR`).
pub const CROSSHAIR_STYLE_LINE: u8 = 7;
/// Shape ids `cg_drawCrosshair` accepts, 0 (off) through the line.
pub const CROSSHAIR_STYLE_MAX: u8 = CROSSHAIR_STYLE_LINE;
/// Largest `cg_crosshairStrength`: 200%.
pub const CROSSHAIR_STRENGTH_MAX: f32 = 2.0;
/// Stock image crosshairs, `gfx/2d/crosshaira` .. `crosshairj` (jaPRO adds `j`).
pub const CROSSHAIR_IMAGE_COUNT: u8 = 10;
/// `gfx/2d` names of the image crosshairs, `cg_crosshairImage` 1.. in order.
pub const CROSSHAIR_IMAGE_NAMES: [&str; CROSSHAIR_IMAGE_COUNT as usize] = [
    "gfx/2d/crosshaira",
    "gfx/2d/crosshairb",
    "gfx/2d/crosshairc",
    "gfx/2d/crosshaird",
    "gfx/2d/crosshaire",
    "gfx/2d/crosshairf",
    "gfx/2d/crosshairg",
    "gfx/2d/crosshairh",
    "gfx/2d/crosshairi",
    "gfx/2d/crosshairj",
];

/// What the crosshair is currently on, resolved by the app each aim scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiCrosshairTarget {
    /// RGB (0..=1) the crosshair takes; `None` keeps its configured colour.
    pub color: Option<[f32; 3]>,
    /// Whether the current TaystJK policy uses muzzle/world-point placement.
    /// The high-rate endpoint itself rides the render/subframe mailbox, not UI commands.
    pub dynamic: bool,
    pub name: Option<UiCrosshairName>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiCrosshairName {
    pub text: String,
    pub color: [f32; 3],
    pub alpha: f32,
}

impl Default for CrosshairSettings {
    fn default() -> Self {
        Self {
            style: 1,
            image: 0,
            size: 24.0,
            // Keep DinurdoJK's existing white crosshair by default while using
            // TaystJK's four-component cg_crosshairColor storage convention.
            color: [255, 255, 255, 230],
            strength: 1.0,
            // OpenJK/TaystJK default.
            dynamic: 1,
            // jaPRO defaults.
            identify_target: true,
            names: 1.0,
            names_colours: true,
            names_opacity: 1.0,
        }
    }
}

/// TaystJK `cg_drawPlayerNames` settings. 0 disables labels, 1 draws names,
/// and values above 1 also draw the target's health bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerNameSettings {
    pub mode: i32,
    pub scale: f32,
}

impl Default for PlayerNameSettings {
    fn default() -> Self {
        Self {
            mode: 0,
            scale: 0.5,
        }
    }
}

/// World-space input for the render-thread player-label pass. Visibility is
/// decided by CGame/app traces; projection is deliberately deferred until the
/// final render camera so late-latched mouse motion keeps labels glued to heads.
#[derive(Debug, Clone, PartialEq)]
pub struct UiWorldPlayerLabel {
    pub anchor: [f32; 3],
    pub text: String,
    pub health_fraction: Option<f32>,
    pub health_color: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiWorldPlayerNames {
    pub scale: f32,
    pub labels: std::sync::Arc<Vec<UiWorldPlayerLabel>>,
}

/// TaystJK-compatible movement-key HUD controls. `mode` matches cg_movementKeys:
/// 0 off, 1 original, 2 original + attack, 3 compact centered, 4 compact movable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementKeysSettings {
    pub mode: u8,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub walk: bool,
}

impl Default for MovementKeysSettings {
    fn default() -> Self {
        Self {
            mode: 0,
            x: 0.0,
            y: 0.0,
            size: 1.0,
            walk: false,
        }
    }
}

pub const SHELPER_ORIGINAL: u32 = 1 << 0;
pub const SHELPER_UPDATED: u32 = 1 << 1;
pub const SHELPER_CGAZ: u32 = 1 << 2;
/// DinurdoJK-only world-space presentation. TaystJK currently occupies
/// bits 0..=20 (`SHELPER_ACCELZONES` is bit 20), so keep this extension next.
pub const SHELPER_CINEMATIC: u32 = 1 << 21;
pub const SHELPER_W: u32 = 1 << 5;
pub const SHELPER_WA: u32 = 1 << 6;
pub const SHELPER_WD: u32 = 1 << 7;
pub const SHELPER_A: u32 = 1 << 8;
pub const SHELPER_D: u32 = 1 << 9;
pub const SHELPER_REAR: u32 = 1 << 10;
pub const SHELPER_CENTER: u32 = 1 << 11;
/// jaPRO's line crosshair: replaces the normal crosshair with a short vertical line.
pub const SHELPER_CROSSHAIR: u32 = 1 << 14;
pub const SHELPER_S: u32 = 1 << 15;
pub const SHELPER_SA: u32 = 1 << 16;
pub const SHELPER_SD: u32 = 1 << 17;
pub const SHELPER_TINY: u32 = 1 << 18;
pub const SHELPER_ACCELMETER: u32 = 1 << 12;
pub const SHELPER_MAX: u32 = 1 << 19;
pub const SHELPER_STYLE_MASK: u32 =
    SHELPER_ORIGINAL | SHELPER_UPDATED | SHELPER_CGAZ | SHELPER_CINEMATIC;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrafeHelperSettings {
    pub flags: u32,
    pub fps: f32,
    pub offset: f32,
    pub line_width: f32,
    pub precision: u32,
    pub cutoff: f32,
    pub active_color: [u8; 4],
    pub inactive_alpha: u8,
}

impl Default for StrafeHelperSettings {
    fn default() -> Self {
        Self {
            // TaystJK default: WA|WD|A|D|CENTER, with no visual style bit.
            flags: 3008,
            fps: 0.0,
            offset: 75.0,
            line_width: 1.0,
            precision: 256,
            cutoff: 0.0,
            active_color: [0, 255, 0, 200],
            inactive_alpha: 200,
        }
    }
}

/// Everything the movement HUD (strafehelper, movement keys) reads per frame.
/// Angles are q3 degrees (pitch positive looks down), positions q3 units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementHudState {
    pub forward_move: i8,
    pub right_move: i8,
    pub up_move: i8,
    pub buttons: i32,
    pub velocity: [f32; 3],
    /// Player origin in JKA world coordinates. Cinematic strafehelper rays are
    /// anchored from this point; 2D styles ignore it.
    pub origin: [f32; 3],
    /// Rendered view orientation; the strafehelper projects through it.
    pub view_yaw: f32,
    pub view_pitch: f32,
    pub view_roll: f32,
    pub player_speed: f32,
    /// On the ground now, and on the previous pmove step (friction needs both).
    pub grounded: bool,
    pub was_grounded: bool,
    pub fov_x: f32,
    /// jaPRO `STAT_MOVEMENTSTYLE` (`MV_JKA` off jaPRO servers).
    pub move_style: i32,
    pub knockback: bool,
    /// `pm_type == PM_JETPACK` and `EF_JETPACK_ACTIVE`.
    pub jetpack_pm_type: bool,
    pub jetpack_active: bool,
    pub in_vehicle: bool,
    /// True only while the local view is a free-roaming spectator. TaystJK's
    /// strafehelper describes player movement, not PM_SPECTATOR fly movement;
    /// followed spectators remain eligible because PMF_FOLLOW supplies a real
    /// movement subject.
    pub spectator_free_roam: bool,
    pub third_person: bool,
    /// Player origin minus the rendered eye position.
    pub eye_to_origin: [f32; 3],
}

impl Default for MovementHudState {
    fn default() -> Self {
        Self {
            forward_move: 0,
            right_move: 0,
            up_move: 0,
            buttons: 0,
            velocity: [0.0; 3],
            origin: [0.0; 3],
            view_yaw: 0.0,
            view_pitch: 0.0,
            view_roll: 0.0,
            player_speed: 250.0,
            grounded: false,
            was_grounded: false,
            fov_x: 90.0,
            move_style: crate::strafehelper::mv::JKA,
            knockback: false,
            jetpack_pm_type: false,
            jetpack_active: false,
            in_vehicle: false,
            spectator_free_roam: false,
            third_person: false,
            eye_to_origin: [0.0; 3],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleSelection {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleSearchMatch {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// Render-only span for an engine-authored clickable local filesystem path.
/// The actual target path remains on the app side and is revalidated on click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsolePathLinkUi {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleSuggestKind {
    Cvar,
    Command,
    Server,
}

/// One row of the console's live command/cvar filter popup.
#[derive(Debug, Clone)]
pub struct ConsoleSuggestion {
    pub name: &'static str,
    pub kind: ConsoleSuggestKind,
    /// Bit `i` set = byte `i` of `name` matched what was typed.
    pub mask: u64,
    /// Current cvar value (empty for commands or unknown values).
    pub value: String,
    /// The value differs from the registered default.
    pub modified: bool,
    pub default_value: &'static str,
    pub range: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConsoleSize {
    #[default]
    Normal,
    Half,
    Full,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PerfStats {
    pub fps: f64,
    pub frame_ms: f64,
    pub cpu_prepare_ms: f64,
    pub cpu_acquire_ms: f64,
    pub cpu_encode_ms: f64,
    pub cpu_submit_ms: f64,
    pub cpu_present_ms: f64,
    pub dynamic_model_prepare_ms: f64,
    pub dynamic_model_surfaces: u32,
    pub dynamic_model_vertices: u64,
    pub dynamic_model_indices: u64,
    pub client_total_ms: f64,
    pub client_snapshot_ms: f64,
    pub client_audio_ms: f64,
    pub client_events_ms: f64,
    pub client_event_prepare_ms: f64,
    pub client_event_worker_jobs: u32,
    pub client_event_worker_threads: u32,
    pub client_event_worker_parallel: bool,
    pub client_event_sound_decode_ms: f64,
    pub client_event_sound_decode_jobs: u32,
    pub client_event_sound_decode_parallel: bool,
    pub client_entity_present_ms: f64,
    pub client_player_present_ms: f64,
    pub client_followed_player_ms: f64,
    pub client_fx_ms: f64,
    pub client_fx_tessellate_ms: f64,
    pub client_fx_draws: u32,
    pub client_fx_sprites: u32,
    pub client_fx_oriented_quads: u32,
    pub client_fx_lines: u32,
    pub client_fx_quads: u32,
    pub client_fx_meshes: u32,
    pub client_fx_cylinders: u32,
    pub client_fx_render_surfaces: u32,
    pub client_fx_cpu_geom_surfaces: u32,
    pub client_fx_cpu_vertices: u64,
    pub client_fx_cpu_indices: u64,
    pub client_fx_gpu_sprite_batches: u32,
    pub client_fx_gpu_sprite_instances: u32,
    pub ghoul2_pose_ms: f64,
    pub ghoul2_motion_pose_ms: f64,
    pub ghoul2_skin_ms: f64,
    pub ghoul2_bolt_ms: f64,
    pub ghoul2_pose_evals: u32,
    pub ghoul2_motion_pose_evals: u32,
    pub ghoul2_bolt_queries: u32,
    pub ghoul2_surfaces: u32,
    pub ghoul2_vertices: u64,
    pub ghoul2_frustum_tests: u32,
    pub ghoul2_frustum_culled: u32,
    pub ghoul2_lod_counts: [u32; 4],
    pub gpu_ms: Option<f64>,
    pub gpu_depth_ms: Option<f64>,
    pub gpu_hiz_ms: Option<f64>,
    pub gpu_cull_ms: Option<f64>,
    pub gpu_cluster_ms: Option<f64>,
    pub gpu_world_ms: Option<f64>,
    pub gpu_fx_sprites_ms: Option<f64>,
    pub gpu_post_ms: Option<f64>,
    pub gpu_ui_ms: Option<f64>,
    pub cull_visible: u32,
    pub cull_frustum_rejected: u32,
    pub cull_hiz_rejected: u32,
    pub cull_pvs_rejected: u32,
    pub cull_area_rejected: u32,
    pub input_event_to_sim_ms: Option<f64>,
    pub input_sim_to_render_ms: Option<f64>,
    pub input_event_to_latch_ms: Option<f64>,
    pub input_latch_to_submit_ms: Option<f64>,
    pub input_latch_to_present_call_ms: Option<f64>,
    pub input_event_to_present_call_ms: Option<f64>,
    pub input_event_to_present_call_max_ms: Option<f64>,
    pub input_latency_samples: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadPerfStats {
    pub name: &'static str,
    pub task: &'static str,
    pub active: bool,
    pub busy_percent: f32,
}

impl Default for ThreadPerfStats {
    fn default() -> Self {
        Self {
            name: "THREAD",
            task: "IDLE",
            active: false,
            busy_percent: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MapLoadingBar {
    pub label: &'static str,
    pub completed: u32,
    pub total: u32,
    pub skipped: bool,
}

#[derive(Debug, Clone)]
pub struct MapLoadingUi {
    pub map_name: String,
    /// Active fs_game/search directory for this load, if any. The renderer uses
    /// it to resolve mod-provided levelshots after first presenting OpenJK MP's
    /// resident `menu/art/unknownmap_mp` fallback.
    pub active_game_dir: Option<String>,
    pub preparation_finished: bool,
    pub bars: Vec<MapLoadingBar>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictionDebugUi {
    /// True while cg_predictionDebug is enabled. The HUD panel is shown even
    /// before the first miss so a tester can verify instrumentation is active.
    pub show_panel: bool,
    /// Flash the screen edge for a recent miss above the configured threshold.
    pub flash: bool,
    pub threshold: f32,
    pub last_miss: Option<f32>,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiForceSelect {
    pub selected: u8,
    pub known_bits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiDuelMiniScore {
    pub name: String,
    pub score: Option<i32>,
    /// Cell in the shared lazy player-icon atlas, matching clientInfo.modelIcon.
    pub model_icon: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiMiniScores {
    Team {
        mode: i32,
        red: Option<i32>,
        blue: Option<i32>,
    },
    Duel {
        icon_paths: Vec<String>,
        blue: UiDuelMiniScore,
        red: UiDuelMiniScore,
    },
}

#[derive(Debug, Clone)]
pub struct UiSnapshot {
    pub setup_selected: usize,
    pub mode: OverlayMode,
    pub console_input: String,
    /// UTF-8 byte offset of the editable caret in `console_input`.
    pub console_cursor: usize,
    pub console_status: String,
    pub console_lines: Vec<String>,
    pub console_scroll: usize,
    pub console_size: ConsoleSize,
    pub console_selection: Option<ConsoleSelection>,
    pub console_search_open: bool,
    pub console_search_query: String,
    pub console_search_total: usize,
    pub console_search_index: Option<usize>,
    pub console_search_matches: Vec<ConsoleSearchMatch>,
    pub console_search_active: Option<ConsoleSearchMatch>,
    pub console_path_links: Vec<ConsolePathLinkUi>,
    /// Total retained log lines and how many the view is scrolled up from the newest.
    pub console_total_lines: usize,
    pub console_scrolled: usize,
    pub console_suggest_enabled: bool,
    pub console_suggestions: Vec<ConsoleSuggestion>,
    pub console_suggest_total: usize,
    /// Highlighted row (row 0 while nothing has been explicitly picked).
    pub console_suggest_selected: usize,
    /// Single-entry argument hint shown while typing after a complete name.
    pub console_suggest_hint: bool,
    /// Pointer is over the header `?` button.
    pub console_help_hover: bool,
    pub chat_mode: ChatMode,
    pub chat_input: String,
    pub chat_lines: Vec<UiChatLine>,
    pub center_print: Option<UiCenterPrint>,
    pub follow_name: Option<String>,
    /// TaystJK/OpenJK `cg_drawTimer` elapsed level time, preformatted as M:SS.
    pub game_timer: Option<String>,
    /// TaystJK `cg_drawScores` compact score HUD.
    pub mini_scores: Option<UiMiniScores>,
    /// jaPRO `cg_raceTimer` / `cg_raceStart` readout (updates every frame of a run).
    pub race_timer: Option<crate::japro_cg::RaceTimerUi>,
    /// `CG_DrawVote`: the open vote's summary line (colour codes included).
    pub vote_line: Option<String>,
    pub scoreboard: Option<UiScoreboard>,
    /// TaystJK scoreboard ownership: `cg.snap->ps.clientNum`. While following
    /// another player this is the followed client, so their row is highlighted
    /// instead of the spectator's own row.
    pub scoreboard_focus_client: Option<i32>,
    pub demo_timeline: Option<DemoTimelineUi>,
    pub prediction_debug: Option<PredictionDebugUi>,
    pub hud: Option<HudState>,
    pub team_overlay: Option<TeamOverlayUi>,
    pub hud_layout: HudLayout,
    pub crosshair: CrosshairSettings,
    pub crosshair_target: UiCrosshairTarget,
    /// OpenJK/TaystJK force-power selector shown briefly after forcenext/forceprev.
    pub force_select: Option<UiForceSelect>,
    pub movement_keys: MovementKeysSettings,
    pub strafe_helper: StrafeHelperSettings,
    pub movement_hud: MovementHudState,
    /// jaPRO `cg_speedometer` draw list for this frame.
    pub speedometer: Option<crate::speedometer::Ui>,
    /// jaPRO `CG_DrawLagometer` draw list for this frame (graph and connection warning).
    pub lagometer: Option<crate::lagometer::Ui>,
    pub video: VideoSettings,
    pub perf: PerfStats,
    pub threads: [ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
    pub surface_inspector: Option<crate::runtime::SurfaceInspectorInfo>,
    /// One deliberately presented frame of the preloaded static splash before
    /// startup kicks the first map request. This prevents the first visible WGPU
    /// frame from being an empty/no-world clear.
    pub startup_splash: bool,
    pub loading: Option<MapLoadingUi>,
    pub static_ao_progress: Option<MapLoadingBar>,
}
impl Default for UiSnapshot {
    fn default() -> Self {
        Self {
            setup_selected: 1,
            mode: OverlayMode::None,
            console_input: String::new(),
            console_cursor: 0,
            console_status: String::new(),
            console_lines: Vec::new(),
            console_scroll: 0,
            console_size: ConsoleSize::Normal,
            console_selection: None,
            console_search_open: false,
            console_search_query: String::new(),
            console_search_total: 0,
            console_search_index: None,
            console_search_matches: Vec::new(),
            console_search_active: None,
            console_path_links: Vec::new(),
            console_total_lines: 0,
            console_scrolled: 0,
            console_suggest_enabled: true,
            console_suggestions: Vec::new(),
            console_suggest_total: 0,
            console_suggest_selected: 0,
            console_suggest_hint: false,
            console_help_hover: false,
            chat_mode: ChatMode::Global,
            chat_input: String::new(),
            chat_lines: Vec::new(),
            center_print: None,
            follow_name: None,
            game_timer: None,
            mini_scores: None,
            race_timer: None,
            vote_line: None,
            scoreboard: None,
            scoreboard_focus_client: None,
            demo_timeline: None,
            prediction_debug: None,
            hud: None,
            team_overlay: None,
            hud_layout: HudLayout::default(),
            crosshair: CrosshairSettings::default(),
            crosshair_target: UiCrosshairTarget::default(),
            force_select: None,
            movement_keys: MovementKeysSettings::default(),
            strafe_helper: StrafeHelperSettings::default(),
            movement_hud: MovementHudState::default(),
            speedometer: None,
            lagometer: None,
            video: VideoSettings::default(),
            perf: PerfStats::default(),
            threads: [ThreadPerfStats::default(); crate::thread_activity::SLOT_COUNT],
            surface_inspector: None,
            startup_splash: false,
            loading: None,
            static_ao_progress: None,
        }
    }
}

#[cfg(test)]
#[path = "ui/tests/crosshair_tests.rs"]
mod crosshair_tests;

#[cfg(test)]
#[path = "ui/tests/shadow_mode_tests.rs"]
mod shadow_mode_tests;
