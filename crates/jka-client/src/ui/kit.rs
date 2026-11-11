//! Native (Taffy + WGPU) widget kit for full-screen tool pages.
//!
//! Pages compute their rectangles with `ui::flex`; the app uses the same
//! rectangles for winit hit testing and this kit draws them as `UiVertex`
//! quads. The look is the native console's rain-slick night: indigo asphalt
//! gradient body, cyan neon edges and accents, magenta title ticks, tungsten
//! amber for carets and warnings. Colours come from `palette::hud`, so the
//! kit follows `ui_palette` exactly like the console. No egui is involved.
use super::{
    disc, flex::Bounds, rect, rect_gradient, rect_outline, text::UiText, textured_rect_with_source,
    UiSnapshot, UiVertex,
};

pub(crate) type Rgba = [f32; 4];

pub(crate) fn alpha(color: Rgba, a: f32) -> Rgba {
    [color[0], color[1], color[2], a]
}

pub(crate) fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// The console shades (`palette::hud::CONSOLE_*`) for the active scheme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub body_top: Rgba,
    pub body_bottom: Rgba,
    pub card: Rgba,
    pub chip: Rgba,
    pub field: Rgba,
    pub bar: Rgba,
    pub cyan: Rgba,
    pub magenta: Rgba,
    pub amber: Rgba,
    pub danger: Rgba,
    pub text: Rgba,
    pub bright: Rgba,
    pub dim: Rgba,
    pub faint: Rgba,
}

impl Palette {
    pub fn console() -> Self {
        use crate::palette::hud as h;
        Self {
            body_top: h::CONSOLE_BODY_TOP(),
            body_bottom: h::CONSOLE_BODY_BOTTOM(),
            card: h::CONSOLE_CARD(),
            chip: h::CONSOLE_CHIP(),
            field: h::CONSOLE_FIELD(),
            bar: h::CONSOLE_BAR(),
            cyan: h::CONSOLE_CYAN(),
            magenta: h::CONSOLE_MAGENTA(),
            amber: h::CONSOLE_AMBER(),
            danger: h::TEAM_RED(),
            text: h::CONSOLE_TEXT(),
            bright: h::CONSOLE_BRIGHT(),
            dim: h::CONSOLE_DIM(),
            faint: h::CONSOLE_FAINT(),
        }
    }

    pub fn tone(&self, tone: Tone) -> Rgba {
        match tone {
            Tone::Faint => self.faint,
            Tone::Dim => self.dim,
            Tone::Text => self.text,
            Tone::Accent => self.cyan,
            Tone::Magenta => self.magenta,
            Tone::Warning => self.amber,
            Tone::Danger => self.danger,
        }
    }
}

/// Semantic text colour, resolved against the console [`Palette`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Faint,
    Dim,
    Text,
    Accent,
    Magenta,
    Warning,
    Danger,
}

/// A single-line text field as drawn: `caret` is a byte offset and is only
/// present while the field has keyboard focus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldView {
    pub text: String,
    pub caret: Option<usize>,
    pub all_selected: bool,
}

/// Editing state for the one focused native field of a page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldEdit {
    pub caret: usize,
    pub all_selected: bool,
}

/// What a key did to a focused field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditResult {
    /// Not an editing key; the page may use it.
    Ignored,
    /// Caret or selection moved only.
    Moved,
    Changed,
}

impl FieldEdit {
    pub fn at_end(text: &str) -> Self {
        Self { caret: text.len(), all_selected: false }
    }

    pub fn view(self, text: &str) -> FieldView {
        FieldView { text: text.to_owned(), caret: Some(self.caret.min(text.len())), all_selected: self.all_selected }
    }

    pub fn insert(&mut self, text: &mut String, value: &str) {
        let value: String = value.chars().filter(|c| !c.is_control()).collect();
        if value.is_empty() {
            return;
        }
        if self.all_selected {
            text.clear();
            self.caret = 0;
            self.all_selected = false;
        }
        let at = self.caret.min(text.len());
        text.insert_str(at, &value);
        self.caret = at + value.len();
    }

