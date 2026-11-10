//! Hud crosshair.
use crate::ui::{
    disc, fixed_charset_text, gameplay_hud_visible, icon_cell_uv, proportional_text_uniform,
    proportional_text_width, rect, rect_outline, textured_rect_with_source, visible_jka_chars,
    CrosshairSettings, ProportionalFont, UiSnapshot, UiVertex, CROSSHAIR_IMAGE_COUNT,
    CROSSHAIR_STRENGTH_MAX, CROSSHAIR_STYLE_LINE, ICON_CROSSHAIR_BASE, ICON_FORCE_BASE,
    ICON_TEXTURE_SOURCE, SHELPER_CROSSHAIR,
};

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn hud_line(
    out: &mut Vec<UiVertex>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width_px: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len <= f32::EPSILON {
        return;
    }
    let half = width_px.max(0.25) * 0.5;
    let nx = -dy / len * half;
    let ny = dx / len * half;
    let to_ndc = |x: f32, y: f32| {
        [
            x / width.max(1) as f32 * 2.0 - 1.0,
            1.0 - y / height.max(1) as f32 * 2.0,
        ]
    };
    let make = |x: f32, y: f32| UiVertex {
        position: to_ndc(x, y),
        uv: [0.0, 0.0],
        color: UiVertex::pack_color(color),
        textured: 0.0,
    };
    let a = make(x0 + nx, y0 + ny);
    let b = make(x0 - nx, y0 - ny);
    let c = make(x1 - nx, y1 - ny);
    let d = make(x1 + nx, y1 + ny);
    out.extend_from_slice(&[a, b, c, a, c, d]);
}

/// `CG_DrawCrosshairNames`: the aimed-at player's name, centred at y = 170.
pub(in crate::ui) fn build_crosshair_name(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    if ui.falling_death_active { return; }
    let Some(name) = &ui.crosshair_target.name else {
        return;
    };
    if (ui.crosshair.style == 0 && !ui.studio_crosshair_name) || name.alpha <= 0.001 {
        return;
    }
    let color = [
        name.color[0],
        name.color[1],
        name.color[2],
        name.alpha.clamp(0.0, 1.0),
    ];
    let y = 170.0 * h as f32 / 480.0;
    if let Some(font) = small_font {
        // The medium-font role on the shared OCR font, as for the centre print.
        let scale = 0.78;
        let text_width = proportional_text_width(&name.text, font, scale) * h as f32 / 480.0;
        let x = (w as f32 - text_width) * 0.5;
        proportional_text_uniform(out, &name.text, font, x, y, scale, color, true, w, h);
        return;
    }
    let glyph_w = h as f32 * (10.0 / 480.0);
    let glyph_h = h as f32 * (16.0 / 480.0);
    let x = (w as f32 - visible_jka_chars(&name.text) as f32 * glyph_w) * 0.5;
    fixed_charset_text(
        out, &name.text, x, y, glyph_w, glyph_h, glyph_w, color, true, w, h,
    );
}

