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
    MapEdit,
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrosshairSettings {
    /// 0 disables the crosshair. Non-zero values select a local HUD shape.
    pub style: u8,
    /// JKA-compatible cg_crosshairSize value. 24 preserves the stock default.
    pub size: f32,
    /// RGBA, matching TaystJK's cg_crosshairColor 0..255 convention.
    pub color: [u8; 4],
}

impl Default for CrosshairSettings {
    fn default() -> Self {
        Self {
            style: 1,
            size: 24.0,
            // Keep DinurdoJK's existing white crosshair by default while using
            // TaystJK's four-component cg_crosshairColor storage convention.
            color: [255, 255, 255, 230],
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudElementId {
    Health,
    Shield,
    Ammo,
    Force,
}

impl HudElementId {
    pub const ALL: [Self; 4] = [Self::Health, Self::Shield, Self::Ammo, Self::Force];

    pub fn label(self) -> &'static str {
        match self {
            Self::Health => "HEALTH",
            Self::Shield => "SHIELD",
            Self::Ammo => "AMMO",
            Self::Force => "FORCE",
        }
    }

    pub fn cvar_name(self) -> &'static str {
        match self {
            Self::Health => "cg_hudHealth",
            Self::Shield => "cg_hudShield",
            Self::Ammo => "cg_hudAmmo",
            Self::Force => "cg_hudForce",
        }
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
    pub snap_to_grid: bool,
    pub grid_size: f32,
}

impl Default for HudLayout {
    fn default() -> Self {
        Self {
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
        }
    }

    pub fn element_mut(&mut self, id: HudElementId) -> &mut HudElementLayout {
        match id {
            HudElementId::Health => &mut self.health,
            HudElementId::Shield => &mut self.shield,
            HudElementId::Ammo => &mut self.ammo,
            HudElementId::Force => &mut self.force,
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

pub fn hud_element_rect(
    id: HudElementId,
    layout: HudElementLayout,
    w: u32,
    h: u32,
) -> HudRect {
    let base_panel_w = 218.0_f32.min((w as f32 * 0.30).max(160.0));
    let base_size = match id {
        HudElementId::Health | HudElementId::Shield | HudElementId::Ammo | HudElementId::Force => {
            [base_panel_w, 24.0]
        }
    };
    let scale = layout.scale.clamp(0.5, 2.0);
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
pub const SHELPER_CENTER: u32 = 1 << 11;
pub const SHELPER_S: u32 = 1 << 15;
pub const SHELPER_SA: u32 = 1 << 16;
pub const SHELPER_SD: u32 = 1 << 17;
pub const SHELPER_TINY: u32 = 1 << 18;
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementHudState {
    pub forward_move: i8,
    pub right_move: i8,
    pub up_move: i8,
    pub buttons: i32,
    pub velocity: [f32; 3],
    pub view_yaw: f32,
    pub player_speed: f32,
    pub grounded: bool,
    pub fov_x: f32,
}

impl Default for MovementHudState {
    fn default() -> Self {
        Self {
            forward_move: 0, right_move: 0, up_move: 0, buttons: 0,
            velocity: [0.0; 3], view_yaw: 0.0, player_speed: 250.0, grounded: false, fov_x: 90.0,
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
pub enum PvsMode {
    Off,
    Minimal,
    Full,
    #[default]
    Auto,
    Auto2,
    Auto3,
    Auto4,
}

impl PvsMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Minimal => "MINIMAL",
            Self::Full => "FULL",
            Self::Auto => "AUTO",
            Self::Auto2 => "AUTO 2",
            Self::Auto3 => "AUTO 3",
            Self::Auto4 => "AUTO 4",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Full => "full",
            Self::Auto => "auto",
            Self::Auto2 => "auto2",
            Self::Auto3 => "auto3",
            Self::Auto4 => "auto4",
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
}

impl ColorLutPreset {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::KodakVision3_250d,
        Self::KodakPortra400,
        Self::FujiEterna500,
        Self::FujiVelvia50,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::KodakVision3_250d => "KODAK VISION3 250D",
            Self::KodakPortra400 => "KODAK PORTRA 400",
            Self::FujiEterna500 => "FUJI ETERNA 500",
            Self::FujiVelvia50 => "FUJI VELVIA 50",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::KodakVision3_250d => "kodak_vision3_250d",
            Self::KodakPortra400 => "kodak_portra_400",
            Self::FujiEterna500 => "fuji_eterna_500",
            Self::FujiVelvia50 => "fuji_velvia_50",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "kodak_vision3_250d" | "vision3_250d" | "kodak250d" => Some(Self::KodakVision3_250d),
            "kodak_portra_400" | "portra_400" | "portra400" => Some(Self::KodakPortra400),
            "fuji_eterna_500" | "eterna_500" | "eterna500" => Some(Self::FujiEterna500),
            "fuji_velvia_50" | "velvia_50" | "velvia50" => Some(Self::FujiVelvia50),
            _ => None,
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
        Self::Legacy,
        Self::Vertex,
        Self::ClusteredLite,
        Self::PerPixelForwardPlus,
        Self::RayTracedHardware,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Legacy => "LEGACY",
            Self::Vertex => "VERTEX",
            Self::ClusteredLite => "CLUSTERED LITE",
            Self::PerPixelForwardPlus => "PER-PIXEL (FORWARD+)",
            Self::RayTracedHardware => "RAY TRACED (HARDWARE) [WIP]",
        }
    }

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
    BlobStencilLegacy,
    CascadedShadowMaps,
    CascadedShadowMapsBevy,
    RayTraced,
}

impl DynamicShadowsMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::BlobStencilLegacy,
        Self::CascadedShadowMaps,
        Self::CascadedShadowMapsBevy,
        Self::RayTraced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::BlobStencilLegacy => "BLOB / STENCIL (LEGACY) [WIP]",
            Self::CascadedShadowMaps => "CASCADED SHADOW MAPS (CSM)",
            Self::CascadedShadowMapsBevy => "CASCADED SHADOW MAPS (BEVY)",
            Self::RayTraced => "RAY TRACED SHADOWS [WIP]",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::BlobStencilLegacy => "blob_stencil",
            Self::CascadedShadowMaps => "csm",
            Self::CascadedShadowMapsBevy => "csm_bevy",
            Self::RayTraced => "ray_traced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Self::Off),
            "blob" | "stencil" | "blob_stencil" | "1" => Some(Self::BlobStencilLegacy),
            "csm" | "cascaded" | "cascaded_shadow_maps" | "2" => Some(Self::CascadedShadowMaps),
            "csm_bevy" | "bevy_csm" | "cascaded_shadow_maps_bevy" => Some(Self::CascadedShadowMapsBevy),
            "ray_traced" | "raytraced" | "3" | "4" => Some(Self::RayTraced),
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
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::LegacyDrawFog1 => "legacy1",
            Self::LegacyDrawFog2 => "legacy2",
            Self::Volumetric => "volumetric",
        }
    }

    pub fn is_legacy(self) -> bool {
        matches!(self, Self::LegacyDrawFog1 | Self::LegacyDrawFog2)
    }

    pub fn drawfog_value(self) -> u8 {
        match self {
            Self::LegacyDrawFog1 => 1,
            Self::LegacyDrawFog2 => 2,
            _ => 0,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectionQuality {
    /// No enhanced reflections and no authored tcGen environment stages.
    Off,
    /// Vanilla/authored tcGen environment stages only; no enhanced reflection path.
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

    /// Runtime reflection-technique tier. Off and Legacy deliberately share the
    /// exact same zero-cost shader/runtime policy; their only difference is the
    /// map-load material specialization that strips tcGen environment stages in
    /// Off mode.
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
            Self::Off | Self::Legacy | Self::Low | Self::Medium => 0,
            Self::High => 1,
            Self::Ultra => 4,
        }
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
    pub wireframe_mask: u32,
    pub skip_ui: bool,
    pub pvs_mode: PvsMode,
    pub fps_cap: u32,
    /// Continuous projectile FX sampling rate. `0` preserves legacy JKA's
    /// render-frame-driven behavior; non-zero values are fixed Hz.
    pub fx_fps: u32,
    pub draw_fps: u8,
    pub developer_tools: bool,
    pub perf_trace: bool,
    pub gpu_timings: bool,
    pub ghoul2_skinning: Ghoul2SkinningMode,
    pub ghoul2_early_cull: bool,
    pub ghoul2_lod_bias: i32,
    pub ghoul2_batch_draws: Ghoul2BatchMode,
    pub physics_msec: u32,
    pub input_subframe: bool,
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
    pub footprints: FootprintMode,
    pub grass: bool,
    /// Runtime A/B diagnostic: hoist root wind/clump work to a compute pass.
    pub grass_precompute: bool,
    /// Runtime A/B diagnostic: use the 5-triangle middle blade LOD.
    pub grass_mid_lod: bool,
    /// Runtime A/B diagnostic: order opaque grass near-to-far for early-Z.
    pub grass_front_to_back: bool,
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
    pub local_light_shadows: bool,
    pub cascaded_shadows: bool,
    pub cull_debug: CullDebugMode,
    pub planar_reflection_debug: PlanarReflectionDebugMode,
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
            wireframe_mask: 0,
            skip_ui: false,
            pvs_mode: PvsMode::Auto,
            fps_cap: 0,
            fx_fps: crate::fx::FX_FPS_DEFAULT,
            draw_fps: 1,
            developer_tools: false,
            perf_trace: false,
            gpu_timings: false,
            ghoul2_skinning: Ghoul2SkinningMode::Gpu,
            ghoul2_early_cull: true,
            ghoul2_lod_bias: 0,
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            physics_msec: 8,
            input_subframe: false,
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
            footprints: FootprintMode::ThreeD,
            grass: true,
            grass_precompute: true,
            grass_mid_lod: true,
            grass_front_to_back: true,
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
            map_light_simulation: false,
            world_lighting: true,
            vertex_lighting: false,
            lightmap_only: false,
            modern_sabers: false,
            saber_marks: SaberMarkMode::Legacy,
            pbr: true,
            allow_asset_overrides: true,
            gen_normal_maps: false,
            deluxe_mapping: true,
            deluxe_specular: 1.0,
            emissive_area_lights: false,
            voxel_probe_gi: false,
            dynamic_shadows: DynamicShadowsMode::Off,
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
    pub client_entity_present_ms: f64,
    pub client_player_present_ms: f64,
    pub client_followed_player_ms: f64,
    pub client_fx_ms: f64,
    pub client_fx_tessellate_ms: f64,
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
    /// it to resolve mod-provided levelshots before falling back to the generic
    /// splash image.
    pub active_game_dir: Option<String>,
    pub preparation_finished: bool,
    pub bars: Vec<MapLoadingBar>,
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
    pub chat_mode: ChatMode,
    pub chat_input: String,
    pub chat_lines: Vec<UiChatLine>,
    pub center_print: Option<UiCenterPrint>,
    pub follow_name: Option<String>,
    pub scoreboard: Option<UiScoreboard>,
    pub demo_timeline: Option<DemoTimelineUi>,
    pub hud: Option<HudState>,
    pub hud_layout: HudLayout,
    pub crosshair: CrosshairSettings,
    pub movement_keys: MovementKeysSettings,
    pub strafe_helper: StrafeHelperSettings,
    pub movement_hud: MovementHudState,
    pub video: VideoSettings,
    pub perf: PerfStats,
    pub threads: [ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
    pub surface_inspector: Option<crate::runtime::SurfaceInspectorInfo>,
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
            chat_mode: ChatMode::Global,
            chat_input: String::new(),
            chat_lines: Vec::new(),
            center_print: None,
            follow_name: None,
            scoreboard: None,
            demo_timeline: None,
            hud: None,
            hud_layout: HudLayout::default(),
            crosshair: CrosshairSettings::default(),
            movement_keys: MovementKeysSettings::default(),
            strafe_helper: StrafeHelperSettings::default(),
            movement_hud: MovementHudState::default(),
            video: VideoSettings::default(),
            perf: PerfStats::default(),
            threads: [ThreadPerfStats::default(); crate::thread_activity::SLOT_COUNT],
            surface_inspector: None,
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
    if matches!(ui.mode, OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::MapEdit) {
        build_follow_indicator(&mut out, ui, small_font, width, height);
        build_scoreboard(&mut out, ui, width, height);
    }
    if ui.video.reflection_debug {
        build_reflection_debug_legend(&mut out, ui, width, height);
    }
    match ui.mode {
        OverlayMode::None | OverlayMode::Chat => {}
        OverlayMode::Console => build_console(&mut out, ui, width, height),
        // The Game/Video menus are drawn by egui; see `app::egui_menu`.
        OverlayMode::Video | OverlayMode::Game | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::MapEdit => {}
    }
    if matches!(ui.mode, OverlayMode::None | OverlayMode::Game | OverlayMode::Video) {
        if let Some(info) = &ui.surface_inspector {
            build_surface_inspector(&mut out, info, width, height);
        }
    }
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
        && matches!(ui.mode, OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::MapEdit)
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
    build_crosshair(out, ui.crosshair, width, height);
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
    build_movement_keys(out, ui, width, height);
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
        OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::MapEdit
    );
    if gameplay_overlay {
        build_chat_history(out, ui, small_font, width, height);
        build_center_print(out, ui, small_font, width, height);
        build_demo_timeline(out, ui, width, height);
    }
    // FPS/perf is intentionally visible above menus too, matching the old
    // retained path and submit_ui_overlay ordering.
    match ui.video.draw_fps {
        0 => {}
        1 => build_fps_simple(out, ui, width, height),
        _ => build_perf(out, ui, width, height),
    }
    if ui.mode == OverlayMode::Chat {
        // The input box is anchored to the oldest visible chat line, so keep
        // it in the same transient batch as chat history. This lets fades/new
        // messages reposition it without rebuilding the retained UI.
        build_chat_input(out, ui, small_font, width, height);
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
    let center_y = 480.0 * 0.30 * y_scale;

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
    let Some(name) = ui.follow_name.as_deref() else { return };
    if name.is_empty() {
        return;
    }

    let label = format!("^7FOLLOWING ^2{name}");
    let y = 72.0 * h as f32 / 480.0;
    if let Some(font) = small_font {
        let scale = 0.72;
        let virtual_width = proportional_text_width(&label, font, scale);
        let x = ((640.0 - virtual_width) * 0.5) * w as f32 / 640.0;
        proportional_text(
            out,
            &label,
            font,
            x,
            y,
            scale,
            [1.0, 1.0, 1.0, 1.0],
            true,
            w,
            h,
        );
        return;
    }

    let glyph_w = w as f32 * (8.0 / 640.0);
    let glyph_h = h as f32 * (12.0 / 480.0);
    let advance = w as f32 * (8.0 / 640.0);
    let x = (w as f32 - visible_jka_chars(&label) as f32 * advance) * 0.5;
    fixed_charset_text(
        out,
        &label,
        x,
        y,
        glyph_w,
        glyph_h,
        advance,
        [1.0, 1.0, 1.0, 1.0],
        true,
        w,
        h,
    );
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

fn build_scoreboard(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(board) = &ui.scoreboard else { return };

    // This is intentionally a renderer-native scoreboard rather than a menu:
    // holding +scores must work during normal live play without changing input
    // capture or entering an egui overlay.
    const BOARD_SCALE: f32 = 1.75;
    let base_width = (w as f32 * 0.70).clamp(500.0, 780.0);
    let width = (base_width * BOARD_SCALE).min(w as f32 - 20.0);
    let x = ((w as f32 - width) * 0.5).max(10.0);
    let y = (h as f32 * 0.07).max(18.0);
    let row_h = 19.0 * BOARD_SCALE;
    let header_h = 54.0 * BOARD_SCALE;
    let available_rows = (((h as f32 - y - 10.0 - header_h).max(row_h) / row_h).floor() as usize).max(1);
    let shown = board.entries.len().min(available_rows);
    let height = (header_h + shown as f32 * row_h).min(h as f32 - y - 10.0);

    rect(out, x, y, width, height, [0.0, 0.0, 0.0, 0.82], w, h);
    rect_outline(out, x, y, width, height, 1.0, [0.55, 0.58, 0.64, 0.92], w, h);

    let title = if board.team_game {
        format!("SCOREBOARD     ^1RED {}     ^4BLUE {}", board.team_scores[0], board.team_scores[1])
    } else {
        "SCOREBOARD".to_owned()
    };
    text(
        out,
        &title,
        x + 14.0 * BOARD_SCALE,
        y + 9.0 * BOARD_SCALE,
        1.10 * BOARD_SCALE,
        [0.92, 0.94, 0.98, 1.0],
        w,
        h,
    );

    let header_y = y + 31.0 * BOARD_SCALE;
    let header_scale = 0.96 * BOARD_SCALE;
    text(out, "NAME", x + 14.0 * BOARD_SCALE, header_y, header_scale, [0.70, 0.74, 0.80, 1.0], w, h);
    text(out, "SCORE", x + width - 238.0 * BOARD_SCALE, header_y, header_scale, [0.70, 0.74, 0.80, 1.0], w, h);
    text(out, "PING", x + width - 167.0 * BOARD_SCALE, header_y, header_scale, [0.70, 0.74, 0.80, 1.0], w, h);
    text(out, "TIME", x + width - 108.0 * BOARD_SCALE, header_y, header_scale, [0.70, 0.74, 0.80, 1.0], w, h);
    if board.team_game {
        text(out, "TEAM", x + width - 52.0 * BOARD_SCALE, header_y, header_scale, [0.70, 0.74, 0.80, 1.0], w, h);
    }

    for (index, entry) in board.entries.iter().take(shown).enumerate() {
        let row_y = y + 49.0 * BOARD_SCALE + index as f32 * row_h;
        if index % 2 == 0 {
            rect(out, x + 5.0, row_y - 3.0 * BOARD_SCALE, width - 10.0, row_h, [1.0, 1.0, 1.0, 0.035], w, h);
        }
        let name = truncate_jka_text(&entry.name, if board.team_game { 24 } else { 30 });
        let row_scale = BOARD_SCALE;
        text(out, &name, x + 14.0 * BOARD_SCALE, row_y, row_scale, [0.92, 0.92, 0.92, 1.0], w, h);
        text(out, &entry.score.to_string(), x + width - 238.0 * BOARD_SCALE, row_y, row_scale, [0.92, 0.92, 0.92, 1.0], w, h);
        let ping = if entry.ping < 0 { "CNCT".to_owned() } else { entry.ping.to_string() };
        text(out, &ping, x + width - 167.0 * BOARD_SCALE, row_y, row_scale, [0.92, 0.92, 0.92, 1.0], w, h);
        text(out, &entry.time.to_string(), x + width - 108.0 * BOARD_SCALE, row_y, row_scale, [0.92, 0.92, 0.92, 1.0], w, h);
        if board.team_game {
            let team = match entry.team {
                1 => "^1RED",
                2 => "^4BLUE",
                3 => "SPEC",
                _ => "FREE",
            };
            text(out, team, x + width - 52.0 * BOARD_SCALE, row_y, 0.92 * BOARD_SCALE, [0.86, 0.88, 0.92, 1.0], w, h);
        }
    }

    if board.entries.len() > shown {
        text(
            out,
            &format!("+{} MORE", board.entries.len() - shown),
            x + 14.0 * BOARD_SCALE,
            y + height - 14.0 * BOARD_SCALE,
            0.82 * BOARD_SCALE,
            [0.65, 0.68, 0.74, 1.0],
            w,
            h,
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

fn build_background_progress(out: &mut Vec<UiVertex>, progress: &MapLoadingBar, w: u32, h: u32) {
    let panel_w = 320.0;
    let panel_h = 44.0;
    let x = (w as f32 - panel_w - 18.0).max(8.0);
    let y = (h as f32 - panel_h - 18.0).max(8.0);
    rect(out, x, y, panel_w, panel_h, [0.015, 0.020, 0.028, 0.90], w, h);
    let fraction = if progress.total > 0 {
        (progress.completed as f32 / progress.total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    text(
        out,
        progress.label,
        x + 12.0,
        y + 7.0,
        1.2,
        [0.82, 0.88, 0.94, 1.0],
        w,
        h,
    );
    let bar_x = x + 104.0;
    let bar_y = y + 11.0;
    let bar_w = panel_w - 154.0;
    rect(out, bar_x, bar_y, bar_w, 10.0, [0.08, 0.10, 0.13, 0.96], w, h);
    if fraction > 0.0 {
        rect(
            out,
            bar_x,
            bar_y,
            bar_w * fraction,
            10.0,
            [0.39, 0.62, 0.84, 1.0],
            w,
            h,
        );
    }
    text(
        out,
        &format!("{}%", (fraction * 100.0).round() as u32),
        x + panel_w - 42.0,
        y + 7.0,
        1.15,
        [0.68, 0.73, 0.80, 1.0],
        w,
        h,
    );
}

fn build_loading_screen(
    out: &mut Vec<UiVertex>,
    loading: &MapLoadingUi,
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

    let panel_w = (w as f32 - 48.0).clamp(360.0, 760.0);
    let row_h = 31.0;
    let visible_bar_count = loading
        .bars
        .iter()
        .filter(|bar| !(bar.skipped || (loading.preparation_finished && bar.total == 0)))
        .count();
    let panel_h = 84.0 + visible_bar_count as f32 * row_h + 26.0;
    let x = (w as f32 - panel_w) * 0.5;
    let y = (h as f32 - panel_h - 34.0).max(24.0);
    rect(out, x, y, panel_w, panel_h, [0.015, 0.020, 0.028, 0.88], w, h);
    rect(out, x, y, 4.0, panel_h, [0.39, 0.62, 0.84, 1.0], w, h);

    text(
        out,
        &format!("LOADING {}", loading.map_name.to_ascii_uppercase()),
        x + 22.0,
        y + 18.0,
        2.0,
        [0.95, 0.97, 1.0, 1.0],
        w,
        h,
    );

    let bar_x = x + 174.0;
    let bar_w = panel_w - 270.0;
    for (index, bar) in loading
        .bars
        .iter()
        .filter(|bar| !(bar.skipped || (loading.preparation_finished && bar.total == 0)))
        .enumerate()
    {
        let row_y = y + 58.0 + index as f32 * row_h;
        let done = bar.total > 0 && bar.completed >= bar.total;
        let fraction = if bar.total > 0 {
            (bar.completed as f32 / bar.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        text(
            out,
            bar.label,
            x + 22.0,
            row_y + 5.0,
            1.35,
            [0.78, 0.84, 0.91, 1.0],
            w,
            h,
        );
        rect(out, bar_x, row_y + 5.0, bar_w, 12.0, [0.08, 0.10, 0.13, 0.96], w, h);
        if fraction > 0.0 {
            rect(
                out,
                bar_x,
                row_y + 5.0,
                bar_w * fraction,
                12.0,
                if done {
                    [0.36, 0.72, 0.48, 1.0]
                } else {
                    [0.39, 0.62, 0.84, 1.0]
                },
                w,
                h,
            );
        }
        let status = if bar.total == 0 {
            "WAIT".to_string()
        } else if done {
            "DONE".to_string()
        } else {
            format!("{}/{}", bar.completed, bar.total)
        };
        text(
            out,
            &status,
            x + panel_w - 76.0,
            row_y + 5.0,
            1.25,
            if done {
                [0.55, 0.88, 0.64, 1.0]
            } else {
                [0.68, 0.73, 0.80, 1.0]
            },
            w,
            h,
        );
    }
}

fn build_surface_inspector(
    out: &mut Vec<UiVertex>,
    info: &crate::runtime::SurfaceInspectorInfo,
    w: u32,
    h: u32,
) {
    let panel_w = (w as f32 * 0.42).clamp(500.0, 760.0);
    let max_lines = ((h as f32 - 190.0) / 24.0).floor().max(8.0) as usize;
    let visible_lines = info.lines.len().min(max_lines);
    let panel_h = 92.0 + visible_lines as f32 * 24.0;
    let x = (w as f32 - panel_w - 24.0).max(24.0);
    let y = 86.0;

    rect(
        out,
        x,
        y,
        panel_w,
        panel_h,
        [0.015, 0.025, 0.038, 0.97],
        w,
        h,
    );
    rect(out, x, y, 4.0, panel_h, [0.92, 0.72, 0.20, 1.0], w, h);
    text(
        out,
        "SURFACE INSPECTOR",
        x + 20.0,
        y + 18.0,
        2.2,
        [0.98, 0.92, 0.78, 1.0],
        w,
        h,
    );
    text(
        out,
        "CTRL+C COPY",
        x + panel_w - 150.0,
        y + 22.0,
        1.25,
        [0.64, 0.72, 0.80, 1.0],
        w,
        h,
    );
    text(
        out,
        &info.title,
        x + 20.0,
        y + 48.0,
        1.65,
        [0.72, 0.86, 1.0, 1.0],
        w,
        h,
    );
    for (index, line) in info.lines.iter().take(max_lines).enumerate() {
        text(
            out,
            line,
            x + 20.0,
            y + 78.0 + index as f32 * 24.0,
            1.35,
            [0.90, 0.93, 0.96, 1.0],
            w,
            h,
        );
    }
}

fn build_hud(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else {
        return;
    };

    let health_layout = ui.hud_layout.health;
    let health = hud_element_rect(HudElementId::Health, health_layout, w, h);
    hud_meter(
        out,
        health.x,
        health.y,
        health.width,
        health.height,
        health_layout.scale,
        "HEALTH",
        hud.health,
        hud.max_health.max(1),
        [0.92, 0.28, 0.25, 0.95],
        w,
        h,
    );

    let shield_layout = ui.hud_layout.shield;
    let shield = hud_element_rect(HudElementId::Shield, shield_layout, w, h);
    hud_meter(
        out,
        shield.x,
        shield.y,
        shield.width,
        shield.height,
        shield_layout.scale,
        "SHIELD",
        hud.armor,
        hud.max_health.max(1),
        [0.30, 0.58, 1.0, 0.95],
        w,
        h,
    );

    let ammo_layout = ui.hud_layout.ammo;
    let ammo = hud_element_rect(HudElementId::Ammo, ammo_layout, w, h);
    let ammo_text = hud
        .ammo
        .map_or_else(|| "--".to_owned(), |value| value.max(0).to_string());
    hud_value_panel(
        out,
        ammo.x,
        ammo.y,
        ammo.width,
        ammo.height,
        ammo_layout.scale,
        "AMMO",
        &ammo_text,
        [0.96, 0.78, 0.30, 0.95],
        w,
        h,
    );

    let force_layout = ui.hud_layout.force;
    let force = hud_element_rect(HudElementId::Force, force_layout, w, h);
    hud_meter(
        out,
        force.x,
        force.y,
        force.width,
        force.height,
        force_layout.scale,
        "FORCE",
        hud.force_power,
        hud.force_power_max.max(1),
        [0.38, 0.82, 1.0, 0.95],
        w,
        h,
    );
}

#[allow(clippy::too_many_arguments)]
fn hud_meter(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
    label: &str,
    value: i32,
    maximum: i32,
    accent: [f32; 4],
    w: u32,
    h: u32,
) {
    let scale = scale.clamp(0.5, 2.0);
    rect(out, x, y, width, height, [0.01, 0.018, 0.028, 0.76], w, h);
    let fraction = (value.max(0) as f32 / maximum.max(1) as f32).clamp(0.0, 1.0);
    rect(
        out,
        x + 2.0 * scale,
        y + height - 4.0 * scale,
        (width - 4.0 * scale) * fraction,
        2.0 * scale,
        accent,
        w,
        h,
    );
    text(
        out,
        label,
        x + 8.0 * scale,
        y + 6.0 * scale,
        1.35 * scale,
        [0.72, 0.78, 0.84, 1.0],
        w,
        h,
    );
    let value_text = value.max(0).to_string();
    let value_width = value_text.len() as f32 * 8.1 * scale;
    text(
        out,
        &value_text,
        x + width - value_width - 8.0 * scale,
        y + 5.0 * scale,
        1.55 * scale,
        [0.96, 0.98, 1.0, 1.0],
        w,
        h,
    );
}

#[allow(clippy::too_many_arguments)]
fn hud_value_panel(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
    label: &str,
    value: &str,
    accent: [f32; 4],
    w: u32,
    h: u32,
) {
    let scale = scale.clamp(0.5, 2.0);
    rect(out, x, y, width, height, [0.01, 0.018, 0.028, 0.76], w, h);
    rect(
        out,
        x + 2.0 * scale,
        y + height - 4.0 * scale,
        width - 4.0 * scale,
        2.0 * scale,
        accent,
        w,
        h,
    );
    text(
        out,
        label,
        x + 8.0 * scale,
        y + 6.0 * scale,
        1.35 * scale,
        [0.72, 0.78, 0.84, 1.0],
        w,
        h,
    );
    let value_width = value.len() as f32 * 8.1 * scale;
    text(
        out,
        value,
        x + width - value_width - 8.0 * scale,
        y + 5.0 * scale,
        1.55 * scale,
        [0.96, 0.98, 1.0, 1.0],
        w,
        h,
    );
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


fn movement_key_cell(
    out: &mut Vec<UiVertex>, label: &str, active: bool, x: f32, y: f32, size: f32,
    w: u32, h: u32,
) {
    let background = if active { [0.08, 0.42, 0.12, 0.88] } else { [0.02, 0.025, 0.035, 0.62] };
    rect(out, x, y, size, size, background, w, h);
    rect_outline(out, x, y, size, size, 1.0, [0.72, 0.78, 0.88, 0.72], w, h);
    let scale = (size / 16.0).clamp(0.55, 1.5);
    let label_w = label.len() as f32 * 6.0 * scale;
    text(out, label, x + (size - label_w) * 0.5, y + (size - 8.0 * scale) * 0.5, scale,
        [0.96, 0.98, 1.0, if active { 1.0 } else { 0.72 }], w, h);
}

fn build_movement_keys(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let settings = ui.movement_keys;
    if settings.mode == 0 { return; }
    let state = ui.movement_hud;
    let base_scale = (h as f32 / 480.0).clamp(0.65, 2.5);
    let size_mul = settings.size.clamp(0.25, 4.0);
    let (tile, origin_x, origin_y, compact) = match settings.mode {
        1 => {
            let tile = 16.0 * base_scale * size_mul;
            let center_shift = if settings.walk { 1.0 } else { 1.5 };
            (tile,
             w as f32 * 0.5 + settings.x * base_scale - tile * center_shift,
             h as f32 * 0.90 + settings.y * base_scale - tile,
             false)
        }
        2 => {
            let tile = 16.0 * base_scale * size_mul;
            let center_shift = if settings.walk { 1.5 } else { 2.0 };
            (tile,
             w as f32 * 0.5 + settings.x * base_scale - tile * center_shift,
             h as f32 * 0.90 + settings.y * base_scale - tile,
             false)
        }
        3 => {
            let tile = 6.0 * base_scale * size_mul;
            (tile, w as f32 * 0.5 - tile * 1.5, h as f32 * 0.5 - tile * 1.5, true)
        }
        _ => {
            let tile = 12.0 * base_scale * size_mul;
            (tile, w as f32 * 0.5 + settings.x * base_scale - tile * 1.5,
             h as f32 * 0.90 + settings.y * base_scale - tile * 1.5, true)
        }
    };

    let f = state.forward_move > 0;
    let b = state.forward_move < 0;
    let l = state.right_move < 0;
    let r = state.right_move > 0;
    let jump = state.up_move > 0;
    let crouch = state.up_move < 0;
    let attack = state.buttons & jka_movement::BUTTON_ATTACK != 0;
    let alt = state.buttons & jka_movement::BUTTON_ALT_ATTACK != 0;
    let walking = state.buttons & jka_movement::BUTTON_WALKING != 0;

    let draw = |out: &mut Vec<UiVertex>, label: &str, active: bool, col: f32, row: f32| {
        if compact && !active { return; }
        movement_key_cell(out, label, active, origin_x + col * tile, origin_y + row * tile, tile - 1.0, w, h);
    };
    if compact {
        // TaystJK modes 3/4 only draw active cells in this 3x3 layout.
        draw(out, "J", jump, 0.0, 0.0);
        draw(out, "C", crouch, 2.0, 0.0);
        draw(out, "W", f, 1.0, 0.0);
        draw(out, "A", l, 0.0, 1.0);
        draw(out, "D", r, 2.0, 1.0);
        draw(out, "S", b, 1.0, 2.0);
        draw(out, "M1", attack, 0.0, 2.0);
        draw(out, "M2", alt, 2.0, 2.0);
        if settings.walk { draw(out, "WALK", walking, -1.0, 2.0); }
    } else {
        // TaystJK modes 1/2 keep off-state cells visible.
        draw(out, "J", jump, 0.0, 0.0);
        draw(out, "C", crouch, 2.0, 0.0);
        draw(out, "W", f, 1.0, 0.0);
        draw(out, "A", l, 0.0, 1.0);
        draw(out, "S", b, 1.0, 1.0);
        draw(out, "D", r, 2.0, 1.0);
        if settings.mode == 2 {
            draw(out, "M1", attack, 3.0, 0.0);
            draw(out, "M2", alt, 3.0, 1.0);
        }
        if settings.walk { draw(out, "WALK", walking, -1.0, 1.0); }
    }
}
fn normalize_degrees(mut angle: f32) -> f32 {
    while angle > 180.0 { angle -= 360.0; }
    while angle < -180.0 { angle += 360.0; }
    angle
}

fn strafe_angle_to_screen_x(target_yaw: f32, view_yaw: f32, cg_fov: f32, w: u32, h: u32) -> Option<f32> {
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    let base = cg_fov.clamp(1.0, 140.0).to_radians();
    let actual_fov = 2.0 * ((base * 0.5).tan() * 0.75 * aspect).atan();
    let delta = normalize_degrees(target_yaw - view_yaw).to_radians();
    if delta.abs() >= actual_fov * 0.5 { return None; }
    let focal = (w as f32 * 0.5) / (actual_fov * 0.5).tan();
    Some(w as f32 * 0.5 - delta.tan() * focal)
}

fn build_strafe_helper(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let sh = ui.strafe_helper;
    if sh.flags & SHELPER_STYLE_MASK == 0 { return; }
    let state = ui.movement_hud;
    let vx = state.velocity[0];
    let vy = state.velocity[1];
    let speed = (vx * vx + vy * vy).sqrt();
    if speed < 1.0 { return; }

    // Stock JKA air-CGAZ core: pm_airaccelerate=1.0 and full-input
    // wishspeed=ps.speed. This is the same CGAZ_Opt expression TaystJK uses;
    // its extended movement-style physics can be layered onto this state later.
    let fps = if sh.fps >= 1.0 {
        sh.fps.clamp(1.0, 1000.0)
    } else if ui.video.fps_cap >= 1 {
        (ui.video.fps_cap as f32).clamp(1.0, 1000.0)
    } else {
        125.0
    };
    let wishspeed = state.player_speed.max(1.0);
    let frametime = 1.0 / fps;
    let friction_speed = if state.grounded {
        (speed - speed.max(100.0) * 6.0 * frametime).max(0.0)
    } else {
        speed
    };
    if friction_speed <= f32::EPSILON { return; }
    let acceleration = if state.grounded { 10.0 } else { 1.0 };
    let accel = wishspeed * acceleration * frametime;
    let argument = ((wishspeed - accel) / friction_speed).clamp(-1.0, 1.0);
    let mut optimum = argument.acos().to_degrees() - 45.0;
    if !optimum.is_finite() || optimum < 0.0 { optimum = 0.0; }
    optimum += sh.offset * 0.01;

    let vel_yaw = vy.atan2(vx).to_degrees();
    let fm = state.forward_move.signum();
    let rm = state.right_move.signum();
    let active_for = |forward: i8, right: i8| fm == forward && rm == right;
    let center_active = fm == 0;
    let dirs = [
        // TaystJK draws both optimum W lines because pure forward can accelerate
        // toward either side of the velocity vector.
        (SHELPER_W, 45.0 + optimum, 1, 0, [1.0, 0.75, 0.0, 0.75]),
        (SHELPER_W, -45.0 - optimum, 1, 0, [1.0, 0.75, 0.0, 0.75]),
        (SHELPER_WA, optimum, 1, -1, [1.0, 1.0, 1.0, 0.75]),
        (SHELPER_WD, -optimum, 1, 1, [1.0, 1.0, 1.0, 0.75]),
        (SHELPER_A, -45.0 + optimum, 0, -1, [0.5, 1.0, 1.0, 0.75]),
        (SHELPER_D, 45.0 - optimum, 0, 1, [0.5, 1.0, 1.0, 0.75]),
        (SHELPER_SA, -90.0 + optimum, -1, -1, [0.75, 0.0, 1.0, 0.75]),
        (SHELPER_SD, 90.0 - optimum, -1, 1, [0.75, 0.0, 1.0, 0.75]),
        (SHELPER_S, 225.0 + optimum, -1, 0, [1.0, 0.75, 0.0, 0.75]),
    ];

    let active_color = [
        sh.active_color[0] as f32 / 255.0, sh.active_color[1] as f32 / 255.0,
        sh.active_color[2] as f32 / 255.0, sh.active_color[3] as f32 / 255.0,
    ];
    let inactive_alpha = sh.inactive_alpha as f32 / 255.0;
    let width = sh.line_width.clamp(0.25, 5.0) * (h as f32 / 480.0).clamp(0.75, 2.5);
    let cutoff = sh.cutoff.clamp(0.0, 480.0) * h as f32 / 480.0;
    let draw_line = |out: &mut Vec<UiVertex>, x: f32, active: bool, mut color: [f32; 4]| {
        if active { color = active_color; } else { color[3] = inactive_alpha; }
        if sh.flags & SHELPER_CGAZ != 0 {
            let half = if sh.flags & SHELPER_TINY != 0 { 5.0 } else { (20.0 - sh.cutoff / 16.0).max(5.0) };
            hud_line(out, x, h as f32 * 0.5 - half, x, h as f32 * 0.5 + half, width, color, w, h);
        }
        if sh.flags & SHELPER_UPDATED != 0 {
            hud_line(out, w as f32 * 0.5, h as f32 - cutoff, x, h as f32 * 0.5 - 10.0, width, color, w, h);
        }
        if sh.flags & SHELPER_ORIGINAL != 0 {
            hud_line(out, w as f32 * 0.5, h as f32 * 0.5, x, h as f32 * 0.5, width, color, w, h);
        }
    };

    for (flag, angle, df, dr, color) in dirs {
        if sh.flags & flag == 0 { continue; }
        if let Some(x) = strafe_angle_to_screen_x(vel_yaw + angle, state.view_yaw, state.fov_x, w, h) {
            draw_line(out, x, active_for(df, dr), color);
        }
    }
    if sh.flags & SHELPER_CENTER != 0 {
        if let Some(x) = strafe_angle_to_screen_x(vel_yaw, state.view_yaw, state.fov_x, w, h) {
            draw_line(out, x, center_active, [1.0, 0.75, 0.0, 0.75]);
        }
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

fn build_crosshair(out: &mut Vec<UiVertex>, crosshair: CrosshairSettings, w: u32, h: u32) {
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
    let c = [
        f32::from(crosshair.color[0]) / 255.0,
        f32::from(crosshair.color[1]) / 255.0,
        f32::from(crosshair.color[2]) / 255.0,
        f32::from(crosshair.color[3]) / 255.0,
    ];

    let hbar = |out: &mut Vec<UiVertex>, x: f32, y: f32, width: f32| {
        rect(out, x, y - thickness * 0.5, width, thickness, c, w, h);
    };
    let vbar = |out: &mut Vec<UiVertex>, x: f32, y: f32, height: f32| {
        rect(out, x - thickness * 0.5, y, thickness, height, c, w, h);
    };

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

fn build_perf(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let p = ui.perf;
    let margin = 14.0;
    let panel_w = 900.0_f32.min((w as f32 - margin * 2.0).max(620.0));
    let x = (w as f32 - panel_w - margin).max(margin);
    let y = margin;
    let debug_culling = ui.video.cull_debug != CullDebugMode::Off;
    let gpu_enabled = p.gpu_ms.is_some();
    let header_h = 38.0;
    let cpu_h = 78.0;
    let gpu_h = if gpu_enabled { 112.0 } else { 42.0 };
    let client_h = 126.0;
    let input_h = 108.0;
    let thread_header_h = 28.0;
    let thread_row_h = 32.0;
    let footer_h = if debug_culling { 42.0 } else { 0.0 };
    let panel_h = 12.0
        + header_h
        + cpu_h
        + gpu_h
        + client_h
        + input_h
        + thread_header_h
        + ui.threads.len() as f32 * thread_row_h
        + footer_h
        + 18.0;
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
        next_y + 94.0,
        0.92,
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
    let thread_bar_x = x + 100.0;
    let thread_bar_w = 320.0;
    for (row, thread) in ui.threads.into_iter().enumerate() {
        let ty = next_y + thread_header_h + row as f32 * thread_row_h;
        let name = compact_thread_name(thread.name);
        let (bar_color, task_color) = if thread.active {
            ([0.34, 0.94, 0.48, 1.0], [0.34, 0.94, 0.48, 1.0])
        } else if thread.task == "IDLE" {
            ([0.34, 0.39, 0.44, 1.0], [0.54, 0.60, 0.66, 1.0])
        } else {
            ([0.40, 0.68, 0.92, 1.0], [0.70, 0.84, 0.98, 1.0])
        };

        text(out, name, x + 12.0, ty + 3.0, 1.18, body_color, w, h);
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
            thread_bar_x + thread_bar_w + 14.0,
            ty + 3.0,
            1.12,
            body_color,
            w,
            h,
        );
        text(
            out,
            thread.task,
            thread_bar_x + thread_bar_w + 100.0,
            ty + 3.0,
            1.04,
            task_color,
            w,
            h,
        );
    }

    if debug_culling {
        let cy = next_y + thread_header_h + ui.threads.len() as f32 * thread_row_h + 4.0;
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

pub fn console_visible_line_capacity(h: u32, size: ConsoleSize) -> usize {
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = 76.0;
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
    let first_y = 76.0_f64;
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

fn build_console(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let ph = console_panel_height(h, ui.console_size);
    rect(out, 0.0, 0.0, w as f32, ph, [0.015, 0.02, 0.03, 0.94], w, h);
    rect(
        out,
        0.0,
        ph - 2.0,
        w as f32,
        2.0,
        [0.38, 0.48, 0.62, 0.9],
        w,
        h,
    );
    text(
        out,
        "JKA CLIENT CONSOLE",
        20.0,
        18.0,
        3.0,
        [0.82, 0.90, 1.0, 1.0],
        w,
        h,
    );
    if ui.console_search_open {
        let field_x = 20.0;
        let field_y = 44.0;
        let field_w = (w as f32 * 0.48).clamp(300.0, 640.0);
        rect(
            out,
            field_x,
            field_y,
            field_w,
            28.0,
            [0.025, 0.035, 0.052, 0.98],
            w,
            h,
        );
        rect(
            out,
            field_x,
            field_y + 26.0,
            field_w,
            2.0,
            [0.90, 0.66, 0.18, 0.95],
            w,
            h,
        );
        let max_query_chars = ((field_w - 72.0) / CONSOLE_CHAR_WIDTH) as usize;
        let query = tail_chars(&ui.console_search_query, max_query_chars.max(1));
        console_text(
            out,
            &format!("FIND: {query}_"),
            field_x + 8.0,
            field_y + 6.0,
            [1.0, 1.0, 1.0, 1.0],
            w,
            h,
        );

        let counter = if ui.console_search_query.is_empty() {
            "TYPE TO SEARCH".to_owned()
        } else if ui.console_search_total == 0 {
            "NO MATCHES".to_owned()
        } else if w < 1100 {
            format!(
                "{} / {}   ENTER NEXT   ESC CLOSE",
                ui.console_search_index.unwrap_or(0) + 1,
                ui.console_search_total
            )
        } else {
            format!(
                "{} / {}   ENTER NEXT   SHIFT+ENTER PREV   ESC CLOSE",
                ui.console_search_index.unwrap_or(0) + 1,
                ui.console_search_total
            )
        };
        let counter_w = counter.chars().count() as f32 * CONSOLE_CHAR_WIDTH;
        console_text(
            out,
            &counter,
            (w as f32 - 20.0 - counter_w).max(field_x + field_w + 12.0),
            field_y + 6.0,
            [0.86, 0.76, 0.55, 1.0],
            w,
            h,
        );
    } else {
        text(
            out,
            "LEFT/RIGHT EDIT   CTRL+LEFT/RIGHT WORD   HOME/END   TAB COMPLETE   UP/DOWN HISTORY   CTRL+F FIND",
            20.0,
            52.0,
            2.0,
            [0.58, 0.70, 0.82, 1.0],
            w,
            h,
        );
    }
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = 76.0;
    let max_lines = (((input_y - first_y - 8.0) / line_step).floor().max(0.0)) as usize;
    let history_end = ui.console_lines.len().saturating_sub(ui.console_scroll);
    let history_start = history_end.saturating_sub(max_lines);
    let mut line_y = first_y;
    for (line_index, line) in ui.console_lines[history_start..history_end]
        .iter()
        .enumerate()
    {
        let absolute_line = history_start + line_index;
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
                        [0.20, 0.42, 0.66, 0.62],
                        w,
                        h,
                    );
                }
            }
        }
        console_text(out, line, 20.0, line_y, [0.82, 0.84, 0.88, 1.0], w, h);
        line_y += line_step;
    }
    if ui.console_lines.is_empty() && !ui.console_status.is_empty() {
        console_text(
            out,
            &ui.console_status,
            20.0,
            first_y,
            [0.78, 0.82, 0.72, 1.0],
            w,
            h,
        );
    }
    let cursor = ui.console_cursor.min(ui.console_input.len());
    let cursor = if ui.console_input.is_char_boundary(cursor) { cursor } else { ui.console_input.len() };
    let before = &ui.console_input[..cursor];
    let after = &ui.console_input[cursor..];
    console_text(
        out,
        &format!("] {before}_{after}"),
        20.0,
        input_y,
        [1.0; 4],
        w,
        h,
    );
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

fn proportional_text_width(value: &str, font: &ProportionalFont, scale: f32) -> f32 {
    let bytes = value.as_bytes();
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
    value: &str,
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
        let bytes = value.as_bytes();
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
                },
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
}
