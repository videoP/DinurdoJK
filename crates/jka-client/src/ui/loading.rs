//! Loading.
use crate::ui::{
    lighten, rect, rect_gradient, text, textured_rect_with_source, with_alpha, MapLoadingBar,
    MapLoadingUi, UiVertex, HUD_INK_BOTTOM, HUD_INK_TOP, HUD_VALUE,
};

/// Neon edge (cyan to magenta) with a soft glow on `glow_dir` (+1 below, -1 above).
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn neon_edge(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    width: f32,
    thickness: f32,
    glow_dir: f32,
    w: u32,
    h: u32,
) {
    let l = with_alpha(LOAD_CYAN, 0.95);
    let r = with_alpha(LOAD_MAGENTA, 0.95);
    rect_gradient(out, x, y, width, thickness, l, r, l, r, w, h);
    let mut offset = if glow_dir > 0.0 { thickness } else { 0.0 };
    for (height, alpha) in [(3.0, 0.16), (4.0, 0.07), (6.0, 0.025)] {
        let gl = with_alpha(LOAD_CYAN, alpha);
        let gr = with_alpha(LOAD_MAGENTA, alpha);
        let top = if glow_dir > 0.0 {
            y + offset
        } else {
            y - offset - height
        };
        rect_gradient(out, x, top, width, height, gl, gr, gl, gr, w, h);
        offset += height;
    }
}

/// Thin progress track: dim rail, gradient fill from `from` to `to`.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn progress_track(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    fraction: f32,
    from: [f32; 4],
    to: [f32; 4],
    w: u32,
    h: u32,
) {
    rect(out, x, y, width, height, [0.05, 0.08, 0.14, 0.96], w, h);
    let fill = width * fraction.clamp(0.0, 1.0);
    if fill > 0.5 {
        rect_gradient(out, x, y, fill, height, from, to, from, to, w, h);
        let edge = 2.0_f32.min(fill);
        rect(
            out,
            x + fill - edge,
            y,
            edge,
            height,
            lighten(to, 0.55),
            w,
            h,
        );
    }
}

