//! Strafehelper line geometry, ported from jaPRO/TaystJK `hud_strafehelper.c`
//! (`DF_StrafeHelper`, `DF_GetLine`, `DF_SetAngleToX`, `DF_DrawStrafeLine`,
//! `CGAZ_*`, and the per-movement-style physics getters).
//!
//! Everything here is pure: the caller hands over a [`MovementHudState`] and
//! gets back line segments in the 640x480 virtual HUD space that `cg_draw`
//! uses, so the math can be unit tested without a renderer. `ui.rs` maps the
//! segments to pixels.
//!
//! Intentional differences from the C:
//! * `DF_DrawLine` stamps a chain of `size` x `size` quads per line; we emit
//!   segments and let the caller draw one quad each.
//! * `SHELPER_CENTER` is honoured. In the C the `KEY_CENTER` flag check sits
//!   inside `if (moveDir != KEY_CENTER)` and can never run, so the center line
//!   ignores its own option and the rear center line is drawn at 0 degrees.
//! * `DF_IsSlickSurf` also traces for `SURF_SLICK` under the player; the HUD
//!   has no collision access, so only the style/knockback/dash causes of
//!   "on slick" are modelled.
//! * Vehicles, the WSW/Weze/sound/accel-meter/zone add-ons are not ported.

use crate::ui::{
    MovementHudState, StrafeHelperSettings, SHELPER_A, SHELPER_CENTER, SHELPER_CGAZ, SHELPER_D, SHELPER_MAX,
    SHELPER_ORIGINAL, SHELPER_REAR, SHELPER_S, SHELPER_SA, SHELPER_SD, SHELPER_TINY, SHELPER_UPDATED, SHELPER_W, SHELPER_WA,
    SHELPER_WD,
};

/// `SCREEN_WIDTH` / `SCREEN_HEIGHT`: the virtual space every coordinate here uses.
pub const SCREEN_WIDTH: f32 = 640.0;
pub const SCREEN_HEIGHT: f32 = 480.0;
/// `state.strafeHelper.LINE_HEIGHT`: the screen midpoint.
const LINE_HEIGHT: f32 = 0.5 * SCREEN_HEIGHT;

/// `BUTTON_DASH` / `BUTTON_WALKING` from q_shared.h.
const BUTTON_DASH: i32 = 8192;
const BUTTON_WALKING: i32 = 16;

/// `movementStyle_e` (bg_public.h, with `_SPPHYSICS` and `_COOP` on).
pub mod mv {
    pub const SIEGE: i32 = 0;
    pub const JKA: i32 = 1;
    pub const QW: i32 = 2;
    pub const CPM: i32 = 3;
    pub const Q3: i32 = 4;
    pub const PJK: i32 = 5;
    pub const WSW: i32 = 6;
    pub const RJQ3: i32 = 7;
    pub const RJCPM: i32 = 8;
    pub const JETPACK: i32 = 10;
    pub const SP: i32 = 12;
    pub const SLICK: i32 = 13;
    pub const BOTCPM: i32 = 14;
    pub const OCPM: i32 = 16;
    pub const TRIBES: i32 = 17;
    pub const SURF: i32 = 18;
}

// bg_pmove.c constants.
const PM_STOPSPEED: f32 = 100.0;
const PM_FRICTION: f32 = 6.0;
const PM_VQ3_FRICTION: f32 = 8.0;
const PM_QW_FRICTION: f32 = 4.0;
const PM_QW_AIRSTRAFEWISHSPEED: f32 = 30.0;
const PM_SP_AIRDECELRATE: f32 = 1.35;
const PM_TRIBES_AIRFRICTION: f32 = 1.9;
const PM_TRIBES_GROUNDACCELERATE: f32 = 5.0;
const PM_TRIBES_GROUNDFRICTION: f32 = 0.5;
const PM_SURF_WISHSPEED: f32 = 250.0;
const PM_AIRACCELERATE: f32 = 1.0;

/// `state.cgaz.wasOnGround`: whether the previous pmove step ended on the
/// ground. Keyed by `commandTime` so repeated HUD snapshots within one step
/// (or several renders per step) agree with each other.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroundTracker {
    command_time: Option<i32>,
    grounded: bool,
    was_grounded: bool,
}

impl GroundTracker {
    pub fn update(&mut self, command_time: i32, grounded: bool) -> bool {
        if self.command_time != Some(command_time) {
            self.was_grounded = self.grounded;
            self.command_time = Some(command_time);
        }
        self.grounded = grounded;
        self.was_grounded
    }
}

/// A strafe line in virtual HUD space. `size` is the `DF_DrawLine` square size
/// (the line width in virtual units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrafeSegment {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub size: f32,
    pub color: [f32; 4],
}

