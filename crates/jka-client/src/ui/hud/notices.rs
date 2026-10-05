//! Hud notices.
use crate::ui::{
    center_print_lines, fixed_charset_text, proportional_text, proportional_text_width,
    visible_jka_chars, ProportionalFont, UiSnapshot, UiVertex,
};

pub(in crate::ui) fn build_center_print(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    medium_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(print) = &ui.center_print else {
        return;
    };
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

pub(in crate::ui) fn build_follow_indicator(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(follow) = ui.follow_name.as_deref() else {
        return;
    };
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
            proportional_text(
                out,
                text,
                font,
                x,
                y,
                scale * 0.78,
                [1.0, 1.0, 1.0, 1.0],
                false,
                w,
                h,
            );
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
pub(in crate::ui) fn build_vote(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(line) = ui.vote_line.as_deref() else {
        return;
    };
    let x = 4.0 * w as f32 / 640.0;
    let glyph_w = w as f32 * (8.0 / 640.0);
    let glyph_h = h as f32 * (12.0 / 480.0);
    let mut y = 62.0 * h as f32 / 480.0;
    for text in [line, "or press ESC then click Vote"] {
        fixed_charset_text(
            out,
            text,
            x,
            y,
            glyph_w,
            glyph_h,
            glyph_w,
            [1.0, 1.0, 1.0, 1.0],
            true,
            w,
            h,
        );
        y += h as f32 * (18.0 / 480.0);
    }
}
