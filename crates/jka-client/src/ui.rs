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

#[derive(Debug, Clone)]
pub struct UiScoreEntry {
    pub name: String,
    pub score: i32,
    pub ping: i32,
    pub time: i32,
    pub team: i32,
}

#[derive(Debug, Clone)]
pub struct UiScoreboard {
    pub team_scores: [i32; 2],
    pub team_game: bool,
    /// Duel and power duel keep a win/loss score for the spectators waiting in line;
    /// every other mode shows only ping and time for them, like jaPRO's scoreboard.
    pub spectator_scores: bool,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HudState {
    pub health: i32,
    pub max_health: i32,
    pub armor: i32,
    pub force_power: i32,
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
            // jaPRO defaults.
            identify_target: true,
            names: 1.0,
            names_colours: true,
            names_opacity: 1.0,
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudElementId {
    Health,
    Shield,
    Ammo,
    Force,
    MovementKeys,
    Fps,
    Chat,
    CenterPrint,
    CrosshairName,
    Follow,
    Vote,
    RaceTimer,
    RaceStart,
    SurfaceInspector,
    Speedometer,
    SpeedometerJumps,
    SpeedGraph,
}

impl HudElementId {
    // SurfaceInspector is intentionally excluded: the trace panel it used to
    // position is now the clickable `OverlayMode::Trace` egui menu, which
    // isn't a draggable HUD element. The variant, its cvar prefix and its
    // offset field stay (see `element`/`element_mut`) so old configs still load.
    pub const ALL: [Self; 16] = [
        Self::Health,
        Self::Shield,
        Self::Ammo,
        Self::Force,
        Self::MovementKeys,
        Self::Fps,
        Self::Chat,
        Self::CenterPrint,
        Self::CrosshairName,
        Self::Follow,
        Self::Vote,
        Self::RaceTimer,
        Self::RaceStart,
        Self::Speedometer,
        Self::SpeedometerJumps,
        Self::SpeedGraph,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Health => "HEALTH",
            Self::Shield => "SHIELD",
            Self::Ammo => "AMMO / STYLE",
            Self::Force => "FORCE",
            Self::MovementKeys => "MOVEMENT KEYS",
            Self::Fps => "FPS / PERF",
            Self::Chat => "CHAT",
            Self::CenterPrint => "CENTER PRINT",
            Self::CrosshairName => "CROSSHAIR NAME",
            Self::Follow => "FOLLOW NAME",
            Self::Vote => "VOTE",
            Self::RaceTimer => "RACE TIMER",
            Self::RaceStart => "RACE START SPEED",
            Self::SurfaceInspector => "TRACE INSPECTOR",
            Self::Speedometer => "SPEEDOMETER",
            Self::SpeedometerJumps => "SPEEDOMETER JUMPS",
            Self::SpeedGraph => "SPEED GRAPH",
        }
    }

    pub fn cvar_name(self) -> &'static str {
        match self {
            Self::Health => "cg_hudHealth",
            Self::Shield => "cg_hudShield",
            Self::Ammo => "cg_hudAmmo",
            Self::Force => "cg_hudForce",
            Self::MovementKeys => "cg_hudMovementKeys",
            Self::Fps => "cg_hudFps",
            Self::Chat => "cg_hudChat",
            Self::CenterPrint => "cg_hudCenterPrint",
            Self::CrosshairName => "cg_hudCrosshairName",
            Self::Follow => "cg_hudFollow",
            Self::Vote => "cg_hudVote",
            Self::RaceTimer => "cg_hudRaceTimer",
            Self::RaceStart => "cg_hudRaceStart",
            Self::SurfaceInspector => "cg_hudSurfaceInspector",
            Self::Speedometer => "cg_hudSpeedometer",
            Self::SpeedometerJumps => "cg_hudSpeedometerJumps",
            Self::SpeedGraph => "cg_hudSpeedGraph",
        }
    }

    pub fn from_cvar(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|id| id.cvar_name().eq_ignore_ascii_case(name))
    }

    /// The original four panels position themselves from a screen anchor. Every
    /// other element keeps its own stock layout (cvar-driven 640x480 positions,
    /// text flow, ...) and its layout is an offset/scale applied on top of it.
    pub fn is_anchored(self) -> bool {
        matches!(self, Self::Health | Self::Shield | Self::Ammo | Self::Force)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl HudAnchor {
    pub fn short_name(self) -> &'static str {
        match self {
            Self::TopLeft => "tl",
            Self::TopCenter => "tc",
            Self::TopRight => "tr",
            Self::CenterLeft => "cl",
            Self::Center => "c",
            Self::CenterRight => "cr",
            Self::BottomLeft => "bl",
            Self::BottomCenter => "bc",
            Self::BottomRight => "br",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tl" | "topleft" => Some(Self::TopLeft),
            "tc" | "topcenter" => Some(Self::TopCenter),
            "tr" | "topright" => Some(Self::TopRight),
            "cl" | "centerleft" => Some(Self::CenterLeft),
            "c" | "center" => Some(Self::Center),
            "cr" | "centerright" => Some(Self::CenterRight),
            "bl" | "bottomleft" => Some(Self::BottomLeft),
            "bc" | "bottomcenter" => Some(Self::BottomCenter),
            "br" | "bottomright" => Some(Self::BottomRight),
            _ => None,
        }
    }

    fn screen_fraction(self) -> [f32; 2] {
        match self {
            Self::TopLeft => [0.0, 0.0],
            Self::TopCenter => [0.5, 0.0],
            Self::TopRight => [1.0, 0.0],
            Self::CenterLeft => [0.0, 0.5],
            Self::Center => [0.5, 0.5],
            Self::CenterRight => [1.0, 0.5],
            Self::BottomLeft => [0.0, 1.0],
            Self::BottomCenter => [0.5, 1.0],
            Self::BottomRight => [1.0, 1.0],
        }
    }

    fn element_fraction(self) -> [f32; 2] {
        self.screen_fraction()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudElementLayout {
    pub anchor: HudAnchor,
    /// Pixel offset from the selected screen anchor. Positive X is right and
    /// positive Y is down; the element is aligned to the same anchor point.
    pub offset: [f32; 2],
    pub scale: f32,
}

impl HudElementLayout {
    pub fn to_config(self) -> String {
        format!(
            "{} {:.3} {:.3} {:.3}",
            self.anchor.short_name(),
            self.offset[0],
            self.offset[1],
            self.scale
        )
    }

    pub fn from_config(value: &str) -> Option<Self> {
        let mut words = value.split_whitespace();
        let anchor = HudAnchor::from_config(words.next()?)?;
        let x = words.next()?.parse::<f32>().ok()?;
        let y = words.next()?.parse::<f32>().ok()?;
        let scale = words.next()?.parse::<f32>().ok()?;
        if words.next().is_some() || !x.is_finite() || !y.is_finite() || !scale.is_finite() {
            return None;
        }
        Some(Self {
            anchor,
            offset: [x.clamp(-16384.0, 16384.0), y.clamp(-16384.0, 16384.0)],
            scale: scale.clamp(0.5, 2.0),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudLayout {
    pub health: HudElementLayout,
    pub shield: HudElementLayout,
    pub ammo: HudElementLayout,
    pub force: HudElementLayout,
    pub movement_keys: HudElementLayout,
    pub fps: HudElementLayout,
    pub chat: HudElementLayout,
    pub center_print: HudElementLayout,
    pub crosshair_name: HudElementLayout,
    pub follow: HudElementLayout,
    pub vote: HudElementLayout,
    pub race_timer: HudElementLayout,
    pub race_start: HudElementLayout,
    pub surface_inspector: HudElementLayout,
    pub speedometer: HudElementLayout,
    pub speedometer_jumps: HudElementLayout,
    pub speed_graph: HudElementLayout,
    pub snap_to_grid: bool,
    pub grid_size: f32,
}

/// Layout of an element that has no anchor of its own: no offset, stock scale.
const HUD_UNMOVED: HudElementLayout = HudElementLayout {
    anchor: HudAnchor::TopLeft,
    offset: [0.0, 0.0],
    scale: 1.0,
};

impl Default for HudLayout {
    fn default() -> Self {
        Self {
            movement_keys: HUD_UNMOVED,
            fps: HUD_UNMOVED,
            chat: HUD_UNMOVED,
            center_print: HUD_UNMOVED,
            crosshair_name: HUD_UNMOVED,
            follow: HUD_UNMOVED,
            vote: HUD_UNMOVED,
            race_timer: HUD_UNMOVED,
            race_start: HUD_UNMOVED,
            surface_inspector: HUD_UNMOVED,
            speedometer: HUD_UNMOVED,
            speedometer_jumps: HUD_UNMOVED,
            speed_graph: HUD_UNMOVED,
            // These resolve to the exact pre-editor positions at scale 1.0.
            health: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -58.0],
                scale: 1.0,
            },
            shield: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -28.0],
                scale: 1.0,
            },
            ammo: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -58.0],
                scale: 1.0,
            },
            force: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -28.0],
                scale: 1.0,
            },
            snap_to_grid: true,
            grid_size: 8.0,
        }
    }
}

impl HudLayout {
    pub fn element(self, id: HudElementId) -> HudElementLayout {
        match id {
            HudElementId::Health => self.health,
            HudElementId::Shield => self.shield,
            HudElementId::Ammo => self.ammo,
            HudElementId::Force => self.force,
            HudElementId::MovementKeys => self.movement_keys,
            HudElementId::Fps => self.fps,
            HudElementId::Chat => self.chat,
            HudElementId::CenterPrint => self.center_print,
            HudElementId::CrosshairName => self.crosshair_name,
            HudElementId::Follow => self.follow,
            HudElementId::Vote => self.vote,
            HudElementId::RaceTimer => self.race_timer,
            HudElementId::RaceStart => self.race_start,
            HudElementId::SurfaceInspector => self.surface_inspector,
            HudElementId::Speedometer => self.speedometer,
            HudElementId::SpeedometerJumps => self.speedometer_jumps,
            HudElementId::SpeedGraph => self.speed_graph,
        }
    }

    pub fn element_mut(&mut self, id: HudElementId) -> &mut HudElementLayout {
        match id {
            HudElementId::Health => &mut self.health,
            HudElementId::Shield => &mut self.shield,
            HudElementId::Ammo => &mut self.ammo,
            HudElementId::Force => &mut self.force,
            HudElementId::MovementKeys => &mut self.movement_keys,
            HudElementId::Fps => &mut self.fps,
            HudElementId::Chat => &mut self.chat,
            HudElementId::CenterPrint => &mut self.center_print,
            HudElementId::CrosshairName => &mut self.crosshair_name,
            HudElementId::Follow => &mut self.follow,
            HudElementId::Vote => &mut self.vote,
            HudElementId::RaceTimer => &mut self.race_timer,
            HudElementId::RaceStart => &mut self.race_start,
            HudElementId::SurfaceInspector => &mut self.surface_inspector,
            HudElementId::Speedometer => &mut self.speedometer,
            HudElementId::SpeedometerJumps => &mut self.speedometer_jumps,
            HudElementId::SpeedGraph => &mut self.speed_graph,
        }
    }

    pub fn reset_element(&mut self, id: HudElementId) {
        *self.element_mut(id) = HudLayout::default().element(id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The settings that decide where the non-anchored HUD elements sit on screen,
/// so the renderer and the HUD editor agree on each element's stock rectangle.
#[derive(Debug, Clone, Copy)]
pub struct HudRectContext {
    pub movement_keys: MovementKeysSettings,
    pub draw_fps: u8,
    pub gpu_enabled: bool,
    pub cull_debug: bool,
    pub thread_count: usize,
    /// `cg_raceTimer` x, y (640x480) and text size.
    pub race_timer: [f32; 3],
    /// `cg_raceStart` x, y (640x480).
    pub race_start: [f32; 2],
    pub speedometer: crate::speedometer::Settings,
}

impl HudRectContext {
    pub fn new(
        movement_keys: MovementKeysSettings,
        video: &VideoSettings,
        perf: &PerfStats,
        thread_count: usize,
        race: &crate::japro_cg::JaproCgame,
    ) -> Self {
        Self {
            movement_keys,
            draw_fps: video.draw_fps,
            gpu_enabled: perf.gpu_ms.is_some(),
            cull_debug: video.cull_debug != CullDebugMode::Off,
            thread_count,
            race_timer: [race.race_timer_x, race.race_timer_y, race.race_timer_size],
            race_start: [race.race_start_x, race.race_start_y],
            speedometer: race.speedometer,
        }
    }

    pub fn from_snapshot(ui: &UiSnapshot) -> Self {
        let mut race = crate::japro_cg::JaproCgame::default();
        if let Some(live) = ui.race_timer.as_ref() {
            race.race_timer_x = live.timer_x;
            race.race_timer_y = live.timer_y;
            race.race_timer_size = live.size;
            race.race_start_x = live.start_x;
            race.race_start_y = live.start_y;
        }
        if let Some(speedometer) = ui.speedometer.as_ref() {
            race.speedometer = speedometer.settings;
        }
        Self::new(ui.movement_keys, &ui.video, &ui.perf, ui.threads.len(), &race)
    }
}

/// Stock (untransformed) rectangle of an element that is not anchored. These are
/// the bounds the editor outlines; text elements are sized for typical content.
fn hud_stock_rect(id: HudElementId, ctx: &HudRectContext, w: u32, h: u32) -> HudRect {
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let virtual_rect = |x: f32, y: f32, width: f32, height: f32| HudRect {
        x: x * sx,
        y: y * sy,
        width: width * sx,
        height: height * sy,
    };
    match id {
        HudElementId::MovementKeys => {
            let mut settings = ctx.movement_keys;
            settings.mode = settings.mode.clamp(1, 4);
            let Some((tile, x, y)) = movement_keys_origin(&settings, w, h) else {
                return HudRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
            };
            let first_col = if settings.walk { -1.0 } else { 0.0 };
            let last_col = if settings.mode == 2 { 4.0 } else { 3.0 };
            let rows = if settings.mode >= 3 { 3.0 } else { 2.0 };
            HudRect {
                x: x + first_col * tile,
                y,
                width: (last_col - first_col) * tile,
                height: rows * tile,
            }
        }
        HudElementId::Fps if ctx.draw_fps >= 2 => {
            let (x, y, width, height) =
                perf_panel_rect(w, ctx.gpu_enabled, ctx.cull_debug, ctx.thread_count);
            HudRect { x, y, width, height }
        }
        HudElementId::Fps => {
            let width = 7.0 * 8.0 * 1.55;
            HudRect {
                x: (w as f32 - 12.0 - width).max(12.0),
                y: 12.0,
                width,
                height: 13.0,
            }
        }
        HudElementId::Chat => virtual_rect(30.0, 373.0, 400.0, 52.0),
        HudElementId::CenterPrint => virtual_rect(170.0, 126.0, 300.0, 36.0),
        HudElementId::CrosshairName => virtual_rect(220.0, 168.0, 200.0, 22.0),
        HudElementId::Follow => virtual_rect(4.0, 14.0, 160.0, 16.0),
        HudElementId::Vote => virtual_rect(4.0, 62.0, 340.0, 30.0),
        HudElementId::RaceTimer => {
            let k = ctx.race_timer[2] / 0.75;
            virtual_rect(ctx.race_timer[0], ctx.race_timer[1], 80.0 * k, 60.0 * k)
        }
        HudElementId::RaceStart => {
            let k = ctx.race_timer[2] / 0.75;
            virtual_rect(ctx.race_start[0], ctx.race_start[1], 80.0 * k, 20.0 * k)
        }
        HudElementId::Speedometer => {
            let s = &ctx.speedometer;
            virtual_rect(s.x - 2.0, s.y - 14.0, 212.0, 20.0)
        }
        HudElementId::SpeedometerJumps => {
            let s = &ctx.speedometer;
            virtual_rect(s.jumps_x, s.jumps_y - 14.0, 260.0, 20.0)
        }
        HudElementId::SpeedGraph => {
            use crate::speedometer::flag;
            let flags = ctx.speedometer.flags;
            if flags & flag::SPEEDGRAPHOLD != 0 && flags & flag::SPEEDGRAPH == 0 {
                virtual_rect(544.0, 168.0, 48.0, 144.0)
            } else {
                virtual_rect(245.0, 456.0, 150.0, 22.0)
            }
        }
        HudElementId::SurfaceInspector => {
            let (x, y, width) = surface_inspector_frame(w);
            HudRect { x, y, width, height: 300.0_f32.min((h as f32 - y - 18.0).max(0.0)) }
        }
        HudElementId::Health | HudElementId::Shield | HudElementId::Ammo | HudElementId::Force => {
            HudRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
        }
    }
}

/// Scale a drawn element about the centre of its stock rectangle, then shift it.
fn apply_hud_layout(
    vertices: &mut [UiVertex],
    id: HudElementId,
    layout: HudElementLayout,
    ctx: &HudRectContext,
    w: u32,
    h: u32,
) {
    let scale = layout.scale.clamp(0.5, 2.0);
    if (scale == 1.0 && layout.offset == [0.0, 0.0]) || w == 0 || h == 0 {
        return;
    }
    let stock = hud_stock_rect(id, ctx, w, h);
    let (cx, cy) = (stock.x + stock.width * 0.5, stock.y + stock.height * 0.5);
    let (wf, hf) = (w as f32, h as f32);
    for vertex in vertices {
        let px = (vertex.position[0] + 1.0) * 0.5 * wf;
        let py = (1.0 - vertex.position[1]) * 0.5 * hf;
        let px = cx + (px - cx) * scale + layout.offset[0];
        let py = cy + (py - cy) * scale + layout.offset[1];
        vertex.position = [px / wf * 2.0 - 1.0, 1.0 - py / hf * 2.0];
    }
}

/// Run `draw` and position whatever it emitted according to element `id`'s layout.
fn draw_placed(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    id: HudElementId,
    w: u32,
    h: u32,
    draw: impl FnOnce(&mut Vec<UiVertex>),
) {
    let start = out.len();
    draw(out);
    if out.len() > start {
        let ctx = HudRectContext::from_snapshot(ui);
        apply_hud_layout(&mut out[start..], id, ui.hud_layout.element(id), &ctx, w, h);
    }
}

pub fn hud_element_rect(
    id: HudElementId,
    layout: HudElementLayout,
    ctx: &HudRectContext,
    w: u32,
    h: u32,
) -> HudRect {
    let scale = layout.scale.clamp(0.5, 2.0);
    if !id.is_anchored() {
        let stock = hud_stock_rect(id, ctx, w, h);
        let (width, height) = (stock.width * scale, stock.height * scale);
        return HudRect {
            x: stock.x + stock.width * 0.5 + layout.offset[0] - width * 0.5,
            y: stock.y + stock.height * 0.5 + layout.offset[1] - height * 0.5,
            width,
            height,
        };
    }
    let base_panel_w = 218.0_f32.min((w as f32 * 0.30).max(160.0));
    let base_size = [base_panel_w, 24.0];
    let width = base_size[0] * scale;
    let height = base_size[1] * scale;
    let screen = layout.anchor.screen_fraction();
    let align = layout.anchor.element_fraction();
    let anchor_x = screen[0] * w as f32 + layout.offset[0];
    let anchor_y = screen[1] * h as f32 + layout.offset[1];
    HudRect {
        x: anchor_x - align[0] * width,
        y: anchor_y - align[1] * height,
        width,
        height,
    }
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
        Self { mode: 0, x: 0.0, y: 0.0, size: 1.0, walk: false }
    }
}

pub const SHELPER_ORIGINAL: u32 = 1 << 0;
pub const SHELPER_UPDATED: u32 = 1 << 1;
pub const SHELPER_CGAZ: u32 = 1 << 2;
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
pub const SHELPER_STYLE_MASK: u32 = SHELPER_ORIGINAL | SHELPER_UPDATED | SHELPER_CGAZ;

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
    pub third_person: bool,
    /// Player origin minus the rendered eye position.
    pub eye_to_origin: [f32; 3],
}

impl Default for MovementHudState {
    fn default() -> Self {
        Self {
            forward_move: 0, right_move: 0, up_move: 0, buttons: 0,
            velocity: [0.0; 3], view_yaw: 0.0, view_pitch: 0.0, view_roll: 0.0,
            player_speed: 250.0, grounded: false, was_grounded: false, fov_x: 90.0,
            move_style: crate::strafehelper::mv::JKA,
            knockback: false, jetpack_pm_type: false, jetpack_active: false,
            in_vehicle: false, third_person: false, eye_to_origin: [0.0; 3],
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextureFilter {
    Nearest,
    Bilinear,
    #[default]
    Trilinear,
    Anisotropic2x,
    Anisotropic4x,
    Anisotropic8x,
    Anisotropic16x,
}

impl TextureFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "NEAREST",
            Self::Bilinear => "BILINEAR",
            Self::Trilinear => "TRILINEAR",
            Self::Anisotropic2x => "ANISOTROPIC 2X",
            Self::Anisotropic4x => "ANISOTROPIC 4X",
            Self::Anisotropic8x => "ANISOTROPIC 8X",
            Self::Anisotropic16x => "ANISOTROPIC 16X",
        }
    }

    pub fn anisotropy_clamp(self) -> u16 {
        match self {
            Self::Anisotropic2x => 2,
            Self::Anisotropic4x => 4,
            Self::Anisotropic8x => 8,
            Self::Anisotropic16x => 16,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailTextureMode {
    #[default]
    Off,
    /// Mid-grey-neutral 2x modulation. The detail sample is converted back to
    /// its encoded (legacy fixed-function-like) value before modulation so 128
    /// grey is approximately a no-op.
    Neutral2x,
    /// Direct linear-space equivalent of blendFunc GL_DST_COLOR GL_SRC_COLOR.
    Linear2x,
    /// Previous test path: blendFunc GL_DST_COLOR GL_ONE.
    DstColorOne,
    /// Plain multiplicative modulation: blendFunc GL_DST_COLOR GL_ZERO.
    Multiply,
}

impl DetailTextureMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Neutral2x,
        Self::Linear2x,
        Self::DstColorOne,
        Self::Multiply,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Neutral2x => "NEUTRAL 2X",
            Self::Linear2x => "LINEAR 2X",
            Self::DstColorOne => "DST COLOR + ONE",
            Self::Multiply => "MULTIPLY",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Neutral2x => "neutral2x",
            Self::Linear2x => "linear2x",
            Self::DstColorOne => "dstcolor_one",
            Self::Multiply => "multiply",
        }
    }

    /// Numeric value sent directly to the WGSL override constant.
    pub fn shader_mode(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Neutral2x => 1,
            Self::Linear2x => 2,
            Self::DstColorOne => 3,
            Self::Multiply => 4,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(Self::Off),
            // Preserve configs written by the first detail-texture patch.
            "1" | "on" | "true" | "enhanced" | "neutral2x" | "softlight" => Some(Self::Neutral2x),
            "2" | "linear2x" | "modulate2x" => Some(Self::Linear2x),
            "3" | "dstcolor_one" | "dstcolorone" => Some(Self::DstColorOne),
            "4" | "multiply" | "modulate" => Some(Self::Multiply),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PvsMode {
    Off,
    Minimal,
    Full,
    #[default]
    Auto,
}

impl PvsMode {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Full => "full",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FxGeometryMode {
    /// Current reference path: expand view-facing FX geometry on the presentation thread.
    #[default]
    Cpu,
    /// Preserve the exact same geometry/material semantics while parallelizing
    /// per-material FX expansion over the persistent Rayon worker pool.
    CpuWorkers,
    /// Keep OpenJK FX simulation on CPU, but submit billboard Particle sprites
    /// as compact WGPU instances instead of rebuilding quad vertices/indices.
    Gpu,
}

impl FxGeometryMode {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::CpuWorkers, Self::Gpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::CpuWorkers => "CPU WORKERS",
            Self::Gpu => "GPU",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuWorkers => "workers",
            Self::Gpu => "gpu",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "0" => Some(Self::Cpu),
            "workers" | "cpu_workers" | "cpu-workers" | "1" => Some(Self::CpuWorkers),
            "gpu" | "2" => Some(Self::Gpu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ghoul2SkinningMode {
    /// Faithful scalar CPU reference path.
    Cpu,
    /// Same OpenJK deformation math, batched over a persistent Rayon worker pool.
    CpuWorkers,
    /// CPU pose/bolt evaluation with GLM vertex deformation in the WGPU vertex shader.
    #[default]
    Gpu,
}

impl Ghoul2SkinningMode {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::CpuWorkers, Self::Gpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::CpuWorkers => "CPU WORKERS",
            Self::Gpu => "GPU",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuWorkers => "workers",
            Self::Gpu => "gpu",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "0" => Some(Self::Cpu),
            "workers" | "cpu_workers" | "cpu-workers" | "1" => Some(Self::CpuWorkers),
            "gpu" | "2" => Some(Self::Gpu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ghoul2BatchMode {
    Off,
    #[default]
    Adaptive,
    Force,
}

impl Ghoul2BatchMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Adaptive, Self::Force];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Adaptive => "ADAPTIVE",
            Self::Force => "FORCE",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "0",
            Self::Adaptive => "1",
            Self::Force => "2",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(Self::Off),
            "1" | "adaptive" | "auto" | "on" | "true" => Some(Self::Adaptive),
            "2" | "force" | "forced" => Some(Self::Force),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CullDebugMode {
    #[default]
    Off,
    RejectionReasons,
}

impl CullDebugMode {
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanarReflectionDebugMode {
    #[default]
    Off,
    Candidates,
    SelectedPlane,
    ReflectionTexture,
    AppliedSample,
    BindingTest,
}

impl PlanarReflectionDebugMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Candidates,
        Self::SelectedPlane,
        Self::ReflectionTexture,
        Self::AppliedSample,
        Self::BindingTest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Candidates => "CANDIDATES",
            Self::SelectedPlane => "SELECTED PLANE",
            Self::ReflectionTexture => "REFLECTION TEXTURE",
            Self::AppliedSample => "APPLIED SAMPLE",
            Self::BindingTest => "BINDING TEST",
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Candidates => 1.0,
            Self::SelectedPlane => 2.0,
            Self::ReflectionTexture => 3.0,
            Self::AppliedSample => 4.0,
            Self::BindingTest => 5.0,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Self::Off),
            "candidates" | "candidate" | "1" => Some(Self::Candidates),
            "selected" | "plane" | "selected_plane" | "2" => Some(Self::SelectedPlane),
            "texture" | "reflection_texture" | "3" => Some(Self::ReflectionTexture),
            "applied" | "sample" | "applied_sample" | "4" => Some(Self::AppliedSample),
            "binding" | "binding_test" | "source_test" | "5" => Some(Self::BindingTest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorLutPreset {
    #[default]
    Off,
    KodakVision3_250d,
    KodakPortra400,
    FujiEterna500,
    FujiVelvia50,
    /// Index into the `.cube` files found in `<base>/LUTs` (see `color_lut`).
    External(u16),
}

impl ColorLutPreset {
    const BUILT_IN: [Self; 5] = [
        Self::Off,
        Self::KodakVision3_250d,
        Self::KodakPortra400,
        Self::FujiEterna500,
        Self::FujiVelvia50,
    ];

    /// Built-in looks followed by every external `.cube` file.
    pub fn all() -> Vec<Self> {
        Self::BUILT_IN
            .into_iter()
            .chain((0..crate::color_lut::external_count() as u16).map(Self::External))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::KodakVision3_250d => "KODAK VISION3 250D",
            Self::KodakPortra400 => "KODAK PORTRA 400",
            Self::FujiEterna500 => "FUJI ETERNA 500",
            Self::FujiVelvia50 => "FUJI VELVIA 50",
            Self::External(index) => crate::color_lut::external_label(index),
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::KodakVision3_250d => "kodak_vision3_250d",
            Self::KodakPortra400 => "kodak_portra_400",
            Self::FujiEterna500 => "fuji_eterna_500",
            Self::FujiVelvia50 => "fuji_velvia_50",
            Self::External(index) => crate::color_lut::external_key(index),
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "kodak_vision3_250d" | "vision3_250d" | "kodak250d" => Some(Self::KodakVision3_250d),
            "kodak_portra_400" | "portra_400" | "portra400" => Some(Self::KodakPortra400),
            "fuji_eterna_500" | "eterna_500" | "eterna500" => Some(Self::FujiEterna500),
            "fuji_velvia_50" | "velvia_50" | "velvia50" => Some(Self::FujiVelvia50),
            other => crate::color_lut::find_external(other).map(Self::External),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FogMode {
    #[default]
    Off,
    LegacyDrawFog1,
    LegacyDrawFog2,
    Volumetric,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntityAmbientLightingMode {
    #[default]
    Off,
    BspLightgridClassic,
    BevyIrradianceVolume,
}

impl EntityAmbientLightingMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::BspLightgridClassic, Self::BevyIrradianceVolume];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::BspLightgridClassic => "BSP LIGHTGRID (CLASSIC)",
            Self::BevyIrradianceVolume => "BEVY IRRADIANCE VOLUME (AMBIENT CUBES)",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::BspLightgridClassic => "bsp_lightgrid",
            Self::BevyIrradianceVolume => "bevy_irradiance_volume",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "bsp_lightgrid" => Some(Self::BspLightgridClassic),
            "bevy_irradiance_volume" => Some(Self::BevyIrradianceVolume),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DynamicLightsMode {
    #[default]
    Off,
    Legacy,
    Vertex,
    ClusteredLite,
    PerPixelForwardPlus,
    RayTracedHardware,
}

impl DynamicLightsMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Vertex,
        Self::Legacy,
        Self::ClusteredLite,
        Self::PerPixelForwardPlus,
        Self::RayTracedHardware,
    ];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Vertex => "vertex",
            Self::ClusteredLite => "clustered_lite",
            Self::PerPixelForwardPlus => "forward_plus",
            Self::RayTracedHardware => "ray_traced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "legacy" => Some(Self::Legacy),
            "vertex" => Some(Self::Vertex),
            "clustered_lite" => Some(Self::ClusteredLite),
            "forward_plus" => Some(Self::PerPixelForwardPlus),
            "ray_traced" => Some(Self::RayTracedHardware),
            _ => None,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SunVisibilityMode {
    Legacy,
    #[default]
    SkyPortals,
    Filtered,
}

impl SunVisibilityMode {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::SkyPortals => "sky",
            Self::Filtered => "filtered",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "legacy" | "off" | "0" => Some(Self::Legacy),
            "sky" | "sky_portals" | "1" => Some(Self::SkyPortals),
            "filtered" | "high" | "2" => Some(Self::Filtered),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Legacy => 0.0,
            Self::SkyPortals => 1.0,
            Self::Filtered => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DynamicShadowsMode {
    #[default]
    Off,
    Blob,
    /// Entity-only sun shadow map aimed by the baked lightgrid (players, NPCs, models).
    EntityMap,
    /// Bevy-style directional-light cascades (the only CSM implementation).
    CascadedShadowMaps,
    RayTraced,
}

impl DynamicShadowsMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Blob,
        Self::EntityMap,
        Self::CascadedShadowMaps,
        Self::RayTraced,
    ];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Blob => "blob",
            Self::EntityMap => "entity",
            Self::CascadedShadowMaps => "csm",
            Self::RayTraced => "ray_traced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Self::Off),
            // `blob_stencil` was the old combined placeholder. Preserve old
            // configs by migrating it to the now-functional OpenJK blob mode.
            "blob" | "blob_stencil" | "1" => Some(Self::Blob),
            // `stencil` was a never-implemented placeholder for this slot; keep old
            // configs on the nearest functional entity-shadow technique.
            "entity" | "entity_map" | "stencil" | "stencil_legacy" => Some(Self::EntityMap),
            // Keep the old numeric aliases stable so archived configs do not
            // silently shift when EntityMap gets its own menu slot.
            // The old non-Bevy CSM was removed; its aliases land on the Bevy CSM.
            "csm" | "cascaded" | "cascaded_shadow_maps" | "2" | "csm_bevy" | "bevy_csm"
            | "cascaded_shadow_maps_bevy" => Some(Self::CascadedShadowMaps),
            "ray_traced" | "raytraced" | "3" | "4" => Some(Self::RayTraced),
            _ => None,
        }
    }
}

/// Where the Entity map shadow mode aims its light.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntityShadowLight {
    /// The BSP lightgrid's baked dominant direction at the local player.
    #[default]
    Lightgrid,
    /// The strongest authored map light in range, taken from its real position
    /// (falls back to the lightgrid direction when no light qualifies).
    Authored,
}

impl EntityShadowLight {
    pub const ALL: [Self; 2] = [Self::Lightgrid, Self::Authored];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Lightgrid => "lightgrid",
            Self::Authored => "authored",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "lightgrid" | "grid" | "0" => Some(Self::Lightgrid),
            "authored" | "light" | "lights" | "1" => Some(Self::Authored),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloudType {
    #[default]
    Cumulus,
    Stratus,
    Storm,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloudRenderResolution {
    Full,
    ThreeQuarter,
    #[default]
    Half,
    Quarter,
}

impl CloudRenderResolution {
    pub const ALL: [Self; 4] = [Self::Full, Self::ThreeQuarter, Self::Half, Self::Quarter];


    pub fn config_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::ThreeQuarter => "75",
            Self::Half => "half",
            Self::Quarter => "quarter",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "1" | "100" => Some(Self::Full),
            "75" | "0.75" | "threequarter" | "three-quarter" => Some(Self::ThreeQuarter),
            "half" | "2" | "50" => Some(Self::Half),
            "quarter" | "4" | "25" => Some(Self::Quarter),
            _ => None,
        }
    }

    pub fn scale(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::ThreeQuarter => 0.75,
            Self::Half => 0.5,
            Self::Quarter => 0.25,
        }
    }
}

impl CloudType {
    pub const ALL: [Self; 3] = [Self::Cumulus, Self::Stratus, Self::Storm];


    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cumulus => "cumulus",
            Self::Stratus => "stratus",
            Self::Storm => "storm",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cumulus" | "0" => Some(Self::Cumulus),
            "stratus" | "1" => Some(Self::Stratus),
            "storm" | "2" | "cumulonimbus" => Some(Self::Storm),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Cumulus => 0.0,
            Self::Stratus => 1.0,
            Self::Storm => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RainIntensity {
    Light,
    #[default]
    Rain,
    Heavy,
}

impl RainIntensity {
    pub const ALL: [Self; 3] = [Self::Light, Self::Rain, Self::Heavy];


    pub fn config_value(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Rain => "rain",
            Self::Heavy => "heavy",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" | "lightrain" | "0" => Some(Self::Light),
            "rain" | "normal" | "1" => Some(Self::Rain),
            "heavy" | "heavyrain" | "2" => Some(Self::Heavy),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Light => 0.35,
            Self::Rain => 0.68,
            Self::Heavy => 1.0,
        }
    }

    pub fn particle_count(self) -> u32 {
        // V6 particles are persistent GPU simulation records shared by the
        // compute update, falling-streak render, and impact/splash render.
        match self {
            Self::Light => 6_000,
            Self::Rain => 14_000,
            Self::Heavy => 28_000,
        }
    }

    pub fn splash_particle_count(self) -> u32 {
        match self {
            Self::Light => 1_500,
            Self::Rain => 3_500,
            Self::Heavy => 6_000,
        }
    }

    pub fn haze_strength(self) -> f32 {
        match self {
            Self::Light => 0.24,
            Self::Rain => 0.52,
            Self::Heavy => 0.90,
        }
    }
}

/// How puddles and wet ground are shaded. `High` reuses the GodotOcean water
/// response (roughness-aware Fresnel, GGX sun and light glints) and traces
/// longer, streaked screen-space reflections; `Standard` keeps the cheaper film
/// response. Either way the cost is only paid on wet pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PuddleQuality {
    Standard,
    #[default]
    High,
}

impl PuddleQuality {
    pub const ALL: [Self; 2] = [Self::Standard, Self::High];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "standard" | "normal" | "low" | "0" => Some(Self::Standard),
            "high" | "ocean" | "1" => Some(Self::High),
            _ => None,
        }
    }