/// The eight key combinations, in `KEY_*` order (odd entries are two keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    W,
    Wa,
    A,
    As,
    S,
    Sd,
    D,
    Dw,
}

impl Key {
    const ALL: [Key; 8] = [Key::W, Key::Wa, Key::A, Key::As, Key::S, Key::Sd, Key::D, Key::Dw];

    fn from_axes(forward: i32, right: i32) -> Option<Key> {
        Some(match (forward.signum(), right.signum()) {
            (1, 0) => Key::W,
            (1, -1) => Key::Wa,
            (0, -1) => Key::A,
            (-1, -1) => Key::As,
            (-1, 0) => Key::S,
            (-1, 1) => Key::Sd,
            (0, 1) => Key::D,
            (1, 1) => Key::Dw,
            _ => return None,
        })
    }

    /// `DF_DirToCmd` (axes only; upmove comes from the real command).
    fn axes(self) -> (i32, i32) {
        match self {
            Key::W => (127, 0),
            Key::Wa => (127, -127),
            Key::A => (0, -127),
            Key::As => (-127, -127),
            Key::S => (-127, 0),
            Key::Sd => (-127, 127),
            Key::D => (0, 127),
            Key::Dw => (127, 127),
        }
    }

    fn flag(self) -> u32 {
        match self {
            Key::W => SHELPER_W,
            Key::Wa => SHELPER_WA,
            Key::A => SHELPER_A,
            Key::As => SHELPER_SA,
            Key::S => SHELPER_S,
            Key::Sd => SHELPER_SD,
            Key::D => SHELPER_D,
            Key::Dw => SHELPER_WD,
        }
    }

    /// Yaw offset from the velocity direction for an angular `delta` (degrees),
    /// front line or rear (opposite-side) line.
    fn angle(self, rear: bool, delta: f32) -> f32 {
        match (self, rear) {
            (Key::W, false) => 45.0 + delta,
            (Key::W, true) => -45.0 - delta,
            (Key::Wa, false) => delta,
            (Key::Wa, true) => -90.0 - delta,
            (Key::A, false) => -45.0 + delta,
            (Key::A, true) => 225.0 - delta,
            (Key::As, false) => -90.0 + delta,
            (Key::As, true) => 180.0 - delta,
            (Key::S, false) => 225.0 + delta,
            (Key::S, true) => -225.0 - delta,
            (Key::Sd, false) => 90.0 - delta,
            (Key::Sd, true) => 180.0 + delta,
            (Key::D, false) => 45.0 - delta,
            (Key::D, true) => 135.0 + delta,
            (Key::Dw, false) => -delta,
            (Key::Dw, true) => 90.0 + delta,
        }
    }

