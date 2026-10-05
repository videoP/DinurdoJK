//! Demo.
use crate::ui::{demo_timeline_layout, rect, text, UiSnapshot, UiVertex};

pub(in crate::ui) fn build_demo_timeline(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(state) = &ui.demo_timeline else {
        return;
    };
    let Some(layout) = demo_timeline_layout(w, h) else {
        return;
    };

    if state.outside_authoritative_view {
        rect(
            out,
            0.0,
            0.0,
            w as f32,
            h as f32,
            [0.55, 0.0, 0.0, 0.20],
            w,
            h,
        );
        text(
            out,
            "OUTSIDE RECORDED PVS / PORTAL VISIBILITY",
            18.0,
            18.0,
            1.15,
            [1.0, 0.72, 0.72, 1.0],
            w,
            h,
        );
    }
    let panel_y = h as f32 - 46.0;
    text(
        out,
        &format!(
            "{}   [ / ] POV   F FREE   HOME RECORDED",
            state.camera_label
        ),
        16.0,
        panel_y - 18.0,
        0.90,
        [0.78, 0.84, 0.90, 0.95],
        w,
        h,
    );
    rect(
        out,
        8.0,
        panel_y,
        w as f32 - 16.0,
        42.0,
        [0.01, 0.016, 0.024, 0.84],
        w,
        h,
    );
    rect(
        out,
        8.0,
        panel_y,
        w as f32 - 16.0,
        1.0,
        [0.24, 0.31, 0.40, 0.95],
        w,
        h,
    );

    let paused = state.paused;
    let [px, py, pw, ph] = layout.play;
    rect(out, px, py, pw, ph, [0.07, 0.10, 0.14, 0.96], w, h);
    rect(out, px, py, pw, 1.0, [0.30, 0.40, 0.52, 0.95], w, h);
    text(
        out,
        if paused { ">" } else { "||" },
        px + if paused { 10.0 } else { 6.5 },
        py + 6.0,
        1.35,
        [0.92, 0.96, 1.0, 1.0],
        w,
        h,
    );

    let duration = state.duration_ms.max(1) as f64;
    let live_fraction = (state.elapsed_ms / duration).clamp(0.0, 1.0) as f32;
    let fraction = state
        .scrub_fraction
        .unwrap_or(live_fraction)
        .clamp(0.0, 1.0);
    let preview_ms = if state.scrub_fraction.is_some() {
        (duration * f64::from(fraction)).round() as i32
    } else {
        state.elapsed_ms.round().clamp(0.0, duration) as i32
    };
    let current_text = format_demo_time(preview_ms);
    let total_text = format_demo_time(state.duration_ms);
    text(
        out,
        &current_text,
        52.0,
        panel_y + 13.0,
        1.05,
        [0.82, 0.88, 0.94, 1.0],
        w,
        h,
    );

    let [tx, ty, tw, th] = layout.track;
    let line_y = ty + th * 0.5 - 2.0;
    rect(out, tx, line_y, tw, 4.0, [0.12, 0.16, 0.21, 1.0], w, h);
    rect(
        out,
        tx,
        line_y,
        tw * fraction,
        4.0,
        [0.55, 0.37, 0.88, 1.0],
        w,
        h,
    );

    for marker in &state.kill_markers {
        let mf = marker.fraction.clamp(0.0, 1.0);
        let involved = marker.followed_kill || marker.followed_death;
        let tick_h = if involved { 24.0 } else { 14.0 };
        let tick_w = if involved { 3.0 } else { 1.5 };
        let color = match marker.attacker_team {
            1 => [1.0, 0.30, 0.30, 0.98],
            2 => [0.32, 0.58, 1.0, 0.98],
            _ => [1.0, 0.84, 0.34, 0.98],
        };
        let x = tx + tw * mf - tick_w * 0.5;
        rect(
            out,
            x,
            ty + th * 0.5 - tick_h * 0.5,
            tick_w,
            tick_h,
            color,
            w,
            h,
        );
        if involved {
            rect(
                out,
                x - 1.0,
                ty + th * 0.5 - tick_h * 0.5,
                tick_w + 2.0,
                1.0,
                [1.0, 1.0, 1.0, 0.95],
                w,
                h,
            );
        }
    }

    let thumb_x = tx + tw * fraction;
    rect(
        out,
        thumb_x - 1.5,
        ty + 1.0,
        3.0,
        th - 2.0,
        [0.95, 0.96, 1.0, 1.0],
        w,
        h,
    );

    text(
        out,
        &total_text,
        tx + tw + 10.0,
        panel_y + 13.0,
        1.05,
        [0.62, 0.70, 0.78, 1.0],
        w,
        h,
    );

    let [sx, sy, sw, sh] = layout.speed;
    rect(out, sx, sy, sw, sh, [0.07, 0.10, 0.14, 0.96], w, h);
    rect(out, sx, sy, sw, 1.0, [0.25, 0.50, 0.35, 0.95], w, h);
    let speed = format!("{}x", state.playback_rate);
    text(
        out,
        &speed,
        sx + 8.0,
        sy + 6.0,
        1.15,
        [0.78, 0.94, 0.84, 1.0],
        w,
        h,
    );
}

pub(in crate::ui) fn format_demo_time(ms: i32) -> String {
    let total_seconds = ms.max(0) / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}
