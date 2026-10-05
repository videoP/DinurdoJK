//! Console.
use crate::ui::{
    console_text, disc, glyph_quad_sized, rect, rect_outline, text, ConsoleSize,
    ConsoleSuggestKind, UiSnapshot, UiVertex,
};

pub(in crate::ui) fn console_panel_height(h: u32, size: ConsoleSize) -> f32 {
    match size {
        ConsoleSize::Normal => (h as f32 * 0.42).max(210.0),
        ConsoleSize::Half => h as f32 * 0.50,
        ConsoleSize::Full => h as f32,
    }
}

pub(in crate::ui) const CONSOLE_CHAR_WIDTH: f32 = 8.0;

pub(in crate::ui) const CONSOLE_CHAR_HEIGHT: f32 = 16.0;

/// Top of the first log row, just under the header band.
pub(in crate::ui) const CONSOLE_FIRST_Y: f32 = 52.0;

pub fn console_visible_line_capacity(h: u32, size: ConsoleSize) -> usize {
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = CONSOLE_FIRST_Y;
    (((input_y - first_y - 8.0) / line_step).floor().max(0.0)) as usize
}

pub fn console_text_hit(
    w: u32,
    h: u32,
    size: ConsoleSize,
    x: f64,
    y: f64,
) -> Option<(usize, usize)> {
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT as f64;
    let first_y = CONSOLE_FIRST_Y as f64;
    let max_lines = console_visible_line_capacity(h, size);
    if x < 20.0 || x > w as f64 - 8.0 || y < first_y || y >= input_y as f64 - 8.0 {
        return None;
    }
    let row = ((y - first_y) / line_step).floor() as usize;
    if row >= max_lines {
        return None;
    }
    let char_width = CONSOLE_CHAR_WIDTH as f64;
    let col = ((x - 20.0) / char_width).max(0.0).round() as usize;
    Some((row, col))
}

pub(in crate::ui) fn visible_text_len(value: &str) -> usize {
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut index = 0usize;
    let mut count = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'^' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            index += 2;
        } else {
            count += 1;
            index += 1;
        }
    }
    count
}

pub(in crate::ui) fn tail_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(3);
    let tail: String = value.chars().skip(count.saturating_sub(keep)).collect();
    format!("...{tail}")
}

// Console palette: rain-slick Tokyo night. Indigo asphalt for the body, cyan and
// magenta neon for structure, tungsten amber for anything the user is driving.
pub(in crate::ui) const CON_CYAN: [f32; 4] = [0.28, 0.86, 0.95, 1.0];

pub(in crate::ui) const CON_MAGENTA: [f32; 4] = [1.0, 0.30, 0.56, 1.0];

pub(in crate::ui) const CON_AMBER: [f32; 4] = [1.0, 0.72, 0.28, 1.0];

pub(in crate::ui) const CON_TEXT: [f32; 4] = [0.80, 0.86, 0.93, 1.0];

pub(in crate::ui) const CON_BRIGHT: [f32; 4] = [0.95, 0.97, 1.0, 1.0];

pub(in crate::ui) const CON_DIM: [f32; 4] = [0.50, 0.60, 0.73, 1.0];

pub(in crate::ui) const CON_FAINT: [f32; 4] = [0.32, 0.40, 0.52, 1.0];

pub(in crate::ui) const SUGGEST_ROW_H: f32 = 20.0;

pub(in crate::ui) const SUGGEST_MAX_ROWS: usize = 8;

pub(in crate::ui) const SUGGEST_PAD: f32 = 6.0;

pub(in crate::ui) const SUGGEST_FOOTER_H: f32 = 22.0;

pub(in crate::ui) fn with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

/// Rectangle with a distinct color at each corner (the UI pipeline
/// interpolates vertex colors, so this is a free gradient).
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn rect_gradient(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    top_left: [f32; 4],
    top_right: [f32; 4],
    bottom_left: [f32; 4],
    bottom_right: [f32; 4],
    width: u32,
    height: u32,
) {
    let x0 = x / width.max(1) as f32 * 2.0 - 1.0;
    let x1 = (x + w) / width.max(1) as f32 * 2.0 - 1.0;
    let y0 = 1.0 - y / height.max(1) as f32 * 2.0;
    let y1 = 1.0 - (y + h) / height.max(1) as f32 * 2.0;
    let v = |position, color| UiVertex {
        position,
        uv: [0.0, 0.0],
        color,
        textured: 0.0,
    };
    out.extend_from_slice(&[
        v([x0, y0], top_left),
        v([x0, y1], bottom_left),
        v([x1, y1], bottom_right),
        v([x0, y0], top_left),
        v([x1, y1], bottom_right),
        v([x1, y0], top_right),
    ]);
}

