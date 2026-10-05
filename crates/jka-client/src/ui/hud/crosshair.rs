//! Hud crosshair.
use crate::ui::{
    disc, fixed_charset_text, gameplay_hud_visible, icon_cell_uv, proportional_text,
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
        color,
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
    let Some(name) = &ui.crosshair_target.name else {
        return;
    };
    if ui.crosshair.style == 0 || name.alpha <= 0.001 {
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
        let x =
            ((640.0 - proportional_text_width(&name.text, font, scale)) * 0.5) * w as f32 / 640.0;
        proportional_text(out, &name.text, font, x, y, scale, color, true, w, h);
        return;
    }
    let glyph_w = w as f32 * (10.0 / 640.0);
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
    // OpenJK/TaystJK forcePowerSorted[] and CG_DrawForceSelect().
    const FORCE_POWER_SORTED: [u8; 18] =
        [5, 0, 10, 9, 11, 1, 2, 3, 4, 14, 7, 13, 8, 6, 12, 15, 16, 17];
    const NAMES: [&str; 18] = [
        "Heal",
        "Levitation",
        "Speed",
        "Push",
        "Pull",
        "Mind Trick",
        "Grip",
        "Lightning",
        "Rage",
        "Protect",
        "Absorb",
        "Team Heal",
        "Team Energize",
        "Drain",
        "Seeing",
        "Saber Offense",
        "Saber Defense",
        "Saber Throw",
    ];
    let Some(selector) = ui.force_select else {
        return;
    };
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
        let (uv0, uv1) = icon_cell_uv(ICON_FORCE_BASE + usize::from(power));
        textured_rect_with_source(
            out,
            x * sx,
            y * sy,
            size * sx,
            size * sy,
            uv0,
            uv1,
            [1.0; 4],
            ICON_TEXTURE_SOURCE,
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

    let name = NAMES[usize::from(selector.selected)];
    let text_y = 455.0 * sy;
    if let Some(font) = small_font {
        let scale = 0.55;
        let x = ((640.0 - proportional_text_width(name, font, scale)) * 0.5) * sx;
        proportional_text(out, name, font, x, text_y, scale, [1.0; 4], true, w, h);
    } else {
        let glyph_w = 8.0 * sx;
        let glyph_h = 12.0 * sy;
        let x = (w as f32 - name.len() as f32 * glyph_w) * 0.5;
        fixed_charset_text(
            out, name, x, text_y, glyph_w, glyph_h, glyph_w, [1.0; 4], true, w, h,
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
    if !gameplay_hud_visible(ui, w, h) {
        return;
    }
    let mut crosshair = ui.crosshair;
    if ui.strafe_helper.flags & SHELPER_CROSSHAIR != 0 {
        crosshair.style = CROSSHAIR_STYLE_LINE;
        crosshair.image = 0;
    }
    build_crosshair(
        out,
        crosshair,
        ui.crosshair_target.color,
        screen_position,
        ui.strafe_helper.line_width,
        w,
        h,
    );
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
        let (uv0, uv1) = icon_cell_uv(ICON_CROSSHAIR_BASE + usize::from(crosshair.image) - 1);
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