    /// `DF_SetLineColor` inactive palette.
    fn color(self) -> [f32; 4] {
        match self {
            Key::Wa | Key::Dw => [1.0, 1.0, 1.0, 0.75],
            Key::A | Key::D => [0.5, 1.0, 1.0, 0.75],
            Key::W | Key::S => [1.0, 0.75, 0.0, 0.75],
            Key::As | Key::Sd => [0.75, 0.0, 1.0, 0.75],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gaz {
    Opt,
    Max,
}

#[derive(Debug, Clone, Copy)]
struct Cmd {
    forward: i32,
    right: i32,
    up: i32,
}

/// Movement-style physics constants (`dfstyle`, the `DF_Get*` helpers).
struct Physics {
    duckscale: f32,
    accelerate: f32,
    airaccelerate: f32,
    friction: f32,
    airstopaccelerate: f32,
    airstrafewishspeed: f32,
    airstrafeaccelerate: f32,
    has_air_control: bool,
}

impl Physics {
    fn for_style(style: i32) -> Self {
        let cpm_family = matches!(style, mv::CPM | mv::OCPM | mv::RJCPM | mv::BOTCPM);
        // Styles with air control: A/D turning in the air accelerates hard.
        let air_control = cpm_family || matches!(style, mv::PJK | mv::WSW | mv::SLICK);
        Self {
            duckscale: match style {
                mv::Q3 | mv::RJQ3 => 0.25,
                mv::WSW => 0.3125,
                _ => 0.5,
            },
            accelerate: match style {
                _ if cpm_family => 15.0,
                mv::WSW | mv::SP | mv::SURF => 12.0,
                mv::SLICK => 30.0,
                _ => 10.0,
            },
            airaccelerate: match style {
                _ if air_control => 1.0,
                mv::SP => 4.0,
                mv::QW => 0.7,
                mv::SURF => 100.0,
                mv::TRIBES => 0.25,
                _ => PM_AIRACCELERATE,
            },
            friction: match style {
                mv::CPM | mv::RJCPM | mv::BOTCPM | mv::WSW | mv::SLICK => PM_VQ3_FRICTION,
                _ => PM_FRICTION,
            },
            airstopaccelerate: if air_control { 2.5 } else { 0.0 },
            airstrafewishspeed: if air_control { 30.0 } else { 0.0 },
            airstrafeaccelerate: match style {
                mv::SLICK => 100.0,
                mv::SP => 4.0,
                _ if air_control => 70.0,
                _ => PM_AIRACCELERATE,
            },
            has_air_control: air_control,
        }
    }
}

fn center_only(style: i32) -> bool {
    matches!(style, mv::SURF | mv::TRIBES)
}

/// `CGAZ_Opt`: view-to-velocity angle (degrees) for the best air gain.
fn cgaz_opt(vf: f32, a: f32, s: f32) -> f32 {
    if vf == 0.0 {
        return 0.0;
    }
    let delta = ((s - a) / vf).clamp(-1.0, 1.0).acos().to_degrees() - 45.0;
    if (0.0..=360.0).contains(&delta) { delta } else { 0.0 }
}

/// `CGAZ_Max`: the angle past which no more speed is gained.
fn cgaz_max(ground: bool, v: f32, vf: f32, a: f32) -> f32 {
    if 2.0 * a * vf == 0.0 || 2.0 * v == 0.0 {
        return 0.0;
    }
    let argument = if ground { (v * v - vf * vf - a * a) / (2.0 * a * vf) } else { -(a / (2.0 * v)) };
    let delta = argument.clamp(-1.0, 1.0).acos().to_degrees() - 45.0;
    if (0.0..=360.0).contains(&delta) { delta } else { 0.0 }
}

/// Per-frame strafehelper physics (`dfstate` + `dfcgaz`).
struct Model<'a> {
    hud: &'a MovementHudState,
    physics: Physics,
    style: i32,
    frametime: f32,
    ground_move: bool,
    friction_frame: bool,
    cmd: Cmd,
    v: f32,
    vf: f32,
    offset: f32,
}

impl<'a> Model<'a> {
    fn new(hud: &'a MovementHudState, settings: &StrafeHelperSettings, frametime: f32) -> Self {
        let style = hud.move_style;
        let physics = Physics::for_style(style);
        let walking = hud.buttons & BUTTON_WALKING != 0;
        let dashing = hud.buttons & BUTTON_DASH != 0;
        let ground_move = hud.grounded && hud.was_grounded;
        let on_slick = (style == mv::SLICK && !walking) || (style == mv::TRIBES && dashing) || hud.knockback;
        // Friction only touches a player who has been on the ground for more
        // than one step and is not on an ice-like surface.
        let friction_frame = ground_move && !on_slick;

        let v = hud.velocity[0].hypot(hud.velocity[1]);
        let vf = if friction_frame {
            let control = v.max(PM_STOPSPEED);
            (v - control * physics.friction * frametime).max(0.0)
        } else {
            v
        };
        let cmd = Cmd {
            forward: i32::from(hud.forward_move),
            right: i32::from(hud.right_move),
            up: i32::from(hud.up_move),
        };
        Self {
            hud,
            physics,
            style,
            frametime,
            ground_move,
            friction_frame,
            cmd,
            v,
            vf,
            offset: settings.offset * 0.01,
        }
    }

    fn speed(&self) -> f32 {
        self.hud.player_speed
    }

    fn tribes_ground_ski(&self) -> bool {
        self.style == mv::TRIBES && self.hud.buttons & BUTTON_DASH != 0 && self.ground_move
    }

    fn uses_cmd_scale(&self) -> bool {
        matches!(self.style, mv::OCPM | mv::SP | mv::TRIBES | mv::JETPACK)
    }

    fn jump_clears_upmove(&self) -> bool {
        match self.style {
            mv::SP | mv::OCPM => true,
            mv::TRIBES => !self.hud.jetpack_active,
            _ => false,
        }
    }

    /// Unit ground-plane wish direction for `cmd` (`wishvel` after normalizing).
    fn wish_dir(&self, cmd: Cmd) -> [f32; 2] {
        let (sin, cos) = self.hud.view_yaw.to_radians().sin_cos();
        let (forward, right) = ([cos, sin], [sin, -cos]);
        let (f, r) = (cmd.forward as f32, cmd.right as f32);
        let wish = [forward[0] * f + right[0] * r, forward[1] * f + right[1] * r];
        let length = wish[0].hypot(wish[1]);
        if length > 0.0 { [wish[0] / length, wish[1] / length] } else { [0.0, 0.0] }
    }

    fn velocity_dot(&self, dir: [f32; 2]) -> f32 {
        self.hud.velocity[0] * dir[0] + self.hud.velocity[1] * dir[1]
    }

    /// `DF_GetCmdScale`.
    fn cmd_scale(&self, cmd: Cmd) -> f32 {
        let mut up = 0;
        if self.uses_cmd_scale() {
            up = cmd.up;
            if up > 0 && self.jump_clears_upmove() {
                up = 0;
            }
        }
        let max = cmd.forward.abs().max(cmd.right.abs()).max(up.abs());
        if max == 0 {
            return 0.0;
        }
        let total = ((cmd.forward * cmd.forward + cmd.right * cmd.right + up * up) as f32).sqrt();
        self.speed() * max as f32 / (127.0 * total)
    }

    /// `DF_GetWishspeedInternal`: the emulated `wishspeed` for `cmd`.
    fn wishspeed(&self, cmd: Cmd, uncapped: bool) -> f32 {
        let magnitude = (cmd.forward as f32).hypot(cmd.right as f32);
        let mut wishspeed = if self.uses_cmd_scale() {
            magnitude * self.cmd_scale(cmd)
        } else {
            // magnitude * PM_CmdScale cancels to ps->speed for full presses.
            self.speed()
        };

        // Air-control styles cap A/D-only air strafing.
        if !self.ground_move
            && self.physics.has_air_control
            && wishspeed > self.physics.airstrafewishspeed
            && cmd.forward == 0
            && cmd.right != 0
        {
            wishspeed = self.physics.airstrafewishspeed;
        }
        if self.style == mv::SURF {
            if !self.ground_move {
                // PM_CS_AirAccelerate caps the addspeed threshold at 30.
                wishspeed = 30.0;
            } else if wishspeed > PM_SURF_WISHSPEED {
                wishspeed = PM_SURF_WISHSPEED;
            }
        }
        if self.hud.jetpack_pm_type {
            wishspeed *= if cmd.up <= 0 { 0.8 } else { 2.0 };
        }
        if self.ground_move && cmd.up < 0 {
            wishspeed = wishspeed.min(self.speed() * self.physics.duckscale);
        }
        // SP encourages decelerating against the current velocity, in the air.
        if self.style == mv::SP && !self.ground_move && self.velocity_dot(self.wish_dir(cmd)) < 0.0 {
            wishspeed *= PM_SP_AIRDECELRATE;
        }
        if self.style == mv::QW && !self.ground_move {
            wishspeed = wishspeed.min(PM_QW_AIRSTRAFEWISHSPEED);
        }
        if !uncapped && self.tribes_ground_ski() && wishspeed > 30.0 {
            wishspeed = 30.0;
        }
        wishspeed
    }

    /// `DF_GetAccelWishspeed`: the wishspeed that scales `accelspeed`.
    fn accel_wishspeed(&self, cmd: Cmd) -> f32 {
        if !self.ground_move {
            match self.style {
                mv::QW => return self.speed(),
                mv::SURF => return 127.0,
                _ => {}
            }
        } else if self.tribes_ground_ski() {
            return self.wishspeed(cmd, true);
        }
        self.wishspeed(cmd, false)
    }

    /// `DF_GetAirAccelForCmd`.
    fn air_accel(&self, cmd: Cmd) -> f32 {
        let mut accel = self.physics.airaccelerate;
        if self.physics.has_air_control {
            if cmd.forward == 0 && cmd.right != 0 {
                accel = self.physics.airstrafeaccelerate;
            } else {
                let dir = self.wish_dir(cmd);
                if dir != [0.0, 0.0] && self.velocity_dot(dir) < 0.0 && self.physics.airstopaccelerate > 0.0 {
                    accel = self.physics.airstopaccelerate;
                }
            }
        }
        match self.style {
            mv::QW => accel * PM_QW_FRICTION,
            mv::SURF => accel * PM_FRICTION,
            mv::TRIBES => accel * PM_TRIBES_AIRFRICTION,
            _ => accel,
        }
    }

    /// `DF_UseGroundAccel`.
    fn use_ground_accel(&self) -> bool {
        if !self.ground_move {
            return false;
        }
        if self.hud.knockback {
            return self.style == mv::OCPM;
        }
        match self.style {
            mv::OCPM | mv::SLICK => true,
            _ => self.friction_frame,
        }
    }

    /// `DF_GetAccel`: the acceleration constant pmove applies for `cmd`.
    fn accel(&self, cmd: Cmd) -> f32 {
        if !self.ground_move {
            return self.air_accel(cmd);
        }
        if self.tribes_ground_ski() {
            return PM_TRIBES_GROUNDACCELERATE * PM_TRIBES_GROUNDFRICTION;
        }
        if self.use_ground_accel() {
            self.physics.accelerate
        } else {
            PM_AIRACCELERATE
        }
    }

    /// Degrees from the velocity direction that the requested line sits at,
    /// including the user's `cg_strafeHelperOffset`.
    fn delta(&self, gaz: Gaz, cmd: Cmd) -> f32 {
        let s = self.wishspeed(cmd, false);
        let a = self.accel_wishspeed(cmd) * self.accel(cmd) * self.frametime;
        let delta = match gaz {
            Gaz::Opt => cgaz_opt(self.vf, a, s),
            Gaz::Max => cgaz_max(self.ground_move, self.v, self.vf, a),
        };
        delta + self.offset
    }
}

/// Orientation of the rendered view, in the q3 axis convention
/// (`AnglesToAxis`: forward, left, up).
struct Projection {
    forward: [f32; 3],
    left: [f32; 3],
    up: [f32; 3],
    /// `tan(fov_x / 2)` and `tan(fov_y / 2)` of the rendered view.
    tan_x: f32,
    tan_y: f32,
}

impl Projection {
    fn new(hud: &MovementHudState, aspect: f32) -> Self {
        let (sp, cp) = hud.view_pitch.to_radians().sin_cos();
        let (sy, cy) = hud.view_yaw.to_radians().sin_cos();
        let (sr, cr) = hud.view_roll.to_radians().sin_cos();
        // fov_x is authored for 4:3 and widened to the window (cg_fovAspectAdjust).
        let tan_y = (hud.fov_x.clamp(1.0, 176.0).to_radians() * 0.5).tan() * 0.75;
        Self {
            forward: [cp * cy, cp * sy, -sp],
            left: [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp],
            up: [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
            tan_x: tan_y * aspect,
            tan_y,
        }
    }

    /// `CG_WorldCoordToScreenCoord` for a point `trans` away from the eye.
    fn to_screen(&self, trans: [f32; 3]) -> Option<[f32; 2]> {
        let dot = |axis: [f32; 3]| trans[0] * axis[0] + trans[1] * axis[1] + trans[2] * axis[2];
        let z = dot(self.forward);
        if z <= 0.001 {
            return None;
        }
        let (xc, yc) = (SCREEN_WIDTH * 0.5, SCREEN_HEIGHT * 0.5);
        Some([xc - dot(self.left) * xc / (z * self.tan_x), yc - dot(self.up) * yc / (z * self.tan_y)])
    }
}

struct Line {
    active: bool,
    key: Option<Key>,
    gaz: Gaz,
    /// Screen position of the far end of the line.
    point: [f32; 2],
}

struct Builder<'a> {
    settings: &'a StrafeHelperSettings,
    model: Model<'a>,
    projection: Projection,
    /// Eye-to-anchor offset: zero when lines grow from the eye, the player
    /// origin when they are anchored there (third person, `SHELPER_ORIGINAL`).
    anchor: [f32; 3],
    velocity_yaw: f32,
    sensitivity: f32,
    width: f32,
    out: Vec<StrafeSegment>,
}

impl Builder<'_> {
    fn flag(&self, flag: u32) -> bool {
        self.settings.flags & flag != 0
    }

    fn rear_enabled(&self) -> bool {
        self.flag(SHELPER_REAR) && !center_only(self.model.style)
    }

    /// `DF_SetAngleToX`: project the point `sensitivity` units along `yaw`.
    fn project(&self, yaw_offset: f32) -> Option<[f32; 2]> {
        let (sin, cos) = (self.velocity_yaw + yaw_offset).to_radians().sin_cos();
        let trans = [
            self.anchor[0] + cos * self.sensitivity,
            self.anchor[1] + sin * self.sensitivity,
            self.anchor[2],
        ];
        self.projection.to_screen(trans)
    }

    /// `DF_GetLine` for one key combination.
    fn key_line(&self, key: Key, rear: bool, gaz: Gaz) -> Option<Line> {
        let real = self.model.cmd;
        let (forward, right) = key.axes();
        let active = real.forward == forward && real.right == right;
        let draw = if key == Key::S {
            self.flag(SHELPER_S) || self.rear_enabled()
        } else {
            self.flag(key.flag())
        };
        if !draw || center_only(self.model.style) {
            return None;
        }
        let delta = self.model.delta(gaz, Cmd { forward, right, up: real.up });
        let point = self.project(key.angle(rear, delta))?;
        Some(Line { active, key: Some(key), gaz, point })
    }

    /// `DF_GetLine(KEY_CENTER, ...)`: the velocity direction (or its reverse).
    fn center_line(&self, rear: bool) -> Option<Line> {
        if !self.flag(SHELPER_CENTER) {
            return None;
        }
        let real = self.model.cmd;
        let active = real.forward == 0;
        let point = self.project(if rear { 180.0 } else { 0.0 })?;
        Some(Line { active, key: None, gaz: Gaz::Opt, point })
    }

    /// `DF_SetLineColor`.
    fn color(&self, line: &Line) -> [f32; 4] {
        let s = self.settings;
        if line.active {
            if line.gaz == Gaz::Max {
                return [1.0, 0.0, 0.0, 1.0];
            }
            let [r, g, b, a] = s.active_color;
            return [r, g, b, a].map(|c| f32::from(c) / 255.0);
        }
        let mut color = line.key.map_or([1.0, 0.75, 0.0, 0.75], Key::color);
        color[3] = f32::from(s.inactive_alpha) / 255.0;
        color
    }

    fn push_segment(&mut self, from: [f32; 2], to: [f32; 2], y_limit: f32, color: [f32; 4]) {
        let size = self.width;
        if let Some((from, to)) = clip_above(from, to, y_limit, size) {
            self.out.push(StrafeSegment { from, to, size, color });
        }
    }

    /// `DF_DrawStrafeLine`: emit the segments for every enabled style.
    fn draw(&mut self, line: &Line) {
        let color = self.color(line);
        let [x, y] = line.point;
        let cutoff = self.settings.cutoff;
        let size = self.width;

        if self.flag(SHELPER_ORIGINAL) {
            let y_limit = (SCREEN_HEIGHT - cutoff).clamp(LINE_HEIGHT, SCREEN_HEIGHT);
            if let Some([sx, sy]) = self.projection.to_screen(self.anchor) {
                self.push_segment([sx - size * 0.5, sy], [x, y], y_limit, color);
            }
        }
        if self.flag(SHELPER_UPDATED) {
            let (y_limit, height_in) = if self.flag(SHELPER_TINY) {
                (LINE_HEIGHT + 5.0, LINE_HEIGHT - 5.0)
            } else {
                ((SCREEN_HEIGHT - cutoff).clamp(LINE_HEIGHT + 10.0, SCREEN_HEIGHT), LINE_HEIGHT - 10.0)
            };
            self.push_segment([SCREEN_WIDTH * 0.5, SCREEN_HEIGHT], [x, height_in], y_limit, color);
        }
        if self.flag(SHELPER_CGAZ) {
            let half = if cutoff > LINE_HEIGHT { 5.0 } else { 20.0 - cutoff / 16.0 };
            self.push_segment([x, LINE_HEIGHT + half], [x, LINE_HEIGHT - half], SCREEN_HEIGHT, color);
        }
    }

    /// `DF_StrafeHelper`.
    fn build(&mut self) {
        let style = self.model.style;
        if style == mv::SIEGE {
            return;
        }
        let rear_lines = self.rear_enabled();
        if center_only(style) || self.model.physics.has_air_control {
            for rear in [false, true].into_iter().filter(|&rear| !rear || rear_lines) {
                if let Some(line) = self.center_line(rear) {
                    self.draw(&line);
                }
            }
        }
        if center_only(style) {
            return;
        }
        let real = self.model.cmd;
        // Key lines only exist while a direction is held.
        let Some(move_dir) = Key::from_axes(real.forward, real.right) else {
            return;
        };
        // Holding a W combination always shows both optimum sides.
        let both_sides = rear_lines || matches!(move_dir, Key::W | Key::Wa | Key::Dw);

        // Inactive keys first, then the held key on top.
        for key in Key::ALL.into_iter().filter(|&key| key != move_dir) {
            for rear in [false, true].into_iter().filter(|&rear| !rear || rear_lines || key == Key::W) {
                if let Some(line) = self.key_line(key, rear, Gaz::Opt) {
                    self.draw(&line);
                }
            }
        }
        let sides = || [false, true].into_iter().filter(move |&rear| !rear || both_sides);
        if self.flag(SHELPER_MAX) {
            for rear in sides() {
                if let Some(line) = self.key_line(move_dir, rear, Gaz::Max) {
                    self.draw(&line);
                }
            }
        }
        for rear in sides() {
            if let Some(line) = self.key_line(move_dir, rear, Gaz::Opt) {
                self.draw(&line);
            }
        }
    }
}

/// `DF_DrawLine`'s visibility test only stamps squares with `y < y_limit`,
/// `y < SCREEN_HEIGHT` and `x < SCREEN_WIDTH`. Clip the segment to that region
/// and apply the half-size offsets that center the stamped squares on the line.
fn clip_above(from: [f32; 2], to: [f32; 2], y_limit: f32, size: f32) -> Option<([f32; 2], [f32; 2])> {
    let y_limit = y_limit.min(SCREEN_HEIGHT);
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    let mut clip = |p: f32, q: f32| -> bool {
        // Keep the part of the segment where `p + t * q < 0`.
        if q == 0.0 {
            return p < 0.0;
        }
        let t = -p / q;
        if q > 0.0 { t1 = t1.min(t) } else { t0 = t0.max(t) }
        true
    };
    let keep = clip(from[1] - y_limit, to[1] - from[1]) && clip(from[0] - SCREEN_WIDTH, to[0] - from[0]);
    if !keep || t0 >= t1 {
        return None;
    }
    let at = |t: f32| [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t + size * 0.5];
    Some((at(t0), at(t1)))
}

/// `DF_SetFrameTime`: `cg_strafeHelper_FPS`, else `com_maxFPS` (`fps_cap`, 0 =
/// uncapped), else 125.
pub fn frametime(settings: &StrafeHelperSettings, fps_cap: u32) -> f32 {
    let fps = if settings.fps >= 1.0 {
        settings.fps
    } else if fps_cap >= 1 {
        fps_cap as f32
    } else {
        125.0
    }
    .min(1000.0);
    1.0 / fps
}

/// `state.cgaz.s`: `DF_GetWishspeed` for the command in `hud`.
pub fn wishspeed(hud: &MovementHudState, settings: &StrafeHelperSettings, fps_cap: u32) -> f32 {
    let model = Model::new(hud, settings, frametime(settings, fps_cap));
    model.wishspeed(model.cmd, false)
}

/// Strafehelper segments for this frame, in virtual HUD space.
///
/// `fps_cap` is `com_maxfps` (0 = uncapped); `aspect` is window width / height.
pub fn strafe_lines(
    settings: &StrafeHelperSettings,
    hud: &MovementHudState,
    fps_cap: u32,
    aspect: f32,
) -> Vec<StrafeSegment> {
    if hud.in_vehicle {
        return Vec::new();
    }
    let anchored_at_origin = hud.third_person || settings.flags & SHELPER_ORIGINAL != 0;
    let mut builder = Builder {
        settings,
        model: Model::new(hud, settings, frametime(settings, fps_cap)),
        projection: Projection::new(hud, aspect.max(0.01)),
        anchor: if anchored_at_origin { hud.eye_to_origin } else { [0.0; 3] },
        velocity_yaw: hud.velocity[1].atan2(hud.velocity[0]).to_degrees(),
        sensitivity: settings.precision.clamp(100, 10000) as f32,
        width: settings.line_width.clamp(0.25, 5.0),
        out: Vec::new(),
    };
    builder.build();
    builder.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(flags: u32) -> StrafeHelperSettings {
        StrafeHelperSettings { flags, offset: 0.0, ..StrafeHelperSettings::default() }
    }

    /// Airborne at 700 ups heading along +X, view along +X, holding `forward`/`right`.
    fn air_state(forward: i8, right: i8) -> MovementHudState {
        MovementHudState {
            forward_move: forward,
            right_move: right,
            velocity: [700.0, 0.0, 0.0],
            player_speed: 250.0,
            ..MovementHudState::default()
        }
    }

    #[test]
    fn opt_angle_matches_the_air_formula() {
        // acos((250 - 250 * 1 / 125) / 700) - 45 degrees.
        let expected = ((250.0_f32 - 2.0) / 700.0).acos().to_degrees() - 45.0;
        let state = air_state(127, -127);
        let model = Model::new(&state, &settings(0), 1.0 / 125.0);
        let cmd = model.cmd;
        assert!((model.delta(Gaz::Opt, cmd) - expected).abs() < 1e-4);
    }

    #[test]
    fn slow_air_strafe_clamps_to_zero() {
        // At 320 ups the acos term is under 45 degrees, so the line sits on the velocity.
        let mut state = air_state(127, -127);
        state.velocity = [320.0, 0.0, 0.0];
        let model = Model::new(&state, &settings(0), 1.0 / 125.0);
        assert_eq!(model.delta(Gaz::Opt, model.cmd), 0.0);
    }

    #[test]
    fn projection_uses_pitch_like_the_renderer() {
        let mut state = air_state(127, 0);
        state.fov_x = 90.0;
        let aspect = 4.0 / 3.0;
        let level = Projection::new(&state, aspect);
        // Straight ahead is the screen center.
        let [x, y] = level.to_screen([256.0, 0.0, 0.0]).unwrap();
        assert!((x - 320.0).abs() < 1e-3 && (y - 240.0).abs() < 1e-3);
        // A point 45 degrees to the left lands on the left edge of a 90 degree view.
        let [x, _] = level.to_screen([256.0, 256.0, 0.0]).unwrap();
        assert!(x.abs() < 1e-3, "{x}");

        // Looking down spreads horizontal-plane points by 1/cos(pitch).
        state.view_pitch = 60.0;
        let down = Projection::new(&state, aspect);
        let [x_level, _] = level.to_screen([256.0, 128.0, 0.0]).unwrap();
        let [x_down, _] = down.to_screen([256.0, 128.0, 0.0]).unwrap();
        let level_offset = 320.0 - x_level;
        let down_offset = 320.0 - x_down;
        assert!((down_offset / level_offset - 2.0).abs() < 1e-3);
    }

    #[test]
    fn lines_behind_the_view_are_dropped_but_off_screen_ones_kept() {
        let state = air_state(127, 0);
        let aspect = 4.0 / 3.0;
        let projection = Projection::new(&state, aspect);
        assert!(projection.to_screen([-256.0, 0.0, 0.0]).is_none());
        // 80 degrees to the side is well outside a 90 degree view but still in front.
        let [x, _] = projection.to_screen([44.0, 250.0, 0.0]).unwrap();
        assert!(x < 0.0);
    }

    #[test]
    fn no_key_lines_without_input() {
        let lines = strafe_lines(&settings(SHELPER_CGAZ | SHELPER_WA | SHELPER_WD), &air_state(0, 0), 125, 4.0 / 3.0);
        assert!(lines.is_empty());
    }

    #[test]
    fn center_only_shows_with_air_control_styles() {
        let flags = SHELPER_CGAZ | SHELPER_CENTER;
        let mut state = air_state(0, 0);
        assert!(strafe_lines(&settings(flags), &state, 125, 4.0 / 3.0).is_empty());
        state.move_style = mv::CPM;
        assert_eq!(strafe_lines(&settings(flags), &state, 125, 4.0 / 3.0).len(), 1);
    }

    #[test]
    fn holding_w_draws_both_active_sides() {
        // Slow and wide enough that both +/-(45 + delta) lines are on screen.
        let mut state = air_state(127, 0);
        state.velocity = [400.0, 0.0, 0.0];
        state.fov_x = 120.0;
        let lines = strafe_lines(&settings(SHELPER_CGAZ | SHELPER_W), &state, 125, 4.0 / 3.0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].from[0] != lines[1].from[0]);
    }