    pub fn is_high(self) -> bool {
        self == Self::High
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FootprintMode {
    Off,
    TwoD,
    #[default]
    ThreeD,
}

impl FootprintMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::TwoD, Self::ThreeD];
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::TwoD => "2d",
            Self::ThreeD => "3d",
        }
    }
    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => Some(Self::Off),
            "2d" | "2" | "legacy" | "mark" | "decal" => Some(Self::TwoD),
            "3d" | "3" | "deform" | "deformation" | "true" | "on" | "1" => Some(Self::ThreeD),
            _ => None,
        }
    }
    pub fn shader_value(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::TwoD => 1,
            Self::ThreeD => 2,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SaberMarkMode {
    Off,
    #[default]
    Legacy,
    Enhanced,
}

impl SaberMarkMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Legacy, Self::Enhanced];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Enhanced => "enhanced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => Some(Self::Off),
            "legacy" | "1" | "openjk" | "classic" | "on" | "true" => Some(Self::Legacy),
            "enhanced" | "2" | "modern" | "molten" => Some(Self::Enhanced),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Legacy => "Legacy (OpenJK)",
            Self::Enhanced => "Enhanced molten",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DofQuality {
    Performance,
    #[default]
    Adaptive,
    High,
}

impl DofQuality {
    pub const ALL: [Self; 3] = [Self::Performance, Self::Adaptive, Self::High];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Adaptive => "adaptive",
            Self::High => "high",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "performance" | "perf" | "low" | "0" => Some(Self::Performance),
            "adaptive" | "auto" | "1" => Some(Self::Adaptive),
            "high" | "2" => Some(Self::High),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Performance => 0.0,
            Self::Adaptive => 1.0,
            Self::High => 2.0,
        }
    }
}

pub const MAX_FOG_STRENGTH: f32 = 10.0;
pub const MAX_DISTANCE_CULL_SCALE: f32 = 10.0;
pub const CLOUD_HEIGHT_MIN: f32 = -8192.0;
pub const CLOUD_HEIGHT_MAX: f32 = 8192.0;
pub const CLOUD_THICKNESS_MIN: f32 = 256.0;
pub const CLOUD_THICKNESS_MAX: f32 = 4096.0;
pub const VIDEO_ROW_FULLSCREEN: usize = 0;
pub const VIDEO_ROW_RESOLUTION: usize = 1;
pub const VIDEO_ROW_VSYNC: usize = 2;
pub const VIDEO_ROW_FPS_CAP: usize = 3;
pub const VIDEO_ROW_PHYSICS_FPS: usize = 4;
pub const VIDEO_ROW_BRIGHTNESS: usize = 5;
pub const VIDEO_ROW_ANTI_ALIASING: usize = 6;
pub const VIDEO_ROW_TEXTURE_FILTER: usize = 7;
pub const VIDEO_ROW_PVS: usize = 8;
pub const VIDEO_ROW_DISTANCE_CULL: usize = 9;
pub const VIDEO_ROW_GPU_DRIVEN: usize = 10;
pub const VIDEO_ROW_HIZ: usize = 11;
pub const VIDEO_ROW_ENTITY_AMBIENT_LIGHTING: usize = 12;
pub const VIDEO_ROW_DYNAMIC_LIGHTS: usize = 13;
pub const VIDEO_ROW_EMISSIVE_AREA_LIGHTS: usize = 14;
pub const VIDEO_ROW_AMBIENT_OCCLUSION: usize = 15;
pub const VIDEO_ROW_VOXEL_PROBE_GI: usize = 16;
pub const VIDEO_ROW_DYNAMIC_SHADOWS: usize = 17;
pub const VIDEO_ROW_LOCAL_LIGHT_SHADOWS: usize = 18;
pub const VIDEO_ROW_CONTACT_SHADOWS: usize = 19;
pub const VIDEO_ROW_SSR: usize = 20;
pub const VIDEO_ROW_PLANAR_REFLECTIONS: usize = 21;
pub const VIDEO_ROW_PLANAR_REFLECTION_DEBUG: usize = 22;
pub const VIDEO_ROW_HDR: usize = 23;
pub const VIDEO_ROW_TONE_MAPPING: usize = 24;
pub const VIDEO_ROW_BLOOM: usize = 25;
pub const VIDEO_ROW_MOTION_BLUR: usize = 26;
pub const VIDEO_ROW_DEPTH_OF_FIELD: usize = 27;
pub const VIDEO_ROW_DOF_QUALITY: usize = 28;
pub const VIDEO_ROW_HALATION: usize = 29;
pub const VIDEO_ROW_CHROMATIC_ABERRATION: usize = 30;
pub const VIDEO_ROW_VIGNETTE: usize = 31;
pub const VIDEO_ROW_FILM_GRAIN: usize = 32;
pub const VIDEO_ROW_COLOR_LUT: usize = 33;
pub const VIDEO_ROW_LUT_STRENGTH: usize = 34;
pub const VIDEO_ROW_WIREFRAME: usize = 35;
pub const VIDEO_ROW_CULL_DEBUG: usize = 36;
pub const VIDEO_ROW_DRAW_FPS: usize = 37;
pub const VIDEO_ROW_DEVELOPER_TOOLS: usize = 38;
pub const VIDEO_ROW_PERF_TRACE: usize = 39;
pub const VIDEO_ROW_GPU_TIMINGS: usize = 40;
pub const VIDEO_ROW_RENDER_BACKEND: usize = 41;
pub const VIDEO_ROW_BAKED_AO_SAMPLES: usize = 42;
pub const VIDEO_ROW_BAKED_AO_RESOLUTION: usize = 43;
pub const VIDEO_ROW_BAKED_AO_STRENGTH: usize = 44;
pub const VIDEO_ROW_BAKED_AO_RANGE: usize = 45;
pub const VIDEO_ROW_BAKED_AO_CURRENT_CELL: usize = 46;
pub const VIDEO_ROW_VID_RESTART: usize = 47;
pub const VIDEO_ROW_PBR: usize = 48;
pub const VIDEO_ROW_QUALITY_PRESET: usize = 49;
pub const VIDEO_ROW_AUTO_EXPOSURE: usize = 50;
pub const VIDEO_ROW_FLOAT_LIGHTMAP: usize = 51;
pub const VIDEO_ROW_GEN_NORMAL_MAPS: usize = 52;
pub const VIDEO_ROW_DELUXE_MAPPING: usize = 53;
pub const VIDEO_ROW_DELUXE_SPECULAR: usize = 54;
pub const VIDEO_ROW_ASSET_OVERRIDES: usize = 55;
pub const VIDEO_ROW_GHOUL2_SKINNING: usize = 56;
pub const VIDEO_ROW_GHOUL2_LOD_BIAS: usize = 57;
pub const VIDEO_ROW_GHOUL2_BATCH_DRAWS: usize = 58;
pub const VIDEO_ROW_GHOUL2_EARLY_CULL: usize = 59;
pub const VIDEO_ROW_MODERN_SABERS: usize = 60;
pub const VIDEO_ROW_FX_FPS: usize = 61;
pub const VIDEO_ROW_WORLD_LIGHTING: usize = 62;
pub const VIDEO_ROW_VERTEX_LIGHTING: usize = 63;
pub const VIDEO_ROW_LIGHTMAP_ONLY: usize = 64;
pub const VIDEO_ROW_MAP_LIGHT_SIMULATION: usize = 65;
pub const VIDEO_ROW_SABER_MARKS: usize = 66;
pub const VIDEO_ROW_MAX_FRAME_LATENCY: usize = 67;
pub const VIDEO_ROW_DETAIL_TEXTURES: usize = 68;
pub const VIDEO_ROW_FLARES: usize = 69;
pub const VIDEO_ROW_SABER_IMPACT_FX: usize = 70;
pub const VIDEO_ROW_FX_GEOMETRY: usize = 71;
pub const VIDEO_ROW_FX_ZERO_ALPHA_DISCARD: usize = 72;
pub const VIDEO_ROW_DRAW_MAP_MODELS: usize = 73;
pub const VIDEO_ROW_MODEL_BRIGHTNESS: usize = 74;
pub const VIDEO_ROW_DLIGHT_BRIGHTNESS: usize = 75;
pub const VIDEO_ROW_DRAW_TRIGGERS: usize = 76;
pub const VIDEO_ROW_DRAW_CLIP_BRUSHES: usize = 77;
pub const VIDEO_ROW_FX_PHYSICS: usize = 78;
pub const VIDEO_ROW_FX_LOD: usize = 79;
pub const VIDEO_ROW_ENTITY_SHADOW_LIGHT: usize = 80;
pub const VIDEO_ROW_DRAW_ENTITIES: usize = 81;

pub const ENV_ROW_FOG_MODE: usize = 0;
pub const ENV_ROW_FOG_STRENGTH: usize = 1;
pub const ENV_ROW_CLOUDS: usize = 2;
pub const ENV_ROW_CLOUD_TYPE: usize = 3;
pub const ENV_ROW_CLOUD_QUALITY: usize = 4;
pub const ENV_ROW_CLOUD_COVERAGE: usize = 5;
pub const ENV_ROW_CLOUD_HEIGHT: usize = 6;
pub const ENV_ROW_CLOUD_THICKNESS: usize = 7;
pub const ENV_ROW_WEATHER_WIND_SPEED: usize = 8;
pub const ENV_ROW_WEATHER_WIND_DIRECTION: usize = 9;
pub const ENV_ROW_CLOUD_SHADOWS: usize = 10;
pub const ENV_ROW_CLOUD_RENDER_RESOLUTION: usize = 11;
pub const ENV_ROW_CLOUD_TEMPORAL: usize = 12;
pub const ENV_ROW_CLOUD_TUNING: usize = 13;
pub const ENV_ROW_RAIN: usize = 14;
pub const ENV_ROW_RAIN_INTENSITY: usize = 15;
pub const ENV_ROW_FOOTPRINTS: usize = 16;
pub const ENV_ROW_GRASS: usize = 17;
pub const ENV_ROW_OCEAN: usize = 18;
pub const ENV_ROW_OCEAN_SETTINGS: usize = 19;
pub const ENV_ROW_VID_RESTART: usize = 20;
pub const ENV_ROW_SUN_SOURCE: usize = 21;
pub const ENV_ROW_SUN_YAW: usize = 22;
pub const ENV_ROW_SUN_PITCH: usize = 23;
pub const ENV_ROW_SUN_INTENSITY: usize = 24;
pub const ENV_ROW_SUN_COLOR: usize = 25;
pub const ENV_ROW_SUN_VISIBILITY: usize = 26;
pub const ENV_ROW_WEATHER_GUST_STRENGTH: usize = 27;
pub const ENV_ROW_WEATHER_DIRECTION_VARIATION: usize = 28;
pub const ENV_ROW_PUDDLE_WATER: usize = 29;
pub const ENV_ROW_PUDDLE_SCATTER: usize = 30;
pub const ENV_ROW_RAIN_GRADE: usize = 31;
pub const ENV_ROW_ENTITY_SUN_LIGHTING: usize = 32;

pub const SUN_INTENSITY_MAX: f32 = 4000.0;

pub const CLOUD_ROW_TEMPORAL_DEPTH_FIX: usize = 0;
pub const CLOUD_ROW_SHEAR: usize = 1;
pub const CLOUD_ROW_BASE_VARIATION: usize = 2;
pub const CLOUD_ROW_AERIAL: usize = 3;
pub const CLOUD_ROW_SKY_AMBIENT: usize = 4;
pub const CLOUD_ROW_HISTORY_BLEND: usize = 5;
pub const CLOUD_ROW_MOTION_REJECT: usize = 6;
pub const CLOUD_ROW_HISTORY_DEPTH_REJECT: usize = 7;
pub const CLOUD_ROW_THICKNESS_VARIATION: usize = 8;
pub const CLOUD_ROW_SIZE: usize = 9;
pub const CLOUD_ROW_SHAPE_EVOLUTION: usize = 11;
pub const CLOUD_ROW_TERRAIN_INTERACTION: usize = 12;
pub const CLOUD_ROW_EMPTY_SKIP: usize = 13;


impl FogMode {
    pub fn is_legacy(self) -> bool {
        matches!(self, Self::LegacyDrawFog1 | Self::LegacyDrawFog2)
    }

