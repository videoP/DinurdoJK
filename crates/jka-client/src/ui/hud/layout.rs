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
    Score,
    MiniScores,
    EnemyInfo,
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
    /// DinurdoJK `cg_killfeed`: top-right obituary list.
    KillFeed,
}

impl HudElementId {
    // SurfaceInspector is intentionally excluded: the trace panel it used to
    // position is now the clickable `OverlayMode::Trace` egui menu, which
    // isn't a draggable HUD element. The variant, its cvar prefix and its
    // offset field stay (see `element`/`element_mut`) so old configs still load.
    pub const ALL: [Self; 29] = [
        Self::Health,
        Self::Shield,
        Self::Ammo,
        Self::Force,
        Self::Score,
        Self::MiniScores,
        Self::EnemyInfo,
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
        Self::KillFeed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Health => "HEALTH",
            Self::Shield => "SHIELD",
            Self::Ammo => "AMMO / STYLE",
            Self::Force => "FORCE",
            Self::Score => "HUD SCORE",
            Self::MiniScores => "TEAM / DUEL SCORES",
            Self::EnemyInfo => "ENEMY INFO",
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
            Self::KillFeed => "KILL FEED",

        }
    }

    pub fn cvar_name(self) -> &'static str {
        match self {
            Self::Health => "cg_hudHealth",
            Self::Shield => "cg_hudShield",
            Self::Ammo => "cg_hudAmmo",
            Self::Force => "cg_hudForce",
            Self::Score => "cg_hudScore",
            Self::MiniScores => "cg_hudMiniScores",
            Self::EnemyInfo => "cg_hudEnemyInfo",
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
            Self::KillFeed => "cg_hudKillFeed",

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
    pub score: HudElementLayout,
    pub mini_scores: HudElementLayout,
    pub enemy_info: HudElementLayout,
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
    pub kill_feed: HudElementLayout,

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
            score: HUD_UNMOVED,
            mini_scores: HUD_UNMOVED,
            enemy_info: HUD_UNMOVED,
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
            kill_feed: HUD_UNMOVED,

            // These resolve to the exact pre-editor positions at scale 1.0.
            health: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -56.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            shield: HudElementLayout {
                anchor: HudAnchor::BottomLeft,
                offset: [24.0, -24.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            ammo: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -56.0],
                scale: 1.0,
                extent: [1.0, 1.0],
            },
            force: HudElementLayout {
                anchor: HudAnchor::BottomRight,
                offset: [-24.0, -24.0],
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
            HudElementId::Score => self.score,
            HudElementId::MiniScores => self.mini_scores,
            HudElementId::EnemyInfo => self.enemy_info,
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
            HudElementId::KillFeed => self.kill_feed,

        }
    }

    pub fn element_mut(&mut self, id: HudElementId) -> &mut HudElementLayout {
        match id {
            HudElementId::Health => &mut self.health,
            HudElementId::Shield => &mut self.shield,
            HudElementId::Ammo => &mut self.ammo,
            HudElementId::Force => &mut self.force,
            HudElementId::Score => &mut self.score,
            HudElementId::MiniScores => &mut self.mini_scores,
            HudElementId::EnemyInfo => &mut self.enemy_info,
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
            HudElementId::KillFeed => &mut self.kill_feed,

        }
    }

    pub fn reset_element(&mut self, id: HudElementId) {
        *self.element_mut(id) = HudLayout::default().element(id);
    }

    /// Render Studio stores positional offsets in a stable 1280x720 reference
    /// frame. Apply the conversion at the actual offscreen target resolution,
    /// not at the editor size, so preview and rendered output remain aligned.
    pub fn studio_for_frame(self, width:u32, height:u32)->Self {
        let mut scaled=self;
        let sx=width.max(1) as f32/1280.0;
        let sy=height.max(1) as f32/720.0;
        for id in HudElementId::ALL {
            let element=scaled.element_mut(id);
            element.offset[0]*=sx;
            element.offset[1]*=sy;
        }
        scaled
    }

    /// Convert existing gameplay pixel offsets into Studio reference offsets
    /// when the user explicitly chooses Copy gameplay.
    pub fn studio_from_frame(self,width:u32,height:u32)->Self {
        let mut virtual_layout=self;
        let sx=width.max(1) as f32/1280.0;
        let sy=height.max(1) as f32/720.0;
        for id in HudElementId::ALL {
            let element=virtual_layout.element_mut(id);
            element.offset[0]/=sx;
            element.offset[1]/=sy;
        }
        virtual_layout
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
    pub draw_timer: bool,
    /// The independently placed cg_drawScores display (Tayst mode 1/2 vs 3).
    pub mini_scores_mode: i32,
    pub score_offset: [f32; 2],
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
    /// The duel Enemy Info panel is up; the kill feed steps left of it.
    pub enemy_info: bool,
    /// Text rows stacked in the top-right corner; the kill feed hangs below.
    pub corner_rows: u8,
}

