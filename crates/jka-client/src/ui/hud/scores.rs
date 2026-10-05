//! Hud scores.
use crate::ui::{
    fixed_charset_text, player_icon_atlas_cell, rect, text, visible_jka_chars, ProportionalFont,
    UiMiniScores, UiSnapshot, UiVertex,
};

pub(in crate::ui) fn build_game_timer(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(label) = ui.game_timer.as_deref() else {
        return;
    };

    // TaystJK CG_DrawUpperRight stacks cg_drawTimer after cg_drawFPS and uses
    // the FPS text style when both are enabled. Our detailed FPS mode is a
    // native diagnostics panel, so keep the timer in its header instead of
    // letting the panel cover it. No millisecond variant is intentionally
    // exposed: cg_drawTimer is just the classic M:SS readout here.
    let scale = 1.55;
    let glyph_w = 6.0 * scale;
    let x = (w as f32 - 12.0 - label.chars().count() as f32 * glyph_w).max(12.0);
    let y = if ui.video.draw_fps == 1 { 34.0 } else { 12.0 };
    text(
        out,
        label,
        x + 1.0,
        y + 1.0,
        scale,
        [0.0, 0.0, 0.0, 0.72],
        w,
        h,
    );
    text(out, label, x, y, scale, [1.0; 4], w, h);
}