    #[test]
    fn updated_style_starts_at_the_bottom_center_and_honours_cutoff() {
        let flags = SHELPER_UPDATED | SHELPER_WA;
        let mut custom = settings(flags);
        let state = air_state(127, -127);
        let lines = strafe_lines(&custom, &state, 125, 4.0 / 3.0);
        let line = lines[0];
        assert!((line.from[0] - 320.0).abs() < 1e-3);
        assert!((line.to[1] - (LINE_HEIGHT - 10.0 + line.size * 0.5)).abs() < 1e-3);

        // The cutoff moves where the line starts along the same slanted line.
        custom.cutoff = 100.0;
        let cut = strafe_lines(&custom, &state, 125, 4.0 / 3.0)[0];
        assert!(cut.from[1] < line.from[1]);
        let slope = |l: &StrafeSegment| (l.to[0] - l.from[0]) / (l.to[1] - l.from[1]);
        assert!((slope(&cut) - slope(&line)).abs() < 1e-3);
    }

    #[test]
    fn ground_tracker_reports_the_previous_step() {
        let mut tracker = GroundTracker::default();
        assert!(!tracker.update(100, true));
        // Sampled again within the same step: history is unchanged.
        assert!(!tracker.update(100, true));
        assert!(tracker.update(108, true));
        assert!(tracker.update(108, true));
        assert!(tracker.update(116, false));
        assert!(!tracker.update(124, false));
    }

    #[test]
    fn grounded_friction_needs_two_frames_on_the_ground() {
        let mut state = air_state(127, -127);
        state.velocity = [400.0, 0.0, 0.0];
        state.grounded = true;
        state.was_grounded = false;
        let landing = Model::new(&state, &settings(0), 1.0 / 125.0);
        assert_eq!(landing.vf, 400.0);
        state.was_grounded = true;
        let rolling = Model::new(&state, &settings(0), 1.0 / 125.0);
        assert!((rolling.vf - (400.0 - 400.0 * 6.0 / 125.0)).abs() < 1e-3);
        assert_eq!(rolling.accel(rolling.cmd), 10.0);
    }
}