impl HudRectContext {
    pub fn new(
        movement_keys: MovementKeysSettings,
        video: &VideoSettings,
        perf: &PerfStats,
        thread_count: usize,
        race: &crate::japro_cg::JaproCgame,
        strafe_helper: crate::ui::StrafeHelperSettings,
        draw_timer: bool,
        score_offset: [f32; 2],
        mini_scores_mode: i32,
    ) -> Self {
        Self {
            movement_keys,
            draw_fps: video.draw_fps,
            draw_timer,
            mini_scores_mode,
            score_offset,
            gpu_enabled: perf.gpu_ms.is_some(),
            cull_debug: video.cull_debug != CullDebugMode::Off,
            thread_count,
            race_timer: [race.race_timer_x, race.race_timer_y, race.race_timer_size],
            race_start: [race.race_start_x, race.race_start_y],
            lagometer: race.lagometer,
            speedometer: race.speedometer,
            strafe_helper,
            enemy_info: false,
            corner_rows: u8::from(video.draw_fps != 0) + u8::from(draw_timer),
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
        let mut context = Self::new(
            ui.movement_keys,
            &ui.video,
            &ui.perf,
            ui.threads.len(),
            &race,
            ui.strafe_helper,
            ui.game_timer.is_some(),
            ui.hud_score.as_ref().map_or([0.0; 2], |score| score.offset),
            match ui.mini_scores.as_ref() {
                Some(crate::ui::UiMiniScores::Team { mode, .. }) => *mode,
                Some(crate::ui::UiMiniScores::Duel { .. }) => 3,
                None => 1,
            },
        );
        context.enemy_info = ui.enemy_info.is_some();
        context.corner_rows = crate::ui::killfeed::corner_rows(ui);
        context
    }
}

/// Stock (untransformed) rectangle of an element that is not anchored. These are
/// the bounds the editor outlines; text elements are sized for typical content.
pub(crate) fn hud_stock_rect(
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
        HudElementId::Score => {
            // TaystJK's score is right-aligned to the left side of the right
            // player HUD. Keep this stock location separate from the editable
            // Force panel so either HUD element can move independently.
            let force = hud_element_rect(HudElementId::Force, HudLayout::default().force, ctx, w, h);
            let ratio = crate::lagometer::width_ratio_coef(w, h);
            let right = force.x - 8.0 * sy + ctx.score_offset[0] * sy * ratio;
            let bottom = force.y + force.height - (7.0 + ctx.score_offset[1]) * sy;
            HudRect { x: right - 122.0*sy, y: bottom - 38.0*sy,
                      width: 122.0*sy, height: 38.0*sy }
        }
        HudElementId::MiniScores => {
            // Match the actual cg_drawScores drawing origin in hud/scores.rs.
            // A separate layout leaves cg_drawScore's Force-adjacent readout alone.
            if ctx.mini_scores_mode == 3 {
                // The centred 2-box and duel layouts are symmetric about x=320.
                virtual_rect(265.0, 8.0, 110.0, 30.0)
            } else {
                let rows = (ctx.draw_fps != 0) as u8 as f32 + ctx.draw_timer as u8 as f32;
                // The upper-right stack uses square HUD glyphs, not a stretched
                // full-screen width. This is the selectable label's typical width.
                let width = 150.0 * sy;
                HudRect {
                    x: (w as f32 - 10.0 - width).max(8.0),
                    y: (12.0 + rows * 22.0) * sy,
                    width,
                    height: 12.0 * sy,
                }
            }
        }
        HudElementId::EnemyInfo => {
            let rows = (ctx.draw_fps != 0) as u8 + ctx.draw_timer as u8;
            // Matches CG_DrawUpperRight stacking. An editor-sized rectangle
            // includes the icon, name, title, optional duel score and bar.
            virtual_rect(500.0, 17.0 + 22.0*rows as f32, 135.0, 112.0)
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
            // The actual speedometer configuration is authored in 640x480
            // coordinates (jaPRO defaults: x=132, y=459). Do not force the
            // HUD editor and both render providers into the screen center.
            let speed = ctx.speedometer;
            HudRect {
                x: speed.x * sx,
                y: (speed.y - 24.0) * sy,
                width: 124.0 * sy,
                height: 24.0 * sy,
            }
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
        // Same column `killfeed::build_killfeed` draws into, six rows tall.
        HudElementId::KillFeed => {
            let [x, y, width, height] =
                crate::ui::killfeed::column_bounds(w, h, ctx.corner_rows, ctx.enemy_info);
            HudRect { x, y, width, height }
        }
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


/// Convert a freeform scale to a scale that makes the displayed rectangle's
/// width OR height an exact grid multiple. These HUD elements use uniform scale
/// (nonuniform scaling would distort fonts/icons), so both dimensions cannot
/// always be multiples simultaneously. Pick the closest achievable scale.
pub fn snap_hud_scale_to_grid(
    id: HudElementId, mut layout: HudElementLayout, requested: f32,
    ctx: &HudRectContext, w: u32, h: u32, grid: f32,
) -> f32 {
    layout.scale = 1.0;
    let base = hud_element_rect(id, layout, ctx, w, h);
    snap_hud_dimensions_scale([base.width, base.height], requested, grid)
}

/// The scale quantizer shared by gameplay and Studio (whose grid is expressed
/// in its 1280x720 reference coordinates rather than the output framebuffer).
pub fn snap_hud_dimensions_scale(dimensions: [f32; 2], requested: f32, grid: f32) -> f32 {
    let mut best = requested.clamp(0.5, 2.0);
    let mut error = f32::INFINITY;
    for dimension in dimensions {
        if dimension < 1.0 || !dimension.is_finite() { continue; }
        let candidate = ((dimension * requested / grid).round().max(1.0) * grid
            / dimension).clamp(0.5, 2.0);
        let candidate_error = (candidate - requested).abs();
        if candidate_error < error {
            error = candidate_error;
            best = candidate;
        }
    }
    best
}

/// Snap the *actual screen rectangle*, not the stored offset. Offsets are
/// relative to different stock positions/anchors and cannot be snapped to a
/// screen-wide grid directly. Neighbor candidates include aligned near edges,
/// matched opposite edges, and touching edges, with each axis independent.
/// Position is returned in the same coordinates as `moving`.
pub fn snap_hud_rect_position(
    moving: HudRect, neighbors: impl IntoIterator<Item = HudRect>,
    grid: f32, threshold: f32, nearby: f32,
) -> ([f32; 2], [bool; 2]) {
    let mut position = [
        (moving.x / grid).round() * grid,
        (moving.y / grid).round() * grid,
    ];
    let mut distances = [threshold, threshold];
    let mut matched = [false, false];
    for neighbor in neighbors {
        if neighbor.width <= 0.0 || neighbor.height <= 0.0 { continue; }
        let gap_x = (neighbor.x - (moving.x + moving.width))
            .max(moving.x - (neighbor.x + neighbor.width)).max(0.0);
        let gap_y = (neighbor.y - (moving.y + moving.height))
            .max(moving.y - (neighbor.y + neighbor.height)).max(0.0);
        // Compare X edges for horizontally adjacent or stacked panels, and Y
        // edges for vertically adjacent or side-by-side panels.
        if gap_y <= nearby {
            for x in [neighbor.x, neighbor.x + neighbor.width - moving.width,
                      neighbor.x - moving.width, neighbor.x + neighbor.width] {
                let diff = (moving.x - x).abs();
                if diff < distances[0] { distances[0] = diff; position[0] = x; matched[0] = true; }
            }
        }
        if gap_x <= nearby {
            for y in [neighbor.y, neighbor.y + neighbor.height - moving.height,
                      neighbor.y - moving.height, neighbor.y + neighbor.height] {
                let diff = (moving.y - y).abs();
                if diff < distances[1] { distances[1] = diff; position[1] = y; matched[1] = true; }
            }
        }
    }
    (position, matched)
}

/// Quantize default scales and position all HUD rectangles on editor entry
/// or reset. Keep custom scales while normalizing their visible screen edges;
/// old archived offsets can otherwise leave the default grid misaligned.
/// The grid is in real screen pixels, not 640x480 stock coordinates.
impl HudLayout {
    pub fn align_default_elements_to_grid(
        &mut self, ctx: &HudRectContext, w: u32, h: u32,
    ) -> bool {
        if !self.snap_to_grid || w == 0 || h == 0 { return false; }
        let defaults = Self::default();
        let grid = self.grid_size.clamp(1.0, 64.0);
        let mut changed = false;
        for id in HudElementId::ALL {
            let original = self.element(id);
            // The preceding release used -58 / -28 for the core bars.
            // Treat those untouched archived offsets as defaults as well.
            let legacy_core = if id.is_anchored() && original.scale == 1.0
                && original.extent == [1.0, 1.0] {
                let old_offset = match id {
                    HudElementId::Health => Some([24.0, -58.0]),
                    HudElementId::Shield => Some([24.0, -28.0]),
                    HudElementId::Ammo => Some([-24.0, -58.0]),
                    HudElementId::Force => Some([-24.0, -28.0]),
                    _ => None,
                };
                old_offset == Some(original.offset) && original.anchor == defaults.element(id).anchor
            } else { false };
            if original != defaults.element(id) && !legacy_core { continue; }
            let mut layout = original;
            layout.scale = snap_hud_scale_to_grid(id, layout, layout.scale, ctx, w, h, grid);
            let rect = hud_element_rect(id, layout, ctx, w, h);
            let (position, _) = snap_hud_rect_position(rect, [], grid, 0.0, 0.0);
            layout.offset[0] += position[0] - rect.x;
            layout.offset[1] += position[1] - rect.y;
            if layout != original {
                *self.element_mut(id) = layout;
                changed = true;
            }
        }
        changed |= self.align_all_positions_to_grid(ctx, w, h);
        changed
    }

    /// Explicit editor reset and entry always normalize every displayed
    /// rectangle's actual position, even when old archived HUD offsets are
    /// present. Snapping only layouts equal to `Default` left existing configs
    /// visibly off-grid after opening HUD Edit. Keep customized scale/size;
    /// RESET ALL starts from defaults so those will be quantized separately.
    pub fn align_all_positions_to_grid(
        &mut self, ctx: &HudRectContext, w: u32, h: u32,
    ) -> bool {
        if !self.snap_to_grid || w == 0 || h == 0 { return false; }
        let grid = self.grid_size.clamp(1.0, 64.0);
        let mut changed = false;
        for id in HudElementId::ALL {
            let original = self.element(id);
            let rect = hud_element_rect(id, original, ctx, w, h);
            if rect.width <= 0.0 || rect.height <= 0.0 { continue; }
            // The grid is screen-origin anchored. Match the upper-left edge,
            // not the anchor-relative offset (which differs per HUD element).
            let dx = (rect.x / grid).round() * grid - rect.x;
            let dy = (rect.y / grid).round() * grid - rect.y;
            if dx.abs() > 0.0001 || dy.abs() > 0.0001 {
                let target = self.element_mut(id);
                target.offset[0] += dx;
                target.offset[1] += dy;
                changed = true;
            }
        }
        changed
    }

    pub fn align_reset_element_to_grid(
        &mut self, id: HudElementId, ctx: &HudRectContext, w: u32, h: u32,
    ) {
        if !self.snap_to_grid { return; }
        let grid = self.grid_size.clamp(1.0, 64.0);
        let mut layout = self.element(id);
        layout.scale = snap_hud_scale_to_grid(id, layout, layout.scale, ctx, w, h, grid);
        let rect = hud_element_rect(id, layout, ctx, w, h);
        let (position, _) = snap_hud_rect_position(rect, [], grid, 0.0, 0.0);
        layout.offset[0] += position[0] - rect.x;
        layout.offset[1] += position[1] - rect.y;
        *self.element_mut(id) = layout;
    }
}

/// Snaps a resize to the dimensions of nearby, currently visible HUD items.
/// Operates in *screen pixels* (or preview points when passed preview bounds),
/// never grid coordinates, so it also works with non-grid-sized neighbors.
/// Axes are independent: e.g. a chat box can match the width of one adjacent
/// item and the height of another. The caller keeps the opposite drag edges fixed.
/// Returns the snapped width/height and whether each axis matched.
pub fn snap_hud_neighbor_size(
    moving: HudRect,
    neighbors: impl IntoIterator<Item = HudRect>,
    threshold: f32,
    nearby: f32,
) -> ([f32; 2], [bool; 2]) {
    let mut result = [moving.width, moving.height];
    let mut distances = [threshold, threshold];
    let mut matched = [false, false];
    for neighbor in neighbors {
        if neighbor.width <= 0.0 || neighbor.height <= 0.0 { continue; }
        // Gaps are zero when the two screen intervals overlap. Only compare
        // sizes of actual neighbors, not arbitrary HUD items far across screen.
        let gap_x = (neighbor.x - moving.x - moving.width)
            .max(moving.x - neighbor.x - neighbor.width).max(0.0);
        let gap_y = (neighbor.y - moving.y - moving.height)
            .max(moving.y - neighbor.y - neighbor.height).max(0.0);
        if gap_x > nearby || gap_y > nearby { continue; }
        for (axis, target) in [neighbor.width, neighbor.height].into_iter().enumerate() {
            // Compare against the unmodified proposed size, so iteration order
            // never determines which of several nearby items wins.
            let proposed = if axis == 0 { moving.width } else { moving.height };
            let diff = (proposed - target).abs();
            if diff < distances[axis] {
                result[axis] = target;
                distances[axis] = diff;
                matched[axis] = true;
            }
        }
    }
    (result, matched)
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
    let base_panel_w = 216.0_f32.min((w as f32 * 0.30).max(160.0));
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

#[cfg(test)]
mod neighbor_resize_tests {
    use super::{snap_hud_neighbor_size, snap_hud_rect_position, HudRect};


    #[test]
    fn snaps_real_edges_to_grid_not_a_stock_relative_offset() {
        let moving=HudRect{x:237.0,y:49.0,width:233.0,height:41.0};
        let (position,matched)=snap_hud_rect_position(moving,[],8.0,12.0,32.0);
        assert_eq!(position,[240.0,48.0]);
        assert_eq!(matched,[false,false]);
    }

    #[test]
    fn touching_neighbors_override_grid_and_align_independent_axes() {
        let moving=HudRect{x:238.0,y:55.0,width:220.0,height:24.0};
        let neighbor=HudRect{x:20.0,y:56.0,width:216.0,height:24.0};
        let (position,matched)=snap_hud_rect_position(moving,[neighbor],8.0,12.0,32.0);
        assert_eq!(position,[236.0,56.0]);
        assert_eq!(matched,[true,true]);
    }

    #[test]
    fn distant_neighbors_cannot_pull_placement_off_grid() {
        let moving=HudRect{x:238.0,y:55.0,width:220.0,height:24.0};
        let neighbor=HudRect{x:20.0,y:480.0,width:216.0,height:24.0};
        assert_eq!(snap_hud_rect_position(moving,[neighbor],8.0,12.0,32.0).0,
                   [240.0,56.0]);
    }

    #[test]
    fn matches_both_dimensions_of_touching_neighbors() {
        let moving=HudRect{x:100.0,y:100.0,width:197.0,height:49.0};
        let matching=HudRect{x:300.0,y:95.0,width:200.0,height:50.0};
        assert_eq!(snap_hud_neighbor_size(moving,[matching],8.0,32.0),
            ([200.0,50.0],[true,true]));
    }

    #[test]
    fn does_not_snap_to_distant_matching_panel() {
        let moving=HudRect{x:100.0,y:100.0,width:197.0,height:49.0};
        let far=HudRect{x:1000.0,y:700.0,width:200.0,height:50.0};
        assert_eq!(snap_hud_neighbor_size(moving,[far],8.0,32.0),
            ([197.0,49.0],[false,false]));
    }

    #[test]
    fn each_axis_chooses_closest_eligible_size_independently() {
        let moving=HudRect{x:100.0,y:100.0,width:197.0,height:49.0};
        let a=HudRect{x:298.0,y:100.0,width:200.0,height:55.0};
        let b=HudRect{x:99.0,y:151.0,width:210.0,height:50.0};
        assert_eq!(snap_hud_neighbor_size(moving,[a,b],12.0,32.0),
            ([200.0,50.0],[true,true]));
    }
}
