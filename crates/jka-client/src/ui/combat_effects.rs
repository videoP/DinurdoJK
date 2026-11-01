//! TaystJK CG_DrawZoomMask / CG_DrawRocketLocking presentation.
//! The server owns targeting and zoom state; this module draws only the artwork.
//! The render thread projects lock targets with its latest view-projection matrix.

use super::{geometry::{rect, textured_rect_with_source, glyph_quad_sized}, hud::movement::{ICON_COMBAT_BASE, ICON_FORCE_BASE, ICON_RAGE_RECOVERY, ICON_TEXTURE_SOURCE}, UiSnapshot, UiVertex};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UiZoomScope {
    /// 1: Disruptor scope, 2: binoculars (JKA zoomMode).
    pub mode: i32,
    pub fov: f32,
    pub time: i32,
    pub yaw_deg: f32,
    pub ammo: i32,
    pub double_ammo: bool,
    pub charge_started: Option<i32>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct UiRocketLock {
    pub target: [f32; 3],
    pub viewer: [f32; 3],
    /// Number of wedges already earned, 0..=8.
    pub wedges: u8,
    pub time: i32,
}

/// Rotate source-textured art about its center without resampling a sprite on
/// the CPU. Source 5 is the native-resolution stock icon atlas.
#[allow(clippy::too_many_arguments)]
fn rotated_icon(out: &mut Vec<UiVertex>, ui: &UiSnapshot, index: usize,
    cx: f32, cy: f32, w: f32, h: f32, degrees: f32, color: [f32; 4],
    viewport_w: u32, viewport_h: u32)
{
    let (uv0, uv1) = ui.icon_uv(index);
    // The atlas assigns missing images a zero-sized UV, not a fake checker.
    if uv1[0] - uv0[0] < 0.00001 || uv1[1] - uv0[1] < 0.00001 { return; }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let corners = [(-0.5_f32, -0.5_f32), (-0.5, 0.5), (0.5, 0.5), (0.5, -0.5)];
    let uvs = [[uv0[0],uv0[1]], [uv0[0],uv1[1]], [uv1[0],uv1[1]], [uv1[0],uv0[1]]];
    let vertex = |i: usize| {
        let (dx,dy) = corners[i];
        let x = cx + dx*w*cos - dy*h*sin;
        let y = cy + dx*w*sin + dy*h*cos;
        UiVertex {
            position: [2.0*x/viewport_w.max(1) as f32-1.0, 1.0-2.0*y/viewport_h.max(1) as f32],
            uv: uvs[i], color, textured: ui.icon_texture_source(index),
        }
    };
    out.extend_from_slice(&[vertex(0),vertex(1),vertex(2),vertex(0),vertex(2),vertex(3)]);
}

#[allow(clippy::too_many_arguments)]
fn icon(out: &mut Vec<UiVertex>, ui: &UiSnapshot, index: usize,
    x: f32, y: f32, w: f32, h: f32, color: [f32;4], viewport_w: u32, viewport_h: u32)
{
    let (uv0, uv1) = ui.icon_uv(index);
    if uv1[0] - uv0[0] < 0.00001 || uv1[1] - uv0[1] < 0.00001 { return; }
    textured_rect_with_source(out, x, y, w, h, uv0, uv1, color,
        ui.icon_texture_source(index), viewport_w, viewport_h);
}

/// TaystJK CG_DrawActivePowers: actual snapshot active bits, sorted in
/// OpenJK forcePowerSorted order, not the currently selected Force power.
/// Match the 640x480 legacy origin, size (40) and step (42), scaling to screen.
fn draw_active_force_icons(out: &mut Vec<UiVertex>, ui: &UiSnapshot, width: u32, height: u32) {
    const FORCE_POWER_SORTED: [usize; 18] = [5, 0, 10, 9, 11, 1, 2, 3, 4, 14, 7, 13, 8, 6, 12, 15, 16, 17];
    const DURATION_POWER_MASK: i32 = (1 << 0) | (1 << 2) | (1 << 5) | (1 << 8)
        | (1 << 9) | (1 << 10) | (1 << 14);
    let Some(hud) = ui.hud else { return; };
    if hud.force_icons_hidden || ui.zoom_scope.is_some() { return; }
    let active = hud.force_powers_active & DURATION_POWER_MASK;
    let recovering = hud.rage_recovery_until_ms > hud.time_ms;
    if active == 0 && !recovering { return; }
    let sx = width as f32 / 640.0;
    let sy = height as f32 / 480.0;
    let mut x = 96.0_f32;
    let mut y = 400.0_f32;
    for power in FORCE_POWER_SORTED {
        if active & (1 << power) == 0 { continue; }
        icon(out, ui, ICON_FORCE_BASE + power, x * sx, y * sy,
            40.0 * sx, 40.0 * sy, [1.0; 4], width, height);
        x += 42.0;
        if x + 40.0 >= 560.0 { x = 96.0; y += 42.0; }
    }
    if recovering {
        // TaystJK additionally draws gfx/mp/f_icon_ragerec after the active icons.
        // Subtle alpha blink makes the remaining recovery state noticeable even
        // when a mod supplies a static replacement for that shader.
        let alpha = if (hud.time_ms.div_euclid(400) & 1) == 0 { 1.0 } else { 0.45 };
        icon(out, ui, ICON_RAGE_RECOVERY, x * sx, y * sy,
            40.0 * sx, 40.0 * sy, [1.0, 1.0, 1.0, alpha], width, height);
    }
}

/// This is a *late* UI tail: the lock icon must use the same view-projection
/// matrix as the 3D target and animate even between incoming snapshots.
pub(crate) fn build_combat_hud_vertices(
    out: &mut Vec<UiVertex>, ui: &UiSnapshot, rocket_screen: Option<[f32;2]>,
    width: u32, height: u32)
{
    if ui.video.skip_ui || !ui.video.draw_2d || ui.mode != super::OverlayMode::None { return; }
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    draw_active_force_icons(out, ui, width, height);
    if let Some(zoom) = ui.zoom_scope {
        if zoom.mode == 1 {
            // TaystJK's normal scope is a 640x480 mask centered in the 4:3
            // safe area, plus rotating insert. On wide screens the excluded
            // columns are solid black; scope mode 2 is the binocular overlay.
            let aspect = (h * (4.0/3.0)).min(w);
            let offset = (w-aspect)*0.5;
            if offset > 0.0 {
                rect(out, 0.0, 0.0, offset, h, [0.0,0.0,0.0,1.0], width, height);
                rect(out, offset+aspect, 0.0, offset, h, [0.0,0.0,0.0,1.0], width, height);
            }
            icon(out, ui, ICON_COMBAT_BASE, offset, 0.0, aspect, h,
                [1.0;4], width, height);
            let level = ((50.0-zoom.fov)/50.0).clamp(0.0,1.0)*103.0;
            let alpha = if level >= 99.0 {
                (0.7 + (zoom.time as f32*0.01).sin()*0.3).clamp(0.0,1.0)
            } else { 1.0 };
            rotated_icon(out, ui, ICON_COMBAT_BASE+1, w*0.5, h*0.5,
                aspect, h, -level, [1.0,1.0,1.0,alpha], width, height);
            // TaystJK CG_DrawZoomMask: energy ticks travel around the right
            // edge of the ring; depleted powercells shift from green to red.
            let ammo = (zoom.ammo as f32 / if zoom.double_ammo { 600.0 } else { 300.0 }).clamp(0.0,1.0);
            let tint = [(1.0-ammo).min(0.5)*2.0, (ammo*1.5).min(1.0), 0.0, 1.0];
            let scale_x = aspect/640.0;
            let scale_y = h/480.0;
            let mut fi = 18.5_f32;
            while fi <= 18.5 + ammo*58.0 {
                let rad = (fi+90.0).to_radians();
                let x = offset + (320.0+rad.sin()*190.0)*scale_x;
                let y = (240.0+rad.cos()*190.0)*scale_y;
                rotated_icon(out, ui, ICON_COMBAT_BASE+2, x, y, 12.0*scale_x, 24.0*scale_y,
                    90.0-fi, tint, width, height);
                fi += 3.0;
            }
            if let Some(start) = zoom.charge_started {
                let fraction = ((zoom.time-start).max(0) as f32 / 1500.0).clamp(0.0,1.0);
                if fraction > 0.0 {
                    // CG_DrawStretchPic uses the charge fraction as both U
                    // extent and rect width; display the same cropped art.
                    let x = offset + 257.0*scale_x;
                    let y = 435.0*scale_y;
                    let (uv0, uv1) = ui.icon_uv(ICON_COMBAT_BASE+3);
                    if uv1[0]-uv0[0] > 0.00001 {
                        textured_rect_with_source(out, x, y, 134.0*fraction*scale_x, 34.0*scale_y,
                            uv0, [uv0[0]+fraction*(uv1[0]-uv0[0]),uv1[1]], [1.0;4],
                            ui.icon_texture_source(ICON_COMBAT_BASE+3), width, height);
                    }
                }
            }
        } else if zoom.mode == 2 {
            // TaystJK CG_DrawZoomMask binocular branch: six authored images,
            // yaw-number tape, blue mask, pulsing indicator and zoom slider.
            let sx = w / 640.0;
            let sy = h / 480.0;
            let pixel = |x: f32, y: f32, dx: f32, dy: f32| (x*sx, y*sy, dx*sx, dy*sy);
            let (x,y,dx,dy)=pixel(34.0,48.0,570.0,362.0);
            icon(out, ui, ICON_COMBAT_BASE+5, x,y,dx,dy,[1.0;4],width,height);
            let (x,y,dx,dy)=pixel(212.0,367.0,200.0,40.0);
            rect(out, x,y,dx,dy,[0.0,0.0,0.0,1.0],width,height);
            let yaw = zoom.yaw_deg + 180.0;
            let base = ((yaw / 10.0) as i32)*10;
            let offset = yaw - base as f32;
            for i in [-10,0,10,20] {
                if i == -10 && offset <= 3.0 { continue; }
                let v = (base - (i+10)).rem_euclid(360) + 200;
                // CG_DrawNumField(NUM_FONT_CHUNKY): the same compact numbered
                // tape as the original, using our existing JKA font atlas.
                let x = (155.0 + i as f32*10.0 + offset*10.0)*sx;
                for (digit, ch) in format!("{v:03}").bytes().enumerate() {
                    glyph_quad_sized(out, ch, x+digit as f32*24.0*sx,374.0*sy,
                        24.0*sx,14.0*sy,[0.2,0.4,0.2,0.3],width,height);
                }
                let (x,y,dx,dy)=pixel(245.0+(i-1) as f32*10.0+offset*10.0,376.0,6.0,6.0);
                rect(out,x,y,dx,dy,[0.2,0.4,0.2,0.3],width,height);
            }
            let (x,y,dx,dy)=pixel(212.0,367.0,200.0,28.0);
            icon(out, ui, ICON_COMBAT_BASE+11,x,y,dx,dy,[1.0;4],width,height);
            let pulse = ((zoom.time as f32*0.01).sin()*0.5+0.5).powi(2);
            let (x,y,dx,dy)=pixel(82.0,94.0,16.0,16.0);
            icon(out,ui,ICON_COMBAT_BASE+9,x,y,dx,dy,[pulse,pulse,pulse,1.0],width,height);
            // Original Q_flrand flicker is nondeterministic; use a time-step
            // seeded flicker so demo scrubbing and output remain reproducible.
            let flicker=((zoom.time as u32).wrapping_mul(1664525).wrapping_add(1013904223) >> 16) as f32 / 65535.0;
            icon(out,ui,ICON_COMBAT_BASE+4,0.0,0.0,w,h,
                [0.6+0.2*flicker,0.7+0.2*flicker,0.6+0.2*flicker,1.0],width,height);
            let level=((80.0-zoom.fov)/80.0).clamp(0.0,1.0)*162.0;
            let (x,y,dx,dy)=pixel(4.0,282.0-level,16.0,16.0);
            icon(out,ui,ICON_COMBAT_BASE+6,x,y,dx,dy,[1.0;4],width,height);
            let flipped = ((zoom.time as u32 / 1024) & 1) != 0;
            let (x,y,dx,dy)=if flipped {pixel(330.0,60.0,-26.0,-30.0)}
                else {pixel(307.0,40.0,26.0,30.0)};
            icon(out,ui,ICON_COMBAT_BASE+10,x,y,dx,dy,[1.0;4],width,height);
        }
    }
    if let (Some(lock), Some(screen)) = (ui.rocket_lock, rocket_screen) {
        let distance = lock.target.iter().zip(lock.viewer.iter())
            .map(|(a,b)| (a-b)*(a-b)).sum::<f32>().sqrt();
        let size = (1.0-(distance/1024.0).clamp(0.0,1.0)).powi(2)*32.0+6.0;
        let (cx, cy) = (screen[0], screen[1]+size*0.5);
        for i in 0..lock.wedges.min(8) {
            let angle = f32::from(i)*45.0;
            rotated_icon(out, ui, ICON_COMBAT_BASE+7, cx, cy, size*2.0, size*2.0,
                angle, [1.0,0.0,0.0,0.2 + f32::from(i)*0.1], width, height);
        }
        if lock.wedges >= 8 {
            let pulse = (lock.time as f32*0.05).sin()*0.5+0.5;
            icon(out, ui, ICON_COMBAT_BASE+8, cx-size, cy-size*2.0,
                size*2.0, size*2.0, [pulse,pulse,pulse,1.0], width, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_state_adds_no_draws() {
        let mut out = Vec::new();
        let ui = UiSnapshot::default();
        build_combat_hud_vertices(&mut out, &ui, None, 1920, 1080);
        assert!(out.is_empty());
    }

    // Regression for the missing scope/utility overlays. The actual art is
    // loaded by the renderer from the game's VFS; these synthetic UVs prove
    // the draw path emits correctly textured batches when zoomMode changes.
    fn zoom_test_ui(mode: i32) -> UiSnapshot {
        let mut ui = UiSnapshot::default();
        ui.video.draw_2d = true;
        ui.video.skip_ui = false;
        ui.zoom_scope = Some(UiZoomScope {
            mode,
            fov: 30.0,
            time: 1000,
            yaw_deg: 90.0,
            ammo: 150,
            double_ammo: false,
            charge_started: None,
        });
        ui.icon_atlas_uvs = vec![([0.0,0.0],[0.8,0.8]); ICON_COMBAT_BASE+12];
        ui
    }

    #[test]
    fn disruptor_scope_draws_mask_and_rotating_insert_at_16_9() {
        let mut out = Vec::new();
        build_combat_hud_vertices(&mut out, &zoom_test_ui(1), None, 1920, 1080);
        // Widescreen black bars, a masked circle and multiple colored insert ticks.
        assert!(out.len() >= 24);
        assert!(out.iter().any(|v| v.textured == ICON_TEXTURE_SOURCE));
        assert!(out.iter().any(|v| v.textured == 0.0));
    }

    #[test]
    fn electrobinoculars_draw_all_layers_and_animated_yaw_tape() {
        let mut out = Vec::new();
        build_combat_hud_vertices(&mut out, &zoom_test_ui(2), None, 1920, 1080);
        // Window, blackout, tape glyphs, numerical overlay, pulsing indicator,
        // mask, arrow and upper triangular aperture.
        assert!(out.len() >= 90);
        assert!(out.iter().any(|v| v.textured == ICON_TEXTURE_SOURCE));
        assert!(out.iter().any(|v| v.textured == 1.0));
    }

    #[test]
    fn active_force_icons_follow_snapshot_bits_and_rage_recovery_timer() {
        let mut ui = UiSnapshot::default();
        ui.video.draw_2d = true;
        ui.video.skip_ui = false;
        ui.icon_atlas_uvs = vec![([0.0, 0.0], [0.8, 0.8]); ICON_RAGE_RECOVERY + 1];
        ui.hud = Some(crate::ui::HudState {
            force_powers_active: (1 << 14) | (1 << 8),
            rage_recovery_until_ms: 3000,
            time_ms: 2000,
            ..Default::default()
        });
        let mut vertices = Vec::new();
        build_combat_hud_vertices(&mut vertices, &ui, None, 1920, 1080);
        // Force Seeing, Force Rage and rage-recovery are each one 6-vertex quad.
        assert_eq!(vertices.len(), 18);
        ui.hud.as_mut().unwrap().time_ms = 3001;
        vertices.clear();
        build_combat_hud_vertices(&mut vertices, &ui, None, 1920, 1080);
        assert_eq!(vertices.len(), 12);
        ui.hud.as_mut().unwrap().force_icons_hidden = true;
        vertices.clear();
        build_combat_hud_vertices(&mut vertices, &ui, None, 1920, 1080);
        assert!(vertices.is_empty());
    }

    #[test]
    fn binocular_authored_shaders_keep_painter_order_with_native_yaw_tape() {
        let mut ui = zoom_test_ui(2);
        ui.icon_shader_flags = vec![false; super::super::ICON_NAMES.len()];
        ui.icon_shader_flags[ICON_COMBAT_BASE + 5] = true; // window
        ui.icon_shader_flags[ICON_COMBAT_BASE + 11] = true; // number-overlay
        let mut vertices = Vec::new();
        build_combat_hud_vertices(&mut vertices, &ui, None, 1920, 1080);
        let window = ui.icon_texture_source(ICON_COMBAT_BASE + 5);
        let numbers = ui.icon_texture_source(ICON_COMBAT_BASE + 11);
        let window_pos = vertices.iter().position(|v| v.textured == window).unwrap();
        let numbers_pos = vertices.iter().position(|v| v.textured == numbers).unwrap();
        assert!(window_pos < numbers_pos);
        assert!(vertices[window_pos+6..numbers_pos].iter().any(|v| v.textured == 1.0));
        assert_eq!(vertices[window_pos].uv, [0.0, 0.0]);
    }

    #[test]
    fn zoom_off_emits_no_scope_or_binocular_art() {
        let mut ui = zoom_test_ui(2);
        ui.zoom_scope = None;
        let mut out = Vec::new();
        build_combat_hud_vertices(&mut out, &ui, None, 1920, 1080);
        assert!(out.is_empty());
    }
}