    /// `r_drawfog`: OpenJK's 0/1/2, plus 3 for DinurdoJK's volumetric fog.
    pub fn drawfog_value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::LegacyDrawFog1 => 1,
            Self::LegacyDrawFog2 => 2,
            Self::Volumetric => 3,
        }
    }

    pub fn from_drawfog(value: &str) -> Option<Self> {
        match value.trim() {
            "0" => Some(Self::Off),
            "1" => Some(Self::LegacyDrawFog1),
            "2" => Some(Self::LegacyDrawFog2),
            "3" => Some(Self::Volumetric),
            _ => None,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectionQuality {
    /// No enhanced reflections and no authored tcGen environment stages.
    Off,
    /// Vanilla/authored tcGen environment stages and JKA portal mirrors only;
    /// no promoted environment-planar reflection path.
    Legacy,
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl ReflectionQuality {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Legacy,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Ultra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Legacy => "LEGACY",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
            Self::Ultra => "ULTRA",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Self::Off),
            "legacy" | "classic" => Some(Self::Legacy),
            "low" | "1" => Some(Self::Low),
            "medium" | "med" | "2" => Some(Self::Medium),
            "high" | "3" => Some(Self::High),
            "ultra" | "4" => Some(Self::Ultra),
            _ => None,
        }
    }

    /// Runtime reflection-technique tier. Off is the explicit compatibility-breaking
    /// "strip reflections" mode. Legacy preserves authored JKA reflection behavior
    /// (tcGen environment stages and portal mirrors) without promoting arbitrary
    /// environment-mapped surfaces into enhanced planar reflections.
    pub fn shader_value(self) -> u32 {
        match self {
            Self::Off | Self::Legacy => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Ultra => 4,
        }
    }

    pub fn omits_environment_stages(self) -> bool {
        matches!(self, Self::Off)
    }

    pub fn ssr_enabled(self) -> bool {
        self >= Self::Medium
    }

    pub fn planar_slot_budget(self) -> usize {
        match self {
            Self::Off => 0,
            // Authored JKA `portal` mirrors are part of the legacy material
            // contract, not an enhanced reflection feature. Keep one baseline
            // planar slot available from Legacy upward.
            Self::Legacy | Self::Low | Self::Medium | Self::High => 1,
            Self::Ultra => 4,
        }
    }

    pub fn promotes_environment_planars(self) -> bool {
        matches!(self, Self::High | Self::Ultra)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanarReflectionMode {
    Off,
    #[default]
    Authored,
    Environment,
}

impl PlanarReflectionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Authored => "AUTHORED MIRRORS",
            Self::Environment => "AUTHORED + ENVIRONMENT",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RendererBackend {
    #[default]
    Vulkan,
    Dx12,
}

impl RendererBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "VULKAN",
            Self::Dx12 => "DX12",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Dx12 => "dx12",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => Some(Self::Vulkan),
            "dx12" | "d3d12" | "directx12" => Some(Self::Dx12),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VsyncMode {
    #[default]
    Off,
    On,
    Fast,
    Adaptive,
}

impl VsyncMode {
    pub const ALL: [Self; 4] = [Self::Off, Self::On, Self::Fast, Self::Adaptive];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::On => "ON",
            Self::Fast => "FAST",
            Self::Adaptive => "ADAPTIVE",
        }
    }

    pub fn config_value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Fast => 2,
            Self::Adaptive => 3,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "off" => Some(Self::Off),
            "1" | "true" | "on" | "fifo" => Some(Self::On),
            "2" | "fast" | "mailbox" => Some(Self::Fast),
            "3" | "adaptive" | "relaxed" | "fifo_relaxed" => Some(Self::Adaptive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FullscreenMode {
    #[default]
    Windowed,
    Borderless,
    Exclusive,
}

impl FullscreenMode {
    pub fn config_value(self) -> u8 {
        match self {
            Self::Windowed => 0,
            Self::Borderless => 1,
            Self::Exclusive => 2,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "off" | "windowed" => Some(Self::Windowed),
            "1" | "true" | "on" | "borderless" => Some(Self::Borderless),
            "2" | "exclusive" => Some(Self::Exclusive),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windowed => "WINDOWED",
            Self::Borderless => "BORDERLESS",
            Self::Exclusive => "EXCLUSIVE",
        }
    }

    pub fn is_windowed(self) -> bool {
        matches!(self, Self::Windowed)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VideoSettings {
    pub fullscreen: FullscreenMode,
    pub renderer_backend: RendererBackend,
    pub vsync: VsyncMode,
    /// Maximum number of frames the WGPU presentation surface may keep in flight.
    /// Lower values favor latency; higher values favor throughput.
    pub max_frame_latency: u32,
    pub msaa_samples: u32,
    pub resolution: [u32; 2],
    pub window_position: Option<[i32; 2]>,
    pub window_maximized: bool,
    pub texture_filter: TextureFilter,
    pub detail_textures: DetailTextureMode,
    /// Port of the standard distance-based detail fade: blend the detail contribution
    /// back to its neutral identity as camera distance approaches the configured range.
    pub detail_texture_fade: bool,
    pub detail_texture_fade_distance: f32,
    pub wireframe_mask: u32,
    pub skip_ui: bool,
    pub pvs_mode: PvsMode,
    pub fps_cap: u32,
    /// Continuous projectile FX sampling rate. `0` preserves legacy JKA's
    /// render-frame-driven behavior; non-zero values are fixed Hz.
    pub fx_fps: u32,
    /// `fx_physics`: 0 off, 2 authored expensivePhysics (default), 3 force all.
    pub fx_physics: u32,
    /// `fx_lod`: 0 stock, 1 authored cullRange, 2 adaptive screen-size density.
    pub fx_lod: u32,
    /// Stock `fx_countScale` (0..1).
    pub fx_count_scale: f32,
    /// `r_fxLodScale`: multiplies authored EFX cullRange (default 5, like r_lodscale).
    pub fx_lod_scale: f32,
    /// `r_lodScale`: Ghoul2 projected-size LOD scale (OpenJK default 5).
    pub lod_scale: f32,
    /// View-dependent EFX geometry expansion path. FX simulation itself already
    /// runs on the dedicated `jka-fx` worker; this controls the later sprite/line
    /// tessellation step against the final camera.
    pub fx_geometry: FxGeometryMode,
    /// A/B diagnostic for GPU-instanced EFX sprites. When enabled, fragments
    /// whose final source alpha is exactly zero are discarded before fog/blend.
    /// This preserves non-zero-alpha edges and is intentionally limited to
    /// source-alpha sprite blend modes.
    pub fx_zero_alpha_discard: bool,
    /// Server-placed MD3 map props (misc_model_* / func_static models). Inline
    /// BSP brush models and geometry compiled into the BSP are unaffected.
    pub draw_map_models: bool,
    /// Debug overlays for trigger volumes and clip/collision-only brushes.
    /// Session-only: never written to the config.
    pub draw_triggers: bool,
    pub draw_clip_brushes: bool,
    /// `r_drawEntities`: NetRadiant-style colored boxes, classname labels and
    /// target/targetname link lines for every map entity. Session-only.
    pub draw_entities: bool,
    pub draw_fps: u8,
    pub developer_tools: bool,
    pub perf_trace: bool,
    /// World renderer choice (`r_worldPath unified`): keeps the world on
    /// the unified renderer even when the FastBaseline feature envelope matches,
    /// so "minimal unified" can be timed against the untouched baseline.
    pub force_unified_world: bool,
    /// Session-only: parallax occlusion on top of `r_pbr` (`r_pom`, default on).
    pub pom: bool,
    pub gpu_timings: bool,
    pub ghoul2_skinning: Ghoul2SkinningMode,
    pub ghoul2_early_cull: bool,
    pub ghoul2_lod_bias: i32,
    pub ghoul2_batch_draws: Ghoul2BatchMode,
    /// `r_ghoul2animsmooth`: jaPRO `CBoneCache::SmoothLow` renderer bone-history
    /// filter factor. Matches jaPRO's own default of 0.3; only active strictly
    /// between 0 and 1 (0 or >=1 disables it).
    pub ghoul2_anim_smooth: f32,
    pub physics_msec: u32,
    pub input_subframe: bool,
    /// Request a 1 ms Windows multimedia timer period for timeout/sleep waits.
    /// Raw mouse events already wake the event loop independently.
    pub timer_resolution_1ms: bool,
    /// Experimental render-thread late latch. Only effective with
    /// `cl_input_subframe`; the renderer resamples the newest real view
    /// orientation at its last coherent camera point for the active path.
    pub input_latelatch: bool,
    // Client-side visual physics (Rapier integration target). These never replace
    // authoritative JKA/OpenJK player movement or server entity state.
    pub client_physics: bool,
    pub client_physics_hz: u32,
    pub client_physics_max_substeps: u32,
    pub client_physics_ccd: bool,
    pub client_physics_sleeping: bool,
    pub ragdolls: bool,
    pub ragdoll_max: u32,
    pub ragdoll_lifetime: f32,
    pub ragdoll_self_collision: bool,
    pub physics_props: bool,
    pub physics_prop_max: u32,
    pub physics_debris: bool,
    pub physics_debris_max: u32,
    pub physics_debris_lifetime: f32,
    pub physics_player_push: bool,
    pub physics_weapon_impulses: bool,
    pub physics_explosion_impulses: bool,
    pub physics_force_impulses: bool,
    pub physics_debug_draw: bool,
    pub physics_stats: bool,
    pub gamma: f32,
    /// Model lighting brightness on the same scale as `gamma`. While
    /// `model_brightness_locked` it tracks the master slider and adds nothing.
    pub model_brightness: f32,
    pub model_brightness_locked: bool,
    /// Dynamic (runtime) light brightness, same scale and lock rules.
    pub dynamic_light_brightness: f32,
    pub dynamic_light_brightness_locked: bool,
    pub hdr: bool,
    pub float_lightmap: bool,
    pub tone_mapping: bool,
    pub auto_exposure: bool,
    pub bloom: bool,
    pub halation: bool,
    pub ssao: bool,
    pub static_bsp_ao: bool,
    /// false = cached per-vertex AO, true = cached lightmap-space AO.
    pub static_bsp_ao_lightmap: bool,
    pub static_bsp_ao_samples: u32,
    /// Odd supersampling scales preserve the original lightmap sample lattice.
    pub static_bsp_ao_resolution: u32,
    /// Baked AO blend strength, percentage (25..100).
    pub static_bsp_ao_strength: u32,
    /// Baked AO distance scale, percentage of the 32/128/512-unit defaults.
    pub static_bsp_ao_range: u32,
    /// Debug/testing mode: bake only the nearest AO lightmap tile to the camera.
    pub static_bsp_ao_current_cell: bool,
    pub fxaa: bool,
    pub smaa: bool,
    pub taa: bool,
    pub contact_shadows: bool,
    pub fog_mode: FogMode,
    pub fog_strength: f32,
    /// When false, use the strongest q3map-authored sky sun from the current map.
    pub sun_override: bool,
    /// q3map azimuth in degrees: 0=east, 90=north.
    pub sun_yaw: f32,
    /// q3map elevation in degrees above the horizon.
    pub sun_pitch: f32,
    pub sun_intensity: f32,
    /// Unnormalized RGB chosen by the user; renderer normalizes it like q3map.
    pub sun_color: [f32; 3],
    /// Controls whether direct shader-sun light requires an authored sky portal.
    pub sun_visibility: SunVisibilityMode,
    /// Strips the baked map sun out of the entity lightgrid sample and re-adds
    /// it as a directional light that follows the runtime sun (color, intensity,
    /// direction). Needs Entity ambient lighting = BSP lightgrid.
    pub entity_sun_lighting: bool,
    /// 0 = map/default distanceCull; otherwise multiplier applied to that base.
    pub distance_cull_scale: f32,
    pub clouds: bool,
    pub cloud_type: CloudType,
    pub cloud_quality: f32,
    pub cloud_coverage: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub cloud_shadows: bool,
    /// One authoritative atmospheric wind shared by clouds, precipitation, grass, and ocean.
    pub weather_wind: crate::ocean::OceanWind,
    pub rain: bool,
    pub rain_intensity: RainIntensity,
    pub puddle_quality: PuddleQuality,
    /// How readily rain collects in scattered puddles on large flat ground, 0..1.
    pub puddle_scatter: f32,
    /// Strength of the wet-weather colour grade (cool shadows, warm highlights,
    /// richer neon) while it rains, 0..1.
    pub rain_grade: f32,
    pub footprints: FootprintMode,
    pub grass: bool,
    /// Runtime A/B diagnostic: hoist root wind/clump work to a compute pass.
    pub grass_precompute: bool,
    /// Runtime A/B diagnostic: use the 5-triangle middle blade LOD.
    pub grass_mid_lod: bool,
    /// Runtime A/B diagnostic: order opaque grass near-to-far for early-Z.
    pub grass_front_to_back: bool,
    pub contact_shadow_debug: u8,
    pub ocean: bool,
    pub ocean_settings: crate::ocean::OceanSettings,
    pub cloud_render_resolution: CloudRenderResolution,
    pub cloud_temporal: bool,
    pub cloud_temporal_depth_fix: bool,
    pub cloud_shear: f32,
    pub cloud_base_variation: f32,
    pub cloud_shape_evolution: bool,
    pub cloud_terrain_interaction: bool,
    pub cloud_empty_skip: bool,
    pub cloud_aerial: f32,
    pub cloud_sky_ambient: bool,
    pub cloud_history_blend: f32,
    pub cloud_motion_reject: f32,
    pub cloud_history_depth_reject: bool,
    pub cloud_thickness_variation: f32,
    pub cloud_size: f32,
    /// Player-facing source of truth for reflection technique budgets.
    pub reflection_quality: ReflectionQuality,
    /// Development overlay showing the resolver result per visible surface/pixel.
    pub reflection_debug: bool,
    pub chromatic_aberration: f32,
    pub vignette: bool,
    pub film_grain_strength: f32,
    pub motion_blur_strength: f32,
    pub depth_of_field_strength: f32,
    pub dof_quality: DofQuality,
    pub color_lut: ColorLutPreset,
    pub color_lut_strength: f32,
    pub gpu_driven: bool,
    pub hiz_occlusion: bool,
    pub entity_ambient_lighting: EntityAmbientLightingMode,
    pub dynamic_lights: DynamicLightsMode,
    pub rt_samples: u32,
    /// 0 = stock JKA 1 - d^2/r^2, 1 = windowed inverse-square.
    pub dynamic_light_falloff: u32,
    pub rt_half_resolution: bool,
    /// Source `.map` only: approximate a compiled lighting pass from authored light entities.
    pub map_light_simulation: bool,
    /// Classic world-lighting master. False corresponds to vanilla r_fullbright 1.
    pub world_lighting: bool,
    /// Use BSP vertex colors instead of baked lightmaps for world static lighting.
    pub vertex_lighting: bool,
    /// Debug view equivalent to vanilla r_lightmap: show baked lighting without diffuse textures.
    pub lightmap_only: bool,
    /// Optional continuous-ribbon saber presentation. False keeps the OpenJK-style
    /// RT_SABER_GLOW sprite chain + RT_LINE core.
    pub modern_sabers: bool,
    /// Screen-space flare overlays such as the OpenJK saber clash flash.
    /// This does not disable the underlying impact FX, sounds, or marks.
    pub flares: bool,
    /// Authored saber-hit/block EFX presentation. The FX system retains tagged
    /// live primitives so this can be A/B toggled on a paused demo frame.
    pub saber_impact_fx: bool,
    /// Saber/world contact presentation. Legacy mirrors OpenJK's sparks, burn/glow
    /// marks and contact sound; Enhanced adds a hotter molten pass and drips.
    pub saber_marks: SaberMarkMode,
    /// Master switch for the complete Rend2-style PBR material profile.
    pub pbr: bool,
    /// Scope base-color replacements owned by PBR-enhanced MTR materials to
    /// PBR map preparation instead of letting them override classic rendering.
    pub allow_asset_overrides: bool,
    /// Generate a fallback normal map from diffuse luminance when no authored normal map exists.
    pub gen_normal_maps: bool,
    /// Use q3map2/Rend2 directional lightmaps when available; falls back to the BSP lightgrid.
    pub deluxe_mapping: bool,
    /// Scale the specular response produced by directional baked lighting.
    pub deluxe_specular: f32,
    pub emissive_area_lights: bool,
    pub voxel_probe_gi: bool,
    pub dynamic_shadows: DynamicShadowsMode,
    pub entity_shadow_light: EntityShadowLight,
    pub local_light_shadows: bool,
    pub cascaded_shadows: bool,
    pub cull_debug: CullDebugMode,
    pub planar_reflection_debug: PlanarReflectionDebugMode,
}

impl VideoSettings {
    /// Extra linear multiplier for model lighting. Locked (the default) leaves the
    /// master gamma as the only brightness control; unlocked, the slider value is
    /// expressed on the master's scale, so unlocking never changes the picture.
    pub fn effective_model_brightness(&self) -> f32 {
        Self::relative_brightness(self.model_brightness_locked, self.model_brightness, self.gamma)
    }

    /// Same rule as [`Self::effective_model_brightness`] for runtime dynamic lights.
    pub fn effective_dynamic_light_brightness(&self) -> f32 {
        Self::relative_brightness(
            self.dynamic_light_brightness_locked,
            self.dynamic_light_brightness,
            self.gamma,
        )
    }

    fn relative_brightness(locked: bool, value: f32, gamma: f32) -> f32 {
        if locked {
            1.0
        } else {
            (value / gamma.max(0.01)).clamp(0.0, 6.0)
        }
    }

    /// Master slider moved: linked brightness sliders follow it.
    pub fn sync_linked_brightness(&mut self) {
        if self.model_brightness_locked {
            self.model_brightness = self.gamma;
        }
        if self.dynamic_light_brightness_locked {
            self.dynamic_light_brightness = self.gamma;
        }
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            fullscreen: FullscreenMode::Windowed,
            renderer_backend: RendererBackend::Vulkan,
            vsync: VsyncMode::Off,
            max_frame_latency: 3,
            msaa_samples: 1,
            resolution: [1280, 800],
            window_position: None,
            window_maximized: false,
            texture_filter: TextureFilter::Trilinear,
            detail_textures: DetailTextureMode::Off,
            detail_texture_fade: false,
            detail_texture_fade_distance: 512.0,
            wireframe_mask: 0,
            skip_ui: false,
            pvs_mode: PvsMode::Auto,
            fps_cap: 0,
            fx_fps: crate::fx::FX_FPS_DEFAULT,
            fx_physics: crate::fx::FX_PHYSICS_DEFAULT,
            fx_lod: crate::fx::FX_LOD_DEFAULT,
            fx_count_scale: 1.0,
            fx_lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            fx_geometry: FxGeometryMode::Cpu,
            fx_zero_alpha_discard: false,
            draw_map_models: true,
            draw_triggers: false,
            draw_clip_brushes: false,
            draw_entities: false,
            draw_fps: 1,
            developer_tools: false,
            perf_trace: false,
            force_unified_world: false,
            pom: true,
            gpu_timings: false,
            ghoul2_skinning: Ghoul2SkinningMode::Gpu,
            ghoul2_early_cull: true,
            ghoul2_lod_bias: 0,
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            ghoul2_anim_smooth: 0.3,
            physics_msec: 8,
            input_subframe: false,
            timer_resolution_1ms: false,
            input_latelatch: false,
            client_physics: false,
            client_physics_hz: 60,
            client_physics_max_substeps: 4,
            client_physics_ccd: true,
            client_physics_sleeping: true,
            ragdolls: true,
            ragdoll_max: 8,
            ragdoll_lifetime: 20.0,
            ragdoll_self_collision: false,
            physics_props: true,
            physics_prop_max: 96,
            physics_debris: true,
            physics_debris_max: 192,
            physics_debris_lifetime: 10.0,
            physics_player_push: true,
            physics_weapon_impulses: true,
            physics_explosion_impulses: true,
            physics_force_impulses: true,
            physics_debug_draw: false,
            physics_stats: false,
            gamma: 1.0,
            model_brightness: 1.0,
            model_brightness_locked: true,
            dynamic_light_brightness: 1.0,
            dynamic_light_brightness_locked: true,
            hdr: false,
            float_lightmap: false,
            tone_mapping: false,
            auto_exposure: false,
            bloom: false,
            halation: false,
            ssao: false,
            static_bsp_ao: false,
            static_bsp_ao_lightmap: true,
            static_bsp_ao_samples: 32,
            static_bsp_ao_resolution: 3,
            static_bsp_ao_strength: 75,
            static_bsp_ao_range: 100,
            static_bsp_ao_current_cell: false,
            fxaa: false,
            smaa: false,
            taa: false,
            contact_shadows: false,
            fog_mode: FogMode::Off,
            fog_strength: 0.0,
            sun_override: false,
            // Matches the renderer's legacy fallback direction when a map has no authored sun.
            sun_yaw: 314.25595,
            sun_pitch: 57.04725,
            sun_intensity: 250.0,
            sun_color: [1.0, 1.0, 1.0],
            sun_visibility: SunVisibilityMode::SkyPortals,
            entity_sun_lighting: false,
            distance_cull_scale: 0.0,
            clouds: true,
            cloud_type: CloudType::Storm,
            cloud_quality: 1.0,
            cloud_coverage: 0.6,
            cloud_height: 4600.0,
            cloud_thickness: 1200.0,
            cloud_shadows: true,
            weather_wind: crate::ocean::OceanWind {
                speed: 167.0,
                direction: 220.0,
                gust: 0.2,
                shift: 0.0,
            },
            rain: false,
            rain_intensity: RainIntensity::Rain,
            puddle_quality: PuddleQuality::High,
            puddle_scatter: 0.8,
            rain_grade: 0.5,
            footprints: FootprintMode::ThreeD,
            grass: true,
            grass_precompute: true,
            grass_mid_lod: true,
            grass_front_to_back: true,
            contact_shadow_debug: 0,
            ocean: false,
            ocean_settings: crate::ocean::OceanSettings::default(),
            cloud_render_resolution: CloudRenderResolution::Full,
            cloud_temporal: true,
            cloud_temporal_depth_fix: true,
            cloud_shear: 0.2,
            cloud_base_variation: 1.0,
            cloud_shape_evolution: false,
            cloud_terrain_interaction: false,
            cloud_empty_skip: false,
            cloud_aerial: 0.0,
            cloud_sky_ambient: false,
            cloud_history_blend: 0.85,
            cloud_motion_reject: 1.0,
            cloud_history_depth_reject: false,
            cloud_thickness_variation: 0.75,
            cloud_size: 0.333,
            reflection_quality: ReflectionQuality::High,
            reflection_debug: false,
            chromatic_aberration: 0.0,
            vignette: false,
            film_grain_strength: 0.0,
            motion_blur_strength: 0.0,
            depth_of_field_strength: 0.0,
            dof_quality: DofQuality::Adaptive,
            color_lut: ColorLutPreset::Off,
            color_lut_strength: 1.0,
            gpu_driven: false,
            hiz_occlusion: false,
            entity_ambient_lighting: EntityAmbientLightingMode::Off,
            dynamic_lights: DynamicLightsMode::Off,
            rt_samples: 1,
            dynamic_light_falloff: 0,
            rt_half_resolution: false,
            map_light_simulation: false,
            world_lighting: true,
            vertex_lighting: false,
            lightmap_only: false,
            modern_sabers: false,
            flares: true,
            saber_impact_fx: true,
            saber_marks: SaberMarkMode::Legacy,
            pbr: true,
            allow_asset_overrides: true,
            gen_normal_maps: false,
            deluxe_mapping: true,
            deluxe_specular: 1.0,
            emissive_area_lights: false,
            voxel_probe_gi: false,
            dynamic_shadows: DynamicShadowsMode::Off,
            entity_shadow_light: EntityShadowLight::Lightgrid,
            local_light_shadows: false,
            cascaded_shadows: false,
            cull_debug: CullDebugMode::Off,
            planar_reflection_debug: PlanarReflectionDebugMode::Off,
        }
    }
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
    /// jaPRO `cg_raceTimer` / `cg_raceStart` readout (updates every frame of a run).
    pub race_timer: Option<crate::japro_cg::RaceTimerUi>,
    /// `CG_DrawVote`: the open vote's summary line (colour codes included).
    pub vote_line: Option<String>,
    pub scoreboard: Option<UiScoreboard>,
    pub demo_timeline: Option<DemoTimelineUi>,
    pub prediction_debug: Option<PredictionDebugUi>,
    pub hud: Option<HudState>,
    pub hud_layout: HudLayout,
    pub crosshair: CrosshairSettings,
    pub crosshair_target: UiCrosshairTarget,
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
            race_timer: None,
            vote_line: None,
            scoreboard: None,
            demo_timeline: None,
            prediction_debug: None,
            hud: None,
            hud_layout: HudLayout::default(),
            crosshair: CrosshairSettings::default(),
            crosshair_target: UiCrosshairTarget::default(),
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct UiVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// 0.0 = solid color, 1.0 = charsgrid, 2.0 = proportional font, 3.0 = splash.
    pub textured: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProportionalGlyph {
    pub width: i16,
    pub height: i16,
    pub horiz_advance: i16,
    pub horiz_offset: i16,
    pub baseline: i32,
    pub s: f32,
    pub t: f32,
    pub s2: f32,
    pub t2: f32,
}

#[derive(Debug, Clone)]
pub struct ProportionalFont {
    pub glyphs: [ProportionalGlyph; 256],
    pub point_size: i16,
    pub height: i16,
}

pub fn parse_fontdat(bytes: &[u8]) -> Result<ProportionalFont, String> {
    const GLYPH_COUNT: usize = 256;
    const GLYPH_BYTES: usize = 28;
    const HEADER_BYTES: usize = GLYPH_COUNT * GLYPH_BYTES + 10;
    if bytes.len() < HEADER_BYTES {
        return Err(format!(
            "fontdat is {} bytes; expected at least {HEADER_BYTES}",
            bytes.len()
        ));
    }

    fn read_i16(bytes: &[u8], offset: &mut usize) -> i16 {
        let value = i16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
        *offset += 2;
        value
    }
    fn read_i32(bytes: &[u8], offset: &mut usize) -> i32 {
        let value = i32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }
    fn read_f32(bytes: &[u8], offset: &mut usize) -> f32 {
        let value = f32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }

    let mut offset = 0usize;
    let mut glyphs = [ProportionalGlyph::default(); GLYPH_COUNT];
    for glyph in &mut glyphs {
        *glyph = ProportionalGlyph {
            width: read_i16(bytes, &mut offset),
            height: read_i16(bytes, &mut offset),
            horiz_advance: read_i16(bytes, &mut offset),
            horiz_offset: read_i16(bytes, &mut offset),
            baseline: read_i32(bytes, &mut offset),
            s: read_f32(bytes, &mut offset),
            t: read_f32(bytes, &mut offset),
            s2: read_f32(bytes, &mut offset),
            t2: read_f32(bytes, &mut offset),
        };
        if glyph.width < 0
            || glyph.height < 0
            || glyph.horiz_advance < 0
            || ![glyph.s, glyph.t, glyph.s2, glyph.t2]
                .into_iter()
                .all(f32::is_finite)
        {
            return Err("fontdat contains invalid glyph metrics".into());
        }
    }
    let point_size = read_i16(bytes, &mut offset);
    let height = read_i16(bytes, &mut offset);
    let _ascender = read_i16(bytes, &mut offset);
    let _descender = read_i16(bytes, &mut offset);
    let _korean_hack = read_i16(bytes, &mut offset);
    if point_size <= 0 || height <= 0 {
        return Err(format!(
            "fontdat has invalid dimensions point_size={point_size} height={height}"
        ));
    }

    Ok(ProportionalFont {
        glyphs,
        point_size,
        height,
    })
}













pub fn build_vertices(
    ui: &UiSnapshot,
    width: u32,
    height: u32,
    small_font: Option<&ProportionalFont>,
    splash_size: Option<[u32; 2]>,
) -> Vec<UiVertex> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(16_384);
    if ui.startup_splash {
        build_splash_background(&mut out, width, height, splash_size);
        return out;
    }
    if let Some(loading) = &ui.loading {
        build_loading_screen(&mut out, loading, width, height, splash_size);
        // Keep the console usable during background map preparation/upload.
        // It is intentionally layered over the loading screen rather than
        // replacing it, so load progress remains visible behind the console.
        if ui.mode == OverlayMode::Console {
            build_console(&mut out, ui, width, height);
        }
        return out;
    }
    if matches!(ui.mode, OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::CameraEdit | OverlayMode::MapEdit) {
        build_scoreboard(&mut out, ui, small_font, width, height);
    }
    if ui.video.reflection_debug {
        build_reflection_debug_legend(&mut out, ui, width, height);
    }
    match ui.mode {
        OverlayMode::None | OverlayMode::Chat => {}
        OverlayMode::Console => build_console(&mut out, ui, width, height),
        // The Game/Video menus are drawn by egui; see `app::egui_menu`.
        OverlayMode::Video | OverlayMode::Game | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::CameraEdit | OverlayMode::MapEdit | OverlayMode::EntityGraph | OverlayMode::Trace => {}
    }
    // The trace result panel used to be drawn here from `ui.surface_inspector`
    // (`build_surface_inspector`, a static non-interactive readout). It's now
    // the clickable `OverlayMode::Trace` egui menu (`app::egui_trace_menu`);
    // `ui.surface_inspector` still mirrors the selected entry for Ctrl+C, but
    // nothing draws it on the HUD any more.
    if let Some(progress) = &ui.static_ao_progress {
        build_background_progress(&mut out, progress, width, height);
    }
    out
}

fn gameplay_hud_visible(ui: &UiSnapshot, width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && !ui.video.skip_ui
        && ui.loading.is_none()
        && matches!(ui.mode, OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::CameraEdit | OverlayMode::MapEdit)
}

/// Stable gameplay HUD geometry. This changes when HUD values/settings change,
/// not merely because velocity/view input produced another movement sample.
pub fn build_dynamic_static_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    width: u32,
    height: u32,
) {
    if !gameplay_hud_visible(ui, width, height) {
        return;
    }
    build_hud(out, ui, width, height);
}

/// Hot movement-driven HUD geometry. Strafehelper and MovementKeys are the only
/// legacy-HUD pieces that need to follow every new input/simulation snapshot.
pub fn build_movement_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    width: u32,
    height: u32,
) {
    if !gameplay_hud_visible(ui, width, height) {
        return;
    }
    build_strafe_helper(out, ui, width, height);
    draw_placed(out, ui, HudElementId::MovementKeys, width, height, |out| {
        build_movement_keys(out, ui, width, height);
    });
}

/// Chat history and center-print alpha are periodic presentation changes, not
/// retained UI changes. Rebuild just this small text batch when their fade moves.
pub fn build_transient_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    if width == 0 || height == 0 || ui.video.skip_ui || ui.loading.is_some() {
        return;
    }
    let gameplay_overlay = matches!(
        ui.mode,
        OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::CameraEdit | OverlayMode::MapEdit
    );
    if gameplay_hud_visible(ui, width, height) {
        // Colour and name follow the aim target, so they share the small batch
        // that is rebuilt when it changes rather than the stable HUD prefix.
        let mut crosshair = ui.crosshair;
        if ui.strafe_helper.flags & SHELPER_CROSSHAIR != 0 {
            // jaPRO draws its line crosshair from cg_strafeHelper and suppresses the normal one.
            crosshair.style = CROSSHAIR_STYLE_LINE;
            crosshair.image = 0;
        }
        build_crosshair(
            out,
            crosshair,
            ui.crosshair_target.color,
            ui.strafe_helper.line_width,
            width,
            height,
        );
        draw_placed(out, ui, HudElementId::CrosshairName, width, height, |out| {
            build_crosshair_name(out, ui, small_font, width, height);
        });
    }
    if gameplay_overlay {
        // Follows the snapshot's client every frame (CG_DrawFollow), so it lives
        // in the batch that is republished on change, not the retained UI.
        draw_placed(out, ui, HudElementId::Follow, width, height, |out| {
            build_follow_indicator(out, ui, small_font, width, height);
        });
        build_race_timer(out, ui, small_font, width, height);
        build_speedometer(out, ui, small_font, width, height);
        build_lagometer(out, ui, small_font, width, height);
        draw_placed(out, ui, HudElementId::Vote, width, height, |out| {
            build_vote(out, ui, width, height);
        });
        draw_placed(out, ui, HudElementId::Chat, width, height, |out| {
            build_chat_history(out, ui, small_font, width, height);
        });
        draw_placed(out, ui, HudElementId::CenterPrint, width, height, |out| {
            build_center_print(out, ui, small_font, width, height);
        });
        build_demo_timeline(out, ui, width, height);
    }
    build_prediction_debug(out, ui, width, height);
    // FPS/perf is intentionally visible above menus too, matching the old
    // retained path and submit_ui_overlay ordering.
    draw_placed(out, ui, HudElementId::Fps, width, height, |out| match ui.video.draw_fps {
        0 => {}
        1 => build_fps_simple(out, ui, width, height),
        _ => build_perf(out, ui, width, height),
    });
    if ui.mode == OverlayMode::Chat {
        // The input box is anchored to the oldest visible chat line, so keep
        // it in the same transient batch as chat history. This lets fades/new
        // messages reposition it without rebuilding the retained UI.
        draw_placed(out, ui, HudElementId::Chat, width, height, |out| {
            build_chat_input(out, ui, small_font, width, height);
        });
    }
}

fn build_prediction_debug(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(debug) = &ui.prediction_debug else {
        return;
    };

    if debug.flash {
        // A border instead of a full-screen wash keeps the world readable while
        // still making a one/few-frame prediction discontinuity impossible to miss.
        let t = 8.0_f32.min((w.min(h) as f32 * 0.02).max(3.0));
        let c = [1.0, 0.05, 0.02, 0.72];
        rect(out, 0.0, 0.0, w as f32, t, c, w, h);
        rect(out, 0.0, h as f32 - t, w as f32, t, c, w, h);
        rect(out, 0.0, 0.0, t, h as f32, c, w, h);
        rect(out, w as f32 - t, 0.0, t, h as f32, c, w, h);
    }

    if !debug.show_panel {
        return;
    }

    let x = 18.0;
    // Keep clear of the simple/detailed FPS block in the upper-left.
    let y = if ui.video.draw_fps == 0 { 18.0 } else { 92.0 };
    let line_h = 10.0;
    let line_count = debug.lines.len().max(1) as f32;
    let panel_w = (w as f32 * 0.72).clamp(420.0, 980.0);
    let panel_h = 34.0 + line_count * line_h;
    rect(out, x - 8.0, y - 7.0, panel_w, panel_h, [0.0, 0.0, 0.0, 0.68], w, h);
    rect(out, x - 8.0, y - 7.0, 3.0, panel_h, [0.9, 0.18, 0.05, 0.95], w, h);

    let heading = debug.last_miss.map_or_else(
        || format!("PREDICTION DIAGNOSTICS  threshold {:.1}u  no miss captured yet", debug.threshold),
        |miss| format!("PREDICTION DIAGNOSTICS  last miss {:.2}u  threshold {:.1}u", miss, debug.threshold),
    );
    text(out, &heading, x, y, 1.0, [1.0, 0.72, 0.45, 1.0], w, h);
    for (index, line) in debug.lines.iter().enumerate() {
        text(
            out,
            line,
            x,
            y + 13.0 + index as f32 * line_h,
            0.9,
            [0.92, 0.94, 0.98, 1.0],
            w,
            h,
        );
    }
}

fn build_demo_timeline(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(state) = &ui.demo_timeline else {
        return;
    };
    let Some(layout) = demo_timeline_layout(w, h) else {
        return;
    };

    if state.outside_authoritative_view {
        rect(out, 0.0, 0.0, w as f32, h as f32, [0.55, 0.0, 0.0, 0.20], w, h);
        text(
            out,
            "OUTSIDE RECORDED PVS / PORTAL VISIBILITY",
            18.0,
            18.0,
            1.15,
            [1.0, 0.72, 0.72, 1.0],
            w,
            h,
        );
    }
    let panel_y = h as f32 - 46.0;
    text(
        out,
        &format!("{}   [ / ] POV   F FREE   HOME RECORDED", state.camera_label),
        16.0,
        panel_y - 18.0,
        0.90,
        [0.78, 0.84, 0.90, 0.95],
        w,
        h,
    );
    rect(out, 8.0, panel_y, w as f32 - 16.0, 42.0, [0.01, 0.016, 0.024, 0.84], w, h);
    rect(out, 8.0, panel_y, w as f32 - 16.0, 1.0, [0.24, 0.31, 0.40, 0.95], w, h);

    let paused = state.paused;
    let [px, py, pw, ph] = layout.play;
    rect(out, px, py, pw, ph, [0.07, 0.10, 0.14, 0.96], w, h);
    rect(out, px, py, pw, 1.0, [0.30, 0.40, 0.52, 0.95], w, h);
    text(
        out,
        if paused { ">" } else { "||" },
        px + if paused { 10.0 } else { 6.5 },
        py + 6.0,
        1.35,
        [0.92, 0.96, 1.0, 1.0],
        w,
        h,
    );

    let duration = state.duration_ms.max(1) as f64;
    let live_fraction = (state.elapsed_ms / duration).clamp(0.0, 1.0) as f32;
    let fraction = state.scrub_fraction.unwrap_or(live_fraction).clamp(0.0, 1.0);
    let preview_ms = if state.scrub_fraction.is_some() {
        (duration * f64::from(fraction)).round() as i32
    } else {
        state.elapsed_ms.round().clamp(0.0, duration) as i32
    };
    let current_text = format_demo_time(preview_ms);
    let total_text = format_demo_time(state.duration_ms);
    text(out, &current_text, 52.0, panel_y + 13.0, 1.05, [0.82, 0.88, 0.94, 1.0], w, h);

    let [tx, ty, tw, th] = layout.track;
    let line_y = ty + th * 0.5 - 2.0;
    rect(out, tx, line_y, tw, 4.0, [0.12, 0.16, 0.21, 1.0], w, h);
    rect(out, tx, line_y, tw * fraction, 4.0, [0.55, 0.37, 0.88, 1.0], w, h);

    for marker in &state.kill_markers {
        let mf = marker.fraction.clamp(0.0, 1.0);
        let involved = marker.followed_kill || marker.followed_death;
        let tick_h = if involved { 24.0 } else { 14.0 };
        let tick_w = if involved { 3.0 } else { 1.5 };
        let color = match marker.attacker_team {
            1 => [1.0, 0.30, 0.30, 0.98],
            2 => [0.32, 0.58, 1.0, 0.98],
            _ => [1.0, 0.84, 0.34, 0.98],
        };
        let x = tx + tw * mf - tick_w * 0.5;
        rect(out, x, ty + th * 0.5 - tick_h * 0.5, tick_w, tick_h, color, w, h);
        if involved {
            rect(out, x - 1.0, ty + th * 0.5 - tick_h * 0.5, tick_w + 2.0, 1.0, [1.0, 1.0, 1.0, 0.95], w, h);
        }
    }

    let thumb_x = tx + tw * fraction;
    rect(out, thumb_x - 1.5, ty + 1.0, 3.0, th - 2.0, [0.95, 0.96, 1.0, 1.0], w, h);

    text(out, &total_text, tx + tw + 10.0, panel_y + 13.0, 1.05, [0.62, 0.70, 0.78, 1.0], w, h);

    let [sx, sy, sw, sh] = layout.speed;
    rect(out, sx, sy, sw, sh, [0.07, 0.10, 0.14, 0.96], w, h);
    rect(out, sx, sy, sw, 1.0, [0.25, 0.50, 0.35, 0.95], w, h);
    let speed = format!("{}x", state.playback_rate);
    text(out, &speed, sx + 8.0, sy + 6.0, 1.15, [0.78, 0.94, 0.84, 1.0], w, h);
}

