//! Hud layout.
use crate::ui::{
    movement_keys_origin, perf_panel_rect, surface_inspector_frame, CullDebugMode,
    MovementKeysSettings, PerfStats, UiSnapshot, UiVertex, VideoSettings, CHATBOX_CUTOFF,
};

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
    Lagometer,
    SurfaceInspector,
    Speedometer,
    SpeedometerJumps,
    SpeedGraph,
    StrafeGuide,
    AccelMeter,
    VelocityCompass,
    GroundSpeed,
    JumpHeight,
    JumpDistance,
    VerticalSpeed,
    YawSpeed,
}

impl HudElementId {
    // SurfaceInspector is intentionally excluded: the trace panel it used to
    // position is now the clickable `OverlayMode::Trace` egui menu, which
    // isn't a draggable HUD element. The variant, its cvar prefix and its
    // offset field stay (see `element`/`element_mut`) so old configs still load.
    pub const ALL: [Self; 25] = [
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
        Self::Lagometer,
        Self::Speedometer,
        Self::SpeedometerJumps,
        Self::SpeedGraph,
        Self::StrafeGuide,
        Self::AccelMeter,
        Self::VelocityCompass,
        Self::GroundSpeed,
        Self::JumpHeight,
        Self::JumpDistance,
        Self::VerticalSpeed,
        Self::YawSpeed,
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
            Self::Lagometer => "LAGOMETER",
            Self::SurfaceInspector => "TRACE INSPECTOR",
            Self::Speedometer => "SPEEDOMETER",
            Self::SpeedometerJumps => "SPEEDOMETER JUMPS",
            Self::SpeedGraph => "SPEED GRAPH",
            Self::StrafeGuide => "STRAFE GUIDE",
            Self::AccelMeter => "ACCELERATION METER",
            Self::VelocityCompass => "VELOCITY COMPASS",
            Self::GroundSpeed => "PRE-SPEED",
            Self::JumpHeight => "JUMP HEIGHT",
            Self::JumpDistance => "JUMP DISTANCE",
            Self::VerticalSpeed => "VERTICAL SPEED",
            Self::YawSpeed => "YAW SPEED",

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
            Self::Lagometer => "cg_hudLagometer",
            Self::SurfaceInspector => "cg_hudSurfaceInspector",
            Self::Speedometer => "cg_hudSpeedometer",
            Self::SpeedometerJumps => "cg_hudSpeedometerJumps",
            Self::SpeedGraph => "cg_hudSpeedGraph",
            Self::StrafeGuide => "cg_hudStrafeGuide",
            Self::AccelMeter => "cg_hudAccelMeter",
            Self::VelocityCompass => "cg_hudVelocityCompass",
            Self::GroundSpeed => "cg_hudGroundSpeed",
            Self::JumpHeight => "cg_hudJumpHeight",
            Self::JumpDistance => "cg_hudJumpDistance",
            Self::VerticalSpeed => "cg_hudVerticalSpeed",
            Self::YawSpeed => "cg_hudYawSpeed",

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

    pub(in crate::ui) fn screen_fraction(self) -> [f32; 2] {
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

    pub(in crate::ui) fn element_fraction(self) -> [f32; 2] {
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
    /// Independent bounds size. Currently used by the chat box so resizing its
    /// area changes wrapping/capacity without stretching the glyphs.
    pub extent: [f32; 2],
}

impl HudElementLayout {
    pub fn to_config(self) -> String {
        format!(
            "{} {:.3} {:.3} {:.3} {:.3} {:.3}",
            self.anchor.short_name(),
            self.offset[0],
            self.offset[1],
            self.scale,
            self.extent[0],
            self.extent[1]
        )
    }

    pub fn from_config(value: &str) -> Option<Self> {
        let mut words = value.split_whitespace();
        let anchor = HudAnchor::from_config(words.next()?)?;
        let x = words.next()?.parse::<f32>().ok()?;
        let y = words.next()?.parse::<f32>().ok()?;
        let scale = words.next()?.parse::<f32>().ok()?;
        let extent_x = words
            .next()
            .map(str::parse::<f32>)
            .transpose()
            .ok()?
            .unwrap_or(1.0);
        let extent_y = words
            .next()
            .map(str::parse::<f32>)
            .transpose()
            .ok()?
            .unwrap_or(1.0);
        if words.next().is_some()
            || !x.is_finite()
            || !y.is_finite()
            || !scale.is_finite()
            || !extent_x.is_finite()
            || !extent_y.is_finite()
        {
            return None;
        }
        Some(Self {
            anchor,
            offset: [x.clamp(-16384.0, 16384.0), y.clamp(-16384.0, 16384.0)],
            scale: scale.clamp(0.5, 2.0),
            extent: [extent_x.clamp(0.25, 4.0), extent_y.clamp(0.25, 8.0)],
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
    pub lagometer: HudElementLayout,
    pub surface_inspector: HudElementLayout,
    pub speedometer: HudElementLayout,
    pub speedometer_jumps: HudElementLayout,
    pub speed_graph: HudElementLayout,
    pub strafe_guide: HudElementLayout,
    pub accel_meter: HudElementLayout,
    pub velocity_compass: HudElementLayout,
    pub ground_speed: HudElementLayout,
    pub jump_height: HudElementLayout,
    pub jump_distance: HudElementLayout,
    pub vertical_speed: HudElementLayout,
    pub yaw_speed: HudElementLayout,

    pub snap_to_grid: bool,
    pub grid_size: f32,
}

/// Layout of an element that has no anchor of its own: no offset, stock scale.
pub(in crate::ui) const HUD_UNMOVED: HudElementLayout = HudElementLayout {
    anchor: HudAnchor::TopLeft,
    offset: [0.0, 0.0],
    scale: 1.0,
    extent: [1.0, 1.0],
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
            lagometer: HUD_UNMOVED,
            surface_inspector: HUD_UNMOVED,
            speedometer: HUD_UNMOVED,
            speedometer_jumps: HUD_UNMOVED,
            speed_graph: HUD_UNMOVED,
            strafe_guide: HUD_UNMOVED,
            accel_meter: HUD_UNMOVED,
            velocity_compass: HUD_UNMOVED,
            ground_speed: HUD_UNMOVED,
            jump_height: HUD_UNMOVED,
            jump_distance: HUD_UNMOVED,
            vertical_speed: HUD_UNMOVED,
            yaw_speed: HUD_UNMOVED,

            // These resolve to the exact pre-editor positions at scale 1.0.
            health: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -58.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            shield: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -28.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            ammo: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -58.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            force: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -28.0],
                scale: 1.0,
                extent: [1.0, 1.0],
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
            HudElementId::Lagometer => self.lagometer,
            HudElementId::SurfaceInspector => self.surface_inspector,
            HudElementId::Speedometer => self.speedometer,
            HudElementId::SpeedometerJumps => self.speedometer_jumps,
            HudElementId::SpeedGraph => self.speed_graph,
            HudElementId::StrafeGuide => self.strafe_guide,
            HudElementId::AccelMeter => self.accel_meter,
            HudElementId::VelocityCompass => self.velocity_compass,
            HudElementId::GroundSpeed => self.ground_speed,
            HudElementId::JumpHeight => self.jump_height,
            HudElementId::JumpDistance => self.jump_distance,
            HudElementId::VerticalSpeed => self.vertical_speed,
            HudElementId::YawSpeed => self.yaw_speed,

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
            HudElementId::Lagometer => &mut self.lagometer,
            HudElementId::SurfaceInspector => &mut self.surface_inspector,
            HudElementId::Speedometer => &mut self.speedometer,
            HudElementId::SpeedometerJumps => &mut self.speedometer_jumps,
            HudElementId::SpeedGraph => &mut self.speed_graph,
            HudElementId::StrafeGuide => &mut self.strafe_guide,
            HudElementId::AccelMeter => &mut self.accel_meter,
            HudElementId::VelocityCompass => &mut self.velocity_compass,
            HudElementId::GroundSpeed => &mut self.ground_speed,
            HudElementId::JumpHeight => &mut self.jump_height,
            HudElementId::JumpDistance => &mut self.jump_distance,
            HudElementId::VerticalSpeed => &mut self.vertical_speed,
            HudElementId::YawSpeed => &mut self.yaw_speed,

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
    pub lagometer: crate::lagometer::Settings,
    pub speedometer: crate::speedometer::Settings,
    pub strafe_helper: crate::ui::StrafeHelperSettings,
}

impl HudRectContext {
    pub fn new(
        movement_keys: MovementKeysSettings,
        video: &VideoSettings,
        perf: &PerfStats,
        thread_count: usize,
        race: &crate::japro_cg::JaproCgame,
        strafe_helper: crate::ui::StrafeHelperSettings,
    ) -> Self {
        Self {
            movement_keys,
            draw_fps: video.draw_fps,
            gpu_enabled: perf.gpu_ms.is_some(),
            cull_debug: video.cull_debug != CullDebugMode::Off,
            thread_count,
            race_timer: [race.race_timer_x, race.race_timer_y, race.race_timer_size],
            race_start: [race.race_start_x, race.race_start_y],
            lagometer: race.lagometer,
            speedometer: race.speedometer,
            strafe_helper,
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
        race.speedometer = ui.speedometer_settings;
        race.lagometer = ui.lagometer_settings;
        Self::new(
            ui.movement_keys,
            &ui.video,
            &ui.perf,
            ui.threads.len(),
            &race,
            ui.strafe_helper,
        )
    }
}

/// Stock (untransformed) rectangle of an element that is not anchored. These are
/// the bounds the editor outlines; text elements are sized for typical content.
pub(in crate::ui) fn hud_stock_rect(
    id: HudElementId,
    ctx: &HudRectContext,
    w: u32,
    h: u32,
) -> HudRect {
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
                return HudRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                };
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
                perf_panel_rect(w, h, ctx.gpu_enabled, ctx.cull_debug, ctx.thread_count);
            HudRect {
                x,
                y,
                width,
                height,
            }
        }
        HudElementId::Fps => {
            let width = 7.0 * 6.0 * 1.55;
            HudRect {
                x: (w as f32 - 12.0 - width).max(12.0),
                y: 12.0,
                width,
                height: 13.0,
            }
        }
        HudElementId::Chat => virtual_rect(30.0, 373.0, CHATBOX_CUTOFF, 52.0),
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
        HudElementId::Lagometer => {
            let lag = ctx.lagometer;
            let coef = crate::lagometer::width_ratio_coef(w, h);
            let x = 640.0 - lag.x as f32 * coef;
            let y = 480.0 - lag.y as f32 - 16.0;
            let height = if lag.mode == 4 { 60.0 } else { 48.0 };
            virtual_rect(x - coef, y, 49.0 * coef, height)
        }
        HudElementId::Speedometer => {
            HudRect { x:320.0*sx-62.0*sy, y:200.0*sy, width:124.0*sy, height:30.0*sy }
        }
        HudElementId::SpeedometerJumps => {
            let s = &ctx.speedometer;
            let rows = ((s.jumps.clamp(1, 511) + 4) / 5) as f32;
            virtual_rect(s.jumps_x, s.jumps_y + 96.0, 260.0, 20.0 * rows)
        }
        HudElementId::SpeedGraph => {
            use crate::speedometer::flag;
            let flags = ctx.speedometer.flags;
            if flags & flag::SPEEDGRAPHOLD != 0 && flags & flag::SPEEDGRAPH == 0 {
                let coef = crate::lagometer::width_ratio_coef(w, h);
                let (x, y) = crate::lagometer::speed_graph_origin(&ctx.lagometer, coef);
                let framed = matches!(ctx.lagometer.mode, 1 | 2);
                virtual_rect(x - coef, if framed {y} else {y-96.0}, 49.0*coef, if framed {48.0} else {144.0})
            } else {
                virtual_rect(245.0, 368.0, 150.0, 22.0)
            }
        }
        HudElementId::StrafeGuide => {
            let flags = ctx.strafe_helper.flags;
            if flags & crate::ui::SHELPER_WARSOW_HUD != 0 { virtual_rect(80.0, 168.0, 480.0, 20.0) }
            else if flags & crate::ui::SHELPER_XONOTIC != 0 { virtual_rect(192.0, 260.0, 256.0, 26.0) }
            else if flags & crate::ui::SHELPER_Q3DF != 0 { virtual_rect(0.0, 268.0, 640.0, 12.0) }
            else { virtual_rect(0.0, 240.0, 640.0, 240.0) }
        }
        HudElementId::VelocityCompass => virtual_rect(0.0, 288.0, 640.0, 32.0),
        HudElementId::AccelMeter if ctx.strafe_helper.flags & crate::ui::SHELPER_Q3DF_ACCEL != 0 => virtual_rect(210.0, 232.0, 220.0, 16.0),
        HudElementId::AccelMeter | HudElementId::GroundSpeed | HudElementId::JumpHeight
        | HudElementId::JumpDistance | HudElementId::VerticalSpeed | HudElementId::YawSpeed => {
            let [x,y] = crate::speedometer::component_origin(id, &ctx.speedometer);
            virtual_rect(x-2.0, y-14.0, if id == HudElementId::AccelMeter {42.0} else {70.0}, 20.0)
        }
        HudElementId::SurfaceInspector => {
            let (x, y, width) = surface_inspector_frame(w);
            HudRect {
                x,
                y,
                width,
                height: 300.0_f32.min((h as f32 - y - 18.0).max(0.0)),
            }
        }
        HudElementId::Health | HudElementId::Shield | HudElementId::Ammo | HudElementId::Force => {
            HudRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            }
        }
    }
}

/// Scale a drawn element about the centre of its stock rectangle, then shift it.
pub(in crate::ui) fn apply_hud_layout(
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
    // Chat's authored rectangle is bottom-left anchored: history grows upward from
    // CHATBOX_Y. Its editor rectangle uses the same anchor so scaling the chat
    // cannot visually drift outside the selected bounds. Other HUD elements keep
    // their historical centre-based scale behavior.
    let (anchor_x, anchor_y) = if id == HudElementId::Chat {
        (stock.x, stock.y + stock.height)
    } else {
        (stock.x + stock.width * 0.5, stock.y + stock.height * 0.5)
    };
    let (wf, hf) = (w as f32, h as f32);
    for vertex in vertices {
        let px = (vertex.position[0] + 1.0) * 0.5 * wf;
        let py = (1.0 - vertex.position[1]) * 0.5 * hf;
        let px = anchor_x + (px - anchor_x) * scale + layout.offset[0];
        let py = anchor_y + (py - anchor_y) * scale + layout.offset[1];
        vertex.position = [px / wf * 2.0 - 1.0, 1.0 - py / hf * 2.0];
    }
}

/// Run `draw` and position whatever it emitted according to element `id`'s layout.
pub(in crate::ui) fn draw_placed(
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
        let extent = if id == HudElementId::Chat {
            layout.extent
        } else {
            [1.0, 1.0]
        };
        let (width, height) = (
            stock.width * scale * extent[0],
            stock.height * scale * extent[1],
        );
        if id == HudElementId::Chat {
            return HudRect {
                x: stock.x + layout.offset[0],
                y: stock.y + stock.height + layout.offset[1] - height,
                width,
                height,
            };
        }
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
