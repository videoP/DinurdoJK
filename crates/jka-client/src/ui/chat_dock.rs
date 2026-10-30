//! Docked chat (`cg_chatStyle 1`): the typing field sits at the bottom of the
//! chat feed, inside one glass panel with the recent history, instead of a
//! bar across the top of the screen. It eases in and out, the caret blinks,
//! and three dots come and go while you pause mid-message.
//!
//! Drawn inside `draw_placed(Chat)`, so HUD Edit moves and sizes the field
//! together with the feed.
use crate::palette::hud as pal;
use crate::ui::flex::{Container, Flex};
use crate::ui::text::UiText;
use crate::ui::{disc, rect, ChatMode, ProportionalFont, UiSnapshot, UiVertex, CHATBOX_CUTOFF, CHATBOX_Y};
use taffy::prelude::AlignItems;

/// Greedy word wrap measured with the text it will be drawn in. Colour codes
/// carry over to the next line so a wrapped message keeps its colour.
fn wrap(txt: &UiText, value: &str, px: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut colour = String::new();
    for word in value.split(' ') {
        let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
        if !line.is_empty() && txt.width(&candidate, px) > max_width {
            lines.push(std::mem::take(&mut line));
            line = format!("{colour}{word}");
        } else {
            line = candidate;
        }
        // Remember the last colour code seen, for the continuation line.
        let bytes = word.as_bytes();
        for i in 0..bytes.len().saturating_sub(1) {
            if bytes[i] == b'^' && bytes[i + 1].is_ascii_alphanumeric() {
                colour = format!("^{}", bytes[i + 1] as char);
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The typed text with its oldest characters dropped until it fits.
fn tail_to_fit(txt: &UiText, value: &str, px: f32, max_width: f32) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut start = 0;
    while start < chars.len() && txt.width(&chars[start..].iter().collect::<String>(), px) > max_width {
        start += 1;
    }
    chars[start..].iter().collect()
}

/// How many of the three dots show, `idle_ms` into a pause: they appear one
/// by one, then leave one by one.
pub(in crate::ui) fn typing_dots(idle_ms: u32) -> usize {
    const START_MS: u32 = 900;
    const STEP_MS: u32 = 190;
    if idle_ms < START_MS {
        return 0;
    }
    [1, 2, 3, 3, 2, 1, 0][((idle_ms - START_MS) / STEP_MS) as usize % 7]
}

pub(in crate::ui) fn build(out: &mut Vec<UiVertex>, ui: &UiSnapshot, small_font: Option<&ProportionalFont>, w: u32, h: u32) {
    let s = (h as f32 / 1080.0).clamp(0.6, 2.0);
    let txt = UiText::new(ui, small_font, w, h);
    let px = 17.5 * s;
    let line_h = 25.0 * s;
    let anim = ui.chat_anim;
    let typing = ui.mode == crate::ui::OverlayMode::Chat;
    let open = if typing { anim.open.max(0.01) } else { anim.open };
    let x0 = 24.0 * s;
    let bottom = CHATBOX_Y / 480.0 * h as f32;
    let width = ((CHATBOX_CUTOFF * ui.hud_layout.chat.extent[0]) * h as f32 / 480.0 * 0.92)
        .clamp(280.0 * s, w as f32 * 0.62);
    let input_h = 38.0 * s;
    let max_lines = if typing {
        usize::from(ui.chat_settings.history_lines).max(1)
    } else {
        ((52.0 * ui.hud_layout.chat.extent[1]) / 13.0).floor().max(1.0) as usize
    };

    // Lines, wrapped, newest at the bottom; keep the last `max_lines`.
    let mut lines: Vec<(String, f32, f32)> = Vec::new();
    for line in &ui.chat_lines {
        let alpha = line.alpha.clamp(0.0, 1.0);
        if alpha <= 0.01 {
            continue;
        }
        for part in wrap(&txt, &line.text, px, width - 20.0 * s) {
            lines.push((part, alpha, line.enter));
        }
    }
    let skip = lines.len().saturating_sub(max_lines);
    let lines = &lines[skip..];

    // The field takes the feed's bottom slot while open; the feed rises above it.
    let rise = (input_h + 8.0 * s) * open;
    let feed_bottom = bottom - rise;
    let feed_top = feed_bottom - lines.len() as f32 * line_h;
    if open > 0.0 {
        let top = if lines.is_empty() { bottom - input_h - 12.0 * s } else { feed_top - 10.0 * s };
        let panel_h = bottom + 6.0 * s - top;
        rect(out, x0 - 10.0 * s, top, width + 20.0 * s, panel_h, pal::fade(pal::SLAB_BOTTOM, 0.70 * open), w, h);
        rect(out, x0 - 10.0 * s, top, (2.0 * s).max(1.0), panel_h, pal::fade(pal::ACCENT, 0.55 * open), w, h);
    }
    for (index, (text, alpha, enter)) in lines.iter().enumerate() {
        let slide = (1.0 - enter.clamp(0.0, 1.0)).powi(2) * 18.0 * s;
        let baseline = feed_top + index as f32 * line_h + (line_h + txt.cap(px)) * 0.5;
        txt.draw(out, text, x0 - slide, baseline, px, pal::fade(pal::TEXT, *alpha * (1.0 - (1.0 - enter).powi(3))));
    }
    if open <= 0.0 {
        return;
    }

    // The field: mode tag, the text (or a hint), caret, and the pause dots.
    let field_y = bottom - input_h + (1.0 - open) * 10.0 * s;
    let field = [x0 - 4.0 * s, field_y, width + 8.0 * s, input_h];
    rect(out, field[0], field[1], field[2], field[3], pal::fade(pal::STRIP, 0.95 * open), w, h);
    rect(out, field[0], field[1] + field[3] - (2.0 * s).max(1.0), field[2], (2.0 * s).max(1.0), pal::fade(pal::ACCENT, 0.8 * open), w, h);
    let (tag, tone) = match ui.chat_mode {
        ChatMode::Global => ("SAY".to_owned(), pal::ACCENT),
        ChatMode::Team => ("TEAM".to_owned(), pal::TEAM_BLUE),
        ChatMode::Direct(client) => (format!("TELL #{client}"), pal::GOLD),
    };
    let tag_px = 12.5 * s;
    let mut flex = Flex::new();
    let tag_node = flex.leaf(txt.width(&tag, tag_px) + 18.0 * s, 22.0 * s);
    let text_node = flex.spring();
    let row = flex.node(Container::row().width(field[2]).height(field[3]).pad(8.0 * s, 0.0).gap(10.0 * s).align(AlignItems::CENTER), &[tag_node, text_node]);
    flex.compute(row, field[2], field[3]);
    let at = |b: [f32; 4]| [field[0] + b[0], field[1] + b[1], b[2], b[3]];
    let tag_b = at(flex.bounds(tag_node));
    rect(out, tag_b[0], tag_b[1], tag_b[2], tag_b[3], pal::fade(tone, 0.22 * open), w, h);
    txt.draw_centered(out, &tag, tag_b, tag_px, pal::fade(tone, open));
    let text_b = at(flex.bounds(text_node));
    let baseline = text_b[1] + (text_b[3] + txt.cap(px)) * 0.5;
    let reserve = 40.0 * s;
    let shown = tail_to_fit(&txt, &ui.chat_input, px, text_b[2] - reserve);
    if shown.is_empty() {
        // Clear of the caret, which rests at the start of the empty field.
        txt.draw(out, "Type a message", text_b[0] + 8.0 * s, baseline, px, pal::fade(pal::TEXT_FAINT, 0.8 * open));
    } else {
        txt.draw(out, &shown, text_b[0], baseline, px, pal::fade(pal::TEXT, open));
    }
    let end = text_b[0] + if shown.is_empty() { 0.0 } else { txt.width(&shown, px) } + 2.0 * s;
    let dots = if shown.is_empty() { 0 } else { typing_dots(anim.idle_ms) };
    if dots == 0 && (anim.caret || anim.idle_ms < 500) {
        rect(out, end, baseline - txt.cap(px) - 2.0 * s, (2.0 * s).max(1.0), txt.cap(px) + 5.0 * s, pal::fade(pal::ACCENT, open), w, h);
    }
    for i in 0..dots {
        let cx = end + 8.0 * s + i as f32 * 9.0 * s;
        disc(out, cx, baseline - 3.0 * s, 2.6 * s, 10, pal::fade(pal::ACCENT, 0.9 * open), w, h);
    }
}

#[cfg(test)]
mod tests {
    use super::typing_dots;

    #[test]
    fn dots_wait_for_a_pause_then_come_and_go_one_by_one() {
        assert_eq!(typing_dots(0), 0);
        assert_eq!(typing_dots(899), 0);
        let sequence: Vec<usize> = (0..7).map(|i| typing_dots(900 + i * 190)).collect();
        assert_eq!(sequence, [1, 2, 3, 3, 2, 1, 0]);
        assert_eq!(typing_dots(900 + 7 * 190), 1);
    }
}
