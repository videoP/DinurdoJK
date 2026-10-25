//! Independent 2D modes; deliberately do not reuse TaystJK's `Builder` lines.
//!
//! Mathematical references:
//! * Q3 DeFRaG 1.91.26 `CGaz_StrafeHud.c`, as recovered by Jelvan1:
//!   https://gist.github.com/Jelvan1/837983b2abaf8f2dd81b2be41078a3cd
//! * Xonotic `qcsrc/client/hud/panel/strafehud.qc`, `strafehud/util.qc`:
//!   https://github.com/xonotic/xonotic-data.pk3dir
//!
//! All coordinates and rectangles are in the native Quake 640x480 space.
//! The Q3DF 1.91.26 air/ground formulae intentionally use vanilla Q3's
//! hard-coded 1/10 acceleration and 6 friction, not the TaystJK model's
//! distinct per-server movement styles; Xonotic uses the modeled style.

use super::{Model, Cmd, cgaz_opt, cgaz_max};
use crate::ui::{
    MovementHudState, StrafeHelperSettings, SHELPER_Q3DF, SHELPER_Q3DF_ACCEL,
    SHELPER_Q3DF_COMPASS, SHELPER_Q3DF_SPEED, SHELPER_XONOTIC,
    SHELPER_XONOTIC_PANORAMIC, SHELPER_XONOTIC_PERSPECTIVE, SHELPER_XONOTIC_VELOCITY, SHELPER_WARSOW_HUD,
};

#[derive(Debug, Clone, PartialEq)]
pub struct StrafeFill { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub color: [f32; 4] }
/// Three vertices in 640x480 virtual HUD coordinates. Needed for actual
/// trapezoid/triangle fills: thin rasterized lines cannot match Warsow wedges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrafeTriangle { pub points: [[f32; 2]; 3], pub color: [f32; 4] }
#[derive(Debug, Clone, PartialEq)]
pub struct StrafeLabel {
    pub x: f32, pub y: f32,
    pub glyph_w: f32, pub glyph_h: f32, pub advance: f32,
    pub color: [f32; 4], pub text: String,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtendedHud { pub fills: Vec<StrafeFill>, pub labels: Vec<StrafeLabel>, pub triangles: Vec<StrafeTriangle> }

impl ExtendedHud {
    // CG_FillRect clips to screen and interprets negative widths by translating
    // the origin. Guard nonfinite coordinates before making geometry.
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) { return; }
        let (x1, x2) = if w >= 0.0 { (x, x + w) } else { (x + w, x) };
        let (y1, y2) = if h >= 0.0 { (y, y + h) } else { (y + h, y) };
        let x1 = x1.max(0.0).min(640.0);
        let x2 = x2.max(0.0).min(640.0);
        let y1 = y1.max(0.0).min(480.0);
        let y2 = y2.max(0.0).min(480.0);
        if x2 > x1 && y2 > y1 && color[3] > 0.0 {
            self.fills.push(StrafeFill { x: x1, y: y1, w: x2-x1, h: y2-y1, color });
        }
    }

    fn triangle(&mut self, points: [[f32; 2]; 3], color: [f32; 4]) {
        if color[3] > 0.0 && points.iter().flatten().all(|v| v.is_finite()) {
            self.triangles.push(StrafeTriangle { points, color });
        }
    }

    fn label(&mut self, x: f32, y: f32, glyph_w: f32, glyph_h: f32, color: [f32;4], text: String) {
        self.labels.push(StrafeLabel { x, y, glyph_w, glyph_h, advance:glyph_w, color, text });
    }
}

fn angle_subtract(a: f32, b: f32) -> f32 { (a - b + 180.0).rem_euclid(360.0) - 180.0 }
fn acos_deg(value: f32) -> f32 { value.clamp(-1.0, 1.0).acos().to_degrees() }

fn physics_frametime(settings: &StrafeHelperSettings, hud: &MovementHudState) -> f32 {
    if settings.fps >= 1.0 {
        1.0 / settings.fps.min(1000.0)
    } else if hud.physics_frame_time.is_finite() && hud.physics_frame_time > 0.0 {
        hud.physics_frame_time.clamp(0.001, 0.066)
    } else { 0.008 }
}