fn format_demo_time(ms: i32) -> String {
    let total_seconds = ms.max(0) / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn build_center_print(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    medium_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(print) = &ui.center_print else { return };
    let alpha = print.alpha.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return;
    }

    let lines = center_print_lines(&print.text);
    if lines.is_empty() {
        return;
    }
    let y_scale = h as f32 / 480.0;
    let center_y = 480.0 * print.y_fraction.clamp(0.0, 1.0) * y_scale;

    if let Some(font) = medium_font {
        // CG_DrawCenterString uses FONT_MEDIUM, scale 1.0, centred around 30%
        // screen height. Our loaded OCR font is shared by the native UI path;
        // use a larger scale here to match the medium-font role.
        let scale = 0.78;
        let line_step = 24.0 * scale * y_scale;
        let mut baseline = center_y - (lines.len().saturating_sub(1) as f32 * line_step * 0.5);
        for line in lines {
            let virtual_width = proportional_text_width(&line, font, scale);
            let x = ((640.0 - virtual_width) * 0.5) * w as f32 / 640.0;
            proportional_text(
                out,
                &line,
                font,
                x,
                baseline,
                scale,
                [1.0, 1.0, 1.0, alpha],
                true,
                w,
                h,
            );
            baseline += line_step;
        }
        return;
    }

    let glyph_w = w as f32 * (10.0 / 640.0);
    let glyph_h = h as f32 * (16.0 / 480.0);
    let advance = w as f32 * (10.0 / 640.0);
    let line_step = h as f32 * (22.0 / 480.0);
    let mut y = center_y - lines.len().saturating_sub(1) as f32 * line_step * 0.5;
    for line in lines {
        let visible = visible_jka_chars(&line) as f32;
        let x = (w as f32 - visible * advance) * 0.5;
        fixed_charset_text(
            out,
            &line,
            x,
            y,
            glyph_w,
            glyph_h,
            advance,
            [1.0, 1.0, 1.0, alpha],
            true,
            w,
            h,
        );
        y += line_step;
    }
}

fn build_follow_indicator(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(follow) = ui.follow_name.as_deref() else { return };
    // Second line (optional) is the jaPRO racemode movement style.
    let (name, style) = match follow.split_once('\n') {
        Some((name, style)) => (name, Some(style)),
        None => (follow, None),
    };
    if name.is_empty() {
        return;
    }

    // jaPRO CG_DrawFollow: CG_Text_Paint(4, 27, 0.85, colorWhite, name, ..,
    // FONT_MEDIUM) - the name, top-left, no drop shadow; in jaPRO racemode the
    // style goes at (4, 44) scale 0.7. 0.85 * 0.78 maps the medium-font role
    // onto the shared OCR font (see build_center_print).
    let x = 4.0 * w as f32 / 640.0;
    let lines = std::iter::once((name, 27.0, 0.85)).chain(style.map(|style| (style, 44.0, 0.7)));
    for (text, y, scale) in lines {
        let y = y * h as f32 / 480.0;
        if let Some(font) = small_font {
            proportional_text(out, text, font, x, y, scale * 0.78, [1.0, 1.0, 1.0, 1.0], false, w, h);
            continue;
        }
        let glyph_scale = scale / 0.85;
        fixed_charset_text(
            out,
            text,
            x,
            y,
            w as f32 * (8.0 / 640.0) * glyph_scale,
            h as f32 * (12.0 / 480.0) * glyph_scale,
            w as f32 * (8.0 / 640.0) * glyph_scale,
            [1.0, 1.0, 1.0, 1.0],
            false,
            w,
            h,
        );
    }
}

/// `CG_DrawVote`: two `CG_DrawSmallString` lines at (4, 62) while a vote is open.
fn build_vote(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(line) = ui.vote_line.as_deref() else { return };
    let x = 4.0 * w as f32 / 640.0;
    let glyph_w = w as f32 * (8.0 / 640.0);
    let glyph_h = h as f32 * (12.0 / 480.0);
    let mut y = 62.0 * h as f32 / 480.0;
    for text in [line, "or press ESC then click Vote"] {
        fixed_charset_text(out, text, x, y, glyph_w, glyph_h, glyph_w, [1.0, 1.0, 1.0, 1.0], true, w, h);
        y += h as f32 * (18.0 / 480.0);
    }
}

/// jaPRO `DF_RaceTimer`: `CG_Text_Paint(x, y, size, ..)` at the configured
/// 640x480 position, shadowed, for the timer block and the start-speed line.
/// Clamp a colour the speedometer derived from `1 / ratio^2` (infinite when the
/// reference speed is zero) into a drawable range.
fn speedometer_color(color: [f32; 4]) -> [f32; 4] {
    color.map(|channel| if channel.is_nan() { 1.0 } else { channel.clamp(0.0, 1.0) })
}

/// jaPRO `cg_speedometer`: text at the 640x480 positions `DF_DrawSpeedometer`
/// and friends pick, in the same shadowed font as the race timer.
fn build_speedometer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(speedo) = ui.speedometer.as_ref() else { return };
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let paint = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Text| {
        let (px, py) = (item.x * sx, item.y * sy);
        let color = speedometer_color(item.color);
        if let Some(font) = small_font {
            // Glyph bytes: the acceleration label is the font's 0xB5 character.
            let bytes: Vec<u8> = item.text.chars().map(|c| c as u32 as u8).collect();
            proportional_text(out, bytes, font, px, py, speedo.size * 0.78, color, true, w, h);
        } else {
            let ascii: String = item.text.chars().map(|c| if c == '\u{b5}' { 'u' } else { c }).collect();
            text(out, &ascii, px, py, speedo.size * 1.4, color, w, h);
        }
    };
    let fill = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Rect| {
        let color = speedometer_color(item.color);
        let (x, y, rw, rh) = (item.x * sx, item.y * sy, item.w * sx, item.h * sy);
        match item.outline {
            Some(thickness) => rect_outline(out, x, y, rw, rh, (thickness * sy).max(1.0), color, w, h),
            None => rect(out, x, y, rw, rh, color, w, h),
        }
    };
    draw_placed(out, ui, HudElementId::Speedometer, w, h, |out| {
        for item in &speedo.rects {
            fill(out, item);
        }
        for item in &speedo.texts {
            paint(out, item);
        }
    });
    draw_placed(out, ui, HudElementId::SpeedometerJumps, w, h, |out| {
        for item in &speedo.jump_texts {
            paint(out, item);
        }
    });
    draw_placed(out, ui, HudElementId::SpeedGraph, w, h, |out| {
        // The old speed graph's lag frame goes under its bars, its readout over them.
        for pic in &speedo.graph_pics {
            lagometer_pic(out, pic, w, h);
        }
        for item in &speedo.graph_rects {
            fill(out, item);
        }
        for item in &speedo.graph_texts {
            lagometer_text(out, item, small_font, w, h);
        }
    });
}

/// Image names of the icon atlas (see `renderer::load_ui_icon_atlas`): the two
/// lagometer images in `crate::lagometer::Icon` order, then the image crosshairs
/// from `ICON_CROSSHAIR_BASE` in `CROSSHAIR_IMAGE_NAMES` order.
pub const ICON_NAMES: [&str; 2 + CROSSHAIR_IMAGE_COUNT as usize] = [
    "gfx/2d/lag",
    "gfx/2d/net",
    CROSSHAIR_IMAGE_NAMES[0],
    CROSSHAIR_IMAGE_NAMES[1],
    CROSSHAIR_IMAGE_NAMES[2],
    CROSSHAIR_IMAGE_NAMES[3],
    CROSSHAIR_IMAGE_NAMES[4],
    CROSSHAIR_IMAGE_NAMES[5],
    CROSSHAIR_IMAGE_NAMES[6],
    CROSSHAIR_IMAGE_NAMES[7],
    CROSSHAIR_IMAGE_NAMES[8],
    CROSSHAIR_IMAGE_NAMES[9],
];
/// Atlas cell of the first image crosshair.
const ICON_CROSSHAIR_BASE: usize = 2;
/// Edge of one atlas cell in texels; the retail 32x32 images are resampled to it.
pub const ICON_CELL: u32 = 64;
/// Texture source id of the icon atlas in `ui.wgsl`.
const ICON_TEXTURE_SOURCE: f32 = 5.0;

/// The whole of atlas cell `index` as (uv0, uv1).
fn icon_cell_uv(index: usize) -> ([f32; 2], [f32; 2]) {
    let cell = ICON_CELL as f32;
    let atlas_w = ICON_NAMES.len() as f32 * cell;
    let index = index as f32;
    // Half-texel inset keeps linear filtering inside this image's cell.
    let u0 = (index * cell + 0.5) / atlas_w;
    let u1 = ((index + 1.0) * cell - 0.5) / atlas_w;
    ([u0, 0.5 / cell], [u1, (cell - 0.5) / cell])
}

/// `CG_DrawPic` of a lagometer image: the whole image, white, alpha blended.
fn lagometer_pic(out: &mut Vec<UiVertex>, pic: &crate::lagometer::Pic, w: u32, h: u32) {
    let index = match pic.icon {
        crate::lagometer::Icon::Lag => 0,
        crate::lagometer::Icon::Net => 1,
    };
    let (uv0, uv1) = icon_cell_uv(index);
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    textured_rect_with_source(
        out,
        pic.x * sx,
        pic.y * sy,
        pic.w * sx,
        pic.h * sy,
        uv0,
        uv1,
        [1.0; 4],
        ICON_TEXTURE_SOURCE,
        w,
        h,
    );
}

/// `CG_Text_Paint(.., 0.5, colorWhite, .., ITEM_TEXTSTYLE_SHADOWEDMORE, FONT_SMALL)`; a
/// right-aligned item subtracts `CG_Text_Width` from its x first.
fn lagometer_text(
    out: &mut Vec<UiVertex>,
    item: &crate::lagometer::Text,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    const SCALE: f32 = 0.5;
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    if let Some(font) = small_font {
        let x = if item.right {
            item.x - proportional_text_width(&item.text, font, SCALE)
        } else {
            item.x
        };
        proportional_text(out, &item.text, font, x * sx, item.y * sy, SCALE, [1.0; 4], true, w, h);
    } else {
        // Asset fallback only (see build_chat_history): the fixed charset at a small size.
        const GLYPH_W: f32 = 5.0;
        let x = if item.right {
            item.x - visible_jka_chars(&item.text) as f32 * GLYPH_W
        } else {
            item.x
        };
        fixed_charset_text(
            out,
            &item.text,
            x * sx,
            item.y * sy,
            GLYPH_W * sx,
            8.0 * sy,
            GLYPH_W * sx,
            [1.0; 4],
            true,
            w,
            h,
        );
    }
}

/// jaPRO `CG_DrawLagometer` / `CG_DrawDisconnect`: the graph in the lag frame, its
/// numbers, and the "Connection Interrupted" warning, at the 640x480 positions the
/// draw list carries (no drop of `widthRatioCoef`, as for the speedometer).
fn build_lagometer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(lag) = ui.lagometer.as_ref() else { return };
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    for pic in &lag.pics {
        lagometer_pic(out, pic, w, h);
    }
    for bar in &lag.rects {
        rect(out, bar.x * sx, bar.y * sy, bar.w * sx, bar.h * sy, bar.color, w, h);
    }
    for item in &lag.texts {
        lagometer_text(out, item, small_font, w, h);
    }
    // CG_DrawBigString: 16x16 glyphs of the fixed charset, white with a shadow.
    for item in &lag.big_texts {
        fixed_charset_text(
            out,
            &item.text,
            item.x * sx,
            item.y * sy,
            item.char_w * sx,
            crate::lagometer::BIGCHAR * sy,
            item.char_w * sx,
            [1.0; 4],
            true,
            w,
            h,
        );
    }
}

fn build_race_timer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(race) = ui.race_timer.as_ref() else { return };
    let paint = |out: &mut Vec<UiVertex>, value: &str, x: f32, y: f32, color: [f32; 4]| {
        if value.is_empty() {
            return;
        }
        let px = x * w as f32 / 640.0;
        let py = y * h as f32 / 480.0;
        if let Some(font) = small_font {
            // 0.78 maps the medium-font role onto the shared OCR font (see build_center_print).
            proportional_text(out, value, font, px, py, race.size * 0.78, color, true, w, h);
        } else {
            text(out, value, px, py, race.size * 1.4, color, w, h);
        }
    };
    draw_placed(out, ui, HudElementId::RaceTimer, w, h, |out| {
        paint(out, &race.timer_text, race.timer_x, race.timer_y, [1.0, 1.0, 1.0, 1.0]);
    });
    let [r, g, b] = race.start_color;
    draw_placed(out, ui, HudElementId::RaceStart, w, h, |out| {
        paint(out, &race.start_text, race.start_x, race.start_y, [r, g, b, 1.0]);
    });
}

fn visible_jka_chars(value: &str) -> usize {
    let mut chars = value.chars().peekable();
    let mut visible = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(&code) = chars.peek() {
                if color_code(code, 1.0).is_some() {
                    let _ = chars.next();
                    continue;
                }
            }
        }
        visible += 1;
    }
    visible
}

fn center_print_lines(value: &str) -> Vec<String> {
    // OpenJK's CG_DrawCenterString wraps each source line at 50 characters and
    // prefers a whitespace break when possible. Keep color escapes zero-width.
    let mut lines = Vec::new();
    for source in value.split('\n') {
        let source = source.trim_end_matches('\r');
        if source.is_empty() {
            lines.push(String::new());
            continue;
        }
        let chars: Vec<char> = source.chars().collect();
        let mut start = 0usize;
        while start < chars.len() {
            let mut index = start;
            let mut visible = 0usize;
            let mut last_space = None;
            while index < chars.len() && visible < 50 {
                if chars[index] == '^'
                    && index + 1 < chars.len()
                    && color_code(chars[index + 1], 1.0).is_some()
                {
                    index += 2;
                    continue;
                }
                if chars[index].is_whitespace() {
                    last_space = Some(index);
                }
                index += 1;
                visible += 1;
            }
            if index >= chars.len() {
                let tail: String = chars[start..].iter().collect();
                lines.push(tail.trim().to_owned());
                break;
            }
            let cut = last_space.filter(|space| *space > start).unwrap_or(index);
            let head: String = chars[start..cut].iter().collect();
            lines.push(head.trim_end().to_owned());
            start = cut;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
        }
    }
    lines
}

