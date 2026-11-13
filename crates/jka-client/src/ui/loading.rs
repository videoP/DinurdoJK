//! Loading.
use super::loading_layout::{self, ConnectionPromptUi};
use crate::ui::{
    lighten, rect, rect_gradient, text, textured_rect_with_source, with_alpha, MapLoadingBar,
    MapLoadingPhase, MapLoadingUi, UiVertex, HUD_INK_BOTTOM, HUD_INK_TOP, HUD_VALUE,
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

/// Keep the file extension and both ends of a long PK3 name visible without
/// drawing into the adjacent progress track. The full path remains in footer.
fn short_download_name(remote_name: &str, max_chars: usize) -> String {
    let filename = remote_name.rsplit(['/', '\\']).next().unwrap_or(remote_name);
    let len = filename.chars().count();
    if len <= max_chars {
        return filename.to_owned();
    }
    let tail = 5.min(max_chars.saturating_sub(3));
    let head = max_chars.saturating_sub(tail + 3);
    let suffix = filename.chars().skip(len - tail).collect::<String>();
    format!("{}...{}", filename.chars().take(head).collect::<String>(), suffix)
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
        HUD_INK_TOP(),
        HUD_INK_TOP(),
        HUD_INK_BOTTOM(),
        HUD_INK_BOTTOM(),
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
        HUD_VALUE(),
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

/// Complete fullscreen loading art, matching JKA/TaystJK's 2D fullscreen
/// presentation. A 4:3 fallback is stretched to the viewport rather than
/// cropped. Native widescreen variants are selected by the renderer first.
pub(in crate::ui) fn build_splash_background(
    out: &mut Vec<UiVertex>, w: u32, h: u32, splash_size: Option<[u32; 2]>,
) {
    rect(out, 0.0, 0.0, w as f32, h as f32, [0.0, 0.0, 0.0, 1.0], w, h);
    if splash_size.is_some_and(|size| size[0] > 0 && size[1] > 0) {
        textured_rect_with_source(out, 0.0, 0.0, w as f32, h as f32,
            [0.0, 0.0], [1.0, 1.0], [1.0; 4], 3.0, w, h);
    }
}

// The entire connecting screen is composed in the native WGPU UI pass.
// Taffy supplies the rectangles; UiText supplies a high-resolution font atlas;
// neon-menu's *actual tube-font geometry* supplies the authored headings.
// No egui painter, egui font or egui pointer response is used here.
use super::text::UiText;
use crate::neon_menu::font::{self as neon_font, Prim};
use super::loading_layout::ConnectionAction;

type B = [f32; 4];

fn ellipsize(txt: &UiText<'_>, value: &str, px: f32, max_w: f32) -> String {
    if txt.width(value, px) <= max_w { return value.to_owned(); }
    let ellipsis = "...";
    let budget = (max_w - txt.width(ellipsis, px)).max(0.0);
    let mut result = String::new();
    for ch in value.chars() {
        let mut candidate = result.clone();
        candidate.push(ch);
        if txt.width(&candidate, px) > budget { break; }
        result.push(ch);
    }
    result.push_str(ellipsis);
    result
}

/// Caption inside a progress rail. Download names keep their PK3 suffix,
/// and a pending READY rail explicitly says what the client is awaiting.
/// Width checks use the real UI font rather than character-count guesses.
fn task_bar_caption(txt: &UiText<'_>, bar: &MapLoadingBar,
    download_name: Option<&str>, px: f32, max_w: f32) -> String
{
    if bar.label == "SERVER CONTENT" {
        if let Some(name) = download_name {
            let prefix = "DL: ";
            let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
            for chars in (8..=basename.chars().count().max(8)).rev() {
                let label = format!("{prefix}{}", short_download_name(name, chars));
                if txt.width(&label, px) <= max_w { return label; }
            }
            return ellipsize(txt, "DOWNLOADING", px, max_w);
        }
    }
    let label = match bar.label {
        "SERVER SNAPSHOT" if bar.total == 0 || bar.completed < bar.total => "AWAIT FIRST SNAPSHOT",
        "VIDEO" => "RESTART VIDEO",
        other => other,
    };
    ellipsize(txt, label, px, max_w)
}

fn draw_line(txt: &UiText<'_>, out: &mut Vec<UiVertex>, value: &str,
    b: B, px: f32, color: [f32; 4], w: u32, h: u32)
{
    let _ = (w, h); // UiText owns viewport dimensions.
    let line = ellipsize(txt, value, px, b[2].max(1.0));
    txt.draw(out, &line, b[0], txt.baseline_in(b, px), px, color);
}

/// Word wrap by *font advance* (not an assumed fixed 6px character width).
/// At most two lines fit in the Taffy-reserved status/explanation cell.
fn wrap_two(txt: &UiText<'_>, value: &str, px: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
        if !line.is_empty() && txt.width(&candidate, px) > max_w {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() { line.push(' '); }
        line.push_str(word);
    }
    if !line.is_empty() { lines.push(line); }
    if lines.len() > 2 {
        let rest = lines[1..].join(" ");
        lines.truncate(1);
        lines.push(ellipsize(txt, &rest, px, max_w));
    }
    for line in &mut lines { *line = ellipsize(txt, line, px, max_w); }
    lines
}

fn draw_two_lines(txt: &UiText<'_>, out: &mut Vec<UiVertex>, value: &str,
    b: B, px: f32, color: [f32; 4])
{
    let lines = wrap_two(txt, value, px, b[2]);
    let line_height = (px * 1.22).min(b[3] * 0.5);
    let block_h = (lines.len() as f32) * line_height;
    let mut y = b[1] + (b[3] - block_h).max(0.0) * 0.5;
    for line in lines {
        txt.draw(out, &line, b[0], txt.baseline_in([b[0], y, b[2], line_height], px), px, color);
        y += line_height;
    }
}

fn neon_stroke(out: &mut Vec<UiVertex>, p: [f32; 2], q: [f32; 2],
    radius: f32, color: [f32; 4], w: u32, h: u32)
{
    let dx = q[0] - p[0];
    let dy = q[1] - p[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 { return; }
    let n = [-dy * radius / len, dx * radius / len];
    let xy = |x: f32, y: f32| [2.0 * x / w.max(1) as f32 - 1.0,
        1.0 - 2.0 * y / h.max(1) as f32];
    let v = |point| UiVertex { position: point, uv: [0.0, 0.0], color: UiVertex::pack_color(color), textured: 0.0 };
    let a = xy(p[0] + n[0], p[1] + n[1]);
    let b = xy(p[0] - n[0], p[1] - n[1]);
    let c = xy(q[0] - n[0], q[1] - n[1]);
    let d = xy(q[0] + n[0], q[1] + n[1]);
    out.extend_from_slice(&[v(a), v(b), v(c), v(a), v(c), v(d)]);
}

/// Draw the very same Star Jedi outline / stroke-tube letterforms as the main
/// neon menu, not a scaled copy of the 6x8 JKA bitmap atlas.
fn neon_heading(out: &mut Vec<UiVertex>, text: &str, b: B, max_cap: f32,
    color: [f32; 4], w: u32, h: u32)
{
    let glyphs = neon_font::place(text);
    let width = neon_font::measure(text);
    if width <= 0.0 { return; }
    let cap = max_cap.min(b[2] / width).min(b[3] * 0.83).max(1.0);
    let base = b[1] + (b[3] + cap) * 0.5;
    let radius = (neon_font::tube_ratio() * cap).clamp(0.55, 1.25);
    for (pass, radius_multiplier, alpha) in [(0, 2.9, 0.14), (1, 1.0, 1.0)] {
        let _ = pass;
        let ink = [color[0], color[1], color[2], color[3] * alpha];
        let r = radius * radius_multiplier;
        for placed in &glyphs {
            let origin_x = b[0] + placed.x * cap;
            let point = |p: [f32; 2]| [origin_x + p[0] * cap, base - p[1] * cap];
            for prim in placed.glyph.prims.iter() {
                match *prim {
                    Prim::Line {a, b} => neon_stroke(out, point(a), point(b), r, ink, w, h),
                    Prim::Arc {c, r: arc_r, start, sweep} => {
                        let segments = ((sweep.abs() / 15.0).ceil() as usize).clamp(2, 32);
                        let mut last = [c[0] + arc_r * start.to_radians().cos(),
                            c[1] + arc_r * start.to_radians().sin()];
                        for i in 1..=segments {
                            let angle = (start + sweep * i as f32 / segments as f32).to_radians();
                            let next = [c[0] + arc_r * angle.cos(), c[1] + arc_r * angle.sin()];
                            neon_stroke(out, point(last), point(next), r, ink, w, h);
                            last = next;
                        }
                    }
                }
            }
        }
    }
}

fn title(txt: &UiText<'_>, out: &mut Vec<UiVertex>, value: &str, b: B,
    cap: f32, color: [f32; 4], w: u32, h: u32)
{
    // Not every installed Star Jedi font contains digits, punctuation or
    // mod map names. Keep the full readable name when it cannot be tubed.
    let supported = value.chars().all(|ch| ch.is_whitespace() || neon_font::glyph(ch).is_some());
    if supported { neon_heading(out, value, b, cap, color, w, h); }
    else { draw_line(txt, out, value, b, cap, color, w, h); }
}

fn paint_button(txt: &UiText<'_>, out: &mut Vec<UiVertex>, bounds: B, label: &str,
    main: bool, enabled: bool, hovered: bool, s: f32, w: u32, h: u32)
{
    let [x, y, bw, bh] = bounds;
    let bg = if !enabled { [0.055, 0.071, 0.103, 0.82] }
        else if hovered && main { [0.08, 0.30, 0.39, 0.99] }
        else if hovered { [0.16, 0.20, 0.32, 0.99] }
        else if main { [0.050, 0.19, 0.25, 0.95] }
        else { [0.071, 0.087, 0.14, 0.95] };
    rect(out, x, y, bw, bh, bg, w, h);
    rect_gradient(out, x, y, bw, if hovered { 2.0 } else { 1.0 },
        if main { LOAD_CYAN } else { LOAD_LABEL },
        if main { LOAD_MAGENTA } else { LOAD_FAINT },
        if main { LOAD_CYAN } else { LOAD_LABEL },
        if main { LOAD_MAGENTA } else { LOAD_FAINT }, w, h);
    if hovered && enabled {
        // Visible feedback on both primary and secondary choices.
        super::rect_outline(out, x, y, bw, bh, 1.3, with_alpha(LOAD_CYAN, 0.73), w, h);
        rect(out, x + 1.0, y + bh - 3.0, bw - 2.0, 2.0, with_alpha(LOAD_CYAN, 0.4), w, h);
    }
    let px = 14.0 * s;
    let text = ellipsize(txt, label, px, (bw - 20.0 * s).max(1.0));
    txt.draw_centered(out, &text, bounds, px, if !enabled { LOAD_FAINT }
        else if hovered { [1.0, 1.0, 1.0, 1.0] }
        else if main { [0.90, 0.98, 1.0, 1.0] } else { LOAD_LABEL });
}

fn paint_connection_prompt(txt: &UiText<'_>, out: &mut Vec<UiVertex>, prompt: &ConnectionPromptUi,
    g: &loading_layout::DialogLayout, hovered: Option<ConnectionAction>, s: f32, w: u32, h: u32)
{
    rect(out, 0.0, 0.0, w as f32, h as f32, [0.002, 0.004, 0.014, 0.69], w, h);
    let [x, y, bw, bh] = g.panel;
    rect_gradient(out, x, y, bw, bh, [0.018, 0.028, 0.063, 0.99], [0.018, 0.028, 0.063, 0.99],
        [0.025, 0.037, 0.072, 0.99], [0.025, 0.037, 0.072, 0.99], w, h);
    neon_edge(out, x, y, bw, 2.0, -1.0, w, h);
    match prompt {
        ConnectionPromptUi::MissingMap { map_name, reason, can_download } => {
            let missing_paks = reason.starts_with("Missing server-referenced PK3");
            draw_line(txt, out, if missing_paks { "SERVER-REFERENCED CONTENT IS MISSING" }
                else { "THE SERVER USES A MAP NOT INSTALLED LOCALLY" },
                g.kicker, 12.5*s, LOAD_CYAN, w, h);
            title(txt, out, if missing_paks { "SERVER PACKAGES MISSING" } else { "MAP NOT FOUND" },
                g.title, 24.0*s, [0.90, 0.97, 1.0, 1.0], w, h);
            rect(out, g.subject[0], g.subject[1], g.subject[2], g.subject[3], [0.045, 0.065, 0.12, 0.99], w, h);
            draw_line(txt, out, map_name, [g.subject[0]+13.0*s, g.subject[1], g.subject[2]-26.0*s, g.subject[3]],
                18.0*s, LOAD_AMBER, w, h);
            draw_two_lines(txt, out, reason, g.explanation, 14.0*s, LOAD_LABEL);
            // Help is two separate lines within the one Taffy reserved cell.
            let half = g.help[3] * 0.5;
            draw_line(txt, out, "Choose how to continue:", [g.help[0],g.help[1],g.help[2],half], 13.0*s, LOAD_LABEL, w, h);
            draw_line(txt, out, if *can_download { "HTTP preferred; legacy UDP used when enabled." }
                else { "No enabled download transport is available." },
                [g.help[0],g.help[1]+half,g.help[2],half], 12.0*s,
                if *can_download { LOAD_FAINT } else { LOAD_AMBER }, w, h);
            paint_button(txt, out, g.primary, "AUTO-DOWNLOAD & CONNECT", true, *can_download,
                hovered == Some(ConnectionAction::Download), s, w, h);
            paint_button(txt, out, g.secondary, "CONNECT ANYWAY", false, true,
                hovered == Some(ConnectionAction::ConnectAnyway), s, w, h);
            paint_button(txt, out, g.tertiary, "ABORT", false, true,
                hovered == Some(ConnectionAction::Abort), s, w, h);
        }
        ConnectionPromptUi::Password { target, message, masked_len } => {
            draw_line(txt, out, "AUTHENTICATION REQUIRED", g.kicker, 12.5*s, LOAD_CYAN, w, h);
            title(txt, out, "SERVER PASSWORD", g.title, 24.0*s, [0.90, 0.97, 1.0, 1.0], w, h);
            rect(out,g.subject[0],g.subject[1],g.subject[2],g.subject[3],[0.05,0.08,0.14,0.99],w,h);
            let max = (g.subject[2] / (10.0*s)).floor().max(1.0) as usize;
            let bullets = "*".repeat((*masked_len).min(max.saturating_sub(1)));
            draw_line(txt, out, if bullets.is_empty() {"PASSWORD"} else {&bullets},
                [g.subject[0]+13.0*s,g.subject[1],g.subject[2]-26.0*s,g.subject[3]],
                17.0*s, if bullets.is_empty() {LOAD_FAINT} else {[0.95,0.98,1.0,1.0]},w,h);
            draw_line(txt, out, target, g.explanation, 14.0*s, LOAD_LABEL, w, h);
            draw_two_lines(txt, out, message.as_deref().unwrap_or("Enter the password to connect."),
                g.help, 13.5*s, if message.is_some() {LOAD_AMBER} else {LOAD_LABEL});
            paint_button(txt, out, g.primary, "CONNECT", true, true,
                hovered == Some(ConnectionAction::PasswordConnect), s, w, h);
            paint_button(txt, out, g.secondary, "CANCEL", false, true,
                hovered == Some(ConnectionAction::PasswordCancel), s, w, h);
        }
    }
}

pub(in crate::ui) fn build_loading_screen(
    out: &mut Vec<UiVertex>, ui: &crate::ui::UiSnapshot,
    loading: &MapLoadingUi, small_font: Option<&crate::ui::ProportionalFont>,
    w: u32, h: u32, splash_size: Option<[u32; 2]>,
) {
    build_splash_background(out,w,h,splash_size);
    let fade_h = h as f32 * 0.59;
    rect_gradient(out, 0.0, h as f32-fade_h, w as f32, fade_h,
        [0.0,0.0,0.02,0.0], [0.0,0.0,0.02,0.0],
        [0.0,0.0,0.02,0.79], [0.0,0.0,0.02,0.79], w, h);
    let g = loading_layout::layout(loading,w,h);
    let hovered = loading.pointer.and_then(|p| g.hit_test(p, loading.connection_prompt.as_ref()));
    let txt = UiText::new(ui, small_font, w, h);
    let s = g.text_scale;
    let [x,y,pw,ph] = g.panel;
    rect_gradient(out,x,y,pw,ph,[0.012,0.016,0.038,0.94],[0.012,0.016,0.038,0.94],
        [0.022,0.028,0.064,0.96],[0.022,0.028,0.064,0.96],w,h);
    neon_edge(out,x,y,pw,2.0,-1.0,w,h);
    title(&txt,out,if loading.joining_server {"JOINING SERVER"} else {"LOADING MAP"},
        g.kicker,14.0*s,LOAD_CYAN,w,h);
    let heading = loading.map_name.to_ascii_uppercase();
    title(&txt,out,&heading,g.heading,25.0*s,[0.92,0.98,1.0,1.0],w,h);
    // Count completed work units, not estimated seconds. A cold shader
    // compile can dominate wall time; the header deliberately says TASKS.
    let visible: Vec<&MapLoadingBar> = loading.bars.iter().filter(|bar| !bar.skipped).collect();
    let completed = visible.iter().filter(|bar| bar.total > 0
        && bar.completed >= bar.total).count();
    let total = visible.len();
    let overall = if total == 0 { 0.0 } else {
        visible.iter().map(|b| if b.label == "SERVER CONTENT" {
            loading.download_overall_fraction.unwrap_or_else(|| loading_fraction(b))
        } else { loading_fraction(b) }).sum::<f32>() / total as f32
    };
    let count = format!("{completed}/{total} TASKS");
    txt.draw_right(out, &count, g.percentage[0] + g.percentage[2],
        txt.baseline_in(g.percentage, 14.0*s), 14.0*s, LOAD_AMBER);
    progress_track(out, g.overall_track[0],g.overall_track[1],g.overall_track[2],
        g.overall_track[3],overall,LOAD_CYAN,LOAD_MAGENTA,w,h);

    for (idx, phase) in MapLoadingPhase::ALL.iter().copied().enumerate() {
        let group: Vec<_> = visible.iter().copied().filter(|bar| bar.phase == phase).collect();
        let layout = &g.phases[idx];
        let done = group.iter().filter(|b| b.total > 0 && b.completed >= b.total).count();
        let active = group.iter().filter(|b| b.total > 0 && b.completed < b.total).count();
        let previous_complete = visible.iter().filter(|b| (b.phase as usize) < idx)
            .all(|b| b.total > 0 && b.completed >= b.total);
        // The first frame cannot be produced while GPU setup is pending.
        let blocked = !previous_complete && active == 0;
        for (bar, bounds) in group.iter().zip(layout.bars.iter()) {
            let complete = bar.total > 0 && bar.completed >= bar.total;
            let (from,to) = if bar.no_work {
                (with_alpha(LOAD_FAINT,0.45), LOAD_FAINT)
            } else if complete {
                (with_alpha(LOAD_DONE,0.72), LOAD_DONE)
            } else {
                (with_alpha(LOAD_CYAN,0.60), LOAD_CYAN)
            };
            progress_track(out, bounds[0],bounds[1],bounds[2],bounds[3],
                loading_fraction(bar),from,to,w,h);
            // Labels live inside the Taffy-sized rails, never in a separate
            // footer. UiText centres the text vertically and supplies a soft
            // drop shadow for contrast over partially filled tracks.
            let px = (12.0*s).min((bounds[3]-5.0*s).max(0.0));
            if px >= 7.0 {
                let inset = 7.0*s;
                let cell = [bounds[0]+inset, bounds[1],
                    (bounds[2]-2.0*inset).max(1.0), bounds[3]];
                let label = task_bar_caption(&txt, bar, loading.download_file_name.as_deref(),
                    px, cell[2]);
                let ink = if bar.no_work { LOAD_LABEL }
                    else if complete { [0.025, 0.10, 0.13, 1.0] }
                    else { [0.89, 0.96, 1.0, 1.0] };
                txt.draw(out, &label, cell[0], txt.baseline_in(cell, px), px, ink);
            }
        }
        let name = format!("{}. {}", idx+1, phase.title());
        let finished = !group.is_empty() && done == group.len();
        txt.draw_centered(out, &name, layout.heading, 12.5*s,
            if finished { LOAD_DONE } else if blocked { LOAD_FAINT } else { LOAD_LABEL });
        let stage_status = if group.is_empty() { "NOT NEEDED".to_owned() }
            else if finished { "COMPLETE".to_owned() }
            else if blocked || (phase == MapLoadingPhase::Ready && !previous_complete) {
                "WAITING".to_owned()
            } else if active > 0 {
                format!("{active} ACTIVE")
            } else { "PENDING".to_owned() };
        txt.draw_centered(out, &stage_status, layout.status, 10.5*s,
            if finished { LOAD_DONE }
            else if active > 0 && !blocked
                && !(phase == MapLoadingPhase::Ready && !previous_complete) { LOAD_CYAN }
            else { LOAD_FAINT });
    }
    if let Some(cancel) = g.cancel {
        paint_button(&txt,out,cancel,"CANCEL CONNECTION",false,true,
            hovered==Some(ConnectionAction::CancelJoin),s,w,h);
    }
}

/// Modal draws last so it stays above the classic console; only the native
/// Taffy UI owns its pointer and keyboard handling.
pub(in crate::ui) fn build_loading_modal(
    out: &mut Vec<UiVertex>, ui: &crate::ui::UiSnapshot, loading: &MapLoadingUi,
    small_font: Option<&crate::ui::ProportionalFont>, w: u32, h: u32,
) {
    let Some(prompt) = loading.connection_prompt.as_ref() else {return;};
    let g = loading_layout::layout(loading,w,h);
    if let Some(dialog) = g.dialog.as_ref() {
        let hovered = loading.pointer.and_then(|p| g.hit_test(p,Some(prompt)));
        let txt = UiText::new(ui, small_font, w, h);
        paint_connection_prompt(&txt,out,prompt,dialog,hovered,g.text_scale,w,h);
    }
}

pub(in crate::ui) const LOAD_CYAN: [f32; 4] = [0.28, 0.86, 0.95, 1.0];

pub(in crate::ui) const LOAD_MAGENTA: [f32; 4] = [1.0, 0.30, 0.56, 1.0];

pub(in crate::ui) const LOAD_AMBER: [f32; 4] = [1.0, 0.72, 0.28, 1.0];

pub(in crate::ui) const LOAD_DONE: [f32; 4] = [0.30, 0.85, 0.66, 1.0];

pub(in crate::ui) const LOAD_LABEL: [f32; 4] = [0.62, 0.72, 0.84, 1.0];

pub(in crate::ui) const LOAD_FAINT: [f32; 4] = [0.38, 0.46, 0.58, 1.0];

/// Compact, noninteractive top-right indicator for live pipeline compilation.
/// Uses the existing legacy UI draw list, so it works during gameplay even
/// when the egui menu is closed and has no dependency on a freshly built PSO.
pub(in crate::ui) fn build_shader_compile_progress(
    out: &mut Vec<UiVertex>,
    progress: &MapLoadingBar,
    stage: Option<&str>,
    w: u32,
    h: u32,
) {
    let panel_w = 340.0;
    let panel_h = if stage.is_some() { 61.0 } else { 43.0 };
    let x = (w as f32 - panel_w - 16.0).max(8.0);
    // Below the top-right FPS/timer readouts rather than on top of them.
    let y = 60.0;
    rect(out, x, y, panel_w, panel_h, [0.025, 0.04, 0.07, 0.82], w, h);
    rect_gradient(
        out, x, y, panel_w, 1.5,
        with_alpha(LOAD_CYAN, 0.9), with_alpha(LOAD_MAGENTA, 0.9),
        with_alpha(LOAD_CYAN, 0.9), with_alpha(LOAD_MAGENTA, 0.9), w, h,
    );
    text(out, progress.label, x + 12.0, y + 9.0, 1.0, LOAD_LABEL, w, h);
    let value = format!("{}/{}", progress.completed, progress.total);
    let value_w = value.len() as f32 * 6.0 * 1.1;
    text(out, &value, x + panel_w - 12.0 - value_w, y + 9.0, 1.1, HUD_VALUE(), w, h);
    if let Some(stage) = stage {
        // Width-limited so diagnostic labels cannot run into the game view.
        let short = stage.chars().take(52).collect::<String>();
        text(out, &short, x + 12.0, y + 27.0, 0.82, LOAD_LABEL, w, h);
    }
    progress_track(
        out, x + 12.0, y + panel_h - 15.0, panel_w - 24.0, 5.0,
        loading_fraction(progress), with_alpha(LOAD_CYAN, 0.6), LOAD_CYAN, w, h,
    );
}

#[cfg(test)]
mod download_label_tests {
    use super::{neon_heading, short_download_name, task_bar_caption, LOAD_CYAN};
    use crate::ui::{MapLoadingBar, MapLoadingPhase};
    use crate::ui::text::UiText;

    #[test]
    fn loading_headings_reuse_neon_menu_vector_letterforms() {
        let mut out = Vec::new();
        neon_heading(&mut out, "MAP NOT FOUND", [20.0, 10.0, 360.0, 38.0],
            24.0, LOAD_CYAN, 1280, 720);
        assert!(!out.is_empty());
        assert!(out.iter().all(|v| v.textured == 0.0));
    }

    #[test]
    fn pending_snapshot_is_named_in_its_ready_bar() {
        let font = UiText::legacy_only(None, 1280, 720);
        let bar = MapLoadingBar { phase: MapLoadingPhase::Ready,
            label: "SERVER SNAPSHOT", completed: 0, total: 1,
            skipped: false, no_work: false };
        assert_eq!(task_bar_caption(&font, &bar, None, 12.0, 180.0),
            "AWAIT FIRST SNAPSHOT");
    }

    #[test]
    fn download_bar_preserves_filename_suffix_within_real_font_width() {
        let font = UiText::legacy_only(None, 1280, 720);
        let bar = MapLoadingBar { phase: MapLoadingPhase::Assets,
            label: "SERVER CONTENT", completed: 7, total: 20,
            skipped: false, no_work: false };
        let caption = task_bar_caption(&font, &bar,
            Some("japro/extremely_long_custom_map_pack.pk3"), 12.0, 142.0);
        assert!(caption.starts_with("DL: "));
        assert!(caption.ends_with("k.pk3"));
        assert!(font.width(&caption, 12.0) <= 142.0);
    }

    #[test]
    fn download_label_updates_filename_without_overflowing_its_column() {
        assert_eq!(short_download_name("japro/map.pk3", 19), "map.pk3");
        let long = short_download_name("japro/extremely_long_custom_map_pack.pk3", 19);
        assert!(long.chars().count() <= 19);
        assert!(long.ends_with("k.pk3"));
    }
}