/// Source CGaz 1.91.26 projection. The left/right edge are clipped in angle
/// space BEFORE tangent projection (not simply clamped after projection).
fn q3_project(delta: f32, fov: f32) -> f32 {
    let fov = fov.clamp(1.0, 170.0);
    let angle = delta.clamp(-fov / 2.0, fov / 2.0);
    320.0 + 320.0 * angle.to_radians().tan() / (fov * 0.5).to_radians().tan()
}

fn q3df(settings: &StrafeHelperSettings, hud: &MovementHudState) -> ExtendedHud {
    let mut out = ExtendedHud::default();
    let flags = settings.flags;
    let speed = hud.velocity[0].hypot(hud.velocity[1]);
    let fov = hud.fov_x;
    let yaw = hud.view_yaw;
    let vel_yaw = hud.velocity[1].atan2(hud.velocity[0]).to_degrees();
    // Original opacity is a cgaz() call argument. Default to fully opaque;
    // the translucent compass palette and 0.8 alpha accel bar match the C.
    let opacity = 1.0_f32;

    // CGaz_StrafeHud.c integer/fraction styling, centered for the component HUD.
    if flags & SHELPER_Q3DF_SPEED != 0 {
        let integer = format!("{:.0}", speed.floor());
        let integer_x = 320.0 - (integer.len() as f32*16.0 + 60.0)*0.5;
        out.label(integer_x, 200.0, 16.0, 24.0, [1.0,1.0,1.0,1.0], integer.clone());
        out.label(integer_x + integer.len() as f32*16.0, 204.0, 12.0, 18.0, [1.0,1.0,1.0,1.0],
            format!(".{:01}^3u/s", (speed.fract()*10.0).floor() as u8));
    }
    if flags & SHELPER_Q3DF_ACCEL != 0 {
        // Source width = (speed-old_speed) / commandTime_delta * 256;
        // ground scales width by .1, with markers at ±81.92.
        let width = hud.q3df_accel_bar_width;
        let gap = hud.crosshair_size.clamp(0.0, 96.0);
        if width > 0.0 {
            out.fill(320.0 + gap, 232.0, width, 16.0, [1.0,0.0,0.0,0.8*opacity]);
        } else if width < 0.0 {
            out.fill(320.0 - gap, 232.0, width, 16.0, [1.0,0.0,0.0,0.8*opacity]);
        }
        out.fill(320.0 + gap + 81.92, 236.0, 1.0, 8.0, [1.0,1.0,1.0,opacity]);
        out.fill(320.0 - gap - 81.92, 236.0, -1.0, 8.0, [1.0,1.0,1.0,opacity]);
    }
    // The CGaz 1.91.26 compass is independent of movement/speed.
    if flags & SHELPER_Q3DF_COMPASS != 0 {
        let colors: [[f32;4];4] = [
            [1.0,1.0,0.5,0.5*opacity], [0.0,1.0,0.5,0.5*opacity],
            [0.0,0.0,0.5,0.5*opacity], [1.0,0.0,0.5,0.5*opacity],
        ];
        for i in 0..8 {
            let angle = (i * 45) as f32;
            let xa = q3_project(angle_subtract(yaw, angle), fov);
            let xb = q3_project(angle_subtract(yaw, angle + 90.0), fov);
            if xa >= xb {
                if i % 2 == 0 {
                    out.fill(xb, 288.0, xa-xb, 16.0, colors[i/2]);
                }
                out.fill(xa - 1.0, if i % 2 == 0 { 296.0 } else { 300.0 },
                    2.0, if i % 2 == 0 {8.0} else {4.0}, [1.0,1.0,1.0,opacity]);
            }
        }
        if speed > 0.0 {
            let mut angle = angle_subtract(yaw, vel_yaw);
            let down = angle.abs() > 90.0;
            if down { angle = angle_subtract(angle, 180.0); }
            let x = q3_project(angle, fov);
            let arrow = if hud.velocity[0] * hud.velocity[1] != 0.0 {
                [1.0,1.0,1.0,opacity]
            } else { [1.0,0.5,0.0,opacity] };
            // Original uses characters 135 / 134 of cgs.media.charsetShader.
            // Two tapered steps reproduce the silhouette without a missing glyph.
            for n in 0..8 {
                let half_w = if down { (8-n) as f32 } else { (n+1) as f32 };
                out.fill(x-half_w, 304.0 + n as f32*2.0, 2.0*half_w, 2.0, arrow);
            }
        }
    }
    if flags & SHELPER_Q3DF == 0 || speed <= 0.0 { return out; }
    // EXACT DeFRaG vq3 CGaz formula with pmove_msec / 1000. This intentionally
    // doesn't call Model::delta, as that is TaystJK's other CGAZ style.
    let dt = physics_frametime(settings, hud);
    let mut current_speed = speed;
    let mut accel = hud.player_speed * dt;
    if hud.grounded {
        accel *= 10.0;
        current_speed *= 1.0 - 6.0 * dt;
    }
    if current_speed <= 0.0 || current_speed <= hud.player_speed - accel { return out; }
    let max_angle = acos_deg(-0.5 * accel / current_speed);
    let opt_angle = acos_deg((hud.player_speed - accel) / current_speed);
    let min_angle = if current_speed > hud.player_speed {
        acos_deg(hud.player_speed / current_speed)
    } else { 0.0 };
    // Q3's movementDir indexing (0 W, 1 WA, 2 A, 3 SA, ... 7 WD).
    // The real usercmd is used instead of an unrelated local spectator key.
    let movement_dir = match (hud.forward_move.signum(), hud.right_move.signum()) {
        (1,0)=>0, (1,-1)=>1, (0,-1)=>2, (-1,-1)=>3,
        (-1,0)=>4, (-1,1)=>5, (0,1)=>6, (1,1)=>7,
        _=>0,
    };
    let adjusted_yaw = yaw + 45.0 * movement_dir as f32;
    let angles = [vel_yaw-max_angle, vel_yaw-90.0, vel_yaw-opt_angle,
        vel_yaw-min_angle, vel_yaw, vel_yaw+min_angle, vel_yaw+opt_angle,
        vel_yaw+90.0, vel_yaw+max_angle];
    let colors: [[f32;4];8] = [
        [1.0,1.0,0.0,opacity], [0.0,0.25,0.25,opacity],
        [0.0,1.0,0.0,opacity], [0.25,0.25,0.25,opacity],
        [0.25,0.25,0.25,opacity], [0.0,1.0,0.0,opacity],
        [0.0,0.25,0.25,opacity], [1.0,1.0,0.0,opacity],
    ];
    for i in 0..8 {
        let xa = q3_project(angle_subtract(adjusted_yaw, angles[i]), fov);
        let xb = q3_project(angle_subtract(adjusted_yaw, angles[i+1]), fov);
        if xa >= xb {
            out.fill(xb, 268.0, xa - xb, 12.0, colors[i]);
            if i == 4 { out.fill(xa - 1.0, 270.0, 2.0, 8.0, [0.5,0.5,1.0,opacity]); }
        }
    }
    out
}