fn build_scoreboard(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(board) = &ui.scoreboard else { return };

    // Renderer-native on purpose: +scores stays a lightweight gameplay overlay
    // and never changes input capture / enters the egui menu stack.
    // Layout is authored in a 1080p-ish screen-space and scales with height so
    // the board keeps the same visual weight from 720p through 4K.
    let s = (h as f32 / 1080.0).clamp(0.72, 2.0);
    let width = (960.0 * s).min((w as f32 - 32.0 * s).max(320.0 * s));
    let x = ((w as f32 - width) * 0.5).max(16.0 * s);
    let y = (h as f32 * 0.065).max(24.0 * s);

    let title_h = 48.0 * s;
    let columns_h = 27.0 * s;
    let row_h = 36.0 * s;
    let bottom_pad = 8.0 * s;
    let overflow_h = if board.entries.is_empty() { 0.0 } else { 20.0 * s };
    let section_h = 30.0 * s;

    // jaPRO's CG_DrawOldScoreboard: players first (a team game lists the leading
    // team, then the other, then anyone on neither), with everyone spectating in a
    // block of their own underneath. Each group keeps the server's score order.
    enum Line<'a> {
        Entry(&'a UiScoreEntry, usize),
        Section(&'static str, usize),
    }
    let mut lines: Vec<Line> = Vec::with_capacity(board.entries.len() + 1);
    let team_order: &[i32] = if !board.team_game {
        &[0]
    } else if board.team_scores[0] >= board.team_scores[1] {
        &[1, 2, 0]
    } else {
        &[2, 1, 0]
    };
    for &team in team_order {
        let group = board.entries.iter().filter(|entry| entry.team == team || (!board.team_game && entry.team != 3));
        for (index, entry) in group.enumerate() {
            lines.push(Line::Entry(entry, index));
        }
        if !board.team_game {
            break;
        }
    }
    let players = lines.len();
    let spectators = board.entries.iter().filter(|entry| entry.team == 3).count();
    if spectators > 0 {
        lines.push(Line::Section("SPECTATORS", spectators));
        for (index, entry) in board.entries.iter().filter(|entry| entry.team == 3).enumerate() {
            lines.push(Line::Entry(entry, index));
        }
    }

    let room_for_rows = (h as f32 - y - 18.0 * s - title_h - columns_h - bottom_pad).max(row_h);
    let available = (room_for_rows - overflow_h).max(row_h);
    let line_height = |line: &Line| if matches!(line, Line::Section(..)) { section_h } else { row_h };
    let mut shown = 0usize;
    let mut used = 0.0f32;
    for line in &lines {
        if used + line_height(line) > available && shown > 0 {
            break;
        }
        used += line_height(line);
        shown += 1;
    }
    // Never end on a heading with nothing under it.
    if shown > 0 && matches!(lines[shown - 1], Line::Section(..)) {
        used -= section_h;
        shown -= 1;
    }
    let overflow = lines[shown..].iter().filter(|line| matches!(line, Line::Entry(..))).count();
    let footer_h = if overflow > 0 { overflow_h } else { 0.0 };
    let height = title_h + columns_h + used + footer_h + bottom_pad;

    const PANEL_TOP: [f32; 4] = [0.045, 0.055, 0.075, 0.965];
    const PANEL_BOTTOM: [f32; 4] = [0.014, 0.018, 0.028, 0.955];
    const TITLE_LEFT: [f32; 4] = [0.075, 0.105, 0.145, 0.98];
    const TITLE_RIGHT: [f32; 4] = [0.030, 0.040, 0.060, 0.98];
    const ACCENT: [f32; 4] = [0.28, 0.78, 0.94, 1.0];
    const TEXT_BRIGHT: [f32; 4] = [0.94, 0.965, 0.99, 1.0];
    const TEXT: [f32; 4] = [0.82, 0.86, 0.91, 1.0];
    const TEXT_DIM: [f32; 4] = [0.51, 0.58, 0.67, 1.0];

    // A soft, offset shadow plus a low-contrast outer keyline reads much cleaner
    // than the old bright grey rectangle without making the overlay feel like a
    // menu window.
    rect(
        out,
        x - 4.0 * s,
        y + 5.0 * s,
        width + 8.0 * s,
        height + 4.0 * s,
        [0.0, 0.0, 0.0, 0.30],
        w,
        h,
    );
    rect_gradient(
        out,
        x,
        y,
        width,
        height,
        PANEL_TOP,
        PANEL_TOP,
        PANEL_BOTTOM,
        PANEL_BOTTOM,
        w,
        h,
    );
    rect_outline(
        out,
        x,
        y,
        width,
        height,
        (1.0 * s).max(1.0),
        [0.30, 0.38, 0.48, 0.58],
        w,
        h,
    );

    // Header: restrained blue-grey panel with a thin cyan identity line. Team
    // scores (when relevant) live here rather than being mixed into the title.
    rect_gradient(
        out,
        x + 1.0 * s,
        y + 1.0 * s,
        width - 2.0 * s,
        title_h - 1.0 * s,
        TITLE_LEFT,
        TITLE_RIGHT,
        [0.045, 0.060, 0.085, 0.98],
        [0.022, 0.030, 0.046, 0.98],
        w,
        h,
    );
    rect(out, x + 1.0 * s, y + 1.0 * s, width - 2.0 * s, (2.0 * s).max(1.0), ACCENT, w, h);

    let font_x_scale = w.max(1) as f32 / 640.0;
    let title_font_scale = 0.52;
    let header_font_scale = 0.31;
    let row_font_scale = 0.42;
    let meta_font_scale = 0.32;
    let fallback_title_scale = 1.34 * s;
    let fallback_header_scale = 0.92 * s;
    let fallback_row_scale = 1.06 * s;
    let fallback_meta_scale = 0.88 * s;

    let draw = |out: &mut Vec<UiVertex>,
                value: &str,
                tx: f32,
                baseline_y: f32,
                prop_scale: f32,
                fallback_scale: f32,
                color: [f32; 4]| {
        if let Some(font) = small_font {
            proportional_text(out, value, font, tx, baseline_y, prop_scale, color, true, w, h);
        } else {
            // The proportional font is normally resident. Keep the charsgrid
            // fallback so a missing font asset can never hide the scoreboard.
            text(out, value, tx, baseline_y - 8.0 * fallback_scale, fallback_scale, color, w, h);
        }
    };
    let text_width = |value: &str, prop_scale: f32, fallback_scale: f32| -> f32 {
        if let Some(font) = small_font {
            proportional_text_width(value, font, prop_scale) * font_x_scale
        } else {
            visible_jka_chars(value) as f32 * 6.0 * fallback_scale
        }
    };

    let pad = 18.0 * s;
    let title_baseline = y + title_h * 0.68;
    draw(
        out,
        "SCOREBOARD",
        x + pad,
        title_baseline,
        title_font_scale,
        fallback_title_scale,
        TEXT_BRIGHT,
    );

    let meta = if board.team_game {
        format!("^1RED  {}    ^7|    ^4BLUE  {}", board.team_scores[0], board.team_scores[1])
    } else {
        format!("{} PLAYER{}", players, if players == 1 { "" } else { "S" })
    };
    let meta_width = text_width(&meta, meta_font_scale, fallback_meta_scale);
    draw(
        out,
        &meta,
        x + width - pad - meta_width,
        y + title_h * 0.66,
        meta_font_scale,
        fallback_meta_scale,
        if board.team_game { TEXT } else { TEXT_DIM },
    );

    let columns_y = y + title_h;
    rect(
        out,
        x + 1.0 * s,
        columns_y,
        width - 2.0 * s,
        columns_h,
        [0.75, 0.82, 0.92, 0.055],
        w,
        h,
    );
    rect(
        out,
        x + 10.0 * s,
        columns_y + columns_h - (1.0 * s).max(1.0),
        width - 20.0 * s,
        (1.0 * s).max(1.0),
        [0.45, 0.56, 0.70, 0.26],
        w,
        h,
    );

    // Numeric columns are right aligned. This removes the ragged, debug-table
    // look the old scoreboard had while still preserving all vanilla fields.
    let team_right = x + width - pad;
    let time_right = if board.team_game { team_right - 82.0 * s } else { team_right };
    let ping_right = time_right - 92.0 * s;
    let score_right = ping_right - 92.0 * s;
    let player_x = x + pad + 7.0 * s;
    let header_baseline = columns_y + columns_h * 0.69;

    draw(out, "PLAYER", player_x, header_baseline, header_font_scale, fallback_header_scale, TEXT_DIM);
    for (label, right) in [("SCORE", score_right), ("PING", ping_right), ("TIME", time_right)] {
        let tw = text_width(label, header_font_scale, fallback_header_scale);
        draw(out, label, right - tw, header_baseline, header_font_scale, fallback_header_scale, TEXT_DIM);
    }
    if board.team_game {
        let tw = text_width("TEAM", header_font_scale, fallback_header_scale);
        draw(out, "TEAM", team_right - tw, header_baseline, header_font_scale, fallback_header_scale, TEXT_DIM);
    }

    let row_start = columns_y + columns_h;
    let mut cursor = row_start;
    for line in lines.iter().take(shown) {
        let (entry, index) = match *line {
            Line::Section(label, count) => {
                rect(
                    out,
                    x + 10.0 * s,
                    cursor + 4.0 * s,
                    width - 20.0 * s,
                    (1.0 * s).max(1.0),
                    [0.45, 0.56, 0.70, 0.26],
                    w,
                    h,
                );
                draw(
                    out,
                    &format!("{label}  {count}"),
                    player_x,
                    cursor + section_h * 0.76,
                    header_font_scale,
                    fallback_header_scale,
                    TEXT_DIM,
                );
                cursor += section_h;
                continue;
            }
            Line::Entry(entry, index) => (entry, index),
        };
        let row_top = cursor;
        cursor += row_h;
        let row_y = row_top + row_h * 0.68;
        let spectating = entry.team == 3;

        let row_fill = if index % 2 == 0 {
            [0.80, 0.86, 0.96, 0.050]
        } else {
            [0.55, 0.62, 0.72, 0.022]
        };
        rect(
            out,
            x + 6.0 * s,
            row_top + 2.0 * s,
            width - 12.0 * s,
            row_h - 3.0 * s,
            row_fill,
            w,
            h,
        );

        let accent = match entry.team {
            1 => [0.92, 0.22, 0.24, 0.95],
            2 => [0.24, 0.48, 0.96, 0.95],
            3 => [0.50, 0.55, 0.62, 0.72],
            _ => ACCENT,
        };
        rect(
            out,
            x + 6.0 * s,
            row_top + 2.0 * s,
            (3.0 * s).max(2.0),
            row_h - 3.0 * s,
            accent,
            w,
            h,
        );

        let name = truncate_jka_text(&entry.name, if board.team_game { 24 } else { 30 });
        let row_text = if entry.team == 3 {
            [TEXT[0], TEXT[1], TEXT[2], 0.72]
        } else {
            TEXT
        };
        draw(out, &name, player_x, row_y, row_font_scale, fallback_row_scale, row_text);

        if !spectating || board.spectator_scores {
            let score = entry.score.to_string();
            let score_w = text_width(&score, row_font_scale, fallback_row_scale);
            draw(
                out,
                &score,
                score_right - score_w,
                row_y,
                row_font_scale,
                fallback_row_scale,
                TEXT_BRIGHT,
            );
        }

        let ping = if entry.ping < 0 { "CNCT".to_owned() } else { entry.ping.to_string() };
        let ping_w = text_width(&ping, row_font_scale, fallback_row_scale);
        let ping_color = if entry.ping < 0 {
            TEXT_DIM
        } else if entry.ping <= 60 {
            [0.52, 0.88, 0.66, 1.0]
        } else if entry.ping <= 120 {
            [0.88, 0.84, 0.50, 1.0]
        } else {
            [0.94, 0.55, 0.52, 1.0]
        };
        draw(out, &ping, ping_right - ping_w, row_y, row_font_scale, fallback_row_scale, ping_color);

        let time = entry.time.to_string();
        let time_w = text_width(&time, row_font_scale, fallback_row_scale);
        draw(out, &time, time_right - time_w, row_y, row_font_scale, fallback_row_scale, TEXT);

        if board.team_game && !spectating {
            let team = match entry.team {
                1 => "^1RED",
                2 => "^4BLUE",
                _ => "FREE",
            };
            let team_w = text_width(team, meta_font_scale, fallback_meta_scale);
            draw(out, team, team_right - team_w, row_y, meta_font_scale, fallback_meta_scale, TEXT_DIM);
        }
    }

    if overflow > 0 {
        let footer_top = row_start + used;
        rect(
            out,
            x + 10.0 * s,
            footer_top,
            width - 20.0 * s,
            (1.0 * s).max(1.0),
            [0.42, 0.52, 0.64, 0.20],
            w,
            h,
        );
        let more = format!("+{} MORE PLAYER{}", overflow, if overflow == 1 { "" } else { "S" });
        draw(
            out,
            &more,
            x + pad,
            footer_top + footer_h * 0.70,
            meta_font_scale,
            fallback_meta_scale,
            TEXT_DIM,
        );
    }
}

fn truncate_jka_text(value: &str, max_visible: usize) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().min(max_visible + 8));
    let mut visible = 0usize;
    let mut index = 0usize;
    while index < bytes.len() && visible < max_visible {
        if bytes[index] == b'^' && index + 1 < bytes.len() && color_code(bytes[index + 1] as char, 1.0).is_some() {
            out.extend_from_slice(&bytes[index..index + 2]);
            index += 2;
            continue;
        }
        out.push(bytes[index]);
        visible += 1;
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn build_reflection_debug_legend(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let x = 14.0;
    let y = 14.0;
    let width = 246.0;
    let height = 112.0;
    rect(out, x, y, width, height, [0.01, 0.015, 0.022, 0.88], w, h);
    text(
        out,
        &format!("REFLECTIONS: {}", ui.video.reflection_quality.label()),
        x + 10.0,
        y + 8.0,
        1.15,
        [0.92, 0.95, 1.0, 1.0],
        w,
        h,
    );
    let rows = [
        ("PLANAR", [1.0, 0.05, 0.85, 1.0]),
        ("SSR HIT", [0.05, 1.0, 0.18, 1.0]),
        ("PROBE / CUBEMAP", [0.06, 0.20, 1.0, 1.0]),
        ("SSR ELIGIBLE / NO HIT", [0.18, 0.18, 0.18, 1.0]),
        ("NO ENHANCED REFLECTION", [0.0, 0.0, 0.0, 1.0]),
    ];
    for (index, (label, color)) in rows.into_iter().enumerate() {
        let row_y = y + 29.0 + index as f32 * 15.0;
        rect(out, x + 10.0, row_y + 1.0, 10.0, 10.0, color, w, h);
        text(out, label, x + 27.0, row_y, 0.92, [0.82, 0.88, 0.95, 1.0], w, h);
    }
}

const LOAD_CYAN: [f32; 4] = [0.28, 0.86, 0.95, 1.0];
const LOAD_MAGENTA: [f32; 4] = [1.0, 0.30, 0.56, 1.0];
const LOAD_AMBER: [f32; 4] = [1.0, 0.72, 0.28, 1.0];
const LOAD_DONE: [f32; 4] = [0.30, 0.85, 0.66, 1.0];
const LOAD_LABEL: [f32; 4] = [0.62, 0.72, 0.84, 1.0];
const LOAD_FAINT: [f32; 4] = [0.38, 0.46, 0.58, 1.0];

/// Neon edge (cyan to magenta) with a soft glow on `glow_dir` (+1 below, -1 above).
#[allow(clippy::too_many_arguments)]
fn neon_edge(out: &mut Vec<UiVertex>, x: f32, y: f32, width: f32, thickness: f32, glow_dir: f32, w: u32, h: u32) {
    let l = with_alpha(LOAD_CYAN, 0.95);
    let r = with_alpha(LOAD_MAGENTA, 0.95);
    rect_gradient(out, x, y, width, thickness, l, r, l, r, w, h);
    let mut offset = if glow_dir > 0.0 { thickness } else { 0.0 };
    for (height, alpha) in [(3.0, 0.16), (4.0, 0.07), (6.0, 0.025)] {
        let gl = with_alpha(LOAD_CYAN, alpha);
        let gr = with_alpha(LOAD_MAGENTA, alpha);
        let top = if glow_dir > 0.0 { y + offset } else { y - offset - height };
        rect_gradient(out, x, top, width, height, gl, gr, gl, gr, w, h);
        offset += height;
    }
}

/// Thin progress track: dim rail, gradient fill from `from` to `to`.
#[allow(clippy::too_many_arguments)]
fn progress_track(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    fraction: f32,
    from: [f32; 4],
    to: [f32; 4],
    w: u32,
    h: u32,
) {
    rect(out, x, y, width, height, [0.05, 0.08, 0.14, 0.96], w, h);
    let fill = width * fraction.clamp(0.0, 1.0);
    if fill > 0.5 {
        rect_gradient(out, x, y, fill, height, from, to, from, to, w, h);
        let edge = 2.0_f32.min(fill);
        rect(out, x + fill - edge, y, edge, height, lighten(to, 0.55), w, h);
    }
}

fn loading_fraction(bar: &MapLoadingBar) -> f32 {
    if bar.total > 0 {
        (bar.completed as f32 / bar.total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn build_background_progress(out: &mut Vec<UiVertex>, progress: &MapLoadingBar, w: u32, h: u32) {
    let panel_w = 320.0;
    let panel_h = 44.0;
    let x = (w as f32 - panel_w - 18.0).max(8.0);
    let y = (h as f32 - panel_h - 18.0).max(8.0);
    rect_gradient(
        out,
        x,
        y,
        panel_w,
        panel_h,
        HUD_INK_TOP,
        HUD_INK_TOP,
        HUD_INK_BOTTOM,
        HUD_INK_BOTTOM,
        w,
        h,
    );
    rect(out, x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.10], w, h);
    let l = with_alpha(LOAD_CYAN, 0.9);
    let r = with_alpha(LOAD_MAGENTA, 0.9);
    rect_gradient(out, x, y, panel_w, 1.5, l, r, l, r, w, h);
    let fraction = loading_fraction(progress);
    text(out, progress.label, x + 12.0, y + 9.0, 1.1, LOAD_LABEL, w, h);
    let percent = format!("{}%", (fraction * 100.0).round() as u32);
    let percent_w = percent.len() as f32 * 6.0 * 1.3;
    text(out, &percent, x + panel_w - 12.0 - percent_w, y + 8.0, 1.3, HUD_VALUE, w, h);
    progress_track(
        out,
        x + 12.0,
        y + 29.0,
        panel_w - 24.0,
        5.0,
        fraction,
        with_alpha(LOAD_CYAN, 0.55),
        LOAD_CYAN,
        w,
        h,
    );
}

fn build_splash_background(
    out: &mut Vec<UiVertex>,
    w: u32,
    h: u32,
    splash_size: Option<[u32; 2]>,
) {
    rect(out, 0.0, 0.0, w as f32, h as f32, [0.0, 0.0, 0.0, 1.0], w, h);

    if let Some([image_w, image_h]) = splash_size.filter(|size| size[0] > 0 && size[1] > 0) {
        // Cover the window without distorting the original splash artwork.
        let scale = (w as f32 / image_w as f32).max(h as f32 / image_h as f32);
        let draw_w = image_w as f32 * scale;
        let draw_h = image_h as f32 * scale;
        let x = (w as f32 - draw_w) * 0.5;
        let y = (h as f32 - draw_h) * 0.5;
        textured_rect_with_source(
            out,
            x,
            y,
            draw_w,
            draw_h,
            [0.0, 0.0],
            [1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            3.0,
            w,
            h,
        );
    }
}

fn build_loading_screen(
    out: &mut Vec<UiVertex>,
    loading: &MapLoadingUi,
    w: u32,
    h: u32,
    splash_size: Option<[u32; 2]>,
) {
    build_splash_background(out, w, h, splash_size);

    // Darken the lower half so the panel reads over any splash artwork.
    let fade_h = h as f32 * 0.55;
    let clear = [0.0, 0.0, 0.02, 0.0];
    let dark = [0.0, 0.0, 0.02, 0.80];
    rect_gradient(out, 0.0, h as f32 - fade_h, w as f32, fade_h, clear, clear, dark, dark, w, h);

    // Preparation finishing is not the end of the load: the renderer still has
    // to upload the world and present its first frame. That runs on the render
    // thread with no measurable progress, so show it as a final row that stays
    // WORKING (and keeps the total below 100%) until the loading screen closes.
    let upload_bar = MapLoadingBar {
        label: "GPU UPLOAD",
        completed: 0,
        total: 1,
        skipped: false,
    };
    let mut visible: Vec<&MapLoadingBar> = loading
        .bars
        .iter()
        .filter(|bar| !(bar.skipped || (loading.preparation_finished && bar.total == 0)))
        .collect();
    if loading.preparation_finished {
        visible.push(&upload_bar);
    }
    let overall = if visible.is_empty() {
        0.0
    } else {
        visible.iter().map(|bar| loading_fraction(bar)).sum::<f32>() / visible.len() as f32
    };

    let panel_w = (w as f32 - 48.0).clamp(360.0, 760.0);
    let row_h = 27.0;
    let rows_y = 82.0;
    let panel_h = rows_y + visible.len() as f32 * row_h + 20.0;
    let x = (w as f32 - panel_w) * 0.5;
    let y = (h as f32 - panel_h - 34.0).max(24.0);
    rect_gradient(
        out,
        x,
        y,
        panel_w,
        panel_h,
        [0.012, 0.016, 0.038, 0.92],
        [0.012, 0.016, 0.038, 0.92],
        [0.022, 0.028, 0.064, 0.94],
        [0.022, 0.028, 0.064, 0.94],
        w,
        h,
    );
    neon_edge(out, x, y, panel_w, 2.0, -1.0, w, h);

    // Header: kicker, map name, overall percentage.
    text(out, "LOADING", x + 22.0, y + 15.0, 1.1, LOAD_CYAN, w, h);
    text(
        out,
        &loading.map_name.to_ascii_uppercase(),
        x + 22.0,
        y + 29.0,
        2.2,
        [0.95, 0.97, 1.0, 1.0],
        w,
        h,
    );
    let percent = format!("{}%", (overall * 100.0).round() as u32);
    let percent_w = percent.len() as f32 * 6.0 * 2.2;
    text(out, &percent, x + panel_w - 22.0 - percent_w, y + 29.0, 2.2, LOAD_AMBER, w, h);
    progress_track(
        out,
        x + 22.0,
        y + 58.0,
        panel_w - 44.0,
        6.0,
        overall,
        LOAD_CYAN,
        LOAD_MAGENTA,
        w,
        h,
    );

    let bar_x = x + 174.0;
    let bar_w = panel_w - 270.0;
    for (index, bar) in visible.iter().enumerate() {
        let row_y = y + rows_y + index as f32 * row_h;
        let done = bar.total > 0 && bar.completed >= bar.total;
        let fraction = loading_fraction(bar);
        text(
            out,
            bar.label,
            x + 22.0,
            row_y + 4.0,
            1.25,
            if bar.total == 0 { LOAD_FAINT } else { LOAD_LABEL },
            w,
            h,
        );
        let (from, to) = if done {
            (with_alpha(LOAD_DONE, 0.6), LOAD_DONE)
        } else {
            (with_alpha(LOAD_CYAN, 0.5), LOAD_CYAN)
        };
        progress_track(out, bar_x, row_y + 6.0, bar_w, 6.0, fraction, from, to, w, h);
        let status = if bar.total == 0 {
            "WAIT".to_string()
        } else if done {
            "DONE".to_string()
        } else if bar.total == 1 {
            // A single inline step has no meaningful "0/1".
            "WORKING".to_string()
        } else {
            format!("{}/{}", bar.completed, bar.total)
        };
        text(
            out,
            &status,
            x + panel_w - 76.0,
            row_y + 4.0,
            1.2,
            if done {
                LOAD_DONE
            } else if bar.total == 0 {
                LOAD_FAINT
            } else {
                LOAD_LABEL
            },
            w,
            h,
        );
    }
}

/// `x, y, width` of the trace inspector panel before any HUD layout is applied.
/// Unused since the trace panel moved to the `OverlayMode::Trace` egui menu;
/// kept (not deleted) alongside `build_surface_inspector` in case a non-egui
/// HUD summary is wanted again later.
#[allow(dead_code)]
fn surface_inspector_frame(w: u32) -> (f32, f32, f32) {
    let panel_w = (w as f32 * 0.46).clamp(620.0, 900.0);
    ((w as f32 - panel_w - 24.0).max(24.0), 72.0, panel_w)
}

#[allow(dead_code)]
fn build_surface_inspector(
    out: &mut Vec<UiVertex>,
    info: &crate::runtime::SurfaceInspectorInfo,
    w: u32,
    h: u32,
) {
    fn shortened(value: &str, max_chars: usize) -> String {
        if value.chars().count() <= max_chars {
            return value.to_owned();
        }
        let keep = max_chars.saturating_sub(1);
        let mut text = value.chars().take(keep).collect::<String>();
        text.push('…');
        text
    }

    let (x, y, panel_w) = surface_inspector_frame(w);
    let summary_rows = info.summary.len().min(8);
    let header_h = 104.0;
    let summary_h = if summary_rows == 0 { 0.0 } else { 14.0 + summary_rows as f32 * 30.0 + 10.0 };
    let available_detail_h = (h as f32 - y - header_h - summary_h - 34.0).max(0.0);
    let mut detail_rows = 0usize;
    for section in &info.sections {
        if detail_rows >= 14 {
            break;
        }
        detail_rows += 1; // section heading
        detail_rows += section.lines.len().min(14usize.saturating_sub(detail_rows));
    }
    if detail_rows == 0 {
        detail_rows = info.lines.len().min(8);
    }
    let detail_row_h = 22.0;
    let visible_detail_rows = detail_rows.min((available_detail_h / detail_row_h).floor().max(0.0) as usize);
    let detail_h = visible_detail_rows as f32 * detail_row_h + if visible_detail_rows > 0 { 14.0 } else { 0.0 };
    let panel_h = (header_h + summary_h + detail_h + 16.0).min(h as f32 - y - 18.0);

    rect(out, x, y, panel_w, panel_h, [0.012, 0.020, 0.032, 0.975], w, h);
    rect(out, x, y, 5.0, panel_h, [0.96, 0.72, 0.18, 1.0], w, h);

    text(
        out,
        "TRACE INSPECTOR",
        x + 22.0,
        y + 17.0,
        2.05,
        [0.99, 0.94, 0.82, 1.0],
        w,
        h,
    );
    text(
        out,
        "CTRL+C  COPY FULL DIAGNOSTICS",
        x + panel_w - 250.0,
        y + 20.0,
        1.05,
        [0.57, 0.65, 0.74, 1.0],
        w,
        h,
    );

    // Keep identity on its own row. The old overlay packed every diagnostic
    // into equally weighted text; this makes "what am I looking at?" readable
    // before the eye has to parse any technical detail.
    let kind = shortened(&info.kind, 26);
    let kind_w = (kind.chars().count() as f32 * 7.2 + 22.0).clamp(92.0, 220.0);
    let kind_x = x + 22.0;
    rect(out, kind_x, y + 54.0, kind_w, 27.0, [0.06, 0.13, 0.19, 0.96], w, h);
    text(out, &kind, kind_x + 10.0, y + 61.0, 1.15, [0.45, 0.86, 1.0, 1.0], w, h);
    let title_x = kind_x + kind_w + 14.0;
    let title_chars = ((x + panel_w - 22.0 - title_x) / 11.5).floor().max(18.0) as usize;
    let title = shortened(&info.title, title_chars);
    text(
        out,
        &title,
        title_x,
        y + 58.0,
        1.75,
        [0.76, 0.91, 1.0, 1.0],
        w,
        h,
    );

    let mut cursor_y = y + header_h;
    if summary_rows > 0 {
        rect(
            out,
            x + 16.0,
            cursor_y - 6.0,
            panel_w - 32.0,
            summary_h - 2.0,
            [0.020, 0.036, 0.052, 0.94],
            w,
            h,
        );
        for (label, value) in info.summary.iter().take(summary_rows) {
            let value = shortened(value, ((panel_w - 188.0) / 8.5).floor().max(24.0) as usize);
            text(
                out,
                label,
                x + 30.0,
                cursor_y + 7.0,
                1.12,
                [0.55, 0.66, 0.77, 1.0],
                w,
                h,
            );
            text(
                out,
                &value,
                x + 164.0,
                cursor_y + 5.0,
                1.34,
                [0.96, 0.98, 1.0, 1.0],
                w,
                h,
            );
            cursor_y += 30.0;
        }
        cursor_y += 18.0;
    }

    let max_chars = ((panel_w - 60.0) / 7.5).floor().max(32.0) as usize;
    let mut rows_left = visible_detail_rows;
    if rows_left > 0 {
        for section in &info.sections {
            if rows_left == 0 {
                break;
            }
            text(
                out,
                &section.title,
                x + 24.0,
                cursor_y + 1.0,
                1.10,
                [0.98, 0.72, 0.24, 1.0],
                w,
                h,
            );
            cursor_y += detail_row_h;
            rows_left -= 1;
            for line in &section.lines {
                if rows_left == 0 {
                    break;
                }
                let line = shortened(line, max_chars);
                text(
                    out,
                    &line,
                    x + 34.0,
                    cursor_y + 1.0,
                    1.16,
                    [0.86, 0.90, 0.94, 1.0],
                    w,
                    h,
                );
                cursor_y += detail_row_h;
                rows_left -= 1;
            }
            cursor_y += 3.0;
        }
        if info.sections.is_empty() {
            for line in info.lines.iter().take(rows_left) {
                let line = shortened(line, max_chars);
                text(
                    out,
                    &line,
                    x + 30.0,
                    cursor_y + 1.0,
                    1.16,
                    [0.86, 0.90, 0.94, 1.0],
                    w,
                    h,
                );
                cursor_y += detail_row_h;
            }
        }
    }
}

// Night-Tokyo palette shared with the console: indigo ink, cyan/magenta neon,
// tungsten amber. HUD accents pick one role per panel.
const HUD_INK_TOP: [f32; 4] = [0.010, 0.014, 0.032, 0.84];
const HUD_INK_BOTTOM: [f32; 4] = [0.020, 0.026, 0.058, 0.80];
const HUD_LABEL: [f32; 4] = [0.56, 0.66, 0.79, 1.0];
const HUD_VALUE: [f32; 4] = [0.96, 0.98, 1.0, 1.0];
const HUD_HEALTH: [f32; 4] = [0.92, 0.18, 0.15, 1.0];
const HUD_HEALTH_LOW: [f32; 4] = [1.0, 0.07, 0.05, 1.0];
const HUD_SHIELD: [f32; 4] = [0.26, 0.86, 0.36, 1.0];
const HUD_FORCE: [f32; 4] = [0.22, 0.50, 1.0, 1.0];
const HUD_AMMO: [f32; 4] = [1.0, 0.72, 0.28, 1.0];

fn lighten(color: [f32; 4], amount: f32) -> [f32; 4] {
    [
        color[0] + (1.0 - color[0]) * amount,
        color[1] + (1.0 - color[1]) * amount,
        color[2] + (1.0 - color[2]) * amount,
        color[3],
    ]
}

/// OpenJK `saber_styles_t` as shown on the HUD: name, accent, lit segments (of 3).
fn saber_style_display(style: i32) -> (&'static str, [f32; 4], u32) {
    match style {
        1 => ("FAST", [0.35, 0.72, 1.0, 1.0], 1),
        2 => ("MEDIUM", [1.0, 0.85, 0.30, 1.0], 2),
        3 => ("STRONG", [1.0, 0.28, 0.36, 1.0], 3),
        4 => ("DESANN", [1.0, 0.30, 0.36, 1.0], 3),
        5 => ("TAVION", [0.75, 0.45, 1.0, 1.0], 3),
        6 => ("DUAL", [0.40, 1.0, 0.65, 1.0], 3),
        7 => ("STAFF", [1.0, 0.58, 0.25, 1.0], 3),
        _ => ("--", HUD_LABEL, 0),
    }
}

fn build_hud(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else {
        return;
    };

    let health_layout = ui.hud_layout.health;
    let ctx = HudRectContext::from_snapshot(ui);
    let health = hud_element_rect(HudElementId::Health, health_layout, &ctx, w, h);
    let low_health = hud.health > 0 && hud.health * 4 <= hud.max_health.max(1);
    hud_meter(
        out,
        health,
        health_layout.scale,
        "HEALTH",
        hud.health,
        hud.max_health.max(1),
        if low_health { HUD_HEALTH_LOW } else { HUD_HEALTH },
        w,
        h,
    );

    let shield_layout = ui.hud_layout.shield;
    let shield = hud_element_rect(HudElementId::Shield, shield_layout, &ctx, w, h);
    hud_meter(
        out,
        shield,
        shield_layout.scale,
        "SHIELD",
        hud.armor,
        hud.max_health.max(1),
        HUD_SHIELD,
        w,
        h,
    );

    // The ammo slot doubles as the saber-style readout while the saber is out.
    let ammo_layout = ui.hud_layout.ammo;
    let ammo = hud_element_rect(HudElementId::Ammo, ammo_layout, &ctx, w, h);
    if hud.weapon == HUD_WP_SABER {
        hud_style_panel(out, ammo, ammo_layout.scale, hud.saber_style, w, h);
    } else {
        let ammo_text = hud
            .ammo
            .map_or_else(|| "--".to_owned(), |value| value.max(0).to_string());
        hud_value_panel(out, ammo, ammo_layout.scale, "AMMO", &ammo_text, HUD_AMMO, w, h);
    }

    let force_layout = ui.hud_layout.force;
    let force = hud_element_rect(HudElementId::Force, force_layout, &ctx, w, h);
    hud_meter(
        out,
        force,
        force_layout.scale,
        "FORCE",
        hud.force_power,
        hud.force_power_max.max(1),
        if hud.force_flash { [1.0, 0.15, 0.15, 1.0] } else { HUD_FORCE },
        w,
        h,
    );
}

/// `WP_SABER` in OpenJK's `weapon_t`.
const HUD_WP_SABER: i32 = 3;

/// Panel backing shared by every HUD readout: ink gradient, an accent notch on
/// the left edge and a faint accent hairline along the top.
fn hud_frame(out: &mut Vec<UiVertex>, rect_: HudRect, scale: f32, accent: [f32; 4], w: u32, h: u32) {
    let s = scale.clamp(0.5, 2.0);
    rect_gradient(
        out,
        rect_.x,
        rect_.y,
        rect_.width,
        rect_.height,
        HUD_INK_TOP,
        HUD_INK_TOP,
        HUD_INK_BOTTOM,
        HUD_INK_BOTTOM,
        w,
        h,
    );
    rect(out, rect_.x, rect_.y, rect_.width, 1.0, with_alpha(accent, 0.22), w, h);
    rect(out, rect_.x, rect_.y, 3.0 * s, rect_.height, with_alpha(accent, 0.95), w, h);
}

/// Label top-left and value top-right, in the shared HUD type sizes.
#[allow(clippy::too_many_arguments)]
fn hud_texts(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: &str,
    value_color: [f32; 4],
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    text(out, label, rect_.x + 10.0 * s, rect_.y + 5.5 * s, 1.15 * s, HUD_LABEL, w, h);
    let value_scale = 1.6 * s;
    let value_width = value.len() as f32 * 6.0 * value_scale;
    text(
        out,
        value,
        rect_.x + rect_.width - value_width - 8.0 * s,
        rect_.y + 4.0 * s,
        value_scale,
        value_color,
        w,
        h,
    );
}

#[allow(clippy::too_many_arguments)]
fn hud_meter(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: i32,
    maximum: i32,
    accent: [f32; 4],
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, accent, w, h);
    let fraction = (value.max(0) as f32 / maximum.max(1) as f32).clamp(0.0, 1.0);
    let track_x = rect_.x + 10.0 * s;
    let track_w = rect_.width - 18.0 * s;
    let track_y = rect_.y + rect_.height - 6.0 * s;
    let track_h = 3.0 * s;
    rect(out, track_x, track_y, track_w, track_h, with_alpha(accent, 0.16), w, h);
    let fill_w = track_w * fraction;
    if fill_w > 0.5 {
        let dim = with_alpha(accent, 0.55);
        rect_gradient(out, track_x, track_y, fill_w, track_h, dim, accent, dim, accent, w, h);
        // Bright leading edge.
        let edge = (2.0 * s).min(fill_w);
        rect(
            out,
            track_x + fill_w - edge,
            track_y,
            edge,
            track_h,
            lighten(accent, 0.55),
            w,
            h,
        );
    }
    // Quarter ticks read as a scale without adding text.
    for quarter in 1..4 {
        rect(
            out,
            track_x + track_w * quarter as f32 / 4.0 - 0.5,
            track_y,
            1.0_f32.max(0.6 * s),
            track_h,
            [0.0, 0.0, 0.02, 0.6],
            w,
            h,
        );
    }
    let value_color = if accent == HUD_HEALTH_LOW { [1.0, 0.64, 0.60, 1.0] } else { HUD_VALUE };
    hud_texts(out, rect_, scale, label, &value.max(0).to_string(), value_color, w, h);
}

#[allow(clippy::too_many_arguments)]
fn hud_value_panel(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: &str,
    accent: [f32; 4],
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, accent, w, h);
    rect(
        out,
        rect_.x + 10.0 * s,
        rect_.y + rect_.height - 6.0 * s,
        rect_.width - 18.0 * s,
        3.0 * s,
        with_alpha(accent, 0.34),
        w,
        h,
    );
    hud_texts(out, rect_, scale, label, value, HUD_VALUE, w, h);
}

/// Saber form readout: style name plus three segments that light up with the
/// form's intensity (fast, medium, strong; the special forms fill all three).
fn hud_style_panel(out: &mut Vec<UiVertex>, rect_: HudRect, scale: f32, style: i32, w: u32, h: u32) {
    let s = scale.clamp(0.5, 2.0);
    let (name, accent, lit) = saber_style_display(style);
    hud_frame(out, rect_, scale, accent, w, h);
    let track_x = rect_.x + 10.0 * s;
    let track_w = rect_.width - 18.0 * s;
    let track_y = rect_.y + rect_.height - 6.0 * s;
    let track_h = 3.0 * s;
    let gap = 3.0 * s;
    let segment_w = (track_w - 2.0 * gap) / 3.0;
    for index in 0..3u32 {
        let segment_x = track_x + index as f32 * (segment_w + gap);
        if index < lit {
            let dim = with_alpha(accent, 0.6);
            rect_gradient(out, segment_x, track_y, segment_w, track_h, dim, accent, dim, accent, w, h);
        } else {
            rect(out, segment_x, track_y, segment_w, track_h, with_alpha(accent, 0.16), w, h);
        }
    }
    hud_texts(out, rect_, scale, "STYLE", name, lighten(accent, 0.25), w, h);
}

const CHATBOX_Y: f32 = 425.0;
const CHATBOX_FONT_HEIGHT: f32 = 20.0;
const CHATBOX_FONT_SCALE: f32 = 0.65 * 0.5;
const CHATBOX_CUTOFF: f32 = 550.0;

fn build_chat_history(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    if ui.chat_lines.is_empty() {
        return;
    }

    let Some(font) = small_font else {
        // Asset fallback only. Stock JKA uses proportional FONT_SMALL / ocr_a
        // here; charsgrid is retained solely so chat remains visible if a mod
        // removes the retail font files.
        // Uncolored chat starts with the speaker name. Keep that white;
        // inline ^2/^5/etc codes from JKA/TaystJK then color the message body.
        const CHAT_BASE_WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        let bottom = h as f32 * (CHATBOX_Y / 480.0);
        let line_step = h as f32 * (CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE / 480.0);
        let count = ui.chat_lines.len().min(8);
        let first_y = bottom - count.saturating_sub(1) as f32 * line_step;
        let start = ui.chat_lines.len() - count;
        for (index, line) in ui.chat_lines[start..].iter().enumerate() {
            let mut color = CHAT_BASE_WHITE;
            color[3] = line.alpha.clamp(0.0, 1.0);
            fixed_charset_text(
                out,
                &line.text,
                w as f32 * (30.0 / 640.0),
                first_y + index as f32 * line_step,
                w as f32 * (4.0 / 640.0),
                h as f32 * (8.0 / 480.0),
                w as f32 * (4.5 / 640.0),
                color,
                true,
                w,
                h,
            );
        }
        return;
    };

    // OpenJK CG_ChatBox_DrawStrings uses FONT_SMALL at 0.65 scale. Match a
    // TaystJK-style cg_chatBoxFontSize 0.5 and cg_chatBoxHeight 425 here.
    // JKA's FONT_SMALL default is fonts/ocr_a.{fontdat,tga}.
    const CHATBOX_X: f32 = 30.0;
    // The server chat string carries message colors inline. The initial
    // uncolored run is the player name, so its base color must be white.
    const CHAT_BASE_WHITE: [f32; 3] = [1.0, 1.0, 1.0];

    let mut wrapped = Vec::new();
    for line in &ui.chat_lines {
        let alpha = line.alpha.clamp(0.0, 1.0);
        if alpha <= 0.01 {
            continue;
        }
        let text = wrap_proportional_text(&line.text, font, CHATBOX_FONT_SCALE, CHATBOX_CUTOFF);
        let lines = text.bytes().filter(|&byte| byte == b'\n').count() + 1;
        wrapped.push((text, lines, alpha));
    }
    if wrapped.is_empty() {
        return;
    }

    let total_lines: usize = wrapped.iter().map(|(_, lines, _)| *lines).sum();
    let line_step_virtual = CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE;
    let x = CHATBOX_X * w as f32 / 640.0;
    let mut baseline_y = (CHATBOX_Y - line_step_virtual * total_lines as f32) * h as f32 / 480.0;

    for (value, lines, alpha) in wrapped {
        proportional_text(
            out,
            &value,
            font,
            x,
            baseline_y,
            CHATBOX_FONT_SCALE,
            [
                CHAT_BASE_WHITE[0],
                CHAT_BASE_WHITE[1],
                CHAT_BASE_WHITE[2],
                alpha,
            ],
            true,
            w,
            h,
        );
        baseline_y += line_step_virtual * lines as f32 * h as f32 / 480.0;
    }
}

fn build_chat_input(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    // OpenJK draws the message-entry field with SCR_DrawBigString /
    // Field_BigDraw: 16x16 geometry sampling the same charsgrid_med atlas.
    const BIG_CHAR: f32 = 16.0;
    const CHAT_WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const INPUT_HEIGHT: f32 = 40.0;
    const INPUT_GAP: f32 = 6.0;
    let width = (w as f32 * 0.62).min(840.0).max(360.0);
    let x = 18.0;

    // Anchor the entry box above the oldest visible chat line instead of to a
    // fixed number of pixels from the bottom.  cg_chatBoxHeight is in 640x480
    // virtual coordinates, while this field is drawn in framebuffer pixels.
    let y_scale = h as f32 / 480.0;
    let history_top = if let Some(font) = small_font {
        let total_lines: usize = ui
            .chat_lines
            .iter()
            .filter(|line| line.alpha > 0.01)
            .map(|line| {
                let wrapped =
                    wrap_proportional_text(&line.text, font, CHATBOX_FONT_SCALE, CHATBOX_CUTOFF);
                wrapped.bytes().filter(|&byte| byte == b'\n').count() + 1
            })
            .sum();
        let line_step = CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE;
        (CHATBOX_Y - line_step * (total_lines.max(1) as f32 + 1.0)) * y_scale
    } else {
        let count = ui.chat_lines.len().min(8).max(1);
        let line_step = 6.5 * y_scale;
        CHATBOX_Y * y_scale - count as f32 * line_step
    };
    let y = (history_top - INPUT_GAP - INPUT_HEIGHT).max(0.0);
    rect(out, x, y, width, 40.0, [0.01, 0.018, 0.028, 0.92], w, h);
    rect(out, x, y + 38.0, width, 2.0, [0.25, 0.78, 0.30, 0.95], w, h);
    let prompt = match ui.chat_mode {
        ChatMode::Global => "SAY:",
        ChatMode::Team => "TEAM:",
    };
    fixed_charset_text(
        out,
        prompt,
        x + 8.0,
        y + 11.0,
        BIG_CHAR,
        BIG_CHAR,
        BIG_CHAR,
        CHAT_WHITE,
        true,
        w,
        h,
    );

    let input_x = x + 8.0 + (prompt.chars().count() as f32 + 1.0) * BIG_CHAR;
    fixed_charset_text(
        out,
        &format!("{} _", ui.chat_input),
        input_x,
        y + 11.0,
        BIG_CHAR,
        BIG_CHAR,
        BIG_CHAR,
        CHAT_WHITE,
        true,
        w,
        h,
    );
}


/// `gfx/hud/keys/*` images `DF_DrawMovementKeys` draws, in atlas order (see
/// `renderer::load_ui_key_atlas`). `KeyArt` indexes this table.
pub const MOVEMENT_KEY_ART: [&str; 27] = [
    "crouch_off", "crouch_on", "jump_off", "jump_on", "back_off", "back_on",
    "forward_off", "forward_on", "left_off", "left_on", "right_off", "right_on",
    "attack_off", "attack_on", "alt_off", "alt_on", "walk_off", "walk_on",
    "crouch_on2", "jump_on2", "back_on2", "forward_on2", "left_on2", "right_on2",
    "attack_on2", "alt_on2", "walk_on2",
];
pub const KEY_ART_SIZE: u32 = 128;
pub const KEY_ATLAS_COLUMNS: u32 = 6;
const KEY_ATLAS_ROWS: u32 = (MOVEMENT_KEY_ART.len() as u32).div_ceil(KEY_ATLAS_COLUMNS);
/// Texture source id of the key atlas in `ui.wgsl`.
const KEY_TEXTURE_SOURCE: f32 = 4.0;

#[derive(Clone, Copy)]
#[repr(usize)]
enum KeyArt {
    CrouchOff, CrouchOn, JumpOff, JumpOn, BackOff, BackOn,
    ForwardOff, ForwardOn, LeftOff, LeftOn, RightOff, RightOn,
    AttackOff, AttackOn, AltOff, AltOn, WalkOff, WalkOn,
    CrouchOn2, JumpOn2, BackOn2, ForwardOn2, LeftOn2, RightOn2,
    AttackOn2, AltOn2, WalkOn2,
}

/// `CG_DrawPic` for one key image: the whole 128x128 image, white, alpha blended.
fn movement_key_pic(
    out: &mut Vec<UiVertex>, art: KeyArt, x: f32, y: f32, w: f32, h: f32, width: u32, height: u32,
) {
    let index = art as u32;
    let (column, row) = (index % KEY_ATLAS_COLUMNS, index / KEY_ATLAS_COLUMNS);
    let (atlas_w, atlas_h) = ((KEY_ATLAS_COLUMNS * KEY_ART_SIZE) as f32, (KEY_ATLAS_ROWS * KEY_ART_SIZE) as f32);
    // Half-texel inset keeps linear filtering from pulling in the neighbouring
    // atlas cell; it stands in for the clamp-to-edge of a standalone image.
    let u0 = ((column * KEY_ART_SIZE) as f32 + 0.5) / atlas_w;
    let u1 = (((column + 1) * KEY_ART_SIZE) as f32 - 0.5) / atlas_w;
    let v0 = ((row * KEY_ART_SIZE) as f32 + 0.5) / atlas_h;
    let v1 = (((row + 1) * KEY_ART_SIZE) as f32 - 0.5) / atlas_h;
    textured_rect_with_source(
        out, x, y, w, h, [u0, v0], [u1, v1], [1.0; 4], KEY_TEXTURE_SOURCE, width, height,
    );
}

/// Port of TaystJK `DF_DrawMovementKeys`. Layout is in cgame's 640x480 space with
/// `cl_ratioFix` on (the default): image sizes come out square, x offsets scale
/// with the window width and y offsets with its height.
/// Tile edge in pixels (`w * widthRatioCoef` and `h`, scaled to the window) and
/// the top-left corner of the 3-wide key block; `None` while the keys are off.
fn movement_keys_origin(settings: &MovementKeysSettings, w: u32, h: u32) -> Option<(f32, f32, f32)> {
    if settings.mode == 0 || w == 0 || h == 0 { return None; }
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let size = settings.size;
    let walk = settings.walk;
    Some(match settings.mode {
        1 => {
            let tile = 16.0 * size * sy;
            (tile,
             320.0 * sx + settings.x * sx - tile * if walk { 1.0 } else { 1.5 },
             480.0 * 0.9 * sy + settings.y * sy - tile)
        }
        2 => {
            let tile = 16.0 * size * sy;
            (tile,
             320.0 * sx + settings.x * sx - tile * if walk { 1.5 } else { 2.0 },
             480.0 * 0.9 * sy + settings.y * sy - tile)
        }
        3 => {
            // TaystJK ignores cg_movementKeysX/Y in this mode.
            let tile = 6.0 * size * sy;
            (tile, 320.0 * sx - tile * 1.5, 240.0 * sy - tile * 1.5)
        }
        4 => {
            let tile = 12.0 * size * sy;
            (tile,
             320.0 * sx + settings.x * sx - tile * 1.5,
             480.0 * 0.9 * sy + settings.y * sy - tile * 1.5)
        }
        _ => return None,
    })
}

fn build_movement_keys(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let settings = ui.movement_keys;
    let Some((tile, x, y)) = movement_keys_origin(&settings, w, h) else { return };
    let state = ui.movement_hud;
    let walk = settings.walk;
    let (tw, th) = (tile, tile);

    let forward = state.forward_move;
    let right = state.right_move;
    let up = state.up_move;
    let attack = state.buttons & jka_movement::BUTTON_ATTACK != 0;
    let alt = state.buttons & jka_movement::BUTTON_ALT_ATTACK != 0;
    let walking = state.buttons & jka_movement::BUTTON_WALKING != 0;
    let mut pic = |art: KeyArt, col: f32, row: f32| {
        movement_key_pic(out, art, x + col * tw, y + row * th, tw, th, w, h);
    };

    if settings.mode >= 3 {
        // Compact style: only pressed keys are drawn, each with its "2" art.
        if up < 0 { pic(KeyArt::CrouchOn2, 2.0, 0.0); }
        if up > 0 { pic(KeyArt::JumpOn2, 0.0, 0.0); }
        if forward < 0 { pic(KeyArt::BackOn2, 1.0, 2.0); }
        if forward > 0 { pic(KeyArt::ForwardOn2, 1.0, 0.0); }
        if right < 0 { pic(KeyArt::LeftOn2, 0.0, 1.0); }
        if right > 0 { pic(KeyArt::RightOn2, 2.0, 1.0); }
        if attack { pic(KeyArt::AttackOn2, 0.0, 2.0); }
        if alt { pic(KeyArt::AltOn2, 2.0, 2.0); }
        if walk && walking { pic(KeyArt::WalkOn2, -1.0, 2.0); }
    } else {
        // Original style: every key is drawn, in its on or off art.
        pic(if up < 0 { KeyArt::CrouchOn } else { KeyArt::CrouchOff }, 2.0, 0.0);
        pic(if up > 0 { KeyArt::JumpOn } else { KeyArt::JumpOff }, 0.0, 0.0);
        pic(if forward < 0 { KeyArt::BackOn } else { KeyArt::BackOff }, 1.0, 1.0);
        pic(if forward > 0 { KeyArt::ForwardOn } else { KeyArt::ForwardOff }, 1.0, 0.0);
        pic(if right < 0 { KeyArt::LeftOn } else { KeyArt::LeftOff }, 0.0, 1.0);
        pic(if right > 0 { KeyArt::RightOn } else { KeyArt::RightOff }, 2.0, 1.0);
        if settings.mode == 2 {
            pic(if attack { KeyArt::AttackOn } else { KeyArt::AttackOff }, 3.0, 0.0);
            pic(if alt { KeyArt::AltOn } else { KeyArt::AltOff }, 3.0, 1.0);
        }
        if walk {
            pic(if walking { KeyArt::WalkOn } else { KeyArt::WalkOff }, -1.0, 1.0);
        }
    }
}
fn build_strafe_helper(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let sh = ui.strafe_helper;
    if sh.flags & SHELPER_STYLE_MASK == 0 { return; }
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    let segments = crate::strafehelper::strafe_lines(&sh, &ui.movement_hud, ui.video.fps_cap, aspect);
    // cg_draw's 640x480 space maps straight onto the window on both axes.
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    for segment in segments {
        let [x0, y0] = [segment.from[0] * sx, segment.from[1] * sy];
        let [x1, y1] = [segment.to[0] * sx, segment.to[1] * sy];
        // DF_DrawLine stamps size x size squares (scaled per axis) along the
        // segment; the swept shape is as thick as the square's extent across it.
        let length = (x1 - x0).hypot(y1 - y0);
        if length <= f32::EPSILON { continue; }
        let (nx, ny) = (-(y1 - y0) / length, (x1 - x0) / length);
        let thickness = segment.size * (sx * nx.abs() + sy * ny.abs());
        hud_line(out, x0, y0, x1, y1, thickness, segment.color, w, h);
    }
}

#[allow(clippy::too_many_arguments)]
fn hud_line(out: &mut Vec<UiVertex>, x0: f32, y0: f32, x1: f32, y1: f32, width_px: f32,
            color: [f32; 4], width: u32, height: u32) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len <= f32::EPSILON { return; }
    let half = width_px.max(0.25) * 0.5;
    let nx = -dy / len * half;
    let ny = dx / len * half;
    let to_ndc = |x: f32, y: f32| [x / width.max(1) as f32 * 2.0 - 1.0, 1.0 - y / height.max(1) as f32 * 2.0];
    let make = |x: f32, y: f32| UiVertex { position: to_ndc(x, y), uv: [0.0, 0.0], color, textured: 0.0 };
    let a = make(x0 + nx, y0 + ny);
    let b = make(x0 - nx, y0 - ny);
    let c = make(x1 - nx, y1 - ny);
    let d = make(x1 + nx, y1 + ny);
    out.extend_from_slice(&[a, b, c, a, c, d]);
}

/// `CG_DrawCrosshairNames`: the aimed-at player's name, centred at y = 170.
fn build_crosshair_name(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(name) = &ui.crosshair_target.name else { return };
    if ui.crosshair.style == 0 || name.alpha <= 0.001 {
        return;
    }
    let color = [name.color[0], name.color[1], name.color[2], name.alpha.clamp(0.0, 1.0)];
    let y = 170.0 * h as f32 / 480.0;
    if let Some(font) = small_font {
        // The medium-font role on the shared OCR font, as for the centre print.
        let scale = 0.78;
        let x = ((640.0 - proportional_text_width(&name.text, font, scale)) * 0.5) * w as f32 / 640.0;
        proportional_text(out, &name.text, font, x, y, scale, color, true, w, h);
        return;
    }
    let glyph_w = w as f32 * (10.0 / 640.0);
    let glyph_h = h as f32 * (16.0 / 480.0);
    let x = (w as f32 - visible_jka_chars(&name.text) as f32 * glyph_w) * 0.5;
    fixed_charset_text(out, &name.text, x, y, glyph_w, glyph_h, glyph_w, color, true, w, h);
}

fn build_crosshair(
    out: &mut Vec<UiVertex>,
    crosshair: CrosshairSettings,
    target_color: Option<[f32; 3]>,
    line_width: f32,
    w: u32,
    h: u32,
) {
    if crosshair.style == 0 {
        return;
    }

    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    // Stock JKA's default is cg_crosshairSize 24, but the source artwork has
    // transparent padding. Scale procedural geometry by 2/3 so 24 retains the
    // apparent size of DinurdoJK's previous 16px crosshair.
    let size = crosshair.size.clamp(4.0, 96.0) * (2.0 / 3.0);
    let half = size * 0.5;
    let thickness = (size / 8.0).clamp(1.0, 4.0);
    let gap_max = (half - thickness * 0.5).max(0.5);
    let gap = (size / 8.0).clamp(0.5, gap_max);
    let arm = (half - gap).max(1.0);
    // CG_DrawCrosshair sets the identified colour with alpha 1.
    let mut c = match target_color {
        Some([r, g, b]) => [r, g, b, 1.0],
        None => [
            f32::from(crosshair.color[0]) / 255.0,
            f32::from(crosshair.color[1]) / 255.0,
            f32::from(crosshair.color[2]) / 255.0,
            f32::from(crosshair.color[3]) / 255.0,
        ],
    };
    // Strength below 1 fades everything; above 1 only the images change (below).
    let strength = crosshair.strength.clamp(0.0, CROSSHAIR_STRENGTH_MAX);
    c[3] *= strength.min(1.0);

    let hbar = |out: &mut Vec<UiVertex>, x: f32, y: f32, width: f32| {
        rect(out, x, y - thickness * 0.5, width, thickness, c, w, h);
    };
    let vbar = |out: &mut Vec<UiVertex>, x: f32, y: f32, height: f32| {
        rect(out, x - thickness * 0.5, y, thickness, height, c, w, h);
    };

    if (1..=CROSSHAIR_IMAGE_COUNT).contains(&crosshair.image) {
        // The retail artwork carries transparent padding, so the image is drawn
        // `size` pixels square, which matches the apparent size of the shapes above.
        let side = crosshair.size.clamp(4.0, 96.0);
        let (uv0, uv1) = icon_cell_uv(ICON_CROSSHAIR_BASE + usize::from(crosshair.image) - 1);
        // The stock artwork is thin and translucent. Above 100% strength the same
        // image is drawn again, each pass raising the coverage of its faint pixels;
        // up to three extra passes at the maximum, the last one fractional.
        let extra = (strength - 1.0).max(0.0) * 3.0;
        let passes = 1 + extra.ceil() as usize;
        for pass in 0..passes {
            let weight = if pass == 0 { 1.0 } else { (extra - (pass - 1) as f32).min(1.0) };
            textured_rect_with_source(
                out,
                cx - side * 0.5,
                cy - side * 0.5,
                side,
                side,
                uv0,
                uv1,
                [c[0], c[1], c[2], c[3] * weight],
                ICON_TEXTURE_SOURCE,
                w,
                h,
            );
        }
        return;
    }

    if crosshair.style == CROSSHAIR_STYLE_LINE {
        // A vertical line 1.25x the length of the plus (`size` px, resolution
        // independent like the other shapes), cg_strafeHelperLineWidth units thick in
        // a 640x480 space like the strafehelper's lines.
        let sx = w as f32 / 640.0;
        let line_w = line_width.clamp(0.25, 5.0) * sx;
        let line_h = size * 1.25;
        rect(out, cx - line_w * 0.5, cy - line_h * 0.5, line_w, line_h, c, w, h);
        return;
    }

    match crosshair.style {
        // Classic split cross: the shape DinurdoJK used before this setting.
        1 => {
            hbar(out, cx - half, cy, arm);
            hbar(out, cx + gap, cy, arm);
            vbar(out, cx, cy - half, arm);
            vbar(out, cx, cy + gap, arm);
        }
        // Circular dot.
        2 => {
            let dot = (thickness * 1.6).clamp(2.0, 6.0);
            disc(out, cx, cy, dot * 0.5, 16, c, w, h);
        }
        // Solid plus.
        3 => {
            hbar(out, cx - half, cy, size);
            vbar(out, cx, cy - half, size);
        }
        // Split cross plus center dot.
        4 => {
            hbar(out, cx - half, cy, arm);
            hbar(out, cx + gap, cy, arm);
            vbar(out, cx, cy - half, arm);
            vbar(out, cx, cy + gap, arm);
            let dot = thickness.max(2.0);
            disc(out, cx, cy, dot * 0.5, 16, c, w, h);
        }
        // Side brackets.
        5 => {
            let bracket_h = size * 0.7;
            let cap = (size * 0.2).max(thickness);
            vbar(out, cx - half, cy - bracket_h * 0.5, bracket_h);
            vbar(out, cx + half, cy - bracket_h * 0.5, bracket_h);
            hbar(out, cx - half, cy - bracket_h * 0.5, cap);
            hbar(out, cx - half, cy + bracket_h * 0.5, cap);
            hbar(out, cx + half - cap, cy - bracket_h * 0.5, cap);
            hbar(out, cx + half - cap, cy + bracket_h * 0.5, cap);
        }
        // Square outline.
        _ => {
            rect_outline(
                out,
                cx - half,
                cy - half,
                size,
                size,
                thickness,
                c,
                w,
                h,
            );
        }
    }
}

fn build_fps_simple(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let label = format!("{:.0} FPS", ui.perf.fps);
    let scale = 1.55;
    let glyph_w = 8.0 * scale;
    let x = (w as f32 - 12.0 - label.chars().count() as f32 * glyph_w).max(12.0);
    // Keep the simple FPS counter readable like the chat text, but with a
    // tight screen-space shadow instead of a resolution-scaled offset.
    text(
        out,
        &label,
        x + 1.0,
        13.0,
        scale,
        [0.0, 0.0, 0.0, 0.72],
        w,
        h,
    );
    text(out, &label, x, 12.0, scale, [1.0; 4], w, h);
}

fn compact_thread_name(name: &'static str) -> &'static str {
    match name {
        "MAIN" => "MAIN",
        "RENDER" => "RENDER",
        "MAP LOADER" => "MAP",
        "WORKER 0" => "W0",
        "WORKER 1" => "W1",
        "WORKER 2" => "W2",
        "WORKER 3" => "W3",
        "WORKER 4" => "W4",
        "WORKER 5" => "W5",
        "WORKER 6" => "W6",
        "WORKER 7" => "W7",
        "EVENT 0" => "E0",
        "EVENT 1" => "E1",
        "EVENT 2" => "E2",
        "EVENT 3" => "E3",
        "EVENT 4" => "E4",
        "EVENT 5" => "E5",
        "EVENT 6" => "E6",
        "EVENT 7" => "E7",
        "ASSET 0" => "A0",
        "ASSET 1" => "A1",
        _ => name,
    }
}

fn draw_perf_metric(
    out: &mut Vec<UiVertex>,
    label: &str,
    value: &str,
    x: f32,
    y: f32,
    label_scale: f32,
    value_scale: f32,
    label_color: [f32; 4],
    value_color: [f32; 4],
    w: u32,
    h: u32,
) {
    text(out, label, x, y, label_scale, label_color, w, h);
    text(
        out,
        value,
        x,
        y + 11.0 * label_scale + 4.0,
        value_scale,
        value_color,
        w,
        h,
    );
}

const PERF_HEADER_H: f32 = 38.0;
const PERF_CPU_H: f32 = 78.0;
const PERF_CLIENT_H: f32 = 161.0;
const PERF_INPUT_H: f32 = 108.0;
const PERF_THREAD_HEADER_H: f32 = 28.0;
const PERF_THREAD_ROW_H: f32 = 32.0;

/// `x, y, width, height` of the `cg_drawFPS 2` profiler panel.
fn perf_panel_rect(w: u32, gpu_enabled: bool, debug_culling: bool, thread_count: usize) -> (f32, f32, f32, f32) {
    let margin = 14.0;
    let panel_w = 900.0_f32.min((w as f32 - margin * 2.0).max(620.0));
    let x = (w as f32 - panel_w - margin).max(margin);
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let thread_two_columns = thread_count > 11;
    // The original profiler has 11 core/map slots. Keep those together in
    // the left column and put the event-pool and asset-loader slots in the right column.
    // This preserves the old panel height instead of adding eight more rows.
    let thread_rows = if thread_two_columns { 11 } else { thread_count };
    let footer_h = if debug_culling { 42.0 } else { 0.0 };
    let panel_h = 12.0
        + PERF_HEADER_H
        + PERF_CPU_H
        + gpu_h
        + PERF_CLIENT_H
        + PERF_INPUT_H
        + PERF_THREAD_HEADER_H
        + thread_rows as f32 * PERF_THREAD_ROW_H
        + footer_h
        + 18.0;
    (x, margin, panel_w, panel_h)
}

fn build_perf(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let p = ui.perf;
    let debug_culling = ui.video.cull_debug != CullDebugMode::Off;
    let gpu_enabled = p.gpu_ms.is_some();
    let (x, y, panel_w, panel_h) = perf_panel_rect(w, gpu_enabled, debug_culling, ui.threads.len());
    let header_h = PERF_HEADER_H;
    let cpu_h = PERF_CPU_H;
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let client_h = PERF_CLIENT_H;
    let input_h = PERF_INPUT_H;
    let thread_header_h = PERF_THREAD_HEADER_H;
    let thread_row_h = PERF_THREAD_ROW_H;
    let thread_two_columns = ui.threads.len() > 11;
    let thread_rows = if thread_two_columns { 11 } else { ui.threads.len() };
    rect(out, x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.76], w, h);

    let section_color = [0.60, 0.74, 0.88, 1.0];
    let body_color = [0.88, 0.93, 0.98, 1.0];
    let muted_color = [0.64, 0.72, 0.80, 1.0];

    text(
        out,
        &format!("{:.0} FPS", p.fps),
        x + 12.0,
        y + 6.0,
        2.15,
        [1.0; 4],
        w,
        h,
    );
    text(
        out,
        &format!("CPU RENDER  {:.3} MS", p.frame_ms),
        x + 176.0,
        y + 10.0,
        1.42,
        body_color,
        w,
        h,
    );

    let cpu_colors = [
        [0.22, 0.58, 0.90, 0.98], // prep
        [0.20, 0.78, 0.82, 0.98], // acquire
        [0.72, 0.42, 0.95, 0.98], // encode
        [0.95, 0.58, 0.20, 0.98], // submit
        [0.94, 0.78, 0.28, 0.98], // present API
        [0.32, 0.36, 0.42, 0.98], // other
    ];
    let wall = p.frame_ms.max(0.000_001);
    let prep = p.cpu_prepare_ms.max(0.0).min(wall);
    let acquire = p.cpu_acquire_ms.max(0.0).min((wall - prep).max(0.0));
    let encode = p
        .cpu_encode_ms
        .max(0.0)
        .min((wall - prep - acquire).max(0.0));
    let submit = p
        .cpu_submit_ms
        .max(0.0)
        .min((wall - prep - acquire - encode).max(0.0));
    let present = p
        .cpu_present_ms
        .max(0.0)
        .min((wall - prep - acquire - encode - submit).max(0.0));
    let other = (wall - prep - acquire - encode - submit - present).max(0.0);
    let cpu_segments = [prep, acquire, encode, submit, present, other];
    let bar_x = x + 12.0;
    let bar_y = y + header_h + 2.0;
    let bar_w = panel_w - 24.0;
    let bar_h = 19.0;
    let mut cursor = bar_x;
    for (value, color) in cpu_segments.into_iter().zip(cpu_colors) {
        let width = ((value / wall) as f32 * bar_w).max(0.0);
        if width > 0.0 {
            rect(out, cursor, bar_y, width, bar_h, color, w, h);
            cursor += width;
        }
    }
    rect_outline(
        out,
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        1.0,
        [0.78, 0.84, 0.90, 0.90],
        w,
        h,
    );

    let cpu_legend = [
        ("PREP", prep, cpu_colors[0]),
        ("ACQUIRE", acquire, cpu_colors[1]),
        ("ENCODE", encode, cpu_colors[2]),
        ("SUBMIT", submit, cpu_colors[3]),
        ("PRESENT", present, cpu_colors[4]),
        ("OTHER", other, cpu_colors[5]),
    ];
    let cpu_col_w = bar_w / cpu_legend.len() as f32;
    for (i, (label, value, color)) in cpu_legend.into_iter().enumerate() {
        let lx = x + 12.0 + i as f32 * cpu_col_w;
        text(out, label, lx, bar_y + 25.0, 1.05, color, w, h);
        text(
            out,
            &format!("{value:.3} MS"),
            lx,
            bar_y + 42.0,
            1.18,
            body_color,
            w,
            h,
        );
    }

    let mut next_y = y + header_h + cpu_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    if let Some(gpu_frame) = p.gpu_ms {
        text(
            out,
            &format!("GPU FRAME  {:.3} MS", gpu_frame),
            x + 12.0,
            next_y + 2.0,
            1.30,
            body_color,
            w,
            h,
        );
        let gpu_values = [
            p.gpu_depth_ms.unwrap_or(0.0),
            p.gpu_hiz_ms.unwrap_or(0.0),
            p.gpu_cull_ms.unwrap_or(0.0),
            p.gpu_cluster_ms.unwrap_or(0.0),
            p.gpu_world_ms.unwrap_or(0.0),
            p.gpu_post_ms.unwrap_or(0.0),
            p.gpu_ui_ms.unwrap_or(0.0),
        ];
        let gpu_colors = [
            [0.38, 0.66, 0.94, 0.98],
            [0.30, 0.82, 0.82, 0.98],
            [0.42, 0.88, 0.56, 0.98],
            [0.78, 0.82, 0.32, 0.98],
            [0.96, 0.62, 0.24, 0.98],
            [0.76, 0.42, 0.92, 0.98],
            [0.96, 0.46, 0.68, 0.98],
            [0.32, 0.36, 0.42, 0.98],
        ];
        let known: f64 = gpu_values.iter().sum();
        let gpu_other = (gpu_frame - known).max(0.0);
        let gpu_bar_y = next_y + 20.0;
        let mut cursor = bar_x;
        for (value, color) in gpu_values
            .into_iter()
            .chain(std::iter::once(gpu_other))
            .zip(gpu_colors)
        {
            let width = ((value / gpu_frame.max(0.000_001)) as f32 * bar_w).max(0.0);
            if width > 0.0 {
                rect(out, cursor, gpu_bar_y, width, bar_h, color, w, h);
                cursor += width;
            }
        }
        rect_outline(
            out,
            bar_x,
            gpu_bar_y,
            bar_w,
            bar_h,
            1.0,
            [0.78, 0.84, 0.90, 0.90],
            w,
            h,
        );
        let labels = [
            "DEPTH", "HIZ", "CULL", "CLUSTER", "WORLD", "POST", "UI", "OTHER",
        ];
        let values = [
            p.gpu_depth_ms.unwrap_or(0.0),
            p.gpu_hiz_ms.unwrap_or(0.0),
            p.gpu_cull_ms.unwrap_or(0.0),
            p.gpu_cluster_ms.unwrap_or(0.0),
            p.gpu_world_ms.unwrap_or(0.0),
            p.gpu_post_ms.unwrap_or(0.0),
            p.gpu_ui_ms.unwrap_or(0.0),
            gpu_other,
        ];
        let gpu_col_w = bar_w / 4.0;
        for row in 0..2 {
            for col in 0..4 {
                let i = row * 4 + col;
                let lx = x + 12.0 + col as f32 * gpu_col_w;
                let ly = gpu_bar_y + 27.0 + row as f32 * 23.0;
                text(out, labels[i], lx, ly, 0.98, gpu_colors[i], w, h);
                text(
                    out,
                    &format!("{:.3} MS", values[i]),
                    lx + 68.0,
                    ly,
                    1.04,
                    body_color,
                    w,
                    h,
                );
            }
        }
    } else {
        text(
            out,
            "GPU TIMESTAMPS OFF  (RENDERING MENU)",
            x + 12.0,
            next_y + 8.0,
            1.08,
            muted_color,
            w,
            h,
        );
    }

    next_y += gpu_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        &format!("CLIENT / CGAME  {:.3} MS", p.client_total_ms),
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );
    let client_metrics = [
        ("SNAPSHOT", p.client_snapshot_ms),
        ("EVENTS", p.client_events_ms),
        ("ENTITIES", p.client_entity_present_ms),
        ("PLAYERS", p.client_player_present_ms),
        ("FOLLOWED", p.client_followed_player_ms),
        ("G2 POSE", p.ghoul2_pose_ms),
        ("G2 SKIN", p.ghoul2_skin_ms),
        ("AUDIO", p.client_audio_ms),
    ];
    let client_col_w = (panel_w - 24.0) / 4.0;
    for (i, (label, value)) in client_metrics.into_iter().enumerate() {
        let row = i / 4;
        let col = i % 4;
        let lx = x + 12.0 + col as f32 * client_col_w;
        let ly = next_y + 25.0 + row as f32 * 32.0;
        text(out, label, lx, ly, 0.94, muted_color, w, h);
        text(
            out,
            &format!("{value:.3} MS"),
            lx,
            ly + 15.0,
            1.05,
            body_color,
            w,
            h,
        );
    }
    text(
        out,
        &format!(
            "EVENT PREP {:.3} MS / {} JOBS / {} POOL THREADS / {}   AUDIO DECODE {:.3} MS / {} JOBS / {}",
            p.client_event_prepare_ms,
            p.client_event_worker_jobs,
            p.client_event_worker_threads,
            if p.client_event_worker_parallel { "WORKERS" } else { "INLINE" },
            p.client_event_sound_decode_ms,
            p.client_event_sound_decode_jobs,
            if p.client_event_sound_decode_parallel { "WORKERS" } else { "INLINE" },
        ),
        x + 12.0,
        next_y + 91.0,
        0.92,
        muted_color,
        w,
        h,
    );

    text(
        out,
        &format!(
            "G2 {} POSES (MOTION {} / {:.3} MS) / {} BOLTS ({:.3} MS) / {} SURF / {} VERTS   CULL {}/{} LOD [{}/{}/{}/{}]   FX {:.3} + TESS {:.3} MS   DYN REPACK {:.3} MS / {} SURF / {} VERTS / {} INDICES",
            p.ghoul2_pose_evals,
            p.ghoul2_motion_pose_evals,
            p.ghoul2_motion_pose_ms,
            p.ghoul2_bolt_queries,
            p.ghoul2_bolt_ms,
            p.ghoul2_surfaces,
            p.ghoul2_vertices,
            p.ghoul2_frustum_culled,
            p.ghoul2_frustum_tests,
            p.ghoul2_lod_counts[0],
            p.ghoul2_lod_counts[1],
            p.ghoul2_lod_counts[2],
            p.ghoul2_lod_counts[3],
            p.client_fx_ms,
            p.client_fx_tessellate_ms,
            p.dynamic_model_prepare_ms,
            p.dynamic_model_surfaces,
            p.dynamic_model_vertices,
            p.dynamic_model_indices,
        ),
        x + 12.0,
        next_y + 111.0,
        0.92,
        muted_color,
        w,
        h,
    );

    text(
        out,
        &format!(
            "FX DRAWS {} [SPR {} ORIENT {} LINE {} QUAD {} MESH {} CYL {}]   OUT {} SURF / CPU {} SURF {} V {} I / GPU {} BATCH {} INST / GPU FX {} MS",
            p.client_fx_draws,
            p.client_fx_sprites,
            p.client_fx_oriented_quads,
            p.client_fx_lines,
            p.client_fx_quads,
            p.client_fx_meshes,
            p.client_fx_cylinders,
            p.client_fx_render_surfaces,
            p.client_fx_cpu_geom_surfaces,
            p.client_fx_cpu_vertices,
            p.client_fx_cpu_indices,
            p.client_fx_gpu_sprite_batches,
            p.client_fx_gpu_sprite_instances,
            p.gpu_fx_sprites_ms.map_or_else(|| "--".to_owned(), |ms| format!("{ms:.3}")),
        ),
        x + 12.0,
        next_y + 128.0,
        0.86,
        muted_color,
        w,
        h,
    );

    next_y += client_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        if ui.video.input_subframe && ui.video.input_latelatch {
            "INPUT LATENCY (MOUSE)   SUBFRAME + LATE-LATCH"
        } else if ui.video.input_subframe {
            "INPUT LATENCY (MOUSE)   SUBFRAME EVENT-RATE"
        } else {
            "INPUT LATENCY (MOUSE)   CLIENT-TICK"
        },
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );

    if let (Some(event_to_sim), Some(sim_to_render), Some(event_to_present_call), Some(present_call_max)) = (
        p.input_event_to_sim_ms,
        p.input_sim_to_render_ms,
        p.input_event_to_present_call_ms,
        p.input_event_to_present_call_max_ms,
    ) {
        let metric_y = next_y + 22.0;
        let col_w = (panel_w - 24.0) / 3.0;
        draw_perf_metric(
            out,
            "EVENT>SIM",
            &format!("{event_to_sim:.3} MS"),
            x + 12.0,
            metric_y,
            1.00,
            1.38,
            [0.44, 0.84, 0.98, 1.0],
            body_color,
            w,
            h,
        );
        draw_perf_metric(
            out,
            if ui.video.input_latelatch {
                "SIM>LATCH"
            } else {
                "SIM>RENDER"
            },
            &format!("{sim_to_render:.3} MS"),
            x + 12.0 + col_w,
            metric_y,
            1.00,
            1.38,
            [0.54, 0.90, 0.58, 1.0],
            body_color,
            w,
            h,
        );
        draw_perf_metric(
            out,
            "EVENT>PRESENT CALL",
            &format!("{event_to_present_call:.3} MS"),
            x + 12.0 + col_w * 2.0,
            metric_y,
            1.00,
            1.38,
            [0.66, 0.74, 0.90, 1.0],
            body_color,
            w,
            h,
        );
        text(
            out,
            &if ui.video.input_latelatch {
                format!(
                    "{} SAMPLES / 500 MS   WORST PRESENT CALL {:.3} MS",
                    p.input_latency_samples, present_call_max,
                )
            } else {
                format!(
                    "{} SAMPLES / 500 MS   WORST PRESENT CALL {:.3} MS   CPU PRESENT RETURN, NOT SCANOUT",
                    p.input_latency_samples, present_call_max
                )
            },
            x + 12.0,
            next_y + 69.0,
            0.92,
            muted_color,
            w,
            h,
        );
        if ui.video.input_latelatch {
            text(
                out,
                &format!(
                    "EVENT>LATCH {:.3} MS   LATCH>SUBMIT {:.3} MS   LATCH>PRESENT {:.3} MS   PRESENT=CPU RETURN, NOT SCANOUT",
                    p.input_event_to_latch_ms.unwrap_or(0.0),
                    p.input_latch_to_submit_ms.unwrap_or(0.0),
                    p.input_latch_to_present_call_ms.unwrap_or(0.0),
                ),
                x + 12.0,
                next_y + 84.0,
                0.92,
                muted_color,
                w,
                h,
            );
        }
    } else {
        text(
            out,
            "MOVE THE MOUSE IN-GAME TO SAMPLE LATENCY",
            x + 12.0,
            next_y + 34.0,
            1.18,
            muted_color,
            w,
            h,
        );
    }

    next_y += input_h;
    rect(
        out,
        x + 10.0,
        next_y - 4.0,
        panel_w - 20.0,
        1.0,
        [1.0, 1.0, 1.0, 0.08],
        w,
        h,
    );

    text(
        out,
        "THREAD UTILIZATION  (~500 MS)",
        x + 12.0,
        next_y + 1.0,
        1.18,
        section_color,
        w,
        h,
    );
    let thread_column_w = (panel_w - 24.0) * 0.5;
    for (index, thread) in ui.threads.into_iter().enumerate() {
        let (column, row) = if thread_two_columns && index >= 11 {
            (1, index - 11)
        } else {
            (0, index)
        };
        let column_x = x + 12.0 + column as f32 * thread_column_w;
        let thread_bar_x = if thread_two_columns {
            column_x + 72.0
        } else {
            x + 100.0
        };
        let thread_bar_w = if thread_two_columns { 150.0 } else { 320.0 };
        let ty = next_y + thread_header_h + row as f32 * thread_row_h;
        let name = compact_thread_name(thread.name);
        let (bar_color, task_color) = if thread.active {
            ([0.34, 0.94, 0.48, 1.0], [0.34, 0.94, 0.48, 1.0])
        } else if thread.task == "IDLE" {
            ([0.34, 0.39, 0.44, 1.0], [0.54, 0.60, 0.66, 1.0])
        } else {
            ([0.40, 0.68, 0.92, 1.0], [0.70, 0.84, 0.98, 1.0])
        };

        text(out, name, column_x, ty + 3.0, 1.18, body_color, w, h);
        rect(
            out,
            thread_bar_x,
            ty + 6.0,
            thread_bar_w,
            17.0,
            [0.10, 0.12, 0.14, 0.95],
            w,
            h,
        );
        let fill = thread_bar_w * (thread.busy_percent / 100.0).clamp(0.0, 1.0);
        if fill > 0.0 {
            rect(out, thread_bar_x, ty + 6.0, fill, 17.0, bar_color, w, h);
        }
        rect_outline(
            out,
            thread_bar_x,
            ty + 6.0,
            thread_bar_w,
            17.0,
            1.0,
            [0.42, 0.48, 0.54, 0.9],
            w,
            h,
        );
        text(
            out,
            &format!("{:>5.1}%", thread.busy_percent),
            thread_bar_x + thread_bar_w + 8.0,
            ty + 3.0,
            1.12,
            body_color,
            w,
            h,
        );
        text(
            out,
            thread.task,
            thread_bar_x + thread_bar_w + 64.0,
            ty + 3.0,
            1.04,
            task_color,
            w,
            h,
        );
    }

    if debug_culling {
        let cy = next_y + thread_header_h + thread_rows as f32 * thread_row_h + 4.0;
        rect(
            out,
            x + 10.0,
            cy - 5.0,
            panel_w - 20.0,
            1.0,
            [1.0, 1.0, 1.0, 0.08],
            w,
            h,
        );
        text(
            out,
            &format!(
                "CULL VISIBLE {}  FRUSTUM {}  HI-Z {}  PVS {}  AREA {}",
                p.cull_visible,
                p.cull_frustum_rejected,
                p.cull_hiz_rejected,
                p.cull_pvs_rejected,
                p.cull_area_rejected,
            ),
            x + 12.0,
            cy + 2.0,
            1.10,
            [0.94, 0.90, 0.78, 1.0],
            w,
            h,
        );
        text(
            out,
            "XRAY: FRUSTUM RED  HI-Z MAGENTA  PVS CYAN  AREA YELLOW",
            x + 12.0,
            cy + 20.0,
            1.02,
            [0.78, 0.86, 0.94, 1.0],
            w,
            h,
        );
    }
}
fn console_panel_height(h: u32, size: ConsoleSize) -> f32 {
    match size {
        ConsoleSize::Normal => (h as f32 * 0.42).max(210.0),
        ConsoleSize::Half => h as f32 * 0.50,
        ConsoleSize::Full => h as f32,
    }
}