    /// Standard single-line editing keys, including Ctrl+A/C/X/V.
    pub fn key(&mut self, text: &mut String, key: winit::keyboard::KeyCode, control: bool) -> EditResult {
        use winit::keyboard::KeyCode;
        let at = self.caret.min(text.len());
        let prev = text[..at].char_indices().last().map_or(0, |(i, _)| i);
        let next = at + text[at..].chars().next().map_or(0, char::len_utf8);
        match key {
            KeyCode::KeyA if control => {
                self.all_selected = !text.is_empty();
                self.caret = text.len();
                EditResult::Moved
            }
            KeyCode::KeyC if control => {
                let _ = crate::clipboard::set_text(text);
                EditResult::Moved
            }
            KeyCode::KeyX if control => {
                let _ = crate::clipboard::set_text(text);
                text.clear();
                *self = Self::default();
                EditResult::Changed
            }
            KeyCode::KeyV if control => {
                if let Ok(value) = crate::clipboard::get_text() {
                    self.insert(text, value.lines().next().unwrap_or(""));
                }
                EditResult::Changed
            }
            KeyCode::Backspace | KeyCode::Delete if self.all_selected => {
                text.clear();
                *self = Self::default();
                EditResult::Changed
            }
            KeyCode::Backspace if control => {
                // Delete back to the previous path or word separator.
                const SEPARATORS: &[char] = &['/', ' ', '_'];
                let cut = text[..at].trim_end_matches(SEPARATORS)
                    .rfind(SEPARATORS).map_or(0, |i| i + 1);
                text.replace_range(cut..at, "");
                self.caret = cut;
                EditResult::Changed
            }
            KeyCode::Backspace if at > 0 => {
                text.replace_range(prev..at, "");
                self.caret = prev;
                EditResult::Changed
            }
            KeyCode::Delete if at < text.len() => {
                text.replace_range(at..next, "");
                EditResult::Changed
            }
            KeyCode::Backspace | KeyCode::Delete => EditResult::Moved,
            KeyCode::ArrowLeft => {
                self.caret = if self.all_selected { 0 } else { prev };
                self.all_selected = false;
                EditResult::Moved
            }
            KeyCode::ArrowRight => {
                self.caret = if self.all_selected { text.len() } else { next };
                self.all_selected = false;
                EditResult::Moved
            }
            KeyCode::Home => {
                self.caret = 0;
                self.all_selected = false;
                EditResult::Moved
            }
            KeyCode::End => {
                self.caret = text.len();
                self.all_selected = false;
                EditResult::Moved
            }
            _ => EditResult::Ignored,
        }
    }
}

/// Straight-alpha image region in a renderer-owned atlas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Thumb {
    #[default]
    Pending,
    Missing,
    Ready { uv0: [f32; 2], uv1: [f32; 2], aspect: f32, source: f32 },
}

pub(crate) fn contains(b: Bounds, p: [f32; 2]) -> bool {
    super::flex::contains(b, p)
}

pub(crate) fn inset(b: Bounds, x: f32, y: f32) -> Bounds {
    [b[0] + x, b[1] + y, (b[2] - 2.0 * x).max(0.0), (b[3] - 2.0 * y).max(0.0)]
}

/// Paints kit widgets for one page build. `scale` multiplies every nominal
/// size (points at a 1125-line reference height).
pub(in crate::ui) struct Painter<'a> {
    pub out: &'a mut Vec<UiVertex>,
    pub font: UiText<'a>,
    pub c: Palette,
    pub scale: f32,
    pub pointer: [f32; 2],
    w: u32,
    h: u32,
}

/// Nominal text sizes, shared so every page reads as one family.
pub(crate) mod size {
    pub const TITLE: f32 = 17.0;
    pub const PANE: f32 = 13.0;
    pub const BODY: f32 = 12.5;
    pub const SMALL: f32 = 11.0;
    pub const MICRO: f32 = 10.0;
}

impl<'a> Painter<'a> {
    pub fn new(out: &'a mut Vec<UiVertex>, ui: &'a UiSnapshot, scale: f32, pointer: [f32; 2], w: u32, h: u32)
        -> Self {
        Self { out, font: UiText::new(ui, None, w, h), c: Palette::console(), scale, pointer, w, h }
    }

    pub fn hovered(&self, b: Bounds) -> bool {
        contains(b, self.pointer)
    }