/// Xonotic `StrafeHUD_Project()` from qcsrc/client/hud/panel/strafehud/util.qc
/// with exactly the original linear / tan / half-tan projection functions.
fn xonotic_project(angle: f32, range: f32, flags: u32) -> f32 {
    let ratio = angle / (0.5 * range);
    let range_rad = range.to_radians() * 0.5;
    let projected = if flags & SHELPER_XONOTIC_PANORAMIC != 0 {
        (ratio * range_rad * 0.5).tan() / (range_rad * 0.5).tan()
    } else if flags & SHELPER_XONOTIC_PERSPECTIVE != 0 {
        (ratio * range_rad).tan() / range_rad.tan()
    } else { ratio };
    320.0 + 320.0 * projected
}

fn xonotic(settings: &StrafeHelperSettings, hud: &MovementHudState) -> ExtendedHud {
    let mut out = ExtendedHud::default();
    let model = Model::new(hud, settings, physics_frametime(settings, hud));
    let speed = hud.velocity[0].hypot(hud.velocity[1]);
    let dt = model.frametime;
    let mut cmd = model.cmd;
    // Xonotic source gets real PHYS_INPUT_MOVEVALUES and falls back to maxspeed
    // when no movement command exists. JKA's command scale and movement style
    // are supplied by Model so acceleration remains valid on JAPro servers.
    if speed <= 0.0 {
        // Upstream's `moving` means nonzero velocity, not held movement keys.
        out.fill(0.0, 222.0, 640.0, 18.0, [0.17,0.17,0.17,0.32]);
        return out;
    }
    if cmd.forward == 0 && cmd.right == 0 { cmd.forward = 127; }
    let movespeed = model.wishspeed(cmd, false);
    let maxaccel = model.accel(cmd) * dt * model.accel_wishspeed(cmd);
    let bestspeed = (movespeed - maxaccel).max(0.0);
    let grounded = model.ground_move;
    let strafespeed = if grounded { model.vf } else { speed };
    let (mut best, mut prebest, mut overturn) = if speed <= 0.0 {
        (0.0,0.0,180.0)
    } else if grounded {
        // Xonotic's ground `delta_opt`, `delta_min`, `delta_max` with friction.
        let best = if strafespeed > bestspeed { acos_deg(bestspeed/strafespeed) } else { 0.0 };
        let sq = movespeed*movespeed + strafespeed*strafespeed - speed*speed;
        let pre = if sq > 0.0 && strafespeed > sq.sqrt() {
            acos_deg(sq.sqrt()/strafespeed)
        } else if sq > 0.0 { 0.0 } else { 90.0 };
        let numer = speed*speed - strafespeed*strafespeed - maxaccel*maxaccel;
        let denom = 2.0 * maxaccel * strafespeed;
        let turn = if denom > numer.abs() { acos_deg(numer/denom) }
            else if numer < 0.0 { 180.0 } else { 0.0 };
        (best,pre,turn)
    } else {
        let best = if speed > bestspeed { acos_deg(bestspeed/speed) } else {0.0};
        let pre = if speed > movespeed { acos_deg(movespeed/speed) } else {0.0};
        // maxaccel already includes Model::air_accel's directional stop
        // acceleration. Multiplying by airstopaccelerate again double-counts it.
        let turn = if speed > maxaccel*0.5 {
            acos_deg(-maxaccel*0.5/speed)
        } else {180.0};
        (best,pre,turn)
    };
    if grounded && (overturn < best || best < prebest) {
        // Xonotic STRAFEHUD_ONGROUND_GROUND fallback; avoid inverted bands
        // during high-speed friction deceleration after landing.
        prebest = best;
        overturn = best;
    }
    best = best.clamp(0.0,180.0);
    prebest = prebest.clamp(0.0,best);
    overturn = overturn.clamp(best,180.0);

    // The Xonotic velocity bar is symmetric about the velocity direction.
    // Source `util.qc` can dynamically pick the minimum needed HUD range.
    let wish = model.wish_dir(cmd);
    let wish_angle = if wish == [0.0,0.0] { 0.0 } else {
        angle_subtract(wish[1].atan2(wish[0]).to_degrees(), hud.view_yaw)
    };
    let mut range = (wish_angle.abs().max(overturn - wish_angle.abs()) * 2.0).clamp(20.0,360.0);
    if settings.flags & SHELPER_XONOTIC_PANORAMIC != 0 { range = range.min(350.0); }
    else if settings.flags & SHELPER_XONOTIC_PERSPECTIVE != 0 { range = range.min(170.0); }
    let velocity_yaw = hud.velocity[1].atan2(hud.velocity[0]).to_degrees();
    let view_relative_velocity = angle_subtract(velocity_yaw, hud.view_yaw);
    // Screen angles grow to the right; world yaw grows to the left. Solve
    // wish_yaw = velocity_yaw +/- boundary for the required view heading.
    let offset = wish_angle + if settings.flags & SHELPER_XONOTIC_VELOCITY != 0 {
        0.0
    } else { -view_relative_velocity };

    // Source styles distinguish neutral / pre-acceleration / acceleration /
    // overturn. A small number of contiguous strips keep the geometry static
    // and their exact boundaries analytically calculated, not heuristically.
    let palette: [([f32;4], f32, f32); 7] = [
        ([0.18,0.18,0.18,0.25], -180.0, -overturn),
        ([0.20,0.78,0.48,0.72], -overturn, -best),
        ([1.0,0.75,0.0,0.60], -best, -prebest),
        ([0.17,0.17,0.17,0.32], -prebest, prebest),
        ([1.0,0.75,0.0,0.60], prebest, best),
        ([0.20,0.78,0.48,0.72], best, overturn),
        ([0.18,0.18,0.18,0.25], overturn, 180.0),
    ];
    // Split wrapping into 360°-shifted copies like Xonotic DrawStrafeHUD.
    for (color, a, b) in palette {
        if b <= a {continue;}
        for turns in -1..=1 {
            let shift = 360.0 * turns as f32 + offset;
            let from = (a+shift).max(-range*0.5);
            let to = (b+shift).min(range*0.5);
            if to <= from {continue;}
            let x0 = xonotic_project(from,range,settings.flags);
            let x1 = xonotic_project(to,range,settings.flags);
            out.fill(x0,222.0,x1-x0,18.0,color);
        }
    }
    // Xonotic's optimal/switch angle guides remain distinct from the colored
    // zones and their projection follows the exact util.qc math.
    for sign in [-1.0_f32, 1.0] {
        let angle = sign*best + offset;
        for turns in -1..=1 {
            let value = angle + turns as f32*360.0;
            if value.abs() <= range*0.5 {
                let x = xonotic_project(value,range,settings.flags);
                out.fill(x-1.0,216.0,2.0,30.0,[1.0,1.0,1.0,0.85]);
            }
        }
    }
    let view_line = if settings.flags & SHELPER_XONOTIC_VELOCITY != 0 {
        view_relative_velocity
    } else {0.0};
    if view_line.abs() <= range*0.5 {
        let x = xonotic_project(view_line,range,settings.flags);
        out.fill(x-1.0,216.0,2.0,30.0,[0.7,0.8,1.0,0.95]);
    }
    out
}