pub(in crate::ui) fn build_force_select(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    if ui.native_replaced(crate::extensions::NativeHud::ForceSelect) { return; }
    // OpenJK/TaystJK forcePowerSorted[] and CG_DrawForceSelect().
    use super::movement::{FORCE_POWER_SORTED, FORCE_POWER_NAMES};
    // JK2 source: CODE-mp/cgame/cg_draw.c showPowersName[] / CT_ICON_BLUE.
    const JK2_NAMES: [&str; 18] = [
        "Heal", "Jump", "Speed", "Push", "Pull", "Mind Trick", "Grip",
        "Lightning", "Dark Rage", "Protect", "Absorb", "Team Heal",
        "Team Replenish", "Drain", "Seeing", "Saber Attack",
        "Saber Defend", "Saber Throw",
    ];
    let Some(selector) = ui.force_select else {
        return;
    };
    if ui.jk2_mode && ui.video.hud_style == 1 && selector.remaining_ms <= 130 { return; }
    let valid = |power: u8| {
        power < 18
            && selector.known_bits & (1_u32 << power) != 0
            && !matches!(power, 1 | 15 | 16 | 17)
    };
    if !valid(selector.selected) {
        return;
    }
    let Some(selected_sorted) = FORCE_POWER_SORTED
        .iter()
        .position(|&p| p == selector.selected)
    else {
        return;
    };

    let count = FORCE_POWER_SORTED
        .iter()
        .copied()
        .filter(|&power| valid(power))
        .count();
    if count == 0 {
        return;
    }

    // CG_DrawForceSelect: at most three neighbour icons on either side. For six
    // powers JKA intentionally uses 2 left / 3 right; for seven or more, 3 / 3.
    let hold_count = count - 1;
    let (left_count, right_count) = if hold_count == 0 {
        (0, 0)
    } else if count > 6 {
        (3, 3)
    } else {
        let left = hold_count / 2;
        (left, hold_count - left)
    };

    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let small = 30.0;
    let big = 60.0;
    let pad = 12.0;
    let center_x = 320.0;
    let y = 425.0;

    let draw_icon = |out: &mut Vec<UiVertex>, power: u8, x: f32, y: f32, size: f32| {
        let (uv0, uv1) = ui.icon_uv(ICON_FORCE_BASE + usize::from(power));
        // Virtual 640x480 UI coordinates, but preserve square image pixels
        // on widescreen: the old `sx` made Force icons 33% wider at 16:9.
        let x_px = w as f32 * 0.5 + (x - 320.0) * sy;
        textured_rect_with_source(
            out,
            x_px,
            y * sy,
            size * sy,
            size * sy,
            uv0,
            uv1,
            [1.0; 4],
            ui.icon_texture_source(ICON_FORCE_BASE + usize::from(power)),
            w,
            h,
        );
    };

    // Work backwards/forwards through forcePowerSorted[] with wrap, skipping
    // powers ForcePower_Valid() rejects. This is the important JKA behaviour;
    // neighbours are not simply clipped at either end of the sorted array.
    let mut sorted_index = selected_sorted;
    let mut hold_x = center_x - ((big * 0.5) + pad + small);
    for _ in 0..left_count {
        loop {
            sorted_index = if sorted_index == 0 {
                FORCE_POWER_SORTED.len() - 1
            } else {
                sorted_index - 1
            };
            let power = FORCE_POWER_SORTED[sorted_index];
            if valid(power) {
                draw_icon(out, power, hold_x, y, small);
                hold_x -= small + pad;
                break;
            }
        }
    }

    draw_icon(
        out,
        selector.selected,
        center_x - big * 0.5,
        y - (big - small) * 0.5,
        big,
    );

    sorted_index = selected_sorted;
    hold_x = center_x + big * 0.5 + pad;
    for _ in 0..right_count {
        loop {
            sorted_index = (sorted_index + 1) % FORCE_POWER_SORTED.len();
            let power = FORCE_POWER_SORTED[sorted_index];
            if valid(power) {
                draw_icon(out, power, hold_x, y, small);
                hold_x += small + pad;
                break;
            }
        }
    }

    let is_jk2_native = ui.jk2_mode && ui.video.hud_style == 1;
    let name = if is_jk2_native { JK2_NAMES[usize::from(selector.selected)] }
        else { FORCE_POWER_NAMES[usize::from(selector.selected)] };
    let tint = if is_jk2_native { [0.567, 0.685, 1.0, 0.75] } else { [1.0; 4] };
    let text_y = 455.0 * sy;
    if let Some(font) = small_font {
        let scale = 0.55;
        let x = (w as f32 - proportional_text_width(name, font, scale) * sy) * 0.5;
        proportional_text_uniform(out, name, font, x, text_y, scale, tint, true, w, h);
    } else {
        let glyph_w = 8.0 * sy;
        let glyph_h = 12.0 * sy;
        let x = (w as f32 - name.len() as f32 * glyph_w) * 0.5;
        fixed_charset_text(
            out, name, x, text_y, glyph_w, glyph_h, glyph_w, tint, true, w, h,
        );
    }
}

/// Build the ordinary JKA crosshair at an optional framebuffer coordinate.
/// The render thread uses this for dynamic crosshairs after late-latching.
pub fn build_crosshair_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    screen_position: Option<[f32; 2]>,
    w: u32,
    h: u32,
) {
    if ui.falling_death_active || !gameplay_hud_visible(ui, w, h) {
        return;
    }
    // TaystJK CG_DrawCrosshair: no ordinary crosshair over zoomMode 1 or 2,
    // except cg_crossHairScope 1. This shared entry point covers both the
    // cached screen-centred and the late-latched dynamic-crosshair paths.
    if ui.zoom_scope.is_some() && ui.crosshair.scope_mode != 1 {
        return;
    }
    if ui.studio_hud_virtual_layout && ui.crosshair.style == 0 { return; }
    let mut crosshair = ui.crosshair;
    if ui.strafe_helper.flags & SHELPER_CROSSHAIR != 0 {
        crosshair.style = CROSSHAIR_STYLE_LINE;
        crosshair.image = 0;
    }
    let start=out.len();
    build_crosshair_impl(
        out,
        crosshair,
        ui.crosshair_target.color,
        screen_position,
        ui.strafe_helper.line_width,
        w,
        h,
        Some(&ui.icon_atlas_uvs),
        Some(&ui.icon_shader_flags),
    );
    // The Studio reuses its otherwise-unused CrosshairName layout slot to
    // position the *actual* crosshair. This transform only runs in Studio:
    // cg_hudCrosshairName and normal gameplay remain unchanged.
    if ui.studio_hud_virtual_layout && out.len()>start && w>0 && h>0 {
        let layout=ui.hud_layout.crosshair_name;
        let center=screen_position.unwrap_or([w as f32*0.5,h as f32*0.5]);
        for v in &mut out[start..] {
            let x=(v.position[0]+1.0)*0.5*w as f32;
            let y=(1.0-v.position[1])*0.5*h as f32;
            let x=center[0]+(x-center[0])*layout.scale+layout.offset[0];
            let y=center[1]+(y-center[1])*layout.scale+layout.offset[1];
            v.position=[x*2.0/w as f32-1.0,1.0-y*2.0/h as f32];
        }
    }
}