pub(in crate::ui) fn loading_fraction(bar: &MapLoadingBar) -> f32 {
    if bar.total > 0 {
        (bar.completed as f32 / bar.total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub(in crate::ui) fn build_background_progress(
    out: &mut Vec<UiVertex>,
    progress: &MapLoadingBar,
    w: u32,
    h: u32,
) {
    let panel_w = 320.0;
    let panel_h = 44.0;
    let x = (w as f32 - panel_w - 18.0).max(8.0);
    let y = (h as f32 - panel_h - 18.0).max(8.0);
    rect_gradient(
        out,
        x,
        y,
        panel_w,
        panel_h,
        HUD_INK_TOP,
        HUD_INK_TOP,
        HUD_INK_BOTTOM,
        HUD_INK_BOTTOM,
        w,
        h,
    );
    rect(out, x, y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.10], w, h);
    let l = with_alpha(LOAD_CYAN, 0.9);
    let r = with_alpha(LOAD_MAGENTA, 0.9);
    rect_gradient(out, x, y, panel_w, 1.5, l, r, l, r, w, h);
    let fraction = loading_fraction(progress);
    text(
        out,
        progress.label,
        x + 12.0,
        y + 9.0,
        1.1,
        LOAD_LABEL,
        w,
        h,
    );
    let percent = format!("{}%", (fraction * 100.0).round() as u32);
    let percent_w = percent.len() as f32 * 6.0 * 1.3;
    text(
        out,
        &percent,
        x + panel_w - 12.0 - percent_w,
        y + 8.0,
        1.3,
        HUD_VALUE,
        w,
        h,
    );
    progress_track(
        out,
        x + 12.0,
        y + 29.0,
        panel_w - 24.0,
        5.0,
        fraction,
        with_alpha(LOAD_CYAN, 0.55),
        LOAD_CYAN,
        w,
        h,
    );
}

pub(in crate::ui) fn build_splash_background(
    out: &mut Vec<UiVertex>,
    w: u32,
    h: u32,
    splash_size: Option<[u32; 2]>,
) {
    rect(
        out,
        0.0,
        0.0,
        w as f32,
        h as f32,
        [0.0, 0.0, 0.0, 1.0],
        w,
        h,
    );

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
}

pub(in crate::ui) fn build_loading_screen(
    out: &mut Vec<UiVertex>,
    loading: &MapLoadingUi,
    w: u32,
    h: u32,
    splash_size: Option<[u32; 2]>,
) {
    build_splash_background(out, w, h, splash_size);

    // Darken the lower half so the panel reads over any splash artwork.
    let fade_h = h as f32 * 0.55;
    let clear = [0.0, 0.0, 0.02, 0.0];
    let dark = [0.0, 0.0, 0.02, 0.80];
    rect_gradient(
        out,
        0.0,
        h as f32 - fade_h,
        w as f32,
        fade_h,
        clear,
        clear,
        dark,
        dark,
        w,
        h,
    );

    // Preparation finishing is not the end of the load: the renderer still has
    // to upload the world and present its first frame. That runs on the render
    // thread with no measurable progress, so show it as a final row that stays
    // WORKING (and keeps the total below 100%) until the loading screen closes.
    let upload_bar = MapLoadingBar {
        label: "GPU UPLOAD",
        completed: 0,
        total: 1,
        skipped: false,
    };
    let mut visible: Vec<&MapLoadingBar> = loading
        .bars
        .iter()
        .filter(|bar| !(bar.skipped || (loading.preparation_finished && bar.total == 0)))
        .collect();
    if loading.preparation_finished {
        visible.push(&upload_bar);
    }
    let overall = if visible.is_empty() {
        0.0
    } else {
        visible.iter().map(|bar| loading_fraction(bar)).sum::<f32>() / visible.len() as f32
    };

    let panel_w = (w as f32 - 48.0).clamp(360.0, 760.0);
    let row_h = 27.0;
    let rows_y = 82.0;
    let panel_h = rows_y + visible.len() as f32 * row_h + 20.0;
    let x = (w as f32 - panel_w) * 0.5;
    let y = (h as f32 - panel_h - 34.0).max(24.0);
    rect_gradient(
        out,
        x,
        y,
        panel_w,
        panel_h,
        [0.012, 0.016, 0.038, 0.92],
        [0.012, 0.016, 0.038, 0.92],
        [0.022, 0.028, 0.064, 0.94],
        [0.022, 0.028, 0.064, 0.94],
        w,
        h,
    );
    neon_edge(out, x, y, panel_w, 2.0, -1.0, w, h);

    // Header: kicker, map name, overall percentage.
    text(out, "LOADING", x + 22.0, y + 15.0, 1.1, LOAD_CYAN, w, h);
    text(
        out,
        &loading.map_name.to_ascii_uppercase(),
        x + 22.0,
        y + 29.0,
        2.2,
        [0.95, 0.97, 1.0, 1.0],
        w,
        h,
    );
    let percent = format!("{}%", (overall * 100.0).round() as u32);
    let percent_w = percent.len() as f32 * 6.0 * 2.2;
    text(
        out,
        &percent,
        x + panel_w - 22.0 - percent_w,
        y + 29.0,
        2.2,
        LOAD_AMBER,
        w,
        h,
    );
    progress_track(
        out,
        x + 22.0,
        y + 58.0,
        panel_w - 44.0,
        6.0,
        overall,
        LOAD_CYAN,
        LOAD_MAGENTA,
        w,
        h,
    );

    let bar_x = x + 174.0;
    let bar_w = panel_w - 270.0;
    for (index, bar) in visible.iter().enumerate() {
        let row_y = y + rows_y + index as f32 * row_h;
        let done = bar.total > 0 && bar.completed >= bar.total;
        let fraction = loading_fraction(bar);
        text(
            out,
            bar.label,
            x + 22.0,
            row_y + 4.0,
            1.25,
            if bar.total == 0 {
                LOAD_FAINT
            } else {
                LOAD_LABEL
            },
            w,
            h,
        );
        let (from, to) = if done {
            (with_alpha(LOAD_DONE, 0.6), LOAD_DONE)
        } else {
            (with_alpha(LOAD_CYAN, 0.5), LOAD_CYAN)
        };
        progress_track(
            out,
            bar_x,
            row_y + 6.0,
            bar_w,
            6.0,
            fraction,
            from,
            to,
            w,
            h,
        );
        let status = if bar.total == 0 {
            "WAIT".to_string()
        } else if done {
            "DONE".to_string()
        } else if bar.total == 1 {
            // A single inline step has no meaningful "0/1".
            "WORKING".to_string()
        } else {
            format!("{}/{}", bar.completed, bar.total)
        };
        text(
            out,
            &status,
            x + panel_w - 76.0,
            row_y + 4.0,
            1.2,
            if done {
                LOAD_DONE
            } else if bar.total == 0 {
                LOAD_FAINT
            } else {
                LOAD_LABEL
            },
            w,
            h,
        );
    }
}

pub(in crate::ui) const LOAD_CYAN: [f32; 4] = [0.28, 0.86, 0.95, 1.0];

pub(in crate::ui) const LOAD_MAGENTA: [f32; 4] = [1.0, 0.30, 0.56, 1.0];

pub(in crate::ui) const LOAD_AMBER: [f32; 4] = [1.0, 0.72, 0.28, 1.0];

pub(in crate::ui) const LOAD_DONE: [f32; 4] = [0.30, 0.85, 0.66, 1.0];

pub(in crate::ui) const LOAD_LABEL: [f32; 4] = [0.62, 0.72, 0.84, 1.0];

pub(in crate::ui) const LOAD_FAINT: [f32; 4] = [0.38, 0.46, 0.58, 1.0];