/// Warsow/Warfork `cg_strafeHUD 1` visual port: paired tapered acceleration
/// sectors and a stationary white heading indicator. This is NOT the TaystJK
/// `SHELPER_WSW` red error bar. Its display logic is derived from the documented
/// Warsow behavior and Warfork's movement guide; its packaged `strafe.hud`
/// script has not been independently recovered, so this implementation is NOT
/// asserted to be byte- or pixel-equivalent to that original script.
///
/// Use the actual predicted JKA movement style and pmove interval to place the
/// angle bounds. This avoids assuming Warsow movement constants on JKA servers.
fn warsow_strafehud(settings: &StrafeHelperSettings, hud: &MovementHudState) -> ExtendedHud {
    let mut out = ExtendedHud::default();
    let model = Model::new(hud, settings, physics_frametime(settings, hud));
    let speed = model.v;
    // Warfork movement docs: StrafeHUD appears only above 450 units/second.
    if !speed.is_finite() || speed <= 450.0 || !hud.view_yaw.is_finite() ||
       !hud.velocity[0].is_finite() || !hud.velocity[1].is_finite() {
        return out;
    }

    let velocity_yaw = hud.velocity[1].atan2(hud.velocity[0]).to_degrees();
    let fov = hud.fov_x.clamp(30.0, 150.0);
    // Wide-angle HUDs use a dedicated compact scale around the reticle, while
    // the guide position is still projected using the player's actual FOV.
    let to_x = |target: f32| -> f32 {
        let delta = angle_subtract(hud.view_yaw, target);
        let angle = delta.clamp(-fov * 0.5, fov * 0.5).to_radians();
        (320.0 + 235.0 * angle.tan() / (fov * 0.5).to_radians().tan()).clamp(0.0, 640.0)
    };

    const TIP_Y: f32 = 180.0;
    const BASE_Y_TOP: f32 = 168.0;
    const BASE_Y_BOTTOM: f32 = 180.0;
    // Build both sides, not just the currently pressed WA/WD key. This is why
    // both acceleration wedges remain visible while alternating left/right.
    for (side, right) in [(1.0_f32, -127), (-1.0_f32, 127)] {
        let cmd = Cmd { forward: 127, right, up: model.cmd.up };
        let wishspeed = model.wishspeed(cmd, false);
        let acceleration = model.accel_wishspeed(cmd) * model.accel(cmd) * model.frametime;
        if !wishspeed.is_finite() || !acceleration.is_finite() || acceleration <= 0.0 || model.vf <= 0.0 {
            continue;
        }
        let optimal = cgaz_opt(model.vf, acceleration, wishspeed);
        let maximum = cgaz_max(model.ground_move, model.v, model.vf, acceleration);
        if maximum <= optimal || maximum <= 0.0 { continue; }
        // `cgaz_opt/max` are measured relative to the velocity direction and
        // already subtract 45 degrees for a diagonal forward+strafe command.
        let inner_angle = optimal.clamp(0.0, 85.0);
        let outer_angle = maximum.clamp(inner_angle, 85.0);
        let inner_x = to_x(velocity_yaw + side * inner_angle);
        let outer_x = to_x(velocity_yaw + side * outer_angle);
        if (outer_x - inner_x).abs() < 1.0 { continue; }
        let player_matches = hud.forward_move > 0 && hud.right_move.signum() == right.signum() as i8;
        let marker_inside = (320.0 >= inner_x.min(outer_x)) &&
                            (320.0 <= inner_x.max(outer_x));
        // Predict one pmove step (including ground friction and wishspeed
        // clamping) instead of treating a green zone as guaranteed speed gain.
        let wish_dir = model.wish_dir(cmd);
        let projected_speed = model.velocity_dot(wish_dir) * model.vf / model.v;
        let applied_accel = acceleration.min((wishspeed - projected_speed).max(0.0));
        let expected_speed = (model.vf * model.vf
            + 2.0 * projected_speed * applied_accel
            + applied_accel * applied_accel).max(0.0).sqrt();
        let speed_gaining = expected_speed > model.v;
        // Inactive regions remain visible; the selected correctly aimed,
        // speed-gaining region becomes vivid green.
        let color = if player_matches && marker_inside && speed_gaining {
            [0.02, 0.92, 0.08, 0.98]
        } else {
            [0.04, 0.69, 0.08, 0.88]
        };
        out.triangle([[outer_x, TIP_Y], [inner_x, BASE_Y_TOP], [inner_x, BASE_Y_BOTTOM]], color);
        // The screenshot has a dark vertical edge on the wide end of a wedge.
        out.fill(inner_x - 0.5, BASE_Y_TOP, 1.0, BASE_Y_BOTTOM-BASE_Y_TOP,
                 [0.0, 0.07, 0.01, 0.85]);
    }
    // A narrow white position marker, independent of the velocity/side zones.
    out.fill(319.0, 164.0, 2.0, 22.0, [0.0, 0.0, 0.0, 0.72]);
    out.fill(319.5, 165.0, 1.5, 20.0, [1.0, 1.0, 1.0, 0.96]);
    out
}