const CONSOLE_CHAR_WIDTH: f32 = 8.0;
const CONSOLE_CHAR_HEIGHT: f32 = 16.0;
/// Top of the first log row, just under the header band.
const CONSOLE_FIRST_Y: f32 = 52.0;

pub fn console_visible_line_capacity(h: u32, size: ConsoleSize) -> usize {
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = CONSOLE_FIRST_Y;
    (((input_y - first_y - 8.0) / line_step).floor().max(0.0)) as usize
}

pub fn console_text_hit(
    w: u32,
    h: u32,
    size: ConsoleSize,
    x: f64,
    y: f64,
) -> Option<(usize, usize)> {
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT as f64;
    let first_y = CONSOLE_FIRST_Y as f64;
    let max_lines = console_visible_line_capacity(h, size);
    if x < 20.0 || x > w as f64 - 8.0 || y < first_y || y >= input_y as f64 - 8.0 {
        return None;
    }
    let row = ((y - first_y) / line_step).floor() as usize;
    if row >= max_lines {
        return None;
    }
    let char_width = CONSOLE_CHAR_WIDTH as f64;
    let col = ((x - 20.0) / char_width).max(0.0).round() as usize;
    Some((row, col))
}

fn visible_text_len(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut index = 0usize;
    let mut count = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'^' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            index += 2;
        } else {
            count += 1;
            index += 1;
        }
    }
    count
}