    /// Font line height for a nominal point size.
    pub fn px(&self, size: f32) -> f32 {
        size * self.scale * 1.2
    }

    pub fn fill(&mut self, b: Bounds, color: Rgba) {
        if b[2] > 0.0 && b[3] > 0.0 {
            rect(self.out, b[0], b[1], b[2], b[3], color, self.w, self.h);
        }
    }

    pub fn vgradient(&mut self, b: Bounds, top: Rgba, bottom: Rgba) {
        rect_gradient(self.out, b[0], b[1], b[2], b[3], top, top, bottom, bottom, self.w, self.h);
    }

    pub fn hgradient(&mut self, b: Bounds, left: Rgba, right: Rgba) {
        rect_gradient(self.out, b[0], b[1], b[2], b[3], left, right, left, right, self.w, self.h);
    }

    pub fn outline(&mut self, b: Bounds, thickness: f32, color: Rgba) {
        if b[2] > 0.0 && b[3] > 0.0 {
            rect_outline(self.out, b[0], b[1], b[2], b[3], thickness, color, self.w, self.h);
        }
    }

    pub fn dot(&mut self, center: [f32; 2], radius: f32, color: Rgba) {
        disc(self.out, center[0], center[1], radius, 12, color, self.w, self.h);
    }

    /// Solid ring with two soft outer bands, like the console's neon edge.
    pub fn glow(&mut self, b: Bounds, color: Rgba) {
        let s = self.scale;
        for (grow, a) in [(4.0 * s, 0.07), (2.0 * s, 0.18)] {
            self.outline(inset(b, -grow, -grow), grow - 1.0 * s, alpha(color, a));
        }
        self.outline(b, (2.0 * s).max(2.0), color);
    }

    pub fn width(&self, text: &str, size: f32) -> f32 {
        self.font.width(text, self.px(size))
    }

    /// Longest prefix of `text` that fits `max` (with "..." when cut).
    pub fn fit(&self, text: &str, size: f32, max: f32) -> String {
        let px = self.px(size);
        if self.font.width(text, px) <= max {
            return text.to_owned();
        }
        let budget = max - self.font.width("...", px);
        let mut used = 0.0;
        let mut end = 0;
        let mut buf = [0u8; 4];
        for (i, ch) in text.char_indices() {
            used += self.font.width(ch.encode_utf8(&mut buf), px);
            if used > budget {
                break;
            }
            end = i + ch.len_utf8();
        }
        format!("{}...", &text[..end])
    }

    /// One line, left-aligned and vertically centred in `b`, cut to fit.
    pub fn text(&mut self, b: Bounds, text: &str, size: f32, color: Rgba) {
        if b[2] <= 2.0 || text.is_empty() {
            return;
        }
        let shown = self.fit(text, size, b[2]);
        let px = self.px(size);
        let baseline = self.font.baseline_in(b, px);
        self.font.draw(self.out, &shown, b[0], baseline, px, color);
    }

    pub fn text_right(&mut self, b: Bounds, text: &str, size: f32, color: Rgba) {
        if b[2] <= 2.0 || text.is_empty() {
            return;
        }
        let shown = self.fit(text, size, b[2]);
        let px = self.px(size);
        let baseline = self.font.baseline_in(b, px);
        self.font.draw_right(self.out, &shown, b[0] + b[2], baseline, px, color);
    }

    pub fn text_center(&mut self, b: Bounds, text: &str, size: f32, color: Rgba) {
        if b[2] <= 2.0 || text.is_empty() {
            return;
        }
        let shown = self.fit(text, size, b[2]);
        self.font.draw_centered(self.out, &shown, b, self.px(size), color);
    }