/// Geometry for the chosen extension style; no 3D FX, no TaystJK fallthrough.
pub fn extended_hud(settings: &StrafeHelperSettings, hud: &MovementHudState, aspect: f32) -> ExtendedHud {
    if hud.spectator_free_roam || hud.in_vehicle { return ExtendedHud::default(); }
    // MovementHudState carries the authored 4:3 FOV, as the legacy projection
    // does. Convert it to the rendered horizontal FOV before projecting bands.
    let mut hud = *hud;
    hud.fov_x = (2.0 * ((hud.fov_x.clamp(1.0, 176.0).to_radians() * 0.5).tan()
        * 0.75 * aspect.max(0.1)).atan()).to_degrees();
    let mut out = if settings.flags & SHELPER_WARSOW_HUD != 0 { warsow_strafehud(settings,&hud) }
        else if settings.flags & SHELPER_Q3DF != 0 { return q3df(settings,&hud); }
        else if settings.flags & SHELPER_XONOTIC != 0 {
            let mut panel = xonotic(settings,&hud);
            for fill in &mut panel.fills {
                fill.x = 320.0 + (fill.x-320.0)*0.4; fill.w *= 0.4;
                fill.y = 268.0 + (fill.y-222.0)*0.55; fill.h *= 0.55;
            }
            panel
        }
        else { ExtendedHud::default() };
    let extras = q3df(&StrafeHelperSettings { flags: settings.flags & !SHELPER_Q3DF, ..*settings }, &hud);
    out.fills.extend(extras.fills);
    out.labels.extend(extras.labels);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hud() -> MovementHudState { MovementHudState {
        forward_move: 127, right_move: -127, velocity: [700.0,0.0,0.0],
        player_speed: 250.0, grounded: false, was_grounded: false,
        ..MovementHudState::default() } }
    #[test]
    fn q3df_source_air_boundary_angles() {
        let speed: f32 = 700.0;
        let acceleration = 250.0/125.0;
        let maximum = acos_deg(-0.5*acceleration/speed);
        let optimum = acos_deg((250.0-acceleration)/speed);
        let minimum = acos_deg(250.0/speed);
        assert!(minimum < optimum && optimum < maximum && maximum > 90.0);
    }
    #[test]
    fn q3df_has_eight_colored_bands() {
        let sh = StrafeHelperSettings {flags:SHELPER_Q3DF, ..StrafeHelperSettings::default()};
        let output = extended_hud(&sh,&hud(),4.0/3.0);
        assert!(output.fills.iter().any(|f| f.color[1] == 1.0 && f.color[0] == 0.0));
    }
    #[test]
    fn xonotic_three_projections_have_same_center() {
        for flags in [0,SHELPER_XONOTIC_PERSPECTIVE,SHELPER_XONOTIC_PANORAMIC] {
            assert!((xonotic_project(0.0,120.0,flags)-320.0).abs()<1e-5);
            assert!((xonotic_project(60.0,120.0,flags)-640.0).abs()<1e-4);
        }
    }
    #[test]
    fn xonotic_stationary_velocity_is_neutral() {
        let mut subject = hud();
        subject.velocity = [0.0; 3];
        let sh = StrafeHelperSettings {flags:SHELPER_XONOTIC,..StrafeHelperSettings::default()};
        let result = extended_hud(&sh,&subject,4.0/3.0);
        assert_eq!(result.fills.len(),1);
        assert_eq!(result.fills[0].w,256.0);
    }
    #[test]
    fn xonotic_and_q3df_stay_separate() {
        let base = StrafeHelperSettings::default();
        let a = extended_hud(&StrafeHelperSettings {flags: SHELPER_XONOTIC, ..base}, &hud(),4.0/3.0);
        let b = extended_hud(&StrafeHelperSettings {flags: SHELPER_Q3DF, ..base}, &hud(),4.0/3.0);
        assert!(!a.fills.is_empty() && !b.fills.is_empty());
        assert_ne!(a,b);
    }
    #[test]
    fn warsow_respects_450_ups_cutoff_and_renders_real_triangles() {
        let sh = StrafeHelperSettings { flags: SHELPER_WARSOW_HUD, ..StrafeHelperSettings::default() };
        let mut state = hud();
        state.velocity = [450.0, 0.0, 0.0];
        assert!(extended_hud(&sh, &state, 4.0/3.0).triangles.is_empty());
        state.velocity = [755.0, 0.0, 0.0];
        let output = extended_hud(&sh, &state, 4.0/3.0);
        assert_eq!(output.triangles.len(), 2);
        assert!(output.triangles[0].points[0][0] < 320.0);
        assert!(output.triangles[1].points[0][0] > 320.0);
    }
    #[test]
    fn warsow_selected_good_side_turns_brighter() {
        let sh = StrafeHelperSettings { flags: SHELPER_WARSOW_HUD, ..StrafeHelperSettings::default() };
        let mut state = hud();
        state.view_yaw = 30.0;
        let output = extended_hud(&sh, &state, 4.0/3.0);
        // At this heading the opposite wedge is outside the horizontal FOV.
        // Compare the visible wedge with the same heading but released keys.
        state.forward_move = 0;
        state.right_move = 0;
        let inactive = extended_hud(&sh, &state, 4.0/3.0);
        assert!(!output.triangles.is_empty());
        assert_eq!(output.triangles.len(), inactive.triangles.len());
        assert!(output.triangles[0].color[1] > inactive.triangles[0].color[1]);
    }
    #[test]
    fn no_guide_in_free_spectator() {
        let mut subject = hud(); subject.spectator_free_roam=true;
        let sh = StrafeHelperSettings {flags:SHELPER_Q3DF,..StrafeHelperSettings::default()};
        assert_eq!(extended_hud(&sh,&subject,4.0/3.0),ExtendedHud::default());
    }

    #[test]
    fn xonotic_optimal_heading_aligns_with_the_guide_for_each_input_and_projection() {
        for forward in [-127, 0, 127] {
            for right in [-127, 0, 127] {
                if forward == 0 && right == 0 { continue; }
                for projection in [0, SHELPER_XONOTIC_PERSPECTIVE, SHELPER_XONOTIC_PANORAMIC] {
                    for velocity_yaw in [-179.0_f32, 0.0, 179.0] {
                        let mut state = hud();
                        state.forward_move = forward;
                        state.right_move = right;
                        let (sin, cos) = velocity_yaw.to_radians().sin_cos();
                        state.velocity = [700.0*cos, 700.0*sin, 0.0];
                        let wish_angle = (-(right as f32)).atan2(forward as f32).to_degrees();
                        let optimal = acos_deg((250.0 - 2.0) / 700.0);
                        state.view_yaw = velocity_yaw + optimal - wish_angle;
                        let settings = StrafeHelperSettings { flags: SHELPER_XONOTIC | projection, ..StrafeHelperSettings::default() };
                        let output = extended_hud(&settings, &state, 16.0/9.0);
                        assert!(output.fills.iter().any(|f| f.color == [1.0,1.0,1.0,0.85] && (f.x+f.w*0.5-320.0).abs() < 0.01),
                            "forward={forward}, right={right}, yaw={velocity_yaw}, projection={projection}");
                    }
                }
            }
        }
    }

    #[test]
    fn xonotic_coasting_retains_the_forward_guide() {
        let settings = StrafeHelperSettings { flags: SHELPER_XONOTIC, ..StrafeHelperSettings::default() };
        let mut state = hud();
        state.right_move = 0;
        let forward = extended_hud(&settings, &state, 4.0/3.0);
        state.forward_move = 0;
        assert_eq!(extended_hud(&settings, &state, 4.0/3.0), forward);
    }

    #[test]
    fn xonotic_velocity_centered_marker_meets_the_optimal_guide() {
        for right in [-127, 127] {
            for projection in [0, SHELPER_XONOTIC_PERSPECTIVE, SHELPER_XONOTIC_PANORAMIC] {
                let mut state = hud();
                state.right_move = right;
                let side = -(right as f32).signum();
                state.view_yaw = side * (acos_deg(248.0/700.0) - 45.0);
                let settings = StrafeHelperSettings { flags: SHELPER_XONOTIC | SHELPER_XONOTIC_VELOCITY | projection, ..StrafeHelperSettings::default() };
                let output = extended_hud(&settings, &state, 4.0/3.0);
                let marker = output.fills.iter().find(|f| f.color == [0.7,0.8,1.0,0.95]).unwrap();
                assert!(output.fills.iter().any(|f| f.color == [1.0,1.0,1.0,0.85] && (f.x-marker.x).abs() < 0.01),
                    "right={right}, projection={projection}");
            }
        }
    }

    #[test]
    fn readouts_work_alone_and_with_every_guide() {
        let extras = crate::ui::SHELPER_COMPONENT_MASK;
        let settings = StrafeHelperSettings { flags: extras, ..StrafeHelperSettings::default() };
        let mut state = hud();
        state.q3df_accel_bar_width = 20.0;
        let alone = extended_hud(&settings, &state, 4.0/3.0);
        assert_eq!(alone.labels.len(), 2);
        assert!(!alone.fills.is_empty());
        assert!(alone.fills.iter().all(|f| f.y != 268.0));
        for guide in [SHELPER_XONOTIC, SHELPER_Q3DF, SHELPER_WARSOW_HUD, crate::ui::SHELPER_UPDATED, crate::ui::SHELPER_CINEMATIC] {
            let combined = extended_hud(&StrafeHelperSettings {flags: extras | guide, ..settings}, &state, 4.0/3.0);
            assert_eq!(combined.labels, alone.labels);
            for fill in &alone.fills { assert!(combined.fills.contains(fill)); }
        }
        state.in_vehicle = true;
        assert_eq!(extended_hud(&settings, &state, 4.0/3.0), ExtendedHud::default());
    }

    #[test]
    fn compass_backwards_arrow_is_symmetric_and_points_down() {
        let settings = StrafeHelperSettings { flags: SHELPER_Q3DF_COMPASS, ..StrafeHelperSettings::default() };
        let mut state = hud();
        state.velocity = [500.0, 500.0, 0.0];
        let mut tips = Vec::new();
        for angle in [-170.0, 170.0] {
            state.view_yaw = 45.0 + angle;
            let output = extended_hud(&settings, &state, 4.0/3.0);
            let top = output.fills.iter().find(|f| f.y == 304.0).unwrap();
            let bottom = output.fills.iter().find(|f| f.y == 318.0).unwrap();
            assert!(top.w > bottom.w);
            tips.push(top.x + top.w * 0.5);
        }
        assert!((tips[0]+tips[1]-640.0).abs() < 0.01);
    }

    #[test]
    fn extension_timing_uses_physics_interval_and_honors_manual_override() {
        let mut state = hud();
        let settings = StrafeHelperSettings::default();
        state.physics_frame_time = 0.016;
        assert_eq!(physics_frametime(&settings, &state), 0.016);
        assert_eq!(physics_frametime(&StrafeHelperSettings { fps: 250.0, ..settings }, &state), 0.004);
        state.physics_frame_time = f32::NAN;
        assert_eq!(physics_frametime(&settings, &state), 0.008);
    }
}