fn tail_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(3);
    let tail: String = value.chars().skip(count.saturating_sub(keep)).collect();
    format!("...{tail}")
}

// Console palette: rain-slick Tokyo night. Indigo asphalt for the body, cyan and
// magenta neon for structure, tungsten amber for anything the user is driving.
const CON_CYAN: [f32; 4] = [0.28, 0.86, 0.95, 1.0];
const CON_MAGENTA: [f32; 4] = [1.0, 0.30, 0.56, 1.0];
const CON_AMBER: [f32; 4] = [1.0, 0.72, 0.28, 1.0];
const CON_TEXT: [f32; 4] = [0.80, 0.86, 0.93, 1.0];
const CON_BRIGHT: [f32; 4] = [0.95, 0.97, 1.0, 1.0];
const CON_DIM: [f32; 4] = [0.50, 0.60, 0.73, 1.0];
const CON_FAINT: [f32; 4] = [0.32, 0.40, 0.52, 1.0];

const SUGGEST_ROW_H: f32 = 20.0;
const SUGGEST_MAX_ROWS: usize = 8;
const SUGGEST_PAD: f32 = 6.0;
const SUGGEST_DETAIL_H: f32 = 68.0;
const SUGGEST_FOOTER_H: f32 = 22.0;

fn with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

/// Rectangle with a distinct color at each corner (the UI pipeline
/// interpolates vertex colors, so this is a free gradient).
#[allow(clippy::too_many_arguments)]
fn rect_gradient(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    top_left: [f32; 4],
    top_right: [f32; 4],
    bottom_left: [f32; 4],
    bottom_right: [f32; 4],
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position, color| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    out.extend_from_slice(&[
        v([x0, y0], top_left),
        v([x0, y1], bottom_left),
        v([x1, y1], bottom_right),
        v([x0, y0], top_left),
        v([x1, y1], bottom_right),
        v([x1, y0], top_right),
    ]);
}

/// Like `console_text` but without `^N` color escapes, for values and
/// descriptions that must print literally.
#[allow(clippy::too_many_arguments)]
fn console_plain(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    for byte in value.bytes() {
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                color,
                width,
                height,
            );
        }
        cursor_x += CONSOLE_CHAR_WIDTH;
    }
}

/// Truncate to `max_chars`, ending in `..` when cut.
fn ellipsize(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(2);
    let mut cut: String = value.chars().take(keep).collect();
    cut.push_str("..");
    cut
}

/// Greedy word wrap into at most `max_lines` lines; the last line is
/// ellipsized when the text does not fit.
fn wrap_plain(value: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let max_chars = max_chars.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = value.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let extra = usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + extra + word.chars().count() > max_chars {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                let last = lines.last_mut().expect("just pushed");
                *last = ellipsize(&format!("{last} {word}"), max_chars);
                return lines;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        if words.peek().is_none() {
            lines.push(std::mem::take(&mut current));
        }
    }
    lines.truncate(max_lines);
    lines
}

