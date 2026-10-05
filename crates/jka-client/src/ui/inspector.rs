//! Inspector.
use crate::ui::{rect, text, UiVertex};

/// `x, y, width` of the trace inspector panel before any HUD layout is applied.
/// Unused since the trace panel moved to the `OverlayMode::Trace` egui menu;
/// kept (not deleted) alongside `build_surface_inspector` in case a non-egui
/// HUD summary is wanted again later.
#[allow(dead_code)]
pub(in crate::ui) fn surface_inspector_frame(w: u32) -> (f32, f32, f32) {
    let panel_w = (w as f32 * 0.46).clamp(620.0, 900.0);
    ((w as f32 - panel_w - 24.0).max(24.0), 72.0, panel_w)
}

#[allow(dead_code)]
pub(in crate::ui) fn build_surface_inspector(
    out: &mut Vec<UiVertex>,
    info: &crate::runtime::SurfaceInspectorInfo,
    w: u32,
    h: u32,
) {
    fn shortened(value: &str, max_chars: usize) -> String {
        if value.chars().count() <= max_chars {
            return value.to_owned();
        }
        let keep = max_chars.saturating_sub(1);
        let mut text = value.chars().take(keep).collect::<String>();
        text.push('…');
        text
    }

    let (x, y, panel_w) = surface_inspector_frame(w);
    let summary_rows = info.summary.len().min(8);
    let header_h = 104.0;
    let summary_h = if summary_rows == 0 {
        0.0
    } else {
        14.0 + summary_rows as f32 * 30.0 + 10.0
    };
    let available_detail_h = (h as f32 - y - header_h - summary_h - 34.0).max(0.0);
    let mut detail_rows = 0usize;
    for section in &info.sections {
        if detail_rows >= 14 {
            break;
        }
        detail_rows += 1; // section heading
        detail_rows += section.lines.len().min(14usize.saturating_sub(detail_rows));
    }
    if detail_rows == 0 {
        detail_rows = info.lines.len().min(8);
    }
    let detail_row_h = 22.0;
    let visible_detail_rows =
        detail_rows.min((available_detail_h / detail_row_h).floor().max(0.0) as usize);
    let detail_h = visible_detail_rows as f32 * detail_row_h
        + if visible_detail_rows > 0 { 14.0 } else { 0.0 };
    let panel_h = (header_h + summary_h + detail_h + 16.0).min(h as f32 - y - 18.0);

    rect(
        out,
        x,
        y,
        panel_w,
        panel_h,
        [0.012, 0.020, 0.032, 0.975],
        w,
        h,
    );
    rect(out, x, y, 5.0, panel_h, [0.96, 0.72, 0.18, 1.0], w, h);

    text(
        out,
        "TRACE INSPECTOR",
        x + 22.0,
        y + 17.0,
        2.05,
        [0.99, 0.94, 0.82, 1.0],
        w,
        h,
    );
    text(
        out,
        "CTRL+C  COPY FULL DIAGNOSTICS",
        x + panel_w - 250.0,
        y + 20.0,
        1.05,
        [0.57, 0.65, 0.74, 1.0],
        w,
        h,
    );

    // Keep identity on its own row. The old overlay packed every diagnostic
    // into equally weighted text; this makes "what am I looking at?" readable
    // before the eye has to parse any technical detail.
    let kind = shortened(&info.kind, 26);
    let kind_w = (kind.chars().count() as f32 * 7.2 + 22.0).clamp(92.0, 220.0);
    let kind_x = x + 22.0;
    rect(
        out,
        kind_x,
        y + 54.0,
        kind_w,
        27.0,
        [0.06, 0.13, 0.19, 0.96],
        w,
        h,
    );
    text(
        out,
        &kind,
        kind_x + 10.0,
        y + 61.0,
        1.15,
        [0.45, 0.86, 1.0, 1.0],
        w,
        h,
    );
    let title_x = kind_x + kind_w + 14.0;
    let title_chars = ((x + panel_w - 22.0 - title_x) / 11.5).floor().max(18.0) as usize;
    let title = shortened(&info.title, title_chars);
    text(
        out,
        &title,
        title_x,
        y + 58.0,
        1.75,
        [0.76, 0.91, 1.0, 1.0],
        w,
        h,
    );

    let mut cursor_y = y + header_h;
    if summary_rows > 0 {
        rect(
            out,
            x + 16.0,
            cursor_y - 6.0,
            panel_w - 32.0,
            summary_h - 2.0,
            [0.020, 0.036, 0.052, 0.94],
            w,
            h,
        );
        for (label, value) in info.summary.iter().take(summary_rows) {
            let value = shortened(value, ((panel_w - 188.0) / 8.5).floor().max(24.0) as usize);
            text(
                out,
                label,
                x + 30.0,
                cursor_y + 7.0,
                1.12,
                [0.55, 0.66, 0.77, 1.0],
                w,
                h,
            );
            text(
                out,
                &value,
                x + 164.0,
                cursor_y + 5.0,
                1.34,
                [0.96, 0.98, 1.0, 1.0],
                w,
                h,
            );
            cursor_y += 30.0;
        }
        cursor_y += 18.0;
    }

    let max_chars = ((panel_w - 60.0) / 7.5).floor().max(32.0) as usize;
    let mut rows_left = visible_detail_rows;
    if rows_left > 0 {
        for section in &info.sections {
            if rows_left == 0 {
                break;
            }
            text(
                out,
                &section.title,
                x + 24.0,
                cursor_y + 1.0,
                1.10,
                [0.98, 0.72, 0.24, 1.0],
                w,
                h,
            );
            cursor_y += detail_row_h;
            rows_left -= 1;
            for line in &section.lines {
                if rows_left == 0 {
                    break;
                }
                let line = shortened(line, max_chars);
                text(
                    out,
                    &line,
                    x + 34.0,
                    cursor_y + 1.0,
                    1.16,
                    [0.86, 0.90, 0.94, 1.0],
                    w,
                    h,
                );
                cursor_y += detail_row_h;
                rows_left -= 1;
            }
            cursor_y += 3.0;
        }
        if info.sections.is_empty() {
            for line in info.lines.iter().take(rows_left) {
                let line = shortened(line, max_chars);
                text(
                    out,
                    &line,
                    x + 30.0,
                    cursor_y + 1.0,
                    1.16,
                    [0.86, 0.90, 0.94, 1.0],
                    w,
                    h,
                );
                cursor_y += detail_row_h;
            }
        }
    }
}