    /// Word-wrapped paragraph from the top of `b`; lines that do not fit are dropped.
    pub fn paragraph(&mut self, b: Bounds, text: &str, size: f32, color: Rgba) {
        let line_h = self.px(size) * 1.3;
        let mut y = b[1];
        let mut line = String::new();
        for word in text.split_whitespace() {
            let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if self.width(&candidate, size) > b[2] && !line.is_empty() {
                if y + line_h > b[1] + b[3] + 0.5 {
                    return;
                }
                self.text([b[0], y, b[2], line_h], &line, size, color);
                y += line_h;
                line = word.to_owned();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() && y + line_h <= b[1] + b[3] + 0.5 {
            self.text([b[0], y, b[2], line_h], &line, size, color);
        }
    }

    /// Floating page: indigo gradient body, faint cyan rim, and the console's
    /// cyan-to-magenta neon edge glowing along the bottom.
    pub fn panel(&mut self, b: Bounds) {
        let c = self.c;
        let s = self.scale;
        self.vgradient(b, c.body_top, c.body_bottom);
        self.outline(b, 1.0, alpha(c.cyan, 0.16));
        let y = b[1] + b[3];
        self.hgradient([b[0], y - 2.0 * s, b[2], 2.0 * s], alpha(c.cyan, 0.95), alpha(c.magenta, 0.95));
        for (dy, h, a) in [(0.0, 3.0, 0.17), (3.0, 4.0, 0.07), (7.0, 6.0, 0.025)] {
            self.hgradient([b[0], y + dy * s, b[2], h * s], alpha(c.cyan, a), alpha(c.magenta, a));
        }
    }

    /// Header band across the top of a panel, with the console hairline.
    pub fn header_band(&mut self, b: Bounds) {
        let c = self.c;
        self.fill(b, [0.0, 0.0, 0.02, 0.30]);
        self.fill([b[0], b[1] + b[3] - 1.0, b[2], 1.0], alpha(c.cyan, 0.14));
    }

    /// Page title: magenta tick and bright caption, like "CONSOLE".
    pub fn title(&mut self, b: Bounds, text: &str) {
        let c = self.c;
        let s = self.scale;
        self.fill([b[0], b[1] + b[3] * 0.5 - 11.0 * s, 3.0 * s, 22.0 * s], c.magenta);
        self.text([b[0] + 12.0 * s, b[1], b[2] - 12.0 * s, b[3]], text, size::TITLE, c.bright);
    }

    /// Sub-pane: a slightly lifted well with a faint cyan rim.
    pub fn pane(&mut self, b: Bounds) {
        let c = self.c;
        self.fill(b, alpha(c.bar, 0.42));
        self.outline(b, 1.0, alpha(c.cyan, 0.12));
    }

    /// Magenta tick + caption, right-aligned detail, cyan hairline underneath.
    pub fn pane_title(&mut self, b: Bounds, title: &str, detail: &str) {
        let c = self.c;
        let s = self.scale;
        self.fill([b[0], b[1] + b[3] * 0.5 - 8.0 * s, 3.0 * s, 16.0 * s], c.magenta);
        let title_w = self.width(title, size::PANE);
        self.text([b[0] + 11.0 * s, b[1], b[2] - 11.0 * s, b[3]], title, size::PANE, c.bright);
        if !detail.is_empty() {
            let x = b[0] + 26.0 * s + title_w;
            self.text_right([x, b[1], (b[0] + b[2] - x).max(0.0), b[3]], detail, size::SMALL, c.faint);
        }
        self.fill([b[0], b[1] + b[3] - 1.0, b[2], 1.0], alpha(c.cyan, 0.14));
    }

    /// One segment of a segmented control (console chip).
    pub fn chip(&mut self, b: Bounds, label: &str, selected: bool) {
        let c = self.c;
        let hovered = self.hovered(b);
        self.fill(b, if selected { mix(c.chip, c.cyan, 0.16) } else { c.chip });
        let edge = if selected { alpha(c.cyan, 0.60) } else if hovered { alpha(c.cyan, 0.35) } else { alpha(c.faint, 0.45) };
        self.outline(b, 1.0, edge);
        let color = if selected { c.bright } else if hovered { c.text } else { c.dim };
        self.text_center(b, label, size::SMALL, color);
    }

    /// Primary (cyan neon) or secondary button. `hover` tints a secondary
    /// button's edge on hover: cyan for neutral actions, red for destructive.
    pub fn button(&mut self, b: Bounds, label: &str, primary: bool, enabled: bool, hover: Rgba) {
        let c = self.c;
        let hovered = enabled && self.hovered(b);
        if !enabled {
            self.fill(b, c.chip);
            self.outline(b, 1.0, alpha(c.faint, 0.30));
            self.text_center(b, label, size::BODY, alpha(c.faint, 0.75));
            return;
        }
        if primary {
            self.fill(b, alpha(c.cyan, if hovered { 0.30 } else { 0.18 }));
            self.outline(b, 1.0, alpha(c.cyan, if hovered { 1.0 } else { 0.80 }));
            self.text_center(b, label, size::BODY, c.bright);
        } else {
            self.fill(b, if hovered { mix(c.chip, hover, 0.10) } else { c.chip });
            self.outline(b, 1.0, if hovered { alpha(hover, 0.90) } else { alpha(c.faint, 0.55) });
            self.text_center(b, label, size::BODY, if hovered { c.bright } else { c.text });
        }
    }

    /// Clear ("x") hit box at the right edge of a non-empty field.
    pub fn field_clear_rect(b: Bounds, scale: f32) -> Bounds {
        let w = 28.0 * scale;
        [b[0] + b[2] - w, b[1], w, b[3]]
    }

    /// Console field: dark well, amber caret, cyan underline while focused.
    pub fn field(&mut self, b: Bounds, field: &FieldView, hint: &str, invalid: bool) {
        let c = self.c;
        let s = self.scale;
        let focused = field.caret.is_some();
        self.fill(b, c.field);
        let rim = if focused { alpha(c.cyan, 0.35) } else if self.hovered(b) { alpha(c.cyan, 0.25) } else { alpha(c.faint, 0.40) };
        self.outline(b, 1.0, rim);
        let clear = !field.text.is_empty();
        let text_box = [b[0] + 10.0 * s, b[1], b[2] - 20.0 * s - if clear { 24.0 * s } else { 0.0 }, b[3]];
        if field.text.is_empty() {
            self.text(text_box, hint, size::BODY, c.faint);
            if focused {
                self.fill([text_box[0], b[1] + 6.0 * s, 2.0 * s, b[3] - 12.0 * s], c.amber);
            }
        } else {
            // Keep the caret visible: drop leading characters until the text
            // up to the caret fits, then cut the tail.
            let caret = field.caret.unwrap_or(field.text.len()).min(field.text.len());
            let mut start = 0;
            while start < caret && self.width(&field.text[start..caret], size::BODY) > text_box[2] {
                start += field.text[start..].chars().next().map_or(1, char::len_utf8);
            }
            let shown = &field.text[start..];
            if field.all_selected {
                let w = self.width(shown, size::BODY).min(text_box[2]);
                self.fill([text_box[0] - 2.0 * s, b[1] + 5.0 * s, w + 4.0 * s, b[3] - 10.0 * s], alpha(c.cyan, 0.22));
            }
            self.text(text_box, shown, size::BODY, c.bright);
            if focused && !field.all_selected {
                let x = text_box[0] + self.width(&field.text[start..caret], size::BODY);
                self.fill([x, b[1] + 6.0 * s, 2.0 * s, b[3] - 12.0 * s], c.amber);
            }
            let x_rect = Self::field_clear_rect(b, s);
            let color = if self.hovered(x_rect) { c.bright } else { c.faint };
            self.text_center(x_rect, "\u{d7}", 15.0, color);
        }
        if invalid {
            self.fill([b[0], b[1] + b[3] - 2.0 * s, b[2], 2.0 * s], c.danger);
        } else if focused {
            self.fill([b[0], b[1] + b[3] - 2.0 * s, b[2], 2.0 * s], c.cyan);
        }
    }

    /// Small badge anchored at a corner (`right` anchors its right edge).
    pub fn tag(&mut self, anchor: [f32; 2], right: bool, label: &str, color: Rgba) {
        let s = self.scale;
        let w = self.width(label, size::MICRO) + 10.0 * s;
        let h = 17.0 * s;
        let x = if right { anchor[0] - w } else { anchor[0] };
        let c = self.c;
        self.fill([x, anchor[1], w, h], alpha(c.card, 0.92));
        self.fill([x, anchor[1] + h - 1.0, w, 1.0], alpha(color, 0.55));
        self.text_center([x, anchor[1], w, h], label, size::MICRO, color);
    }

    /// Thumbnail fitted (aspect kept) inside `b` over the card colour.
    pub fn thumb(&mut self, b: Bounds, thumb: Thumb, missing: &str) {
        let c = self.c;
        self.fill(b, c.card);
        match thumb {
            Thumb::Ready { uv0, uv1, aspect, source } if aspect > 0.0 => {
                let (w, h) = if aspect >= b[2] / b[3].max(1.0) { (b[2], b[2] / aspect) } else { (b[3] * aspect, b[3]) };
                let x = b[0] + (b[2] - w) * 0.5;
                let y = b[1] + (b[3] - h) * 0.5;
                textured_rect_with_source(self.out, x, y, w, h, uv0, uv1, [1.0; 4], source, self.w, self.h);
            }
            Thumb::Pending => {
                let s = self.scale;
                let mid = [b[0] + b[2] * 0.5, b[1] + b[3] * 0.5];
                for i in -1..=1 {
                    self.dot([mid[0] + i as f32 * 9.0 * s, mid[1]], 2.0 * s, alpha(c.cyan, 0.35));
                }
            }
            _ => {
                if !missing.is_empty() {
                    self.text_center(b, missing, size::MICRO, c.faint);
                }
            }
        }
    }

    /// Thin scroll indicator inside the right gutter of `b`.
    pub fn scrollbar(&mut self, b: Bounds, first: usize, total: usize, visible: usize) {
        if total <= visible || total == 0 {
            return;
        }
        let c = self.c;
        let s = self.scale;
        let track = [b[0] + b[2] - 5.0 * s, b[1], 3.0 * s, b[3]];
        self.fill(track, alpha(c.faint, 0.20));
        let h = (b[3] * visible as f32 / total as f32).max(20.0 * s).min(b[3]);
        let max_first = total - visible;
        let y = b[1] + (b[3] - h) * (first.min(max_first) as f32 / max_first.max(1) as f32);
        let hot = self.hovered([track[0] - 6.0 * s, b[1], track[2] + 10.0 * s, b[3]]);
        self.fill([track[0], y, track[2], h], alpha(c.cyan, if hot { 0.95 } else { 0.60 }));
    }

    /// Hover card near the pointer, kept on screen.
    pub fn tooltip(&mut self, lines: &[(String, Tone)]) {
        if lines.is_empty() {
            return;
        }
        let c = self.c;
        let s = self.scale;
        let line_h = self.px(size::BODY) * 1.3;
        let width = lines.iter().map(|(t, _)| self.width(t, size::BODY)).fold(0.0, f32::max) + 26.0 * s;
        let height = line_h * lines.len() as f32 + 14.0 * s;
        let mut x = self.pointer[0] + 18.0 * s;
        let mut y = self.pointer[1] + 20.0 * s;
        if x + width > self.w as f32 - 4.0 { x = (self.pointer[0] - width - 10.0 * s).max(4.0); }
        if y + height > self.h as f32 - 4.0 { y = (self.pointer[1] - height - 10.0 * s).max(4.0); }
        self.fill([x, y, width, height], c.card);
        self.outline([x, y, width, height], 1.0, alpha(c.cyan, 0.45));
        self.fill([x, y, 3.0 * s, height], c.magenta);
        for (i, (text, tone)) in lines.iter().enumerate() {
            let color = c.tone(*tone);
            self.text([x + 14.0 * s, y + 7.0 * s + i as f32 * line_h, width - 22.0 * s, line_h], text, size::BODY, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn field_edit_inserts_deletes_and_selects() {
        let mut text = String::from("textures/hoth");
        let mut edit = FieldEdit::at_end(&text);
        edit.insert(&mut text, "/ice");
        assert_eq!(text, "textures/hoth/ice");
        assert_eq!(edit.key(&mut text, KeyCode::Backspace, true), EditResult::Changed);
        assert_eq!(text, "textures/hoth/");
        edit.key(&mut text, KeyCode::Home, false);
        edit.key(&mut text, KeyCode::Delete, false);
        assert_eq!(text, "extures/hoth/");
        edit.key(&mut text, KeyCode::KeyA, true);
        edit.insert(&mut text, "gfx");
        assert_eq!((text.as_str(), edit.caret), ("gfx", 3));
        assert_eq!(edit.key(&mut text, KeyCode::F5, false), EditResult::Ignored);
    }
}