/// Colored keycap + dim label pairs, clipped at `max_x`.
#[allow(clippy::too_many_arguments)]
fn console_keycaps(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    max_x: f32,
    items: &[(&str, &str)],
    w: u32,
    h: u32,
) {
    let mut cursor = x;
    for (key, label) in items {
        let need = (key.len() + label.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
        if cursor + need > max_x {
            break;
        }
        console_plain(out, key, cursor, y, CON_AMBER, w, h);
        cursor += (key.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
        console_plain(out, label, cursor, y, CON_DIM, w, h);
        cursor += (label.len() + 3) as f32 * CONSOLE_CHAR_WIDTH;
    }
}

/// Tone for the gutter marker of echoed input (amber) and error lines (magenta).
fn console_line_tone(line: &str) -> Option<[f32; 4]> {
    let mut rest = line;
    // Optional `^8[hh:mm:ss]^7 ` timestamp prefix from `con_timestamps`.
    if let Some(after) = rest.strip_prefix("^8[") {
        if let Some(index) = after.find("]^7 ") {
            rest = &after[index + 4..];
        }
    }
    if rest.starts_with("^7] ") || rest.starts_with("] ") {
        Some(CON_AMBER)
    } else if rest.starts_with("^1") {
        Some(CON_MAGENTA)
    } else {
        None
    }
}

/// Clickable `SUGGEST ON/OFF` chip in the console header: `(x, y, w, h)`.
pub fn console_suggest_chip_rect(w: u32) -> (f32, f32, f32, f32) {
    let chip_w = 12.0 * CONSOLE_CHAR_WIDTH + 30.0;
    (w as f32 - 20.0 - 24.0 - 8.0 - chip_w, 8.0, chip_w, 24.0)
}

pub fn console_suggest_chip_hit(w: u32, x: f64, y: f64) -> bool {
    let (cx, cy, cw, ch) = console_suggest_chip_rect(w);
    x >= cx as f64 && x < (cx + cw) as f64 && y >= cy as f64 && y < (cy + ch) as f64
}

/// The `?` button in the header's top-right corner: `(x, y, w, h)`.
pub fn console_help_rect(w: u32) -> (f32, f32, f32, f32) {
    (w as f32 - 20.0 - 24.0, 8.0, 24.0, 24.0)
}

pub fn console_help_hit(w: u32, x: f64, y: f64) -> bool {
    let (bx, by, bw, bh) = console_help_rect(w);
    x >= bx as f64 && x < (bx + bw) as f64 && y >= by as f64 && y < (by + bh) as f64
}

/// Where the suggestion popup sits. Shared by drawing and mouse hit-testing.
#[derive(Debug, Clone, Copy)]
pub struct ConsoleSuggestGeometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Index of the first visible suggestion (the list scrolls to keep the selection in view).
    pub first: usize,
    pub visible: usize,
}

pub fn console_suggest_geometry(
    w: u32,
    h: u32,
    size: ConsoleSize,
    count: usize,
    selected: usize,
) -> ConsoleSuggestGeometry {
    let visible = count.clamp(1, SUGGEST_MAX_ROWS);
    let total_h = SUGGEST_PAD + visible as f32 * SUGGEST_ROW_H + SUGGEST_DETAIL_H + SUGGEST_FOOTER_H;
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    // Hang below the panel like a drop-down when there is room, otherwise
    // (full-height console) float just above the input line.
    let below = h as f32 - ph >= total_h + 12.0;
    let y = if below { ph + 8.0 } else { (input_y - 10.0 - total_h).max(4.0) };
    let first = if count <= visible {
        0
    } else {
        selected.saturating_sub(visible / 2).min(count - visible)
    };
    ConsoleSuggestGeometry {
        x: 12.0,
        y,
        w: (w as f32 - 24.0).clamp(240.0, 820.0),
        h: total_h,
        first,
        visible,
    }
}

/// Suggestion row under the pointer, as an index into the full suggestion list.
pub fn console_suggest_hit(
    w: u32,
    h: u32,
    size: ConsoleSize,
    count: usize,
    selected: usize,
    x: f64,
    y: f64,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let g = console_suggest_geometry(w, h, size, count, selected);
    let rows_y = (g.y + SUGGEST_PAD) as f64;
    if x < g.x as f64 || x >= (g.x + g.w) as f64 || y < rows_y {
        return None;
    }
    let row = ((y - rows_y) / SUGGEST_ROW_H as f64).floor() as usize;
    (row < g.visible).then_some(g.first + row)
}

fn build_console(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let ph = console_panel_height(h, ui.console_size);
    let wf = w as f32;
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = CONSOLE_FIRST_Y;

    // Body: indigo asphalt, a touch lighter toward the input so the eye lands there.
    rect_gradient(
        out,
        0.0,
        0.0,
        wf,
        ph,
        [0.012, 0.016, 0.036, 0.975],
        [0.012, 0.016, 0.036, 0.975],
        [0.024, 0.030, 0.066, 0.965],
        [0.024, 0.030, 0.066, 0.965],
        w,
        h,
    );
    // Header band and hairline.
    rect(out, 0.0, 0.0, wf, 40.0, [0.0, 0.0, 0.02, 0.30], w, h);
    rect(out, 0.0, 40.0, wf, 1.0, with_alpha(CON_CYAN, 0.14), w, h);

    // Neon edge along the bottom of the panel, with a soft glow bleeding onto the world below.
    let edge_l = with_alpha(CON_CYAN, 0.95);
    let edge_r = with_alpha(CON_MAGENTA, 0.95);
    rect_gradient(out, 0.0, ph - 2.0, wf, 2.0, edge_l, edge_r, edge_l, edge_r, w, h);
    for (dy, height, alpha) in [(0.0, 3.0, 0.17), (3.0, 4.0, 0.07), (7.0, 6.0, 0.025)] {
        let l = with_alpha(CON_CYAN, alpha);
        let r = with_alpha(CON_MAGENTA, alpha);
        rect_gradient(out, 0.0, ph + dy, wf, height, l, r, l, r, w, h);
    }

    // Title.
    rect(out, 20.0, 9.0, 3.0, 22.0, CON_MAGENTA, w, h);
    text(out, "CONSOLE", 32.0, 12.0, 2.0, CON_BRIGHT, w, h);
    if !ui.console_status.is_empty() && !ui.console_search_open {
        console_text(out, &ui.console_status, 32.0 + 7.0 * 12.0 + 16.0, 16.0, CON_DIM, w, h);
    }

    // Header right: line count and the suggestion switch.
    let (chip_x, chip_y, chip_w, chip_h) = console_suggest_chip_rect(w);
    rect(out, chip_x, chip_y, chip_w, chip_h, [0.04, 0.07, 0.13, 0.92], w, h);
    let chip_edge = if ui.console_suggest_enabled { with_alpha(CON_CYAN, 0.45) } else { with_alpha(CON_FAINT, 0.7) };
    rect_outline(out, chip_x, chip_y, chip_w, chip_h, 1.0, chip_edge, w, h);
    disc(
        out,
        chip_x + 13.0,
        chip_y + chip_h * 0.5,
        3.5,
        12,
        if ui.console_suggest_enabled { CON_CYAN } else { CON_FAINT },
        w,
        h,
    );
    console_plain(
        out,
        if ui.console_suggest_enabled { "SUGGEST ON" } else { "SUGGEST OFF" },
        chip_x + 24.0,
        chip_y + 4.0,
        if ui.console_suggest_enabled { CON_BRIGHT } else { CON_DIM },
        w,
        h,
    );
    let count_label = format!("{} LINES", ui.console_total_lines);
    let count_x = chip_x - 14.0 - count_label.len() as f32 * CONSOLE_CHAR_WIDTH;
    if !ui.console_search_open && count_x > 32.0 + 7.0 * 12.0 + 140.0 {
        console_plain(out, &count_label, count_x, chip_y + 4.0, CON_FAINT, w, h);
    }

    // Help button; the shortcut card appears while it is hovered.
    let (help_x, help_y, help_w, help_h) = console_help_rect(w);
    rect(
        out,
        help_x,
        help_y,
        help_w,
        help_h,
        if ui.console_help_hover { [0.10, 0.16, 0.26, 0.98] } else { [0.04, 0.07, 0.13, 0.92] },
        w,
        h,
    );
    rect_outline(
        out,
        help_x,
        help_y,
        help_w,
        help_h,
        1.0,
        if ui.console_help_hover { with_alpha(CON_AMBER, 0.9) } else { with_alpha(CON_CYAN, 0.45) },
        w,
        h,
    );
    console_plain(
        out,
        "?",
        help_x + (help_w - CONSOLE_CHAR_WIDTH) * 0.5,
        help_y + 4.0,
        if ui.console_help_hover { CON_AMBER } else { CON_DIM },
        w,
        h,
    );

    // The find field lives in the header, in place of the status text.
    if ui.console_search_open {
        let field_x = 32.0 + 7.0 * 12.0 + 16.0;
        let field_y = 6.0;
        let room = (chip_x - 14.0 - field_x).max(160.0);
        let field_w = (room * 0.62).clamp(160.0, 440.0);
        rect(out, field_x, field_y, field_w, 28.0, [0.03, 0.05, 0.10, 0.98], w, h);
        rect(out, field_x, field_y + 26.0, field_w, 2.0, CON_AMBER, w, h);
        let max_query_chars = ((field_w - 72.0) / CONSOLE_CHAR_WIDTH) as usize;
        let query = tail_chars(&ui.console_search_query, max_query_chars.max(1));
        console_plain(out, "FIND", field_x + 8.0, field_y + 6.0, CON_AMBER, w, h);
        console_plain(out, &query, field_x + 8.0 + 5.0 * CONSOLE_CHAR_WIDTH, field_y + 6.0, CON_BRIGHT, w, h);
        let caret_x = field_x + 8.0 + (5 + query.chars().count()) as f32 * CONSOLE_CHAR_WIDTH;
        rect(out, caret_x, field_y + 5.0, 2.0, 18.0, CON_AMBER, w, h);

        let counter = if ui.console_search_query.is_empty() {
            "TYPE TO SEARCH".to_owned()
        } else if ui.console_search_total == 0 {
            "NO MATCHES".to_owned()
        } else {
            format!(
                "{} / {}",
                ui.console_search_index.unwrap_or(0) + 1,
                ui.console_search_total
            )
        };
        console_plain(
            out,
            &counter,
            field_x + field_w + 12.0,
            field_y + 6.0,
            if ui.console_search_total == 0 && !ui.console_search_query.is_empty() {
                CON_MAGENTA
            } else {
                CON_AMBER
            },
            w,
            h,
        );
    }

    // Log.
    let max_lines = (((input_y - first_y - 8.0) / line_step).floor().max(0.0)) as usize;
    let history_end = ui.console_lines.len().saturating_sub(ui.console_scroll);
    let history_start = history_end.saturating_sub(max_lines);
    let mut line_y = first_y;
    for (line_index, line) in ui.console_lines[history_start..history_end]
        .iter()
        .enumerate()
    {
        let absolute_line = history_start + line_index;
        if let Some(tone) = console_line_tone(line) {
            rect(out, 0.0, line_y - 2.0, wf, line_step, with_alpha(tone, 0.055), w, h);
            rect(out, 10.0, line_y - 2.0, 2.0, line_step, with_alpha(tone, 0.85), w, h);
        }
        if ui.console_search_open {
            for hit in ui
                .console_search_matches
                .iter()
                .filter(|hit| hit.line == absolute_line)
            {
                let active = ui
                    .console_search_active
                    .is_some_and(|active| active == *hit);
                rect(
                    out,
                    20.0 + hit.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y - 2.0,
                    (hit.end_col - hit.start_col) as f32 * CONSOLE_CHAR_WIDTH,
                    line_step,
                    if active {
                        [0.95, 0.62, 0.10, 0.88]
                    } else {
                        [0.72, 0.58, 0.12, 0.48]
                    },
                    w,
                    h,
                );
            }
        }
        if let Some(selection) = ui.console_selection {
            let (start_line, start_col, end_line, end_col) =
                if (selection.start_line, selection.start_col)
                    <= (selection.end_line, selection.end_col)
                {
                    (
                        selection.start_line,
                        selection.start_col,
                        selection.end_line,
                        selection.end_col,
                    )
                } else {
                    (
                        selection.end_line,
                        selection.end_col,
                        selection.start_line,
                        selection.start_col,
                    )
                };
            if absolute_line >= start_line && absolute_line <= end_line {
                let visible_len = visible_text_len(line);
                let left_col = if absolute_line == start_line {
                    start_col.min(visible_len)
                } else {
                    0
                };
                let right_col = if absolute_line == end_line {
                    end_col.min(visible_len)
                } else {
                    visible_len
                };
                if right_col > left_col {
                    let char_width = CONSOLE_CHAR_WIDTH;
                    rect(
                        out,
                        20.0 + left_col as f32 * char_width,
                        line_y - 2.0,
                        (right_col - left_col) as f32 * char_width,
                        line_step,
                        [0.08, 0.46, 0.62, 0.55],
                        w,
                        h,
                    );
                }
            }
        }
        console_text(out, line, 20.0, line_y, CON_TEXT, w, h);

        // Trusted engine-authored local paths get conventional hyperlink
        // treatment. Draw this *after* the normal colored console text so a
        // green/yellow log line cannot hide the link, and keep the underline
        // inside the 16 px row (the previous +18 px position fell below it).
        let mut path_links = ui
            .console_path_links
            .iter()
            .filter(|link| link.line == absolute_line)
            .peekable();
        if path_links.peek().is_some() {
            let plain = crate::logging::strip_jka_colors(line);
            for link in path_links {
                if link.end_col <= link.start_col {
                    continue;
                }
                let linked_text: String = plain
                    .chars()
                    .skip(link.start_col)
                    .take(link.end_col - link.start_col)
                    .collect();
                console_plain(
                    out,
                    &linked_text,
                    20.0 + link.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y,
                    CON_CYAN,
                    w,
                    h,
                );
                rect(
                    out,
                    20.0 + link.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y + CONSOLE_CHAR_HEIGHT - 2.0,
                    (link.end_col - link.start_col) as f32 * CONSOLE_CHAR_WIDTH,
                    1.0,
                    with_alpha(CON_CYAN, 0.95),
                    w,
                    h,
                );
            }
        }
        line_y += line_step;
    }
    if ui.console_lines.is_empty() && !ui.console_status.is_empty() {
        console_plain(out, "NO OUTPUT YET", 20.0, first_y, CON_FAINT, w, h);
    }

    // Scroll position: a thin rail on the right plus a "newer output below" pill.
    let rail_top = first_y - 2.0;
    let rail_h = (input_y - 8.0 - rail_top).max(1.0);
    if max_lines > 0 && ui.console_total_lines > max_lines {
        let total = ui.console_total_lines as f32;
        let thumb_h = (rail_h * max_lines as f32 / total).clamp(18.0, rail_h);
        let travel = rail_h - thumb_h;
        let max_scroll = (ui.console_total_lines - max_lines) as f32;
        let from_top = 1.0 - (ui.console_scrolled as f32 / max_scroll).clamp(0.0, 1.0);
        rect(out, wf - 8.0, rail_top, 2.0, rail_h, with_alpha(CON_CYAN, 0.07), w, h);
        rect(
            out,
            wf - 9.0,
            rail_top + travel * from_top,
            4.0,
            thumb_h,
            with_alpha(if ui.console_scrolled > 0 { CON_MAGENTA } else { CON_CYAN }, 0.55),
            w,
            h,
        );
    }
    if ui.console_scrolled > 0 {
        let label = format!("{} NEWER LINES BELOW  PGDN", ui.console_scrolled);
        let pill_w = label.len() as f32 * CONSOLE_CHAR_WIDTH + 20.0;
        let pill_x = wf - 24.0 - pill_w;
        let pill_y = input_y - 34.0;
        rect(out, pill_x, pill_y, pill_w, 22.0, [0.10, 0.03, 0.08, 0.92], w, h);
        rect_outline(out, pill_x, pill_y, pill_w, 22.0, 1.0, with_alpha(CON_MAGENTA, 0.7), w, h);
        console_plain(out, &label, pill_x + 10.0, pill_y + 3.0, CON_MAGENTA, w, h);
    }

    // Input bar.
    let bar_y = input_y - 8.0;
    rect(out, 0.0, bar_y, wf, (ph - 2.0) - bar_y, [0.045, 0.065, 0.125, 0.94], w, h);
    rect(out, 0.0, bar_y, wf, 1.0, with_alpha(CON_CYAN, 0.18), w, h);
    console_plain(out, ">", 20.0, input_y, CON_AMBER, w, h);
    let text_x = 38.0;
    let cursor = ui.console_cursor.min(ui.console_input.len());
    let cursor = if ui.console_input.is_char_boundary(cursor) { cursor } else { ui.console_input.len() };
    if ui.console_input.is_empty() {
        console_plain(
            out,
            "type a command or cvar",
            text_x + 6.0,
            input_y,
            CON_FAINT,
            w,
            h,
        );
    } else {
        console_text(out, &ui.console_input, text_x, input_y, CON_BRIGHT, w, h);
        // Ghost completion: the rest of the highlighted suggestion, dimmed.
        if !ui.console_suggest_hint && cursor == ui.console_input.len() {
            let typed = ui.console_input.trim_start();
            if let Some(best) = ui.console_suggestions.get(ui.console_suggest_selected) {
                let name = best.name;
                if !typed.contains(char::is_whitespace)
                    && name.len() > typed.len()
                    && name.as_bytes()[..typed.len()].eq_ignore_ascii_case(typed.as_bytes())
                {
                    let end_x = text_x + visible_text_len(&ui.console_input) as f32 * CONSOLE_CHAR_WIDTH;
                    console_plain(out, &name[typed.len()..], end_x, input_y, with_alpha(CON_CYAN, 0.42), w, h);
                }
            }
        }
    }
    let caret_x = text_x + visible_text_len(&ui.console_input[..cursor]) as f32 * CONSOLE_CHAR_WIDTH;
    rect(out, caret_x, input_y - 1.0, 2.0, CONSOLE_CHAR_HEIGHT + 2.0, CON_AMBER, w, h);

    if !ui.console_suggestions.is_empty() && !ui.console_search_open {
        build_console_suggestions(out, ui, w, h);
    }
    if ui.console_help_hover {
        build_console_help(out, w, h);
    }
}

/// Shortcut card shown while the header `?` is hovered.
fn build_console_help(out: &mut Vec<UiVertex>, w: u32, h: u32) {
    // Empty key = section heading.
    const ROWS: &[(&str, &str)] = &[
        ("", "TYPING"),
        ("TAB", "complete, or extend shared prefix"),
        ("UP / DOWN", "pick suggestion (history if none)"),
        ("ENTER", "run; fills a picked suggestion"),
        ("ESC", "hide list, then close console"),
        ("CLICK", "pick a suggestion"),
        ("", "EDITING"),
        ("LEFT / RIGHT", "move caret (CTRL = by word)"),
        ("HOME / END", "start / end of line"),
        ("CTRL+V", "paste"),
        ("", "LOG"),
        ("CTRL+F", "find (ENTER next, SHIFT+ENTER prev)"),
        ("PGUP / PGDN", "scroll"),
        ("DRAG", "select; 2x click word, 3x line"),
        ("CTRL+A / C", "select all / copy selection"),
        ("", "CONSOLE"),
        ("~", "open / close"),
        ("SHIFT+~", "half height"),
        ("CTRL+~", "full height"),
    ];
    const ROW_H: f32 = 18.0;
    let key_col = 14.0 * CONSOLE_CHAR_WIDTH;
    let card_w = (key_col + 36.0 * CONSOLE_CHAR_WIDTH + 28.0).min(w as f32 - 24.0);
    let card_h = ROWS.len() as f32 * ROW_H + 20.0;
    let x = w as f32 - 20.0 - card_w;
    let y = 44.0;
    rect(out, x + 3.0, y + 5.0, card_w, card_h, [0.0, 0.0, 0.0, 0.40], w, h);
    rect(out, x, y, card_w, card_h, [0.020, 0.028, 0.056, 0.99], w, h);
    let edge_l = with_alpha(CON_CYAN, 0.6);
    let edge_r = with_alpha(CON_MAGENTA, 0.6);
    rect_gradient(out, x, y, card_w, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect_gradient(out, x, y + card_h - 1.0, card_w, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect(out, x, y, 1.0, card_h, edge_l, w, h);
    rect(out, x + card_w - 1.0, y, 1.0, card_h, edge_r, w, h);
    let label_chars = ((card_w - 28.0 - key_col) / CONSOLE_CHAR_WIDTH) as usize;
    for (i, (key, label)) in ROWS.iter().enumerate() {
        let ry = y + 10.0 + i as f32 * ROW_H;
        if key.is_empty() {
            console_plain(out, label, x + 14.0, ry, CON_CYAN, w, h);
            let rule_x = x + 14.0 + (label.len() as f32 + 1.0) * CONSOLE_CHAR_WIDTH;
            rect(out, rule_x, ry + 8.0, x + card_w - 14.0 - rule_x, 1.0, with_alpha(CON_CYAN, 0.16), w, h);
        } else {
            console_plain(out, key, x + 14.0, ry, CON_AMBER, w, h);
            console_plain(out, &ellipsize(label, label_chars), x + 14.0 + key_col, ry, CON_TEXT, w, h);
        }
    }
}

fn build_console_suggestions(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let count = ui.console_suggestions.len();
    let selected = ui.console_suggest_selected.min(count - 1);
    let hint = ui.console_suggest_hint;
    let g = console_suggest_geometry(w, h, ui.console_size, count, selected);
    let (x, y, pw) = (g.x, g.y, g.w);
    let inner_chars = ((pw - 28.0) / CONSOLE_CHAR_WIDTH) as usize;

    // Lift off the console/world, then the card itself.
    rect(out, x + 3.0, y + 5.0, pw, g.h, [0.0, 0.0, 0.0, 0.38], w, h);
    rect(out, x, y, pw, g.h, [0.020, 0.028, 0.056, 0.985], w, h);
    let edge_l = with_alpha(CON_CYAN, 0.55);
    let edge_r = with_alpha(CON_MAGENTA, 0.55);
    rect_gradient(out, x, y, pw, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect_gradient(out, x, y + g.h - 1.0, pw, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect(out, x, y, 1.0, g.h, with_alpha(CON_CYAN, 0.55), w, h);
    rect(out, x + pw - 1.0, y, 1.0, g.h, with_alpha(CON_MAGENTA, 0.55), w, h);

    let rows_y = y + SUGGEST_PAD;
    for row in 0..g.visible {
        let index = g.first + row;
        let Some(item) = ui.console_suggestions.get(index) else { break };
        let ry = rows_y + row as f32 * SUGGEST_ROW_H;
        let is_selected = !hint && index == selected;
        if is_selected {
            rect_gradient(
                out,
                x + 1.0,
                ry,
                pw - 2.0,
                SUGGEST_ROW_H,
                [0.06, 0.26, 0.36, 0.85],
                [0.06, 0.16, 0.30, 0.55],
                [0.06, 0.26, 0.36, 0.85],
                [0.06, 0.16, 0.30, 0.55],
                w,
                h,
            );
            rect(out, x + 1.0, ry, 3.0, SUGGEST_ROW_H, CON_CYAN, w, h);
        }
        let text_y = ry + (SUGGEST_ROW_H - CONSOLE_CHAR_HEIGHT) * 0.5;
        let (badge, badge_color) = match item.kind {
            ConsoleSuggestKind::Cvar => ("VAR", CON_CYAN),
            ConsoleSuggestKind::Command => ("CMD", CON_AMBER),
            ConsoleSuggestKind::Server => ("SRV", CON_MAGENTA),
        };
        console_plain(out, badge, x + 14.0, text_y, with_alpha(badge_color, if is_selected { 1.0 } else { 0.72 }), w, h);

        // Name, with the characters the query matched picked out in amber.
        let name_x = x + 14.0 + 4.0 * CONSOLE_CHAR_WIDTH;
        let value_room = if item.kind == ConsoleSuggestKind::Cvar { 22 } else { 0 };
        let name_room = inner_chars.saturating_sub(4 + value_room + 1).max(8);
        for (i, byte) in item.name.bytes().take(name_room).enumerate() {
            let matched = i < 64 && item.mask >> i & 1 == 1;
            let color = if matched {
                CON_AMBER
            } else if is_selected {
                CON_BRIGHT
            } else {
                CON_TEXT
            };
            glyph_quad_sized(
                out,
                byte,
                name_x + i as f32 * CONSOLE_CHAR_WIDTH,
                text_y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                color,
                w,
                h,
            );
            if matched {
                rect(
                    out,
                    name_x + i as f32 * CONSOLE_CHAR_WIDTH,
                    text_y + CONSOLE_CHAR_HEIGHT - 1.0,
                    CONSOLE_CHAR_WIDTH,
                    1.0,
                    with_alpha(CON_AMBER, 0.6),
                    w,
                    h,
                );
            }
        }

        if item.kind == ConsoleSuggestKind::Cvar && !item.value.is_empty() {
            let value = ellipsize(&item.value, value_room);
            let vx = x + pw - 14.0 - value.chars().count() as f32 * CONSOLE_CHAR_WIDTH;
            let color = if item.modified { CON_AMBER } else { with_alpha(CON_CYAN, 0.62) };
            console_plain(out, &value, vx, text_y, color, w, h);
        }
    }

    // Detail card for the highlighted entry.
    let detail_y = rows_y + g.visible as f32 * SUGGEST_ROW_H + 2.0;
    rect(out, x + 10.0, detail_y, pw - 20.0, 1.0, with_alpha(CON_CYAN, 0.16), w, h);
    if let Some(item) = ui.console_suggestions.get(selected) {
        let lines = wrap_plain(item.description, inner_chars, 2);
        for (i, line) in lines.iter().enumerate() {
            console_plain(out, line, x + 14.0, detail_y + 6.0 + i as f32 * 18.0, CON_TEXT, w, h);
        }
        let meta_y = detail_y + 6.0 + 2.0 * 18.0 + 3.0;
        let mut mx = x + 14.0;
        let mut meta = |label: &str, value: &str, color: [f32; 4], out: &mut Vec<UiVertex>| {
            if value.is_empty() || mx > x + pw - 40.0 {
                return;
            }
            let room = ((x + pw - 14.0 - mx) / CONSOLE_CHAR_WIDTH) as usize;
            let value = ellipsize(value, room.saturating_sub(label.len() + 1).max(4));
            console_plain(out, label, mx, meta_y, CON_FAINT, w, h);
            mx += (label.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
            console_plain(out, &value, mx, meta_y, color, w, h);
            mx += (value.chars().count() + 3) as f32 * CONSOLE_CHAR_WIDTH;
        };
        match item.kind {
            ConsoleSuggestKind::Cvar => {
                meta("DEFAULT", item.default_value, CON_DIM, out);
                meta("RANGE", item.range, CON_DIM, out);
            }
            ConsoleSuggestKind::Command => meta("COMMAND", "runs locally", CON_DIM, out),
            ConsoleSuggestKind::Server => meta("SERVER COMMAND", "sent to the server", CON_DIM, out),
        }
    }

    // Footer: match count and keys.
    let footer_y = y + g.h - SUGGEST_FOOTER_H;
    rect(out, x + 1.0, footer_y, pw - 2.0, SUGGEST_FOOTER_H - 1.0, [0.0, 0.0, 0.02, 0.35], w, h);
    let summary = if hint {
        "ARGUMENT HINT".to_owned()
    } else if ui.console_suggest_total > count {
        format!("{} OF {} MATCHES", selected + 1, ui.console_suggest_total)
    } else {
        format!("{} OF {} MATCHES", selected + 1, count)
    };
    console_plain(out, &summary, x + 14.0, footer_y + 3.0, CON_CYAN, w, h);
    let keys_x = x + 14.0 + (summary.len() + 3) as f32 * CONSOLE_CHAR_WIDTH;
    if hint {
        console_keycaps(out, keys_x, footer_y + 3.0, x + pw - 8.0, &[("ENTER", "run")], w, h);
    } else {
        console_keycaps(
            out,
            keys_x,
            footer_y + 3.0,
            x + pw - 8.0,
            &[("TAB", "complete"), ("UP/DOWN", "select"), ("CLICK", "pick"), ("ESC", "hide")],
            w,
            h,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += 9.0 * scale;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte == b' ' {
            cursor_x += 6.0 * scale;
            index += 1;
            continue;
        }
        glyph_quad(
            out,
            byte,
            cursor_x,
            cursor_y,
            scale,
            active_color,
            width,
            height,
        );
        cursor_x += 6.0 * scale;
        index += 1;
    }
}

fn proportional_text_width(value: impl AsRef<[u8]>, font: &ProportionalFont, scale: f32) -> f32 {
    let bytes = value.as_ref();
    let mut width = 0.0f32;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'^'
            && index + 1 < bytes.len()
            && color_code(bytes[index + 1] as char, 1.0).is_some()
        {
            index += 2;
            continue;
        }
        if bytes[index] == b'\n' {
            break;
        }
        width += font.glyphs[bytes[index] as usize].horiz_advance.max(0) as f32 * scale;
        index += 1;
    }
    width
}

fn wrap_proportional_text(
    value: &str,
    font: &ProportionalFont,
    scale: f32,
    max_width: f32,
) -> String {
    if proportional_text_width(value, font, 1.0) <= max_width {
        return value.to_owned();
    }

    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len() + value.len() / 32);
    let mut line_width = 0.0f32;
    let mut line_start = 0usize;
    let mut last_space_out = None::<usize>;
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'^'
            && index + 1 < bytes.len()
            && color_code(bytes[index + 1] as char, 1.0).is_some()
        {
            out.push(bytes[index] as char);
            out.push(bytes[index + 1] as char);
            index += 2;
            continue;
        }
        let byte = bytes[index];
        if byte == b'\n' {
            out.push('\n');
            line_width = 0.0;
            line_start = out.len();
            last_space_out = None;
            index += 1;
            continue;
        }

        if byte == b' ' {
            last_space_out = Some(out.len());
        }
        out.push(byte as char);
        line_width += font.glyphs[byte as usize].horiz_advance.max(0) as f32 * scale;

        if line_width >= max_width {
            let break_at = last_space_out.filter(|space| *space >= line_start);
            if let Some(space) = break_at {
                out.replace_range(space..=space, "\n");
                let tail = out[space + 1..].to_owned();
                line_width = proportional_text_width(&tail, font, scale);
                line_start = space + 1;
            } else {
                out.push('\n');
                line_width = 0.0;
                line_start = out.len();
            }
            last_space_out = None;
        }
        index += 1;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn proportional_text(
    out: &mut Vec<UiVertex>,
    value: impl AsRef<[u8]>,
    font: &ProportionalFont,
    x: f32,
    baseline_y: f32,
    scale: f32,
    color: [f32; 4],
    shadow: bool,
    width: u32,
    height: u32,
) {
    let x_scale = width.max(1) as f32 / 640.0;
    let y_scale = height.max(1) as f32 / 480.0;
    let line_step = 20.0 * scale * y_scale;

    let draw_pass = |out: &mut Vec<UiVertex>, shadow_pass: bool| {
        let mut cursor_x = x;
        let mut cursor_y = baseline_y;
        let mut active_color = color;
        let bytes = value.as_ref();
        let mut index = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\n' {
                cursor_x = x;
                cursor_y += line_step;
                index += 1;
                continue;
            }
            if byte == b'^' && index + 1 < bytes.len() {
                if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                    active_color = code;
                    index += 2;
                    continue;
                }
            }

            let glyph = font.glyphs[byte as usize];
            if glyph.width > 0 && glyph.height > 0 && byte != b' ' {
                // OpenJK chat uses the font drop-shadow style. Keep this
                // screen-space tight instead of scaling the offset with the
                // framebuffer, which made the shadow too far away at 1080p+.
                let shadow_offset_x = if shadow_pass { 1.0 } else { 0.0 };
                let shadow_offset_y = if shadow_pass { 1.0 } else { 0.0 };
                let draw_color = if shadow_pass {
                    [0.0, 0.0, 0.0, color[3] * 0.72]
                } else {
                    active_color
                };
                let draw_x =
                    cursor_x + glyph.horiz_offset as f32 * scale * x_scale + shadow_offset_x;
                let draw_y = cursor_y - glyph.baseline as f32 * scale * y_scale + shadow_offset_y;
                textured_rect_with_source(
                    out,
                    draw_x,
                    draw_y,
                    glyph.width as f32 * scale * x_scale,
                    glyph.height as f32 * scale * y_scale,
                    [glyph.s, glyph.t],
                    [glyph.s2, glyph.t2],
                    draw_color,
                    2.0,
                    width,
                    height,
                );
            }
            cursor_x += glyph.horiz_advance.max(0) as f32 * scale * x_scale;
            index += 1;
        }
    };

    if shadow {
        draw_pass(out, true);
    }
    draw_pass(out, false);
}

#[allow(clippy::too_many_arguments)]
fn fixed_charset_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    glyph_w: f32,
    glyph_h: f32,
    advance: f32,
    color: [f32; 4],
    shadow: bool,
    width: u32,
    height: u32,
) {
    if shadow {
        let mut cursor_x = x + 2.0;
        let mut cursor_y = y + 2.0;
        let bytes = value.as_bytes();
        let mut index = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\n' {
                cursor_x = x + 2.0;
                cursor_y += glyph_h;
                index += 1;
                continue;
            }
            if byte == b'^'
                && index + 1 < bytes.len()
                && color_code(bytes[index + 1] as char, color[3]).is_some()
            {
                index += 2;
                continue;
            }
            if byte != b' ' {
                glyph_quad_sized(
                    out,
                    byte,
                    cursor_x,
                    cursor_y,
                    glyph_w,
                    glyph_h,
                    [0.0, 0.0, 0.0, color[3]],
                    width,
                    height,
                );
            }
            cursor_x += advance;
            index += 1;
        }
    }

    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += glyph_h;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                cursor_y,
                glyph_w,
                glyph_h,
                active_color,
                width,
                height,
            );
        }
        cursor_x += advance;
        index += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn console_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += CONSOLE_CHAR_HEIGHT;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                cursor_y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                active_color,
                width,
                height,
            );
        }
        cursor_x += CONSOLE_CHAR_WIDTH;
        index += 1;
    }
}

fn color_code(code: char, alpha: f32) -> Option<[f32; 4]> {
    let rgb = match code {
        // JKA/Quake color escapes are saturated primaries/secondaries.
        // Do not pastelize these: ^2 in particular is the stock chat green.
        '0' => [0.0, 0.0, 0.0],
        '1' => [1.0, 0.0, 0.0],
        '2' => [0.0, 1.0, 0.0],
        '3' => [1.0, 1.0, 0.0],
        '4' => [0.0, 0.0, 1.0],
        '5' => [0.0, 1.0, 1.0],
        '6' => [1.0, 0.0, 1.0],
        '7' => [1.0, 1.0, 1.0],
        '8' => [1.0, 0.62, 0.22],
        '9' => [0.62, 0.62, 0.62],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], alpha])
}

#[allow(clippy::too_many_arguments)]
fn glyph_quad(
    out: &mut Vec<UiVertex>,
    glyph: u8,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    glyph_quad_sized(
        out,
        glyph,
        x,
        y,
        6.0 * scale,
        8.0 * scale,
        color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
fn glyph_quad_sized(
    out: &mut Vec<UiVertex>,
    glyph: u8,
    x: f32,
    y: f32,
    glyph_w: f32,
    glyph_h: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    // Jedi Academy's gfx/2d/charsgrid_med is a 16x16 *logical* grid,
    // but each glyph only occupies the left 8 pixels of its 16-pixel-wide
    // slot. OpenJK therefore advances U by 1/16 per character while only
    // sampling 1/32 of the texture width; V uses the full 1/16 cell.
    // See SCR_DrawChar / SCR_DrawSmallChar in OpenJK cl_scrn.cpp.
    let cell_stride = 1.0 / 16.0;
    let glyph_u_width = 1.0 / 32.0;
    let column = (glyph & 15) as f32;
    let row = (glyph >> 4) as f32;
    let u0 = column * cell_stride;
    let v0 = row * cell_stride;
    let u1 = u0 + glyph_u_width;
    let v1 = v0 + cell_stride;
    textured_rect(
        out,
        x,
        y,
        glyph_w,
        glyph_h,
        [u0, v0],
        [u1, v1],
        color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
fn textured_rect(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    textured_rect_with_source(out, x, y, w, h, uv0, uv1, color, 1.0, width, height);
}

#[allow(clippy::too_many_arguments)]
fn textured_rect_with_source(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv0: [f32; 2],
    uv1: [f32; 2],
    color: [f32; 4],
    texture_source: f32,
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position, uv| UiVertex {
        position,
        uv,
        color,
        textured: texture_source,
    };
    out.extend_from_slice(&[
        v([x0, y0], [uv0[0], uv0[1]]),
        v([x0, y1], [uv0[0], uv1[1]]),
        v([x1, y1], [uv1[0], uv1[1]]),
        v([x0, y0], [uv0[0], uv0[1]]),
        v([x1, y1], [uv1[0], uv1[1]]),
        v([x1, y0], [uv1[0], uv0[1]]),
    ]);
}

#[allow(clippy::too_many_arguments)]
fn disc(
    out: &mut Vec<UiVertex>,
    center_x: f32,
    center_y: f32,
    radius: f32,
    segments: usize,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let to_ndc = |x: f32, y: f32| [x / width * 2.0 - 1.0, 1.0 - y / height * 2.0];
    let center = to_ndc(center_x, center_y);
    let vertex = |position| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    let segments = segments.max(3);
    for index in 0..segments {
        let a0 = std::f32::consts::TAU * index as f32 / segments as f32;
        let a1 = std::f32::consts::TAU * (index + 1) as f32 / segments as f32;
        let p0 = to_ndc(center_x + radius * a0.cos(), center_y + radius * a0.sin());
        let p1 = to_ndc(center_x + radius * a1.cos(), center_y + radius * a1.sin());
        out.extend_from_slice(&[vertex(center), vertex(p0), vertex(p1)]);
    }
}

#[allow(clippy::too_many_arguments)]
fn rect(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    out.extend_from_slice(&[
        v([x0, y0]),
        v([x0, y1]),
        v([x1, y1]),
        v([x0, y0]),
        v([x1, y1]),
        v([x1, y0]),
    ]);
}

#[allow(clippy::too_many_arguments)]
fn rect_outline(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    rect(out, x, y, w, thickness, color, width, height);
    rect(
        out,
        x,
        y + h - thickness,
        w,
        thickness,
        color,
        width,
        height,
    );
    rect(out, x, y, thickness, h, color, width, height);
    rect(
        out,
        x + w - thickness,
        y,
        thickness,
        h,
        color,
        width,
        height,
    );
}

/// Generate a tiny recovery charset using the same atlas layout as JKA's
/// gfx/2d/charsgrid_med: 16x16 logical slots on a 256x256 texture, with each
/// glyph living in the left 8x16 pixels of its 16x16 slot. Keeping the fallback
/// in the same layout means the normal OpenJK-compatible UV path stays valid.
pub fn fallback_font_rgba() -> (u32, u32, Vec<u8>) {
    const SLOT: usize = 16;
    const GRID: usize = 16;
    let width = SLOT * GRID;
    let height = SLOT * GRID;
    let mut rgba = vec![0u8; width * height * 4];
    for code in 0u16..=255 {
        let rows = glyph(code as u8 as char);
        let cell_x = (code as usize & 15) * SLOT;
        let cell_y = (code as usize >> 4) * SLOT;
        for (gy, bits) in rows.into_iter().enumerate() {
            for gx in 0..5 {
                if bits & (1 << (4 - gx)) == 0 {
                    continue;
                }
                let px = cell_x + gx + 1;
                let py = cell_y + gy + 4;
                let offset = (py * width + px) * 4;
                rgba[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    (width as u32, height as u32, rgba)
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ':' => [0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0],
        '>' => [
            0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000,
        ],
        '<' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001,
        ],
        '=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10110, 0b00110, 0],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        ' ' => [0; 7],
        _ => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0, 0b00100],
    }
}

#[cfg(test)]
mod crosshair_tests {
    use super::*;

    #[test]
    fn dot_crosshair_is_circular_and_exactly_centered() {
        for (width, height) in [(1280_u32, 720_u32), (1279_u32, 799_u32)] {
            let mut vertices = Vec::new();
            build_crosshair(
                &mut vertices,
                CrosshairSettings {
                    style: 2,
                    size: 24.0,
                    color: [255, 255, 255, 255],
                    ..CrosshairSettings::default()
                },
                None,
                1.0,
                width,
                height,
            );
            assert_eq!(vertices.len(), 16 * 3);

            let to_pixels = |vertex: &UiVertex| {
                (
                    (vertex.position[0] + 1.0) * 0.5 * width as f32,
                    (1.0 - vertex.position[1]) * 0.5 * height as f32,
                )
            };
            let points = vertices.iter().map(to_pixels).collect::<Vec<_>>();
            let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
            let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
            let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
            let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
            let center_x = (min_x + max_x) * 0.5;
            let center_y = (min_y + max_y) * 0.5;

            assert!((center_x - width as f32 * 0.5).abs() < 0.001);
            assert!((center_y - height as f32 * 0.5).abs() < 0.001);
            assert!(((max_x - min_x) - (max_y - min_y)).abs() < 0.001);

            // Every non-center perimeter vertex lies on one radius in pixel
            // space, so the dot cannot regress to the old axis-aligned square.
            let expected_radius = (max_x - min_x) * 0.5;
            for &(x, y) in points.iter().filter(|&&(x, y)| {
                (x - center_x).abs() > 0.001 || (y - center_y).abs() > 0.001
            }) {
                let radius = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();
                assert!((radius - expected_radius).abs() < 0.002);
            }
        }
    }

    fn pixel_bounds(vertices: &[UiVertex], width: u32, height: u32) -> [f32; 4] {
        let points = vertices.iter().map(|vertex| {
            (
                (vertex.position[0] + 1.0) * 0.5 * width as f32,
                (1.0 - vertex.position[1]) * 0.5 * height as f32,
            )
        });
        points.fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, (x, y)| {
            [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
        })
    }

    #[test]
    fn image_crosshair_is_a_centered_atlas_quad() {
        let (width, height) = (1280_u32, 720_u32);
        let mut vertices = Vec::new();
        let crosshair = CrosshairSettings { style: 1, image: 3, size: 32.0, ..CrosshairSettings::default() };
        build_crosshair(&mut vertices, crosshair, None, 1.0, width, height);
        assert_eq!(vertices.len(), 6);
        assert!(vertices.iter().all(|vertex| vertex.textured == ICON_TEXTURE_SOURCE));

        let [min_x, min_y, max_x, max_y] = pixel_bounds(&vertices, width, height);
        assert!((max_x - min_x - 32.0).abs() < 0.01 && (max_y - min_y - 32.0).abs() < 0.01);
        assert!(((min_x + max_x) * 0.5 - width as f32 * 0.5).abs() < 0.01);
        assert!(((min_y + max_y) * 0.5 - height as f32 * 0.5).abs() < 0.01);

        // Image 3 is the third crosshair, after the two lagometer cells.
        let (uv0, uv1) = icon_cell_uv(ICON_CROSSHAIR_BASE + 2);
        let u_min = vertices.iter().map(|v| v.uv[0]).fold(f32::INFINITY, f32::min);
        let u_max = vertices.iter().map(|v| v.uv[0]).fold(f32::NEG_INFINITY, f32::max);
        assert!((u_min - uv0[0]).abs() < 1e-6 && (u_max - uv1[0]).abs() < 1e-6);
    }

    #[test]
    fn image_zero_and_out_of_range_fall_back_to_the_shape() {
        for image in [0, CROSSHAIR_IMAGE_COUNT + 1] {
            let mut vertices = Vec::new();
            let crosshair = CrosshairSettings { style: 2, image, ..CrosshairSettings::default() };
            build_crosshair(&mut vertices, crosshair, None, 1.0, 1280, 720);
            assert!(vertices.iter().all(|vertex| vertex.textured == 0.0));
            assert_eq!(vertices.len(), 16 * 3);
        }
    }

    #[test]
    fn line_crosshair_is_a_short_vertical_line() {
        let (width, height) = (1280_u32, 960_u32);
        let mut vertices = Vec::new();
        let crosshair = CrosshairSettings { style: CROSSHAIR_STYLE_LINE, size: 24.0, ..CrosshairSettings::default() };
        build_crosshair(&mut vertices, crosshair, None, 2.0, width, height);
        assert_eq!(vertices.len(), 6);
        let [min_x, min_y, max_x, max_y] = pixel_bounds(&vertices, width, height);
        // 2 units wide at 2 px per unit; 1.25x the plus (24 * 2/3 = 16 px) tall.
        assert!((max_x - min_x - 4.0).abs() < 0.01);
        assert!((max_y - min_y - 20.0).abs() < 0.01);
        assert!(((min_x + max_x) * 0.5 - width as f32 * 0.5).abs() < 0.01);
        assert!(((min_y + max_y) * 0.5 - height as f32 * 0.5).abs() < 0.01);

        // The width follows jaPRO's 0.25..=5 clamp.
        let mut thick = Vec::new();
        build_crosshair(&mut thick, crosshair, None, 50.0, width, height);
        let [min_x, _, max_x, _] = pixel_bounds(&thick, width, height);
        assert!((max_x - min_x - 10.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod shadow_mode_tests {
    use super::*;

    #[test]
    fn dynamic_shadow_modes_round_trip_and_keep_old_stencil_configs() {
        for mode in DynamicShadowsMode::ALL {
            assert_eq!(DynamicShadowsMode::from_config(mode.config_value()), Some(mode));
        }
        // `stencil` configs predate the Entity map and must land on it, not Off.
        for legacy in ["stencil", "stencil_legacy", "entity_map"] {
            assert_eq!(DynamicShadowsMode::from_config(legacy), Some(DynamicShadowsMode::EntityMap));
        }
        for source in EntityShadowLight::ALL {
            assert_eq!(EntityShadowLight::from_config(source.config_value()), Some(source));
        }
    }
}
