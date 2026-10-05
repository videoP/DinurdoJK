//! Scoreboard.
use crate::ui::{
    proportional_text, proportional_text_width, rect, rect_gradient, rect_outline, text,
    truncate_jka_text, visible_jka_chars, ProportionalFont, UiScoreEntry, UiSnapshot, UiVertex,
};

pub(in crate::ui) fn build_scoreboard(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(board) = &ui.scoreboard else { return };

    // Renderer-native on purpose: +scores stays a lightweight gameplay overlay
    // and never changes input capture / enters the egui menu stack.
    // Layout is authored in a 1080p-ish screen-space and scales with height so
    // the board keeps the same visual weight from 720p through 4K.
    let s = (h as f32 / 1080.0).clamp(0.72, 2.0);
    let width = (960.0 * s).min((w as f32 - 32.0 * s).max(320.0 * s));
    let x = ((w as f32 - width) * 0.5).max(16.0 * s);
    let y = (h as f32 * 0.065).max(24.0 * s);

    let title_h = 48.0 * s;
    let columns_h = 27.0 * s;
    let row_h = 36.0 * s;
    let bottom_pad = 8.0 * s;
    let overflow_h = if board.entries.is_empty() {
        0.0
    } else {
        20.0 * s
    };
    let section_h = 30.0 * s;

    // jaPRO's CG_DrawOldScoreboard: players first (a team game lists the leading
    // team, then the other, then anyone on neither), with everyone spectating in a
    // block of their own underneath. Each group keeps the server's score order.
    enum Line<'a> {
        Entry(&'a UiScoreEntry, usize),
        Section(&'static str, usize),
    }
    let mut lines: Vec<Line> = Vec::with_capacity(board.entries.len() + 1);
    let team_order: &[i32] = if !board.team_game {
        &[0]
    } else if board.team_scores[0] >= board.team_scores[1] {
        &[1, 2, 0]
    } else {
        &[2, 1, 0]
    };
    for &team in team_order {
        let group = board
            .entries
            .iter()
            .filter(|entry| entry.team == team || (!board.team_game && entry.team != 3));
        for (index, entry) in group.enumerate() {
            lines.push(Line::Entry(entry, index));
        }
        if !board.team_game {
            break;
        }
    }
    let players = lines.len();
    let spectators = board.entries.iter().filter(|entry| entry.team == 3).count();
    if spectators > 0 {
        lines.push(Line::Section("SPECTATORS", spectators));
        for (index, entry) in board
            .entries
            .iter()
            .filter(|entry| entry.team == 3)
            .enumerate()
        {
            lines.push(Line::Entry(entry, index));
        }
    }

    let room_for_rows = (h as f32 - y - 18.0 * s - title_h - columns_h - bottom_pad).max(row_h);
    let available = (room_for_rows - overflow_h).max(row_h);
    let line_height = |line: &Line| {
        if matches!(line, Line::Section(..)) {
            section_h
        } else {
            row_h
        }
    };
    let mut shown = 0usize;
    let mut used = 0.0f32;
    for line in &lines {
        if used + line_height(line) > available && shown > 0 {
            break;
        }
        used += line_height(line);
        shown += 1;
    }
    // Never end on a heading with nothing under it.
    if shown > 0 && matches!(lines[shown - 1], Line::Section(..)) {
        used -= section_h;
        shown -= 1;
    }
    let overflow = lines[shown..]
        .iter()
        .filter(|line| matches!(line, Line::Entry(..)))
        .count();
    let footer_h = if overflow > 0 { overflow_h } else { 0.0 };
    let height = title_h + columns_h + used + footer_h + bottom_pad;

    const PANEL_TOP: [f32; 4] = [0.045, 0.055, 0.075, 0.965];
    const PANEL_BOTTOM: [f32; 4] = [0.014, 0.018, 0.028, 0.955];
    const TITLE_LEFT: [f32; 4] = [0.075, 0.105, 0.145, 0.98];
    const TITLE_RIGHT: [f32; 4] = [0.030, 0.040, 0.060, 0.98];
    const ACCENT: [f32; 4] = [0.28, 0.78, 0.94, 1.0];
    const TEXT_BRIGHT: [f32; 4] = [0.94, 0.965, 0.99, 1.0];
    const TEXT: [f32; 4] = [0.82, 0.86, 0.91, 1.0];
    const TEXT_DIM: [f32; 4] = [0.51, 0.58, 0.67, 1.0];

    // A soft, offset shadow plus a low-contrast outer keyline reads much cleaner
    // than the old bright grey rectangle without making the overlay feel like a
    // menu window.
    rect(
        out,
        x - 4.0 * s,
        y + 5.0 * s,
        width + 8.0 * s,
        height + 4.0 * s,
        [0.0, 0.0, 0.0, 0.30],
        w,
        h,
    );
    rect_gradient(
        out,
        x,
        y,
        width,
        height,
        PANEL_TOP,
        PANEL_TOP,
        PANEL_BOTTOM,
        PANEL_BOTTOM,
        w,
        h,
    );
    rect_outline(
        out,
        x,
        y,
        width,
        height,
        (1.0 * s).max(1.0),
        [0.30, 0.38, 0.48, 0.58],
        w,
        h,
    );

    // Header: restrained blue-grey panel with a thin cyan identity line. Team
    // scores (when relevant) live here rather than being mixed into the title.
    rect_gradient(
        out,
        x + 1.0 * s,
        y + 1.0 * s,
        width - 2.0 * s,
        title_h - 1.0 * s,
        TITLE_LEFT,
        TITLE_RIGHT,
        [0.045, 0.060, 0.085, 0.98],
        [0.022, 0.030, 0.046, 0.98],
        w,
        h,
    );
    rect(
        out,
        x + 1.0 * s,
        y + 1.0 * s,
        width - 2.0 * s,
        (2.0 * s).max(1.0),
        ACCENT,
        w,
        h,
    );

    let font_x_scale = w.max(1) as f32 / 640.0;
    let title_font_scale = 0.52;
    let header_font_scale = 0.31;
    let row_font_scale = 0.42;
    let meta_font_scale = 0.32;
    let fallback_title_scale = 1.34 * s;
    let fallback_header_scale = 0.92 * s;
    let fallback_row_scale = 1.06 * s;
    let fallback_meta_scale = 0.88 * s;

    let draw = |out: &mut Vec<UiVertex>,
                value: &str,
                tx: f32,
                baseline_y: f32,
                prop_scale: f32,
                fallback_scale: f32,
                color: [f32; 4]| {
        if let Some(font) = small_font {
            proportional_text(
                out, value, font, tx, baseline_y, prop_scale, color, true, w, h,
            );
        } else {
            // The proportional font is normally resident. Keep the charsgrid
            // fallback so a missing font asset can never hide the scoreboard.
            text(
                out,
                value,
                tx,
                baseline_y - 8.0 * fallback_scale,
                fallback_scale,
                color,
                w,
                h,
            );
        }
    };
    let text_width = |value: &str, prop_scale: f32, fallback_scale: f32| -> f32 {
        if let Some(font) = small_font {
            proportional_text_width(value, font, prop_scale) * font_x_scale
        } else {
            visible_jka_chars(value) as f32 * 6.0 * fallback_scale
        }
    };

    let pad = 18.0 * s;
    let title_baseline = y + title_h * 0.68;
    draw(
        out,
        "SCOREBOARD",
        x + pad,
        title_baseline,
        title_font_scale,
        fallback_title_scale,
        TEXT_BRIGHT,
    );

    let meta = if board.team_game {
        format!(
            "^1RED  {}    ^7|    ^4BLUE  {}",
            board.team_scores[0], board.team_scores[1]
        )
    } else {
        format!("{} PLAYER{}", players, if players == 1 { "" } else { "S" })
    };
    let meta_width = text_width(&meta, meta_font_scale, fallback_meta_scale);
    draw(
        out,
        &meta,
        x + width - pad - meta_width,
        y + title_h * 0.66,
        meta_font_scale,
        fallback_meta_scale,
        if board.team_game { TEXT } else { TEXT_DIM },
    );

    let columns_y = y + title_h;
    rect(
        out,
        x + 1.0 * s,
        columns_y,
        width - 2.0 * s,
        columns_h,
        [0.75, 0.82, 0.92, 0.055],
        w,
        h,
    );
    rect(
        out,
        x + 10.0 * s,
        columns_y + columns_h - (1.0 * s).max(1.0),
        width - 20.0 * s,
        (1.0 * s).max(1.0),
        [0.45, 0.56, 0.70, 0.26],
        w,
        h,
    );

    // Numeric columns are right aligned. This removes the ragged, debug-table
    // look the old scoreboard had while still preserving all vanilla fields.
    let team_right = x + width - pad;
    let time_right = if board.team_game {
        team_right - 82.0 * s
    } else {
        team_right
    };
    let ping_right = time_right - 92.0 * s;
    let deaths_right = ping_right - 92.0 * s;
    let score_right = if board.show_deaths {
        deaths_right - 92.0 * s
    } else {
        ping_right - 92.0 * s
    };
    let player_x = x + pad + 7.0 * s;
    let header_baseline = columns_y + columns_h * 0.69;

    draw(
        out,
        "PLAYER",
        player_x,
        header_baseline,
        header_font_scale,
        fallback_header_scale,
        TEXT_DIM,
    );
    for (label, right) in [
        ("SCORE", score_right),
        ("PING", ping_right),
        ("TIME", time_right),
    ] {
        let tw = text_width(label, header_font_scale, fallback_header_scale);
        draw(
            out,
            label,
            right - tw,
            header_baseline,
            header_font_scale,
            fallback_header_scale,
            TEXT_DIM,
        );
    }
    if board.show_deaths {
        let tw = text_width("DEATHS", header_font_scale, fallback_header_scale);
        draw(
            out,
            "DEATHS",
            deaths_right - tw,
            header_baseline,
            header_font_scale,
            fallback_header_scale,
            TEXT_DIM,
        );
    }
    if board.team_game {
        let tw = text_width("TEAM", header_font_scale, fallback_header_scale);
        draw(
            out,
            "TEAM",
            team_right - tw,
            header_baseline,
            header_font_scale,
            fallback_header_scale,
            TEXT_DIM,
        );
    }

    let row_start = columns_y + columns_h;
    let mut cursor = row_start;
    for line in lines.iter().take(shown) {
        let (entry, index) = match *line {
            Line::Section(label, count) => {
                rect(
                    out,
                    x + 10.0 * s,
                    cursor + 4.0 * s,
                    width - 20.0 * s,
                    (1.0 * s).max(1.0),
                    [0.45, 0.56, 0.70, 0.26],
                    w,
                    h,
                );
                draw(
                    out,
                    &format!("{label}  {count}"),
                    player_x,
                    cursor + section_h * 0.76,
                    header_font_scale,
                    fallback_header_scale,
                    TEXT_DIM,
                );
                cursor += section_h;
                continue;
            }
            Line::Entry(entry, index) => (entry, index),
        };
        let row_top = cursor;
        cursor += row_h;
        let row_y = row_top + row_h * 0.68;
        let spectating = entry.team == 3;

        let focused = ui.scoreboard_focus_client == Some(entry.client);
        let row_fill = if focused {
            // TaystJK fills the current `cg.snap->ps.clientNum` score line.
            // Keep this renderer's palette, but make the same ownership state
            // unmistakable without introducing a separate "VIEWING" label.
            [0.22, 0.58, 0.82, 0.24]
        } else if index % 2 == 0 {
            [0.80, 0.86, 0.96, 0.050]
        } else {
            [0.55, 0.62, 0.72, 0.022]
        };
        rect(
            out,
            x + 6.0 * s,
            row_top + 2.0 * s,
            width - 12.0 * s,
            row_h - 3.0 * s,
            row_fill,
            w,
            h,
        );
        if focused {
            rect_outline(
                out,
                x + 6.0 * s,
                row_top + 2.0 * s,
                width - 12.0 * s,
                row_h - 3.0 * s,
                (1.0 * s).max(1.0),
                [0.38, 0.82, 1.0, 0.72],
                w,
                h,
            );
        }

        let accent = match entry.team {
            1 => [0.92, 0.22, 0.24, 0.95],
            2 => [0.24, 0.48, 0.96, 0.95],
            3 => [0.50, 0.55, 0.62, 0.72],
            _ => ACCENT,
        };
        rect(
            out,
            x + 6.0 * s,
            row_top + 2.0 * s,
            (3.0 * s).max(2.0),
            row_h - 3.0 * s,
            accent,
            w,
            h,
        );

        let name = truncate_jka_text(&entry.name, if board.team_game { 24 } else { 30 });
        let row_text = if entry.team == 3 {
            [TEXT[0], TEXT[1], TEXT[2], 0.72]
        } else {
            TEXT
        };
        draw(
            out,
            &name,
            player_x,
            row_y,
            row_font_scale,
            fallback_row_scale,
            row_text,
        );

        if !spectating || board.spectator_scores {
            let score = entry.score.to_string();
            let score_w = text_width(&score, row_font_scale, fallback_row_scale);
            draw(
                out,
                &score,
                score_right - score_w,
                row_y,
                row_font_scale,
                fallback_row_scale,
                TEXT_BRIGHT,
            );
            if board.show_deaths {
                if let Some(deaths) = entry.deaths {
                    let deaths = deaths.to_string();
                    let deaths_w = text_width(&deaths, row_font_scale, fallback_row_scale);
                    draw(
                        out,
                        &deaths,
                        deaths_right - deaths_w,
                        row_y,
                        row_font_scale,
                        fallback_row_scale,
                        TEXT,
                    );
                }
            }
        }

        let ping = if entry.ping < 0 {
            "CNCT".to_owned()
        } else {
            entry.ping.to_string()
        };
        let ping_w = text_width(&ping, row_font_scale, fallback_row_scale);
        let ping_color = if entry.ping < 0 {
            TEXT_DIM
        } else if entry.ping <= 60 {
            [0.52, 0.88, 0.66, 1.0]
        } else if entry.ping <= 120 {
            [0.88, 0.84, 0.50, 1.0]
        } else {
            [0.94, 0.55, 0.52, 1.0]
        };
        draw(
            out,
            &ping,
            ping_right - ping_w,
            row_y,
            row_font_scale,
            fallback_row_scale,
            ping_color,
        );

        let time = entry.time.to_string();
        let time_w = text_width(&time, row_font_scale, fallback_row_scale);
        draw(
            out,
            &time,
            time_right - time_w,
            row_y,
            row_font_scale,
            fallback_row_scale,
            TEXT,
        );

        if board.team_game && !spectating {
            let team = match entry.team {
                1 => "^1RED",
                2 => "^4BLUE",
                _ => "FREE",
            };
            let team_w = text_width(team, meta_font_scale, fallback_meta_scale);
            draw(
                out,
                team,
                team_right - team_w,
                row_y,
                meta_font_scale,
                fallback_meta_scale,
                TEXT_DIM,
            );
        }
    }

    if overflow > 0 {
        let footer_top = row_start + used;
        rect(
            out,
            x + 10.0 * s,
            footer_top,
            width - 20.0 * s,
            (1.0 * s).max(1.0),
            [0.42, 0.52, 0.64, 0.20],
            w,
            h,
        );
        let more = format!(
            "+{} MORE PLAYER{}",
            overflow,
            if overflow == 1 { "" } else { "S" }
        );
        draw(
            out,
            &more,
            x + pad,
            footer_top + footer_h * 0.70,
            meta_font_scale,
            fallback_meta_scale,
            TEXT_DIM,
        );
    }
}