pub(in crate::ui) fn build_crosshair(
    out: &mut Vec<UiVertex>,
    crosshair: CrosshairSettings,
    target_color: Option<[f32; 3]>,
    screen_position: Option<[f32; 2]>,
    line_width: f32,
    w: u32,
    h: u32,
) {
    build_crosshair_impl(out, crosshair, target_color, screen_position, line_width, w, h, None, None);
}

fn build_crosshair_impl(
    out: &mut Vec<UiVertex>,
    crosshair: CrosshairSettings,
    target_color: Option<[f32; 3]>,
    screen_position: Option<[f32; 2]>,
    line_width: f32,
    w: u32,
    h: u32,
    atlas_uvs: Option<&[([f32; 2], [f32; 2])]>,
    shader_flags: Option<&[bool]>,
) {
    if crosshair.style == 0 {
        return;
    }

    let [cx, cy] = screen_position.unwrap_or([w as f32 * 0.5, h as f32 * 0.5]);
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
        let index = ICON_CROSSHAIR_BASE + usize::from(crosshair.image) - 1;
        let shader_authored = shader_flags
            .and_then(|flags| flags.get(index).copied()).unwrap_or(false);
        let (uv0, uv1) = if shader_authored { ([0.0, 0.0], [1.0, 1.0]) }
            else { atlas_uvs.and_then(|uvs| uvs.get(index).copied())
                .unwrap_or_else(|| icon_cell_uv(index)) };
        // The stock artwork is thin and translucent. Above 100% strength the same
        // image is drawn again, each pass raising the coverage of its faint pixels;
        // up to three extra passes at the maximum, the last one fractional.
        let extra = (strength - 1.0).max(0.0) * 3.0;
        let passes = 1 + extra.ceil() as usize;
        for pass in 0..passes {
            let weight = if pass == 0 {
                1.0
            } else {
                (extra - (pass - 1) as f32).min(1.0)
            };
            textured_rect_with_source(
                out,
                cx - side * 0.5,
                cy - side * 0.5,
                side,
                side,
                uv0,
                uv1,
                [c[0], c[1], c[2], c[3] * weight],
                if shader_authored {
                    crate::ui::ICON_SHADER_SOURCE_BASE + index as f32
                } else { ICON_TEXTURE_SOURCE },
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
        let line_w = line_width.clamp(0.25, 5.0)
            * super::layout::hud_art_scale(w, h);
        let line_h = size * 1.25;
        rect(
            out,
            cx - line_w * 0.5,
            cy - line_h * 0.5,
            line_w,
            line_h,
            c,
            w,
            h,
        );
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
            rect_outline(out, cx - half, cy - half, size, size, thickness, c, w, h);
        }
    }
}

#[cfg(test)]
mod icon_aspect_tests {
    use super::*;

    #[test]
    fn procedural_line_width_uses_height_not_widescreen_width() {
        let settings = CrosshairSettings { style: CROSSHAIR_STYLE_LINE, ..Default::default() };
        for (w, h) in [(1280, 720), (2560, 720), (3440, 1440)] {
            let mut vertices = Vec::new();
            build_crosshair(&mut vertices, settings, None, None, 2.0, w, h);
            let x = vertices.iter().map(|v| (v.position[0] + 1.0) * w as f32 / 2.0);
            let width = x.clone().fold(f32::NEG_INFINITY, f32::max)
                - x.fold(f32::INFINITY, f32::min);
            assert!((width - 2.0 * h as f32 / 480.0).abs() < 0.01);
        }
    }

    #[test]
    fn force_selector_artwork_remains_square_on_widescreen() {
        let mut ui = UiSnapshot::default();
        ui.video.draw_hud = true;
        ui.force_select = Some(crate::ui::UiForceSelect {
            selected: 5, known_bits: (1 << 5) | (1 << 2),
            remaining_ms: 1400,
        });
        let (w, h) = (1920_u32, 1080_u32);
        let mut vertices = Vec::new();
        build_force_select(&mut vertices, &ui, None, w, h);
        let quad = &vertices[0..6];
        let xx = quad.iter().map(|v| (v.position[0] + 1.0) * w as f32 / 2.0).collect::<Vec<_>>();
        let yy = quad.iter().map(|v| (1.0 - v.position[1]) * h as f32 / 2.0).collect::<Vec<_>>();
        let rw = xx.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xx.iter().cloned().fold(f32::INFINITY, f32::min);
        let rh = yy.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - yy.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!((rw - rh).abs() < 0.01);
        ui.native_hidden = crate::extensions::NativeHudMask::from_names(
            &["force_select".to_string()]).unwrap();
        vertices.clear();
        build_force_select(&mut vertices, &ui, None, w, h);
        assert!(vertices.is_empty());
        ui.video.draw_hud = false;
        build_force_select(&mut vertices, &ui, None, w, h);
        assert!(!vertices.is_empty());
    }
}