/// Like `console_text` but without `^N` color escapes, for values and
/// descriptions that must print literally.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_plain(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    for byte in value.bytes() {
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                color,
                width,
                height,
            );
        }
        cursor_x += CONSOLE_CHAR_WIDTH;
    }
}

/// Truncate to `max_chars`, ending in `..` when cut.
pub(in crate::ui) fn ellipsize(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(2);
    let mut cut: String = value.chars().take(keep).collect();
    cut.push_str("..");
    cut
}

/// Greedy word wrap into at most `max_lines` lines; the last line is
/// ellipsized when the text does not fit.
pub(in crate::ui) fn wrap_plain(value: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let max_chars = max_chars.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = value.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let extra = usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + extra + word.chars().count() > max_chars
        {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                let last = lines.last_mut().expect("just pushed");
                *last = ellipsize(&format!("{last} {word}"), max_chars);
                return lines;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        if words.peek().is_none() {
            lines.push(std::mem::take(&mut current));
        }
    }
    lines.truncate(max_lines);
    lines
}

/// Colored keycap + dim label pairs, clipped at `max_x`.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_keycaps(
    out: &mut Vec<UiVertex>,
    x: f32,
    y: f32,
    max_x: f32,
    items: &[(&str, &str)],
    w: u32,
    h: u32,
) {
    let mut cursor = x;
    for (key, label) in items {
        let need = (key.len() + label.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
        if cursor + need > max_x {
            break;
        }
        console_plain(out, key, cursor, y, CON_AMBER, w, h);
        cursor += (key.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
        console_plain(out, label, cursor, y, CON_DIM, w, h);
        cursor += (label.len() + 3) as f32 * CONSOLE_CHAR_WIDTH;
    }
}

/// Tone for the gutter marker of echoed input (amber) and error lines (magenta).
pub(in crate::ui) fn console_line_tone(line: &str) -> Option<[f32; 4]> {
    let mut rest = line;
    // Optional `^8[hh:mm:ss]^7 ` timestamp prefix from `con_timestamps`.
    if let Some(after) = rest.strip_prefix("^8[") {
        if let Some(index) = after.find("]^7 ") {
            rest = &after[index + 4..];
        }
    }
    if rest.starts_with("^7] ") || rest.starts_with("] ") {
        Some(CON_AMBER)
    } else if rest.starts_with("^1") {
        Some(CON_MAGENTA)
    } else {
        None
    }
}

/// Clickable `SUGGEST ON/OFF` chip in the console header: `(x, y, w, h)`.
pub fn console_suggest_chip_rect(w: u32) -> (f32, f32, f32, f32) {
    let chip_w = 12.0 * CONSOLE_CHAR_WIDTH + 30.0;
    (w as f32 - 20.0 - 24.0 - 8.0 - chip_w, 8.0, chip_w, 24.0)
}

pub fn console_suggest_chip_hit(w: u32, x: f64, y: f64) -> bool {
    let (cx, cy, cw, ch) = console_suggest_chip_rect(w);
    x >= cx as f64 && x < (cx + cw) as f64 && y >= cy as f64 && y < (cy + ch) as f64
}

/// The `?` button in the header's top-right corner: `(x, y, w, h)`.
pub fn console_help_rect(w: u32) -> (f32, f32, f32, f32) {
    (w as f32 - 20.0 - 24.0, 8.0, 24.0, 24.0)
}

pub fn console_help_hit(w: u32, x: f64, y: f64) -> bool {
    let (bx, by, bw, bh) = console_help_rect(w);
    x >= bx as f64 && x < (bx + bw) as f64 && y >= by as f64 && y < (by + bh) as f64
}

/// Where the suggestion popup sits. Shared by drawing and mouse hit-testing.
#[derive(Debug, Clone, Copy)]
pub struct ConsoleSuggestGeometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Index of the first visible suggestion (the list scrolls to keep the selection in view).
    pub first: usize,
    pub visible: usize,
}

pub fn console_suggest_detail_line_count(w: u32, h: u32, description: &str) -> usize {
    let popup_w = (w as f32 - 24.0).clamp(240.0, 820.0);
    let inner_chars = ((popup_w - 28.0) / CONSOLE_CHAR_WIDTH) as usize;
    let natural = wrap_plain(description, inner_chars, usize::MAX)
        .len()
        .max(1);
    // Keep at least one suggestion row and the footer on-screen. Only descriptions
    // taller than the entire framebuffer are forced to truncate.
    let fixed = SUGGEST_PAD + SUGGEST_ROW_H + SUGGEST_FOOTER_H + 28.0;
    let fit = (((h as f32 - fixed).max(18.0)) / 18.0).floor() as usize;
    natural.min(fit.max(1))
}

pub fn console_suggest_geometry(
    w: u32,
    h: u32,
    size: ConsoleSize,
    count: usize,
    selected: usize,
    detail_lines: usize,
) -> ConsoleSuggestGeometry {
    let detail_h = 28.0 + detail_lines.max(1) as f32 * 18.0;
    let max_rows_fit = (((h as f32 - SUGGEST_PAD - detail_h - SUGGEST_FOOTER_H - 8.0)
        / SUGGEST_ROW_H)
        .floor()
        .max(1.0)) as usize;
    let visible = count.clamp(1, SUGGEST_MAX_ROWS.min(max_rows_fit));
    let total_h = SUGGEST_PAD + visible as f32 * SUGGEST_ROW_H + detail_h + SUGGEST_FOOTER_H;
    let ph = console_panel_height(h, size);
    let input_y = ph - 34.0;
    // Hang below the panel like a drop-down when there is room, otherwise
    // (full-height console) float just above the input line.
    let below = h as f32 - ph >= total_h + 12.0;
    let y = if below {
        ph + 8.0
    } else {
        (input_y - 10.0 - total_h).max(4.0)
    };
    let first = if count <= visible {
        0
    } else {
        selected.saturating_sub(visible / 2).min(count - visible)
    };
    ConsoleSuggestGeometry {
        x: 12.0,
        y,
        w: (w as f32 - 24.0).clamp(240.0, 820.0),
        h: total_h,
        first,
        visible,
    }
}

/// Suggestion row under the pointer, as an index into the full suggestion list.
pub fn console_suggest_hit(
    w: u32,
    h: u32,
    size: ConsoleSize,
    count: usize,
    selected: usize,
    detail_lines: usize,
    x: f64,
    y: f64,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let g = console_suggest_geometry(w, h, size, count, selected, detail_lines);
    let rows_y = (g.y + SUGGEST_PAD) as f64;
    if x < g.x as f64 || x >= (g.x + g.w) as f64 || y < rows_y {
        return None;
    }
    let row = ((y - rows_y) / SUGGEST_ROW_H as f64).floor() as usize;
    (row < g.visible).then_some(g.first + row)
}

pub(in crate::ui) fn build_console(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let ph = console_panel_height(h, ui.console_size);
    let wf = w as f32;
    let input_y = ph - 34.0;
    let line_step = CONSOLE_CHAR_HEIGHT;
    let first_y = CONSOLE_FIRST_Y;

    // Body: indigo asphalt, a touch lighter toward the input so the eye lands there.
    rect_gradient(
        out,
        0.0,
        0.0,
        wf,
        ph,
        [0.012, 0.016, 0.036, 0.975],
        [0.012, 0.016, 0.036, 0.975],
        [0.024, 0.030, 0.066, 0.965],
        [0.024, 0.030, 0.066, 0.965],
        w,
        h,
    );
    // Header band and hairline.
    rect(out, 0.0, 0.0, wf, 40.0, [0.0, 0.0, 0.02, 0.30], w, h);
    rect(out, 0.0, 40.0, wf, 1.0, with_alpha(CON_CYAN, 0.14), w, h);

    // Neon edge along the bottom of the panel, with a soft glow bleeding onto the world below.
    let edge_l = with_alpha(CON_CYAN, 0.95);
    let edge_r = with_alpha(CON_MAGENTA, 0.95);
    rect_gradient(
        out,
        0.0,
        ph - 2.0,
        wf,
        2.0,
        edge_l,
        edge_r,
        edge_l,
        edge_r,
        w,
        h,
    );
    for (dy, height, alpha) in [(0.0, 3.0, 0.17), (3.0, 4.0, 0.07), (7.0, 6.0, 0.025)] {
        let l = with_alpha(CON_CYAN, alpha);
        let r = with_alpha(CON_MAGENTA, alpha);
        rect_gradient(out, 0.0, ph + dy, wf, height, l, r, l, r, w, h);
    }

    // Title.
    rect(out, 20.0, 9.0, 3.0, 22.0, CON_MAGENTA, w, h);
    text(out, "CONSOLE", 32.0, 12.0, 2.0, CON_BRIGHT, w, h);
    if !ui.console_status.is_empty() && !ui.console_search_open {
        console_text(
            out,
            &ui.console_status,
            32.0 + 7.0 * 12.0 + 16.0,
            16.0,
            CON_DIM,
            w,
            h,
        );
    }

    // Header right: line count and the suggestion switch.
    let (chip_x, chip_y, chip_w, chip_h) = console_suggest_chip_rect(w);
    rect(
        out,
        chip_x,
        chip_y,
        chip_w,
        chip_h,
        [0.04, 0.07, 0.13, 0.92],
        w,
        h,
    );
    let chip_edge = if ui.console_suggest_enabled {
        with_alpha(CON_CYAN, 0.45)
    } else {
        with_alpha(CON_FAINT, 0.7)
    };
    rect_outline(out, chip_x, chip_y, chip_w, chip_h, 1.0, chip_edge, w, h);
    disc(
        out,
        chip_x + 13.0,
        chip_y + chip_h * 0.5,
        3.5,
        12,
        if ui.console_suggest_enabled {
            CON_CYAN
        } else {
            CON_FAINT
        },
        w,
        h,
    );
    console_plain(
        out,
        if ui.console_suggest_enabled {
            "SUGGEST ON"
        } else {
            "SUGGEST OFF"
        },
        chip_x + 24.0,
        chip_y + 4.0,
        if ui.console_suggest_enabled {
            CON_BRIGHT
        } else {
            CON_DIM
        },
        w,
        h,
    );
    let count_label = format!("{} LINES", ui.console_total_lines);
    let count_x = chip_x - 14.0 - count_label.len() as f32 * CONSOLE_CHAR_WIDTH;
    if !ui.console_search_open && count_x > 32.0 + 7.0 * 12.0 + 140.0 {
        console_plain(out, &count_label, count_x, chip_y + 4.0, CON_FAINT, w, h);
    }

    // Help button; the shortcut card appears while it is hovered.
    let (help_x, help_y, help_w, help_h) = console_help_rect(w);
    rect(
        out,
        help_x,
        help_y,
        help_w,
        help_h,
        if ui.console_help_hover {
            [0.10, 0.16, 0.26, 0.98]
        } else {
            [0.04, 0.07, 0.13, 0.92]
        },
        w,
        h,
    );
    rect_outline(
        out,
        help_x,
        help_y,
        help_w,
        help_h,
        1.0,
        if ui.console_help_hover {
            with_alpha(CON_AMBER, 0.9)
        } else {
            with_alpha(CON_CYAN, 0.45)
        },
        w,
        h,
    );
    console_plain(
        out,
        "?",
        help_x + (help_w - CONSOLE_CHAR_WIDTH) * 0.5,
        help_y + 4.0,
        if ui.console_help_hover {
            CON_AMBER
        } else {
            CON_DIM
        },
        w,
        h,
    );

    // The find field lives in the header, in place of the status text.
    if ui.console_search_open {
        let field_x = 32.0 + 7.0 * 12.0 + 16.0;
        let field_y = 6.0;
        let room = (chip_x - 14.0 - field_x).max(160.0);
        let field_w = (room * 0.62).clamp(160.0, 440.0);
        rect(
            out,
            field_x,
            field_y,
            field_w,
            28.0,
            [0.03, 0.05, 0.10, 0.98],
            w,
            h,
        );
        rect(out, field_x, field_y + 26.0, field_w, 2.0, CON_AMBER, w, h);
        let max_query_chars = ((field_w - 72.0) / CONSOLE_CHAR_WIDTH) as usize;
        let query = tail_chars(&ui.console_search_query, max_query_chars.max(1));
        console_plain(out, "FIND", field_x + 8.0, field_y + 6.0, CON_AMBER, w, h);
        console_plain(
            out,
            &query,
            field_x + 8.0 + 5.0 * CONSOLE_CHAR_WIDTH,
            field_y + 6.0,
            CON_BRIGHT,
            w,
            h,
        );
        let caret_x = field_x + 8.0 + (5 + query.chars().count()) as f32 * CONSOLE_CHAR_WIDTH;
        rect(out, caret_x, field_y + 5.0, 2.0, 18.0, CON_AMBER, w, h);

        let counter = if ui.console_search_query.is_empty() {
            "TYPE TO SEARCH".to_owned()
        } else if ui.console_search_total == 0 {
            "NO MATCHES".to_owned()
        } else {
            format!(
                "{} / {}",
                ui.console_search_index.unwrap_or(0) + 1,
                ui.console_search_total
            )
        };
        console_plain(
            out,
            &counter,
            field_x + field_w + 12.0,
            field_y + 6.0,
            if ui.console_search_total == 0 && !ui.console_search_query.is_empty() {
                CON_MAGENTA
            } else {
                CON_AMBER
            },
            w,
            h,
        );
    }

    // Log.
    let max_lines = (((input_y - first_y - 8.0) / line_step).floor().max(0.0)) as usize;
    let history_end = ui.console_lines.len().saturating_sub(ui.console_scroll);
    let history_start = history_end.saturating_sub(max_lines);
    let mut line_y = first_y;
    for (line_index, line) in ui.console_lines[history_start..history_end]
        .iter()
        .enumerate()
    {
        let absolute_line = history_start + line_index;
        if let Some(tone) = console_line_tone(line) {
            rect(
                out,
                0.0,
                line_y - 2.0,
                wf,
                line_step,
                with_alpha(tone, 0.055),
                w,
                h,
            );
            rect(
                out,
                10.0,
                line_y - 2.0,
                2.0,
                line_step,
                with_alpha(tone, 0.85),
                w,
                h,
            );
        }
        if ui.console_search_open {
            for hit in ui
                .console_search_matches
                .iter()
                .filter(|hit| hit.line == absolute_line)
            {
                let active = ui
                    .console_search_active
                    .is_some_and(|active| active == *hit);
                rect(
                    out,
                    20.0 + hit.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y - 2.0,
                    (hit.end_col - hit.start_col) as f32 * CONSOLE_CHAR_WIDTH,
                    line_step,
                    if active {
                        [0.95, 0.62, 0.10, 0.88]
                    } else {
                        [0.72, 0.58, 0.12, 0.48]
                    },
                    w,
                    h,
                );
            }
        }
        if let Some(selection) = ui.console_selection {
            let (start_line, start_col, end_line, end_col) =
                if (selection.start_line, selection.start_col)
                    <= (selection.end_line, selection.end_col)
                {
                    (
                        selection.start_line,
                        selection.start_col,
                        selection.end_line,
                        selection.end_col,
                    )
                } else {
                    (
                        selection.end_line,
                        selection.end_col,
                        selection.start_line,
                        selection.start_col,
                    )
                };
            if absolute_line >= start_line && absolute_line <= end_line {
                let visible_len = visible_text_len(line);
                let left_col = if absolute_line == start_line {
                    start_col.min(visible_len)
                } else {
                    0
                };
                let right_col = if absolute_line == end_line {
                    end_col.min(visible_len)
                } else {
                    visible_len
                };
                if right_col > left_col {
                    let char_width = CONSOLE_CHAR_WIDTH;
                    rect(
                        out,
                        20.0 + left_col as f32 * char_width,
                        line_y - 2.0,
                        (right_col - left_col) as f32 * char_width,
                        line_step,
                        [0.08, 0.46, 0.62, 0.55],
                        w,
                        h,
                    );
                }
            }
        }
        console_text(out, line, 20.0, line_y, CON_TEXT, w, h);

        // Trusted engine-authored local paths get conventional hyperlink
        // treatment. Draw this *after* the normal colored console text so a
        // green/yellow log line cannot hide the link, and keep the underline
        // inside the 16 px row (the previous +18 px position fell below it).
        let mut path_links = ui
            .console_path_links
            .iter()
            .filter(|link| link.line == absolute_line)
            .peekable();
        if path_links.peek().is_some() {
            let plain = crate::logging::strip_jka_colors(line);
            for link in path_links {
                if link.end_col <= link.start_col {
                    continue;
                }
                let linked_text: String = plain
                    .chars()
                    .skip(link.start_col)
                    .take(link.end_col - link.start_col)
                    .collect();
                console_plain(
                    out,
                    &linked_text,
                    20.0 + link.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y,
                    CON_CYAN,
                    w,
                    h,
                );
                rect(
                    out,
                    20.0 + link.start_col as f32 * CONSOLE_CHAR_WIDTH,
                    line_y + CONSOLE_CHAR_HEIGHT - 2.0,
                    (link.end_col - link.start_col) as f32 * CONSOLE_CHAR_WIDTH,
                    1.0,
                    with_alpha(CON_CYAN, 0.95),
                    w,
                    h,
                );
            }
        }
        line_y += line_step;
    }
    if ui.console_lines.is_empty() && !ui.console_status.is_empty() {
        console_plain(out, "NO OUTPUT YET", 20.0, first_y, CON_FAINT, w, h);
    }

    // Scroll position: a thin rail on the right plus a "newer output below" pill.
    let rail_top = first_y - 2.0;
    let rail_h = (input_y - 8.0 - rail_top).max(1.0);
    if max_lines > 0 && ui.console_total_lines > max_lines {
        let total = ui.console_total_lines as f32;
        let thumb_h = (rail_h * max_lines as f32 / total).clamp(18.0, rail_h);
        let travel = rail_h - thumb_h;
        let max_scroll = (ui.console_total_lines - max_lines) as f32;
        let from_top = 1.0 - (ui.console_scrolled as f32 / max_scroll).clamp(0.0, 1.0);
        rect(
            out,
            wf - 8.0,
            rail_top,
            2.0,
            rail_h,
            with_alpha(CON_CYAN, 0.07),
            w,
            h,
        );
        rect(
            out,
            wf - 9.0,
            rail_top + travel * from_top,
            4.0,
            thumb_h,
            with_alpha(
                if ui.console_scrolled > 0 {
                    CON_MAGENTA
                } else {
                    CON_CYAN
                },
                0.55,
            ),
            w,
            h,
        );
    }
    if ui.console_scrolled > 0 {
        let label = format!("{} NEWER LINES BELOW  PGDN", ui.console_scrolled);
        let pill_w = label.len() as f32 * CONSOLE_CHAR_WIDTH + 20.0;
        let pill_x = wf - 24.0 - pill_w;
        let pill_y = input_y - 34.0;
        rect(
            out,
            pill_x,
            pill_y,
            pill_w,
            22.0,
            [0.10, 0.03, 0.08, 0.92],
            w,
            h,
        );
        rect_outline(
            out,
            pill_x,
            pill_y,
            pill_w,
            22.0,
            1.0,
            with_alpha(CON_MAGENTA, 0.7),
            w,
            h,
        );
        console_plain(out, &label, pill_x + 10.0, pill_y + 3.0, CON_MAGENTA, w, h);
    }

    // Input bar.
    let bar_y = input_y - 8.0;
    rect(
        out,
        0.0,
        bar_y,
        wf,
        (ph - 2.0) - bar_y,
        [0.045, 0.065, 0.125, 0.94],
        w,
        h,
    );
    rect(out, 0.0, bar_y, wf, 1.0, with_alpha(CON_CYAN, 0.18), w, h);
    console_plain(out, ">", 20.0, input_y, CON_AMBER, w, h);
    let text_x = 38.0;
    let cursor = ui.console_cursor.min(ui.console_input.len());
    let cursor = if ui.console_input.is_char_boundary(cursor) {
        cursor
    } else {
        ui.console_input.len()
    };
    if ui.console_input.is_empty() {
        console_plain(
            out,
            "type a command or cvar",
            text_x + 6.0,
            input_y,
            CON_FAINT,
            w,
            h,
        );
    } else {
        console_text(out, &ui.console_input, text_x, input_y, CON_BRIGHT, w, h);
        // Ghost completion: the rest of the highlighted suggestion, dimmed.
        if !ui.console_suggest_hint && cursor == ui.console_input.len() {
            let typed = ui.console_input.trim_start();
            if let Some(best) = ui.console_suggestions.get(ui.console_suggest_selected) {
                let name = best.name;
                if !typed.contains(char::is_whitespace)
                    && name.len() > typed.len()
                    && name.as_bytes()[..typed.len()].eq_ignore_ascii_case(typed.as_bytes())
                {
                    let end_x =
                        text_x + visible_text_len(&ui.console_input) as f32 * CONSOLE_CHAR_WIDTH;
                    console_plain(
                        out,
                        &name[typed.len()..],
                        end_x,
                        input_y,
                        with_alpha(CON_CYAN, 0.42),
                        w,
                        h,
                    );
                }
            }
        }
    }
    let caret_x =
        text_x + visible_text_len(&ui.console_input[..cursor]) as f32 * CONSOLE_CHAR_WIDTH;
    rect(
        out,
        caret_x,
        input_y - 1.0,
        2.0,
        CONSOLE_CHAR_HEIGHT + 2.0,
        CON_AMBER,
        w,
        h,
    );

    if !ui.console_suggestions.is_empty() && !ui.console_search_open {
        build_console_suggestions(out, ui, w, h);
    }
    if ui.console_help_hover {
        build_console_help(out, w, h);
    }
}

/// Shortcut card shown while the header `?` is hovered.
pub(in crate::ui) fn build_console_help(out: &mut Vec<UiVertex>, w: u32, h: u32) {
    // Empty key = section heading.
    const ROWS: &[(&str, &str)] = &[
        ("", "TYPING"),
        ("TAB", "complete, or extend shared prefix"),
        ("UP / DOWN", "pick suggestion (history if none)"),
        ("ENTER", "run; fills a picked suggestion"),
        ("ESC", "hide list, then close console"),
        ("CLICK", "pick a suggestion"),
        ("", "EDITING"),
        ("LEFT / RIGHT", "move caret (CTRL = by word)"),
        ("HOME / END", "start / end of line"),
        ("CTRL+V", "paste"),
        ("", "LOG"),
        ("CTRL+F", "find (ENTER next, SHIFT+ENTER prev)"),
        ("PGUP / PGDN", "scroll"),
        ("DRAG", "select; 2x click word, 3x line"),
        ("CTRL+A / C", "select all / copy selection"),
        ("", "CONSOLE"),
        ("~", "open / close"),
        ("SHIFT+~", "half height"),
        ("CTRL+~", "full height"),
    ];
    const ROW_H: f32 = 18.0;
    let key_col = 14.0 * CONSOLE_CHAR_WIDTH;
    let card_w = (key_col + 36.0 * CONSOLE_CHAR_WIDTH + 28.0).min(w as f32 - 24.0);
    let card_h = ROWS.len() as f32 * ROW_H + 20.0;
    let x = w as f32 - 20.0 - card_w;
    let y = 44.0;
    rect(
        out,
        x + 3.0,
        y + 5.0,
        card_w,
        card_h,
        [0.0, 0.0, 0.0, 0.40],
        w,
        h,
    );
    rect(out, x, y, card_w, card_h, [0.020, 0.028, 0.056, 0.99], w, h);
    let edge_l = with_alpha(CON_CYAN, 0.6);
    let edge_r = with_alpha(CON_MAGENTA, 0.6);
    rect_gradient(out, x, y, card_w, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect_gradient(
        out,
        x,
        y + card_h - 1.0,
        card_w,
        1.0,
        edge_l,
        edge_r,
        edge_l,
        edge_r,
        w,
        h,
    );
    rect(out, x, y, 1.0, card_h, edge_l, w, h);
    rect(out, x + card_w - 1.0, y, 1.0, card_h, edge_r, w, h);
    let label_chars = ((card_w - 28.0 - key_col) / CONSOLE_CHAR_WIDTH) as usize;
    for (i, (key, label)) in ROWS.iter().enumerate() {
        let ry = y + 10.0 + i as f32 * ROW_H;
        if key.is_empty() {
            console_plain(out, label, x + 14.0, ry, CON_CYAN, w, h);
            let rule_x = x + 14.0 + (label.len() as f32 + 1.0) * CONSOLE_CHAR_WIDTH;
            rect(
                out,
                rule_x,
                ry + 8.0,
                x + card_w - 14.0 - rule_x,
                1.0,
                with_alpha(CON_CYAN, 0.16),
                w,
                h,
            );
        } else {
            console_plain(out, key, x + 14.0, ry, CON_AMBER, w, h);
            console_plain(
                out,
                &ellipsize(label, label_chars),
                x + 14.0 + key_col,
                ry,
                CON_TEXT,
                w,
                h,
            );
        }
    }
}

pub(in crate::ui) fn build_console_suggestions(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    w: u32,
    h: u32,
) {
    let count = ui.console_suggestions.len();
    let selected = ui.console_suggest_selected.min(count - 1);
    let hint = ui.console_suggest_hint;
    let description = ui
        .console_suggestions
        .get(selected)
        .map_or("", |item| item.description);
    let detail_lines = console_suggest_detail_line_count(w, h, description);
    let g = console_suggest_geometry(w, h, ui.console_size, count, selected, detail_lines);
    let (x, y, pw) = (g.x, g.y, g.w);
    let inner_chars = ((pw - 28.0) / CONSOLE_CHAR_WIDTH) as usize;

    // Lift off the console/world, then the card itself.
    rect(out, x + 3.0, y + 5.0, pw, g.h, [0.0, 0.0, 0.0, 0.38], w, h);
    rect(out, x, y, pw, g.h, [0.020, 0.028, 0.056, 0.985], w, h);
    let edge_l = with_alpha(CON_CYAN, 0.55);
    let edge_r = with_alpha(CON_MAGENTA, 0.55);
    rect_gradient(out, x, y, pw, 1.0, edge_l, edge_r, edge_l, edge_r, w, h);
    rect_gradient(
        out,
        x,
        y + g.h - 1.0,
        pw,
        1.0,
        edge_l,
        edge_r,
        edge_l,
        edge_r,
        w,
        h,
    );
    rect(out, x, y, 1.0, g.h, with_alpha(CON_CYAN, 0.55), w, h);
    rect(
        out,
        x + pw - 1.0,
        y,
        1.0,
        g.h,
        with_alpha(CON_MAGENTA, 0.55),
        w,
        h,
    );

    let rows_y = y + SUGGEST_PAD;
    for row in 0..g.visible {
        let index = g.first + row;
        let Some(item) = ui.console_suggestions.get(index) else {
            break;
        };
        let ry = rows_y + row as f32 * SUGGEST_ROW_H;
        let is_selected = !hint && index == selected;
        if is_selected {
            rect_gradient(
                out,
                x + 1.0,
                ry,
                pw - 2.0,
                SUGGEST_ROW_H,
                [0.06, 0.26, 0.36, 0.85],
                [0.06, 0.16, 0.30, 0.55],
                [0.06, 0.26, 0.36, 0.85],
                [0.06, 0.16, 0.30, 0.55],
                w,
                h,
            );
            rect(out, x + 1.0, ry, 3.0, SUGGEST_ROW_H, CON_CYAN, w, h);
        }
        let text_y = ry + (SUGGEST_ROW_H - CONSOLE_CHAR_HEIGHT) * 0.5;
        let (badge, badge_color) = match item.kind {
            ConsoleSuggestKind::Cvar => ("VAR", CON_CYAN),
            ConsoleSuggestKind::Command => ("CMD", CON_AMBER),
            ConsoleSuggestKind::Server => ("SRV", CON_MAGENTA),
        };
        console_plain(
            out,
            badge,
            x + 14.0,
            text_y,
            with_alpha(badge_color, if is_selected { 1.0 } else { 0.72 }),
            w,
            h,
        );

        // Name, with the characters the query matched picked out in amber.
        let name_x = x + 14.0 + 4.0 * CONSOLE_CHAR_WIDTH;
        let value_room = if item.kind == ConsoleSuggestKind::Cvar {
            22
        } else {
            0
        };
        let name_room = inner_chars.saturating_sub(4 + value_room + 1).max(8);
        for (i, byte) in item.name.bytes().take(name_room).enumerate() {
            let matched = i < 64 && item.mask >> i & 1 == 1;
            let color = if matched {
                CON_AMBER
            } else if is_selected {
                CON_BRIGHT
            } else {
                CON_TEXT
            };
            glyph_quad_sized(
                out,
                byte,
                name_x + i as f32 * CONSOLE_CHAR_WIDTH,
                text_y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                color,
                w,
                h,
            );
            if matched {
                rect(
                    out,
                    name_x + i as f32 * CONSOLE_CHAR_WIDTH,
                    text_y + CONSOLE_CHAR_HEIGHT - 1.0,
                    CONSOLE_CHAR_WIDTH,
                    1.0,
                    with_alpha(CON_AMBER, 0.6),
                    w,
                    h,
                );
            }
        }

        if item.kind == ConsoleSuggestKind::Cvar && !item.value.is_empty() {
            let value = ellipsize(&item.value, value_room);
            let vx = x + pw - 14.0 - value.chars().count() as f32 * CONSOLE_CHAR_WIDTH;
            let color = if item.modified {
                CON_AMBER
            } else {
                with_alpha(CON_CYAN, 0.62)
            };
            console_plain(out, &value, vx, text_y, color, w, h);
        }
    }

    // Detail card for the highlighted entry.
    let detail_y = rows_y + g.visible as f32 * SUGGEST_ROW_H + 2.0;
    rect(
        out,
        x + 10.0,
        detail_y,
        pw - 20.0,
        1.0,
        with_alpha(CON_CYAN, 0.16),
        w,
        h,
    );
    if let Some(item) = ui.console_suggestions.get(selected) {
        let lines = wrap_plain(item.description, inner_chars, detail_lines);
        for (i, line) in lines.iter().enumerate() {
            console_plain(
                out,
                line,
                x + 14.0,
                detail_y + 6.0 + i as f32 * 18.0,
                CON_TEXT,
                w,
                h,
            );
        }
        let meta_y = detail_y + 6.0 + lines.len().max(1) as f32 * 18.0 + 3.0;
        let mut mx = x + 14.0;
        let mut meta = |label: &str, value: &str, color: [f32; 4], out: &mut Vec<UiVertex>| {
            if value.is_empty() || mx > x + pw - 40.0 {
                return;
            }
            let room = ((x + pw - 14.0 - mx) / CONSOLE_CHAR_WIDTH) as usize;
            let value = ellipsize(value, room.saturating_sub(label.len() + 1).max(4));
            console_plain(out, label, mx, meta_y, CON_FAINT, w, h);
            mx += (label.len() + 1) as f32 * CONSOLE_CHAR_WIDTH;
            console_plain(out, &value, mx, meta_y, color, w, h);
            mx += (value.chars().count() + 3) as f32 * CONSOLE_CHAR_WIDTH;
        };
        match item.kind {
            ConsoleSuggestKind::Cvar => {
                meta("DEFAULT", item.default_value, CON_DIM, out);
                meta("RANGE", item.range, CON_DIM, out);
            }
            ConsoleSuggestKind::Command => meta("COMMAND", "runs locally", CON_DIM, out),
            ConsoleSuggestKind::Server => {
                meta("SERVER COMMAND", "sent to the server", CON_DIM, out)
            }
        }
    }

    // Footer: match count and keys.
    let footer_y = y + g.h - SUGGEST_FOOTER_H;
    rect(
        out,
        x + 1.0,
        footer_y,
        pw - 2.0,
        SUGGEST_FOOTER_H - 1.0,
        [0.0, 0.0, 0.02, 0.35],
        w,
        h,
    );
    let summary = if hint {
        "ARGUMENT HINT".to_owned()
    } else if ui.console_suggest_total > count {
        format!("{} OF {} MATCHES", selected + 1, ui.console_suggest_total)
    } else {
        format!("{} OF {} MATCHES", selected + 1, count)
    };
    console_plain(out, &summary, x + 14.0, footer_y + 3.0, CON_CYAN, w, h);
    let keys_x = x + 14.0 + (summary.len() + 3) as f32 * CONSOLE_CHAR_WIDTH;
    if hint {
        console_keycaps(
            out,
            keys_x,
            footer_y + 3.0,
            x + pw - 8.0,
            &[("ENTER", "run")],
            w,
            h,
        );
    } else {
        console_keycaps(
            out,
            keys_x,
            footer_y + 3.0,
            x + pw - 8.0,
            &[
                ("TAB", "complete"),
                ("UP/DOWN", "select"),
                ("CLICK", "pick"),
                ("ESC", "hide"),
            ],
            w,
            h,
        );
    }
}