pub(in crate::ui) fn build_mini_scores(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    _small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(scores) = ui.mini_scores.as_ref() else {
        return;
    };
    let sx = w.max(1) as f32 / 640.0;
    let sy = h.max(1) as f32 / 480.0;
    // TaystJK uses cgs.widthRatioCoef for horizontally sized HUD pieces.
    // `ratio * sx == sy`, so glyphs/boxes stay square on widescreen instead
    // of stretching with the full 640x480 projection.
    let ratio = crate::lagometer::width_ratio_coef(w, h);
    let score_text =
        |score: Option<i32>| score.map_or_else(|| "-".to_owned(), |score| score.to_string());

    match scores {
        UiMiniScores::Team { mode, red, blue } if *mode == 1 || *mode == 2 => {
            // CG_DrawMiniScoreboard: a single right-aligned line, 0.7 medium
            // font. Mode 2 colours only the numeric team scores.
            let red = score_text(*red);
            let blue = score_text(*blue);
            let label = if *mode == 2 {
                format!("RED: ^1{red}^7 BLUE: ^4{blue}^7")
            } else {
                format!("RED: {red} BLUE: {blue}")
            };
            let scale = 1.25;
            let text_w = visible_jka_chars(&label) as f32 * 6.0 * scale;
            let x = (w as f32 - 10.0 - text_w).max(8.0);
            // In Tayst this receives CG_DrawUpperRight's running y. Our upper
            // right stack currently consists of FPS and the classic timer.
            let rows = (ui.video.draw_fps != 0) as usize + ui.game_timer.is_some() as usize;
            let y = 12.0 + rows as f32 * 22.0;
            fixed_charset_text(
                out,
                &label,
                x,
                y,
                6.0 * scale,
                8.0 * scale,
                6.0 * scale,
                [1.0; 4],
                true,
                w,
                h,
            );
        }
        UiMiniScores::Team { mode: 3, red, blue } => {
            // CG_DrawTeamHUD: equal-width blue-left/red-right boxes, centred
            // standalone when cg_drawTimer != 7. This client currently exposes
            // the classic timer only, so this is exactly that standalone path.
            const BLUE: [f32; 4] = [0.02, 0.40, 0.65, 0.70];
            const RED: [f32; 4] = [0.65, 0.01, 0.02, 0.70];
            let blue = score_text(*blue);
            let red = score_text(*red);
            let glyph_w_v = 10.0 * ratio;
            let glyph_h_v = 16.0;
            let widest = visible_jka_chars(&blue).max(visible_jka_chars(&red)) as f32;
            let box_w_v = widest * glyph_w_v + 10.0 * ratio;
            let box_h_v = 20.0;
            let y_v = 12.0;
            let blue_x_v = 320.0 - box_w_v;
            let red_x_v = 320.0;
            rect(
                out,
                blue_x_v * sx,
                y_v * sy,
                box_w_v * sx,
                box_h_v * sy,
                BLUE,
                w,
                h,
            );
            rect(
                out,
                red_x_v * sx,
                y_v * sy,
                box_w_v * sx,
                box_h_v * sy,
                RED,
                w,
                h,
            );

            let draw_score = |out: &mut Vec<UiVertex>, value: &str, box_x_v: f32| {
                let text_w_v = visible_jka_chars(value) as f32 * glyph_w_v;
                let x = (box_x_v + (box_w_v - text_w_v) * 0.5) * sx;
                let y = (y_v + (box_h_v - glyph_h_v) * 0.5) * sy;
                fixed_charset_text(
                    out,
                    value,
                    x,
                    y,
                    glyph_w_v * sx,
                    glyph_h_v * sy,
                    glyph_w_v * sx,
                    [1.0; 4],
                    true,
                    w,
                    h,
                );
            };
            draw_score(out, &blue, blue_x_v);
            draw_score(out, &red, red_x_v);
        }
        UiMiniScores::Duel {
            icon_paths,
            blue,
            red,
        } => {
            // CG_DrawDuelHUD: duelist1 is blue/left and duelist2 red/right,
            // including each clientInfo modelIcon beside the equal score boxes.
            const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 0.70];
            const BLUE: [f32; 4] = [0.02, 0.40, 0.65, 0.70];
            const RED: [f32; 4] = [0.65, 0.01, 0.02, 0.70];
            const NAME: [f32; 4] = [0.60, 0.60, 0.60, 1.0];
            let blue_score = score_text(blue.score);
            let red_score = score_text(red.score);
            let score_glyph_w_v = 14.0 * ratio;
            let score_glyph_h_v = 22.0;
            let widest = visible_jka_chars(&blue_score).max(visible_jka_chars(&red_score)) as f32;
            let box_w_v = widest * score_glyph_w_v + 15.0 * ratio;
            let box_h_v = 20.0;
            let y_v = 8.0;
            let blue_x_v = 320.0 - box_w_v;
            let red_x_v = 320.0;
            rect(
                out,
                blue_x_v * sx,
                y_v * sy,
                box_w_v * sx,
                box_h_v * sy,
                BLUE,
                w,
                h,
            );
            rect(
                out,
                red_x_v * sx,
                y_v * sy,
                box_w_v * sx,
                box_h_v * sy,
                RED,
                w,
                h,
            );

            // Tayst: iconHeight = background.h - 3, blue icon sits one
            // virtual unit in from the left extension and red two units out.
            let icon_w_v = box_h_v * ratio;
            let icon_h_v = box_h_v - 3.0;
            player_icon_atlas_cell(
                out,
                icon_paths.len(),
                blue.model_icon,
                (blue_x_v - icon_w_v + ratio) * sx,
                (y_v + 1.0) * sy,
                (icon_w_v - 3.0 * ratio) * sx,
                icon_h_v * sy,
                w,
                h,
            );
            player_icon_atlas_cell(
                out,
                icon_paths.len(),
                red.model_icon,
                (red_x_v + box_w_v + 2.0 * ratio) * sx,
                (y_v + 1.0) * sy,
                (icon_w_v - 3.0 * ratio) * sx,
                icon_h_v * sy,
                w,
                h,
            );

            let draw_score = |out: &mut Vec<UiVertex>, value: &str, box_x_v: f32| {
                let text_w_v = visible_jka_chars(value) as f32 * score_glyph_w_v;
                fixed_charset_text(
                    out,
                    value,
                    (box_x_v + (box_w_v - text_w_v) * 0.5) * sx,
                    (y_v - 2.0) * sy,
                    score_glyph_w_v * sx,
                    score_glyph_h_v * sy,
                    score_glyph_w_v * sx,
                    [1.0; 4],
                    false,
                    w,
                    h,
                );
            };
            draw_score(out, &blue_score, blue_x_v);
            draw_score(out, &red_score, red_x_v);

            let name_glyph_w_v = 5.0 * ratio;
            let name_glyph_h_v = 8.0;
            let name_h_v = name_glyph_h_v + 2.0;
            // Tayst's strips include one model-icon-width outside each score box.
            rect(
                out,
                (blue_x_v - icon_w_v) * sx,
                (y_v + box_h_v) * sy,
                (box_w_v + icon_w_v) * sx,
                name_h_v * sy,
                BLACK,
                w,
                h,
            );
            rect(
                out,
                red_x_v * sx,
                (y_v + box_h_v) * sy,
                (box_w_v + icon_w_v) * sx,
                name_h_v * sy,
                BLACK,
                w,
                h,
            );
            let blue_name_w_v = visible_jka_chars(&blue.name) as f32 * name_glyph_w_v;
            fixed_charset_text(
                out,
                &blue.name,
                (blue_x_v + box_w_v - blue_name_w_v - ratio) * sx,
                (y_v + box_h_v + 1.0) * sy,
                name_glyph_w_v * sx,
                name_glyph_h_v * sy,
                name_glyph_w_v * sx,
                NAME,
                false,
                w,
                h,
            );
            fixed_charset_text(
                out,
                &red.name,
                (red_x_v + ratio) * sx,
                (y_v + box_h_v + 1.0) * sy,
                name_glyph_w_v * sx,
                name_glyph_h_v * sy,
                name_glyph_w_v * sx,
                NAME,
                false,
                w,
                h,
            );
        }
        _ => {}
    }
}
