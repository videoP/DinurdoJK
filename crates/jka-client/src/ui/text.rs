//! Text.
use crate::ui::{
    glyph_quad, glyph_quad_sized, rect, textured_rect_with_source, Pod, UiWorldPlayerLabel,
    Zeroable, CONSOLE_CHAR_HEIGHT, CONSOLE_CHAR_WIDTH,
};

// The renderer composes several UI batches independently (sometimes on separate
// threads). Scope NWH parsing to each snapshot's build, rather than enabling a
// global flag that could leak into JKA/ordinary JK2 or demo playback.
thread_local! {
    static NWH_RGB_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Renderer-scoped companion font. Only active while constructing one UI batch,
// like NWH color scope; no global font override and no per-frame rasterization.
thread_local! {
    static UNICODE_FALLBACK: std::cell::RefCell<Option<std::sync::Arc<super::UnicodeFallbackFont>>> =
        const { std::cell::RefCell::new(None) };
    // Each renderer owns its queue. Standalone UI tests do not register one.
    static UNICODE_REQUESTS: std::cell::RefCell<Option<std::sync::Arc<std::sync::Mutex<std::collections::HashSet<char>>>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) struct UnicodeFallbackScope {
    previous_font: Option<std::sync::Arc<super::UnicodeFallbackFont>>,
    previous_requests: Option<std::sync::Arc<std::sync::Mutex<std::collections::HashSet<char>>>>,
}

impl UnicodeFallbackScope {
    #[cfg(test)]
    pub(crate) fn enter(font: Option<std::sync::Arc<super::UnicodeFallbackFont>>) -> Self {
        Self::enter_with_requests(font, None)
    }

    pub(crate) fn enter_with_requests(
        font: Option<std::sync::Arc<super::UnicodeFallbackFont>>,
        requests: Option<std::sync::Arc<std::sync::Mutex<std::collections::HashSet<char>>>>,
    ) -> Self {
        Self {
            previous_font: UNICODE_FALLBACK.with(|cell| cell.replace(font)),
            previous_requests: UNICODE_REQUESTS.with(|cell| cell.replace(requests)),
        }
    }
}

impl Drop for UnicodeFallbackScope {
    fn drop(&mut self) {
        UNICODE_FALLBACK.with(|cell| { cell.replace(self.previous_font.take()); });
        UNICODE_REQUESTS.with(|cell| { cell.replace(self.previous_requests.take()); });
    }
}

/// Only missing glyphs enter the slow path. The caller never opens fonts,
/// rasterizes, allocates GPU resources, or waits for the font worker.
fn jura_glyph(ch: char) -> Option<(ProportionalGlyph, i16, f32)> {
    // Emoji presentation selectors and joiners have no independent ink. This
    // per-codepoint fallback does not yet compose multi-codepoint emoji.
    if matches!(ch as u32, 0x200D | 0xFE0E..=0xFE0F | 0xE0020..=0xE007F | 0xE0100..=0xE01EF) {
        return Some((ProportionalGlyph::default(), 64, 42.0));
    }
    let ready = UNICODE_FALLBACK.with(|cell| {
        let borrowed = cell.borrow();
        let font = borrowed.as_ref()?;
        if font.failed.contains(&ch) { return Some(None); }
        Some(Some((*font.glyphs.get(&ch)?, font.height,
            font.cap_heights.get(&ch).copied().unwrap_or(font.cap_height))))
    });
    let ready = match ready {
        Some(None) => return None,
        Some(Some(ready)) => Some(ready),
        None => None,
    };
    if ready.is_none() {
        UNICODE_REQUESTS.with(|cell| {
            if let Some(queue) = cell.borrow().as_ref() {
                if let Ok(mut pending) = queue.lock() {
                    // A hostile message cannot grow the work queue indefinitely.
                    if pending.len() < 256 { pending.insert(ch); }
                }
            }
        });
    }
    ready
}

const JURA_GLYPH_SOURCE: f32 = 11.0;
const JURA_FIXED_GLYPH_SOURCE: f32 = 12.0;

/// Console cell widths are independent of lazy font availability.
pub(crate) fn console_character_cells(ch: char) -> usize {
    match ch as u32 {
        0x200D | 0xFE0E..=0xFE0F | 0xE0020..=0xE007F | 0xE0100..=0xE01EF => 0,
        _ if super::is_emoji_candidate(ch) => 2,
        _ => 1,
    }
}

/// Display cells before a visible scalar offset, skipping active color codes.
pub(crate) fn console_column_offset(value: &str, count: usize) -> usize {
    let mut chars = value.char_indices().peekable();
    let (mut seen, mut cells) = (0, 0);
    while let Some((index, ch)) = chars.next() {
        if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
            continue;
        }
        if seen == count { break; }
        cells += console_character_cells(ch);
        seen += 1;
    }
    cells
}

fn unicode_glyph_source(ch: char, fixed: bool) -> f32 {
    let colored = UNICODE_FALLBACK.with(|cell| cell.borrow().as_ref().is_some_and(|font| font.colored.contains(&ch)));
    if colored { 13.0 } else if fixed { JURA_FIXED_GLYPH_SOURCE } else { JURA_GLYPH_SOURCE }
}

/// For byte-compatible characters, always select the exact existing JKA glyph
/// and atlas. For everything else, use Jura only when it actually contains it.
fn proportional_glyph(font: &ProportionalFont, ch: char) -> (ProportionalGlyph, f32, f32) {
    if let Ok(byte) = u8::try_from(ch as u32) {
        let stock = font.glyphs[byte as usize];
        if stock.horiz_advance > 0 || stock.width > 0 || ch.is_whitespace() || ch.is_control() {
            return (stock, 1.0, 2.0);
        }
        // A blank fontdat entry is genuinely missing, even if the codepoint
        // fits in one byte. Preserve every actually authored JKA glyph.
    }
    if let Some((glyph, jura_height, _)) = jura_glyph(ch) {
        return (glyph, f32::from(font.height) / f32::from(jura_height.max(1)), unicode_glyph_source(ch, false));
    }
    (font.glyphs[b'?' as usize], 1.0, 2.0)
}

/// Fit a Jura glyph into the existing monospaced JKA character cell, leaving
/// cursor advances, console columns and all legacy bytes unchanged.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn charset_glyph(
    out: &mut Vec<UiVertex>, ch: char, x: f32, y: f32,
    glyph_w: f32, glyph_h: f32, color: [f32; 4], w: u32, h: u32,
) {
    if let Ok(byte) = u8::try_from(ch as u32) {
        glyph_quad_sized(out, byte, x, y, glyph_w, glyph_h, color, w, h);
    } else if let Some((glyph, _, cap_height)) = jura_glyph(ch) {
        if glyph.width == 0 || glyph.height == 0 { return; }
        // The stock 8x16 charsgrid has 14px capitals and a baseline at 15px.
        // Exclude the atlas's transparent padding when fitting the ink.
        let pad = super::ttf_font::PAD as f32;
        let ink_w = f32::from(glyph.width) - 2.0 * pad;
        let ink_h = f32::from(glyph.height) - 2.0 * pad;
        if ink_w <= 0.0 || ink_h <= 0.0 { return; }
        let baseline = glyph.baseline as f32 - pad;
        let scale_y = glyph_h * (14.0 / 16.0) / cap_height.max(1.0);
        let scale_x = scale_y
            .min(glyph_w / f32::from(glyph.horiz_advance.max(0)).max(ink_w).max(1.0));
        let draw_x = x + (glyph_w - f32::from(glyph.horiz_advance.max(0)) * scale_x) * 0.5
            + (f32::from(glyph.horiz_offset) + pad) * scale_x;
        let cell_baseline = y + glyph_h * (15.0 / 16.0);
        let draw_y = cell_baseline - baseline * scale_y;
        let du = (glyph.s2 - glyph.s) / f32::from(glyph.width);
        let dv = (glyph.t2 - glyph.t) / f32::from(glyph.height);
        let (u0, v0) = (glyph.s + pad * du, glyph.t + pad * dv);
        let (u1, v1) = (glyph.s2 - pad * du, glyph.t2 - pad * dv);
        let draw_w = ink_w * scale_x;
        let below = (ink_h - baseline).max(0.0);
        let below_space = (y + glyph_h - cell_baseline).max(0.0);
        if below * scale_y > below_space && baseline > 0.0 && baseline < ink_h {
            // Keep the larger body on the stock baseline, fitting the tail
            // into the remaining cell instead of overlapping the next row.
            let split_v = v0 + baseline * dv;
            textured_rect_with_source(out, draw_x, draw_y, draw_w, baseline * scale_y,
                [u0, v0], [u1, split_v], color, unicode_glyph_source(ch, true), w, h);
            textured_rect_with_source(out, draw_x, cell_baseline, draw_w, below_space,
                [u0, split_v], [u1, v1], color, unicode_glyph_source(ch, true), w, h);
        } else {
            textured_rect_with_source(out, draw_x, draw_y, draw_w, ink_h * scale_y,
                [u0, v0], [u1, v1], color, unicode_glyph_source(ch, true), w, h);
        }
    } else {
        glyph_quad_sized(out, b'?', x, y, glyph_w, glyph_h, color, w, h);
    }
}

/// Console emoji use two cells and preserve the bitmap's aspect ratio.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_charset_glyph(
    out: &mut Vec<UiVertex>, ch: char, x: f32, y: f32,
    glyph_w: f32, glyph_h: f32, color: [f32; 4], w: u32, h: u32,
) {
    if unicode_glyph_source(ch, true) == 13.0 {
        if let Some((glyph, _, _)) = jura_glyph(ch) {
            let pad = super::ttf_font::PAD as f32;
            let ink_w = f32::from(glyph.width) - 2.0 * pad;
            let ink_h = f32::from(glyph.height) - 2.0 * pad;
            if ink_w <= 0.0 || ink_h <= 0.0 { return; }
            let scale = (glyph_w / ink_w).min(glyph_h * (14.0 / 16.0) / ink_h);
            let size = [ink_w * scale, ink_h * scale];
            let du = (glyph.s2 - glyph.s) / f32::from(glyph.width);
            let dv = (glyph.t2 - glyph.t) / f32::from(glyph.height);
            textured_rect_with_source(out, x + (glyph_w - size[0]) * 0.5,
                y + (glyph_h - size[1]) * 0.5, size[0], size[1],
                [glyph.s + pad * du, glyph.t + pad * dv],
                [glyph.s2 - pad * du, glyph.t2 - pad * dv], color, 13.0, w, h);
            return;
        }
    }
    charset_glyph(out, ch, x, y, glyph_w, glyph_h, color, w, h);
}

pub(crate) struct NwhColorScope(bool);

impl NwhColorScope {
    pub(crate) fn enter(enabled: bool) -> Self {
        Self(NWH_RGB_ENABLED.with(|cell| cell.replace(enabled)))
    }
}

impl Drop for NwhColorScope {
    fn drop(&mut self) {
        NWH_RGB_ENABLED.with(|cell| cell.set(self.0));
    }
}

/// The number of bytes in the code and the new active color. Normal ^0-^9
/// handling is unchanged; NWH's 3/4/6/8-digit escapes take priority only
/// inside an NWH JK2 UI build.
#[inline]
pub(in crate::ui) fn color_escape(bytes: &[u8], index: usize, alpha: f32) -> Option<(usize, [f32; 4])> {
    if bytes.get(index) != Some(&b'^') { return None; }
    if NWH_RGB_ENABLED.with(std::cell::Cell::get) {
        if let Some(code) = crate::nwh_colors::parse_hex_escape(bytes, index, alpha) {
            return Some(code);
        }
    }
    color_code(*bytes.get(index + 1)? as char, alpha).map(|code| (2, code))
}

/// Same parser over a `char` array (the legacy centerprint word-wrapper).
pub(in crate::ui) fn color_escape_chars(chars: &[char], index: usize, alpha: f32) -> Option<(usize, [f32; 4])> {
    if chars.get(index) != Some(&'^') { return None; }
    if NWH_RGB_ENABLED.with(std::cell::Cell::get) {
        let digits = match chars.get(index + 1)? {
            'x' => 3, 'X' => 6, 'y' => 4, 'Y' => 8, _ => 0,
        };
        if digits > 0 && chars.get(index + 1 + digits).is_some() {
            // ASCII escapes: convert only their at-most-ten characters.
            let mut bytes = [0u8; 10];
            for (dest, ch) in bytes.iter_mut().zip(chars[index..index + 2 + digits].iter()) {
                *dest = u8::try_from(*ch as u32).ok()?;
            }
            return crate::nwh_colors::parse_hex_escape(&bytes[..2 + digits], 0, alpha);
        }
    }
    color_code(*chars.get(index + 1)?, alpha).map(|code| (2, code))
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct UiVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    /// RGBA8 UNORM (`Unorm8x4`), like JKA's byte4 2D vertex colors. Build it
    /// with `UiVertex::pack_color`; this keeps the vertex at 24 bytes.
    pub color: [u8; 4],
    /// 0.0 = solid color, 1.0 = charsgrid, 2.0 = proportional font, 3.0 = splash.
    pub textured: f32,
}

impl UiVertex {
    #[inline]
    pub fn pack_color(color: [f32; 4]) -> [u8; 4] {
        color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    #[inline]
    pub fn unpack_color(color: [u8; 4]) -> [f32; 4] {
        color.map(|c| f32::from(c) / 255.0)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProportionalGlyph {
    pub width: i16,
    pub height: i16,
    pub horiz_advance: i16,
    pub horiz_offset: i16,
    pub baseline: i32,
    pub s: f32,
    pub t: f32,
    pub s2: f32,
    pub t2: f32,
}

#[derive(Debug, Clone)]
pub struct ProportionalFont {
    pub glyphs: [ProportionalGlyph; 256],
    pub point_size: i16,
    pub height: i16,
    /// Glyphs for U+0400..=U+04FF, in order. Only a TrueType font that has
    /// the block fills it; JKA `.fontdat` fonts leave it empty.
    pub cyrillic: Vec<ProportionalGlyph>,
}

/// First character of `ProportionalFont::cyrillic`.
pub const CYRILLIC_FIRST: u32 = 0x400;
pub const CYRILLIC_COUNT: usize = 256;

/// Text in this client is one char per protocol byte; anything above that
/// range is real Unicode (a translation, or filtered text).
fn is_wide(ch: char) -> bool {
    ch as u32 > 0xFF
}

impl ProportionalFont {
    /// Glyph for a character outside the byte range, if this font has it.
    pub fn wide_glyph(&self, ch: char) -> Option<ProportionalGlyph> {
        let index = (ch as u32).checked_sub(CYRILLIC_FIRST)? as usize;
        self.cyrillic.get(index).copied().filter(|glyph| glyph.horiz_advance > 0)
    }

    /// Glyph for any character: its byte glyph, its Cyrillic glyph, or `?`.
    fn glyph_for(&self, ch: char) -> ProportionalGlyph {
        match u8::try_from(ch as u32) {
            Ok(byte) => self.glyphs[usize::from(byte)],
            Err(_) => self.wide_glyph(ch).unwrap_or(self.glyphs[usize::from(b'?')]),
        }
    }
}

pub fn parse_fontdat(bytes: &[u8]) -> Result<ProportionalFont, String> {
    const GLYPH_COUNT: usize = 256;
    const GLYPH_BYTES: usize = 28;
    const HEADER_BYTES: usize = GLYPH_COUNT * GLYPH_BYTES + 10;
    if bytes.len() < HEADER_BYTES {
        return Err(format!(
            "fontdat is {} bytes; expected at least {HEADER_BYTES}",
            bytes.len()
        ));
    }

    fn read_i16(bytes: &[u8], offset: &mut usize) -> i16 {
        let value = i16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
        *offset += 2;
        value
    }
    fn read_i32(bytes: &[u8], offset: &mut usize) -> i32 {
        let value = i32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }
    fn read_f32(bytes: &[u8], offset: &mut usize) -> f32 {
        let value = f32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }

    let mut offset = 0usize;
    let mut glyphs = [ProportionalGlyph::default(); GLYPH_COUNT];
    for glyph in &mut glyphs {
        *glyph = ProportionalGlyph {
            width: read_i16(bytes, &mut offset),
            height: read_i16(bytes, &mut offset),
            horiz_advance: read_i16(bytes, &mut offset),
            horiz_offset: read_i16(bytes, &mut offset),
            baseline: read_i32(bytes, &mut offset),
            s: read_f32(bytes, &mut offset),
            t: read_f32(bytes, &mut offset),
            s2: read_f32(bytes, &mut offset),
            t2: read_f32(bytes, &mut offset),
        };
        if glyph.width < 0
            || glyph.height < 0
            || glyph.horiz_advance < 0
            || ![glyph.s, glyph.t, glyph.s2, glyph.t2]
                .into_iter()
                .all(f32::is_finite)
        {
            return Err("fontdat contains invalid glyph metrics".into());
        }
    }
    let point_size = read_i16(bytes, &mut offset);
    let height = read_i16(bytes, &mut offset);
    let _ascender = read_i16(bytes, &mut offset);
    let _descender = read_i16(bytes, &mut offset);
    let _korean_hack = read_i16(bytes, &mut offset);
    // Retail arialnb leaves height/ascender/descender zero. OpenJK/TaystJK
    // use point size as height for these legacy fontdat headers.
    let height = if height == 0 { point_size } else { height };
    if point_size <= 0 || height <= 0 {
        return Err(format!(
            "fontdat has invalid dimensions point_size={point_size} height={height}"
        ));
    }

    Ok(ProportionalFont {
        glyphs,
        point_size,
        height,
        cyrillic: Vec::new(),
    })
}

pub(in crate::ui) fn truncate_jka_text(value: &str, max_visible: usize) -> String {
    if max_visible == 0 { return String::new(); }
    let mut result = String::new();
    let mut visible = 0;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if visible >= max_visible { break; }
        if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
            result.push_str(&value[index..index + skip]);
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
        } else {
            result.push(ch);
            visible += 1;
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn text(
    out: &mut Vec<UiVertex>, value: &str, x: f32, y: f32,
    scale: f32, color: [f32; 4], width: u32, height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' {
            cursor_x = x; cursor_y += 9.0 * scale;
            continue;
        }
        if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
            active_color = code;
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
            continue;
        }
        if ch != ' ' {
            if let Ok(byte) = u8::try_from(ch as u32) {
                glyph_quad(out, byte, cursor_x, cursor_y, scale, active_color, width, height);
            } else {
                charset_glyph(out, ch, cursor_x, cursor_y, 6.0 * scale, 8.0 * scale,
                    active_color, width, height);
            }
        }
        cursor_x += 6.0 * scale;
    }
}

pub(in crate::ui) fn proportional_text_width(value: &str, font: &ProportionalFont, scale: f32) -> f32 {
    let mut width = 0.0;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
            continue;
        }
        if ch == '\n' { break; }
        let (glyph, ratio, _) = proportional_glyph(font, ch);
        width += f32::from(glyph.horiz_advance.max(0)) * scale * ratio;
    }
    width
}

pub(in crate::ui) fn wrap_proportional_text(
    value: &str, font: &ProportionalFont, scale: f32, max_width: f32,
) -> String {
    if proportional_text_width(value, font, scale) <= max_width { return value.to_owned(); }
    let mut out = String::with_capacity(value.len() + value.len() / 32);
    let mut line_width = 0.0;
    let mut line_start = 0usize;
    let mut last_space_out = None::<usize>;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
            out.push_str(&value[index..index + skip]);
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
            continue;
        }
        if ch == '\n' {
            out.push('\n'); line_width = 0.0; line_start = out.len(); last_space_out = None;
            continue;
        }
        if ch == ' ' { last_space_out = Some(out.len()); }
        out.push(ch);
        let (glyph, ratio, _) = proportional_glyph(font, ch);
        line_width += f32::from(glyph.horiz_advance.max(0)) * scale * ratio;
        if line_width >= max_width {
            if let Some(space) = last_space_out.filter(|space| *space >= line_start) {
                out.replace_range(space..=space, "\n");
                line_start = space + 1;
                line_width = proportional_text_width(&out[line_start..], font, scale);
            } else {
                out.push('\n'); line_width = 0.0; line_start = out.len();
            }
            last_space_out = None;
        }
    }
    out
}

/// Legacy 640x480 anisotropic text. Keep this entry point for call sites which
/// still explicitly expect that projection (e.g. an offscreen virtual canvas).
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn proportional_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    font: &ProportionalFont,
    x: f32,
    baseline_y: f32,
    scale: f32,
    color: [f32; 4],
    shadow: bool,
    width: u32,
    height: u32,
) {
    proportional_text_with_x_scale(out, value, font, x, baseline_y, scale, color, shadow, width, height,
        width.max(1) as f32 / 640.0, false);
}

/// HUD text: pixels per virtual unit are the SAME on X and Y. Positions remain
/// physical screen pixels, so projected labels and right-edge anchors do not move.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn proportional_text_uniform(
    out: &mut Vec<UiVertex>, value: &str, font: &ProportionalFont,
    x: f32, baseline_y: f32, scale: f32, color: [f32; 4], shadow: bool,
    width: u32, height: u32,
) {
    proportional_text_with_x_scale(out, value, font, x, baseline_y, scale, color, shadow, width, height,
        height.max(1) as f32 / 480.0, false);
}

/// CENTERPRINT is drawn with TaystJK's `ITEM_TEXTSTYLE_SHADOWEDMORE`, which
/// becomes `STYLE_DROPSHADOW` in `CG_Text_Paint` / `RE_Font_DrawString`.
/// Keep this separate from the regular HUD/chat font shadow.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn proportional_centerprint_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    font: &ProportionalFont,
    x: f32,
    baseline_y: f32,
    scale: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    proportional_text_with_x_scale(
        out, value, font, x, baseline_y, scale, color, true, width, height,
        height.max(1) as f32 / 480.0, true,
    );
}

#[allow(clippy::too_many_arguments)]
fn proportional_text_with_x_scale(
    out: &mut Vec<UiVertex>, value: &str, font: &ProportionalFont,
    x: f32, baseline_y: f32, scale: f32, color: [f32; 4], shadow: bool,
    width: u32, height: u32, glyph_x_scale: f32, tayst_centerprint_shadow: bool,
) {
    let x_scale = glyph_x_scale;
    let y_scale = height.max(1) as f32 / 480.0;
    let line_step = 20.0 * scale * y_scale;
    let centerprint_shadow_offset = (f32::from(font.point_size) * scale * 0.075).round();

    let draw_pass = |out: &mut Vec<UiVertex>, shadow_pass: bool| {
        let mut cursor_x = x;
        let mut cursor_y = baseline_y;
        let mut active_color = color;
        let mut chars = value.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if ch == '\n' { cursor_x = x; cursor_y += line_step; continue; }
            if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
                active_color = code;
                while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
                continue;
            }
            let (glyph, factor, texture_source) = proportional_glyph(font, ch);
            if glyph.width > 0 && glyph.height > 0 && ch != ' ' {
                let (shadow_offset_x, shadow_offset_y) = if !shadow_pass {
                    (0.0, 0.0)
                } else if tayst_centerprint_shadow {
                    (centerprint_shadow_offset * x_scale, centerprint_shadow_offset * y_scale)
                } else { (1.0, 1.0) };
                let draw_color = if !shadow_pass { active_color }
                    else if tayst_centerprint_shadow { [0.15, 0.15, 0.15, color[3]] }
                    else { [0.0, 0.0, 0.0, color[3] * 0.72] };
                textured_rect_with_source(
                    out,
                    cursor_x + f32::from(glyph.horiz_offset) * scale * factor * x_scale + shadow_offset_x,
                    cursor_y - glyph.baseline as f32 * scale * factor * y_scale + shadow_offset_y,
                    f32::from(glyph.width) * scale * factor * x_scale,
                    f32::from(glyph.height) * scale * factor * y_scale,
                    [glyph.s, glyph.t], [glyph.s2, glyph.t2], draw_color,
                    texture_source, width, height,
                );
            }
            cursor_x += f32::from(glyph.horiz_advance.max(0)) * scale * factor * x_scale;
        }
    };

    if shadow {
        draw_pass(out, true);
    }
    draw_pass(out, false);
}

/// Append one projected `cg_drawPlayerNames` label. `x`/`y` are physical
/// framebuffer coordinates produced from the final render camera.
pub fn build_world_player_label_vertices(
    out: &mut Vec<UiVertex>,
    label: &UiWorldPlayerLabel,
    x: f32,
    y: f32,
    scale: f32,
    font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
    nwh_hex_colors: bool,
) {
    let _nwh_scope = NwhColorScope::enter(nwh_hex_colors);
    let sy = height.max(1) as f32 / 480.0;
    let scale = scale.clamp(0.05, 4.0);

    if let Some(font) = font {
        // Most player labels are one line, but race-ghost comparison labels can
        // add a second stats line. Center each line independently instead of
        // letting the generic text drawer inherit the first line's x origin.
        let line_step = 20.0 * scale * sy;
        if label.crowned {
            // The crown gets its own box above the first line (and above the
            // health bar when one is drawn), a third of the name's width.
            let first = label.text.split('\n').next().unwrap_or_default();
            let name_w = proportional_text_width(first, font, scale) * sy;
            let cap = f32::from(font.glyphs[usize::from(b'A')].height.max(1)) * scale * sy;
            let top = if label.health_fraction.is_some() { (y - 8.0 * sy).min(y - cap) } else { y - cap };
            let crown = super::glyphs::crown_over([x - name_w * 0.5, top, name_w, cap], height as f32 / 1080.0);
            super::glyphs::crown(out, crown, 0.9, width, height);
        }
        for (line, text) in label.text.split('\n').enumerate() {
            let text_width = proportional_text_width(text, font, scale) * sy;
            proportional_text_uniform(
                out,
                text,
                font,
                x - text_width * 0.5,
                y + line as f32 * line_step,
                scale,
                [1.0, 1.0, 1.0, 1.0],
                true,
                width,
                height,
            );
        }
    } else {
        // Startup/font-load fallback. The ordinary renderer normally has the
        // proportional JKA font by the time a game snapshot can exist.
        let fallback_scale = (1.25 * scale).max(0.25);
        for (line, value) in label.text.split('\n').enumerate() {
            let visible = crate::ui::visible_jka_chars(value) as f32;
            text(
                out,
                value,
                x - visible * 3.0 * fallback_scale,
                y - 4.0 * fallback_scale + line as f32 * 12.0 * fallback_scale,
                fallback_scale,
                [1.0, 1.0, 1.0, 1.0],
                width,
                height,
            );
        }
    }

    let Some(fraction) = label.health_fraction else {
        return;
    };
    // TaystJK HEALTH_WIDTH/HEIGHT are 50x5 virtual pixels. Keep the same
    // authored size and alpha, but render it at native framebuffer resolution.
    let bar_w = 50.0 * sy;
    let bar_h = 5.0 * sy;
    let border = 1.0_f32.max(sy);
    let bar_x = x - bar_w * 0.5;
    let bar_y = y - 7.0 * sy;
    rect(
        out,
        bar_x - border,
        bar_y - border,
        bar_w + border * 2.0,
        bar_h + border * 2.0,
        [0.0, 0.0, 0.0, 0.75],
        width,
        height,
    );
    rect(
        out,
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        [0.5, 0.5, 0.5, 0.4],
        width,
        height,
    );
    rect(
        out,
        bar_x,
        bar_y,
        bar_w * fraction.clamp(0.0, 1.0),
        bar_h,
        label.health_color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn fixed_charset_text(
    out: &mut Vec<UiVertex>, value: &str, x: f32, y: f32,
    glyph_w: f32, glyph_h: f32, advance: f32, color: [f32; 4],
    shadow: bool, width: u32, height: u32,
) {
    for shadow_pass in [true, false] {
        if shadow_pass && !shadow { continue; }
        let mut cursor_x = x + if shadow_pass { 2.0 } else { 0.0 };
        let mut cursor_y = y + if shadow_pass { 2.0 } else { 0.0 };
        let mut active_color = color;
        let mut chars = value.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if ch == '\n' {
                cursor_x = x + if shadow_pass { 2.0 } else { 0.0 };
                cursor_y += glyph_h;
                continue;
            }
            if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
                if !shadow_pass { active_color = code; }
                while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
                continue;
            }
            if ch != ' ' {
                charset_glyph(out, ch, cursor_x, cursor_y, glyph_w, glyph_h,
                    if shadow_pass { [0.0, 0.0, 0.0, color[3]] } else { active_color },
                    width, height);
            }
            cursor_x += advance;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    console_text_sized(out, value, x, y, CONSOLE_CHAR_WIDTH, CONSOLE_CHAR_HEIGHT, color, width, height);
}

/// Console-only sized path. The ^N color parser is shared with the legacy-size path.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_text_sized(
    out: &mut Vec<UiVertex>, value: &str, x: f32, y: f32,
    char_width: f32, char_height: f32, color: [f32; 4], width: u32, height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' { cursor_x = x; cursor_y += char_height; continue; }
        if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
            active_color = code;
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
            continue;
        }
        let cells = console_character_cells(ch) as f32;
        if ch != ' ' && cells > 0.0 {
            console_charset_glyph(out, ch, cursor_x, cursor_y, char_width * cells, char_height,
                active_color, width, height);
        }
        cursor_x += char_width * cells;
    }
}

pub(in crate::ui) fn color_code(code: char, alpha: f32) -> Option<[f32; 4]> {
    let rgb = match code {
        // JKA/Quake color escapes are saturated primaries/secondaries.
        // Do not pastelize these: ^2 in particular is the stock chat green.
        '0' => [0.0, 0.0, 0.0],
        '1' => [1.0, 0.0, 0.0],
        '2' => [0.0, 1.0, 0.0],
        '3' => [1.0, 1.0, 0.0],
        '4' => [0.0, 0.0, 1.0],
        '5' => [0.0, 1.0, 1.0],
        '6' => [1.0, 0.0, 1.0],
        '7' => [1.0, 1.0, 1.0],
        // TaystJK shared/qcommon/q_color.c: g_color_table[8] and [9].
        '8' => [1.0, 0.5, 0.0],
        '9' => [0.5, 0.5, 0.5],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], alpha])
}

/// Generate a tiny recovery charset using the same atlas layout as JKA's
/// gfx/2d/charsgrid_med: 16x16 logical slots on a 256x256 texture, with each
/// glyph living in the left 8x16 pixels of its 16x16 slot. Keeping the fallback
/// in the same layout means the normal OpenJK-compatible UV path stays valid.
pub fn fallback_font_rgba() -> (u32, u32, Vec<u8>) {
    const SLOT: usize = 16;
    const GRID: usize = 16;
    let width = SLOT * GRID;
    let height = SLOT * GRID;
    let mut rgba = vec![0u8; width * height * 4];
    for code in 0u16..=255 {
        let rows = glyph(code as u8 as char);
        let cell_x = (code as usize & 15) * SLOT;
        let cell_y = (code as usize >> 4) * SLOT;
        for (gy, bits) in rows.into_iter().enumerate() {
            for gx in 0..5 {
                if bits & (1 << (4 - gx)) == 0 {
                    continue;
                }
                let px = cell_x + gx + 1;
                let py = cell_y + gy + 4;
                let offset = (py * width + px) * 4;
                rgba[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    (width as u32, height as u32, rgba)
}

pub(in crate::ui) fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ':' => [0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0],
        '>' => [
            0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000,
        ],
        '<' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001,
        ],
        '=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10110, 0b00110, 0],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        ' ' => [0; 7],
        _ => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0, 0b00100],
    }
}

/// A Render Studio player name at its projected spot, faded by `alpha`
/// (cinematic names fade near the lens and far away).
pub fn build_studio_label_vertices(
    out: &mut Vec<UiVertex>,
    label: &crate::ui::UiStudioLabel,
    font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    let (x, y) = (label.pos[0] * width.max(1) as f32, label.pos[1] * height.max(1) as f32);
    let alpha = label.alpha.clamp(0.0, 1.0);
    if alpha <= 0.01 { return; }
    let scale = label.scale.clamp(0.3, 2.0);
    if let Some(font) = font {
        let sy = height.max(1) as f32 / 480.0;
        let text_width = proportional_text_width(&label.text, font, scale) * sy;
        proportional_text_uniform(out, &label.text, font, x - text_width * 0.5, y, scale, [label.color[0],label.color[1],label.color[2],alpha], true, width, height);
    } else {
        let fallback = (1.25 * scale).max(0.25);
        let visible = crate::ui::visible_jka_chars(&label.text) as f32;
        text(out, &label.text, x - visible * 3.0 * fallback, y - 4.0 * fallback, fallback, [label.color[0],label.color[1],label.color[2],alpha], width, height);
    }
}


/// Texture source of the clean TrueType UI font (`ui.wgsl` binding 8).
pub(in crate::ui) const CLEAN_FONT_SOURCE: f32 = 8.0;

/// Text for the redesigned surfaces (scoreboard, kill feed, player popup).
/// Sizes are line heights in physical pixels. With the clean font loaded the
/// glyphs scale uniformly, so text keeps its proportions on widescreen;
/// without it, the JKA proportional font and then the charsgrid stand in.
#[derive(Clone, Copy)]
pub(in crate::ui) struct UiText<'a> {
    jura: bool,
    clean: Option<&'a ProportionalFont>,
    legacy: Option<&'a ProportionalFont>,
    /// TrueType font that lends its Cyrillic glyphs to JKA-font text.
    wide: Option<&'a ProportionalFont>,
    named_emojis: Option<&'a super::ChatEmojiAtlas>,
    w: u32,
    h: u32,
}

/// Split into runs of byte-range text and runs of wide (Unicode) text.
fn wide_runs(value: &str) -> Vec<(bool, &str)> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut current = None;
    for (index, ch) in value.char_indices() {
        let wide = is_wide(ch);
        if current.is_some_and(|was| was != wide) {
            runs.push((!wide, &value[start..index]));
            start = index;
        }
        current = Some(wide);
    }
    if let Some(wide) = current {
        runs.push((wide, &value[start..]));
    }
    runs
}

/// The colour in force after `run`, given the one before it.
fn colour_after(run: &str, mut active: [f32; 4], alpha: f32) -> [f32; 4] {
    let mut chars = run.char_indices().peekable();
    while let Some((index, _)) = chars.next() {
        if let Some((skip, code)) = color_escape(run.as_bytes(), index, alpha) {
            active = code;
            while chars.peek().is_some_and(|(at, _)| *at < index + skip) {
                chars.next();
            }
        }
    }
    active
}

impl<'a> UiText<'a> {
    pub(in crate::ui) fn new(ui: &'a crate::ui::UiSnapshot, legacy: Option<&'a ProportionalFont>, w: u32, h: u32) -> Self {
        Self { jura: false, clean: ui.clean_font.as_deref(), legacy, wide: None, named_emojis: None, w, h }
    }

    /// Render redesigned UI geometry with the original JKA small font.
    pub(in crate::ui) fn legacy_only(legacy: Option<&'a ProportionalFont>, w: u32, h: u32) -> Self {
        Self { jura: false, clean: None, legacy, wide: None, named_emojis: None, w, h }
    }

    /// `legacy_only`, but characters the JKA font cannot have (Cyrillic in
    /// translated or filtered chat) are drawn from the TrueType UI font.
    pub(in crate::ui) fn legacy_with_wide(
        ui: &'a crate::ui::UiSnapshot,
        legacy: Option<&'a ProportionalFont>,
        w: u32,
        h: u32,
    ) -> Self {
        // Jura takes priority for missing glyphs. Without the bundled Jura
        // file, retain the preexisting clean-TTF Cyrillic fallback behavior.
        let jura_loaded = UNICODE_FALLBACK.with(|cell| cell.borrow().is_some());
        let wide = if jura_loaded { None } else {
            ui.clean_font.as_deref().filter(|font| !font.cyrillic.is_empty())
        };
        Self { jura: false, clean: None, legacy, wide: wide.filter(|_| legacy.is_some()), named_emojis: None, w, h }
    }

    pub(in crate::ui) fn chat(ui: &'a crate::ui::UiSnapshot, legacy: Option<&'a ProportionalFont>, w: u32, h: u32) -> Self {
        let mut text = match ui.chat_settings.font() {
            super::ChatFont::Classic => Self::legacy_with_wide(ui, legacy, w, h),
            super::ChatFont::Modern => Self::new(ui, legacy, w, h),
            super::ChatFont::Jura => {
                let mut text = Self::legacy_with_wide(ui, legacy, w, h);
                // Keep legacy chat visible while Jura is being loaded, or
                // indefinitely when no suitable fallback font is installed.
                let ready_h = jura_glyph('H').is_some();
                let ready_question = jura_glyph('?').is_some();
                text.jura = ready_h && ready_question;
                text
            }
        };
        if ui.chat_settings.emojis {
            text.named_emojis = ui.chat_emoji_atlas.as_deref().filter(|atlas| atlas.count > 0);
        }
        text
    }

    /// The user edits and transmits the literal shortcode. Only received
    /// message presentation substitutes images, never the entry field.
    pub(in crate::ui) fn without_chat_emojis(mut self) -> Self {
        self.named_emojis = None;
        self
    }

    fn jura_width(value: &str, px: f32) -> f32 {
        let mut width = 0.0;
        let mut chars = value.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
                while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
                continue;
            }
            if let Some((glyph, height, _)) = jura_glyph(ch).or_else(|| jura_glyph('?')) {
                width += f32::from(glyph.horiz_advance.max(0)) * px / f32::from(height.max(1));
            }
        }
        width
    }

    fn jura_draw(&self, out: &mut Vec<UiVertex>, value: &str, x: f32, baseline: f32, px: f32, color: [f32; 4]) {
        for shadow in [true, false] {
            let mut cursor = x;
            let mut active = color;
            let mut chars = value.char_indices().peekable();
            while let Some((index, ch)) = chars.next() {
                if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
                    active = code;
                    while chars.peek().is_some_and(|(at, _)| *at < index + skip) { chars.next(); }
                    continue;
                }
                let Some((glyph, height, _)) = jura_glyph(ch).or_else(|| jura_glyph('?')) else { continue };
                let scale = px / f32::from(height.max(1));
                let offset = if shadow { 1.0 } else { 0.0 };
                if glyph.width > 0 && glyph.height > 0 && ch != ' ' {
                    textured_rect_with_source(out,
                        cursor + f32::from(glyph.horiz_offset) * scale + offset,
                        baseline - glyph.baseline as f32 * scale + offset,
                        f32::from(glyph.width) * scale, f32::from(glyph.height) * scale,
                        [glyph.s, glyph.t], [glyph.s2, glyph.t2],
                        if shadow { [0.0, 0.0, 0.0, color[3] * 0.55] } else { active },
                        if shadow { JURA_GLYPH_SOURCE } else { unicode_glyph_source(ch, false) }, self.w, self.h);
                }
                cursor += f32::from(glyph.horiz_advance.max(0)) * scale;
            }
        }
    }

    fn clean_width(font: &ProportionalFont, value: &str, px: f32) -> f32 {
        let scale = px / f32::from(font.height.max(1));
        let mut width = 0.0;
        let mut chars = value.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if let Some((skip, _)) = color_escape(value.as_bytes(), index, 1.0) {
                while chars.peek().is_some_and(|(at, _)| *at < index + skip) {
                    chars.next();
                }
                continue;
            }
            if (ch as u32) > 255 && font.wide_glyph(ch).is_none() {
                if let Some((glyph, height, _)) = jura_glyph(ch) {
                    width += f32::from(glyph.horiz_advance.max(0)) * px / f32::from(height.max(1));
                    continue;
                }
            }
            width += f32::from(font.glyph_for(ch).horiz_advance.max(0)) * scale;
        }
        width
    }

    /// Draw with a TrueType font. `color` supplies the alpha and the base
    /// colour; `start` is the colour in force at the first character.
    #[allow(clippy::too_many_arguments)]
    fn clean_draw(&self, out: &mut Vec<UiVertex>, font: &ProportionalFont, value: &str, x: f32, baseline: f32, px: f32, color: [f32; 4], start: [f32; 4]) {
        let (w, h) = (self.w, self.h);
        let scale = px / f32::from(font.height.max(1));
        // Soft one-pixel drop shadow, then the glyphs.
        let shadow = (px / 22.0).clamp(1.0, 2.0);
        for pass in [true, false] {
            let mut cursor = x;
            let mut active = start;
            let mut chars = value.char_indices().peekable();
            while let Some((index, ch)) = chars.next() {
                if let Some((skip, code)) = color_escape(value.as_bytes(), index, color[3]) {
                    active = code;
                    while chars.peek().is_some_and(|(at, _)| *at < index + skip) {
                        chars.next();
                    }
                    continue;
                }
                let fallback = ((ch as u32) > 255 && font.wide_glyph(ch).is_none())
                    .then(|| jura_glyph(ch)).flatten();
                let (glyph, glyph_scale, source) = if let Some((glyph, height, _)) = fallback {
                    (glyph, px / f32::from(height.max(1)), unicode_glyph_source(ch, false))
                } else {
                    (font.glyph_for(ch), scale, CLEAN_FONT_SOURCE)
                };
                if glyph.width > 0 && glyph.height > 0 && ch != ' ' {
                    let offset = if pass { shadow } else { 0.0 };
                    let tint = if pass { [0.0, 0.0, 0.0, color[3] * 0.55] } else { active };
                    textured_rect_with_source(
                        out,
                        cursor + f32::from(glyph.horiz_offset) * glyph_scale + offset,
                        baseline - glyph.baseline as f32 * glyph_scale + offset,
                        f32::from(glyph.width) * glyph_scale,
                        f32::from(glyph.height) * glyph_scale,
                        [glyph.s, glyph.t],
                        [glyph.s2, glyph.t2],
                        tint,
                        if pass && source == 13.0 { JURA_GLYPH_SOURCE } else { source },
                        w,
                        h,
                    );
                }
                cursor += f32::from(glyph.horiz_advance.max(0)) * glyph_scale;
            }
        }
    }

    /// The wide font, when `value` needs it.
    fn wide_for(&self, value: &str) -> Option<&'a ProportionalFont> {
        self.wide.filter(|_| value.chars().any(is_wide))
    }

    fn legacy_scale(&self, font: &ProportionalFont, px: f32) -> f32 {
        px / (f32::from(font.height.max(1)) * self.h.max(1) as f32 / 480.0)
    }

    fn charsgrid_scale(px: f32) -> f32 {
        px / 12.0
    }

    /// Advance width of `value`, color codes excluded. For chat, emotes are
    /// proper inline objects, not glyph-space substitutions.
    pub(in crate::ui) fn width(&self, value: &str, px: f32) -> f32 {
        if let Some(atlas) = self.named_emojis {
            let mut width = 0.0;
            atlas.for_each_part(value, |part| {
                width += match part {
                    super::chat_emoji::ChatPart::Text(text) => self.width_plain(text, px),
                    super::chat_emoji::ChatPart::Image(_) => px * 0.85,
                };
            });
            return width;
        }
        self.width_plain(value, px)
    }

    fn width_plain(&self, value: &str, px: f32) -> f32 {
        if self.jura { return Self::jura_width(value, px); }
        if let Some(font) = self.clean {
            Self::clean_width(font, value, px)
        } else if let (Some(font), Some(wide)) = (self.legacy, self.wide_for(value)) {
            let scale = self.legacy_scale(font, px);
            wide_runs(value)
                .into_iter()
                .map(|(is_wide, run)| {
                    if is_wide {
                        Self::clean_width(wide, run, px)
                    } else {
                        proportional_text_width(run, font, scale) * self.h.max(1) as f32 / 480.0
                    }
                })
                .sum()
        } else if let Some(font) = self.legacy {
            proportional_text_width(value, font, self.legacy_scale(font, px)) * self.h.max(1) as f32 / 480.0
        } else {
            crate::ui::visible_jka_chars(value) as f32 * 6.0 * Self::charsgrid_scale(px)
        }
    }

    /// Height of a capital letter, for centring a line on a box.
    pub(in crate::ui) fn cap(&self, px: f32) -> f32 {
        if self.jura {
            return jura_glyph('H').map_or(px * 0.65, |(_, height, cap)| cap * px / f32::from(height.max(1)));
        }
        if let Some(font) = self.clean {
            let glyph = font.glyphs[usize::from(b'H')];
            (glyph.baseline as f32 - 2.0).max(1.0) * px / f32::from(font.height.max(1))
        } else if let Some(font) = self.legacy {
            f32::from(font.glyphs[usize::from(b'A')].height.max(1)) * self.legacy_scale(font, px) * self.h.max(1) as f32 / 480.0
        } else {
            8.0 * Self::charsgrid_scale(px)
        }
    }

    /// Baseline that vertically centres capitals in `bounds`.
    pub(in crate::ui) fn baseline_in(&self, bounds: [f32; 4], px: f32) -> f32 {
        bounds[1] + (bounds[3] + self.cap(px)) * 0.5
    }

    pub(in crate::ui) fn draw(&self, out: &mut Vec<UiVertex>, value: &str, x: f32, baseline: f32, px: f32, color: [f32; 4]) {
        if let Some(atlas) = self.named_emojis {
            let mut cursor = x;
            let mut active = color;
            let size = px * 0.85;
            atlas.for_each_part(value, |part| {
                match part {
                    super::chat_emoji::ChatPart::Text(text) => {
                        self.draw_plain(out, text, cursor, baseline, px, active);
                        cursor += self.width_plain(text, px);
                        active = colour_after(text, active, color[3]);
                    }
                    super::chat_emoji::ChatPart::Image(cell) => {
                        // Authored RGB never inherits JKA ^color codes; fading
                        // still follows the enclosing chat message opacity.
                        textured_rect_with_source(out, cursor, baseline - size * 0.85,
                            size, size, cell.uv.0, cell.uv.1,
                            [1.0, 1.0, 1.0, color[3]],
                            super::chat_emoji::CHAT_EMOJI_SOURCE, self.w, self.h);
                        cursor += size;
                    }
                }
            });
            return;
        }
        self.draw_plain(out, value, x, baseline, px, color);
    }

    fn draw_plain(&self, out: &mut Vec<UiVertex>, value: &str, x: f32, baseline: f32, px: f32, color: [f32; 4]) {
        if self.jura { self.jura_draw(out, value, x, baseline, px, color); return; }
        let (w, h) = (self.w, self.h);
        if let Some(font) = self.clean {
            self.clean_draw(out, font, value, x, baseline, px, color, color);
        } else if let (Some(font), Some(wide)) = (self.legacy, self.wide_for(value)) {
            // JKA font for what it has, the TrueType font for the rest.
            let scale = self.legacy_scale(font, px);
            let mut cursor = x;
            let mut active = color;
            for (is_wide, run) in wide_runs(value) {
                if is_wide {
                    self.clean_draw(out, wide, run, cursor, baseline, px, color, active);
                    cursor += Self::clean_width(wide, run, px);
                } else {
                    proportional_text_uniform(out, run, font, cursor, baseline, scale, active, true, w, h);
                    cursor += proportional_text_width(run, font, scale) * h.max(1) as f32 / 480.0;
                    active = colour_after(run, active, color[3]);
                }
            }
        } else if let Some(font) = self.legacy {
            proportional_text_uniform(out, value, font, x, baseline, self.legacy_scale(font, px), color, true, w, h);
        } else {
            let scale = Self::charsgrid_scale(px);
            text(out, value, x, baseline - 8.0 * scale, scale, color, w, h);
        }
    }

    pub(in crate::ui) fn draw_right(&self, out: &mut Vec<UiVertex>, value: &str, right: f32, baseline: f32, px: f32, color: [f32; 4]) {
        self.draw(out, value, right - self.width(value, px), baseline, px, color);
    }

    pub(in crate::ui) fn draw_centered(&self, out: &mut Vec<UiVertex>, value: &str, bounds: [f32; 4], px: f32, color: [f32; 4]) {
        let x = bounds[0] + (bounds[2] - self.width(value, px)) * 0.5;
        self.draw(out, value, x, self.baseline_in(bounds, px), px, color);
    }
}

#[cfg(test)]
mod widescreen_font_tests {
    use super::*;

    #[test]
    fn legacy_arialnb_fontdat_uses_point_size_for_zero_height() {
        let mut bytes = vec![0u8; 256 * 28 + 10];
        bytes[256 * 28..256 * 28 + 2].copy_from_slice(&14i16.to_le_bytes());
        let font = parse_fontdat(&bytes).unwrap();
        assert_eq!(font.height, 14);
        assert_eq!(font.point_size, 14);
        bytes[256 * 28 + 2..256 * 28 + 4].copy_from_slice(&(-1i16).to_le_bytes());
        assert!(parse_fontdat(&bytes).is_err());
    }

    #[test]
    fn jura_console_glyph_height_does_not_shrink_with_wide_letters() {
        let mut glyphs = std::collections::HashMap::new();
        for (ch, width) in [('Ж', 40), ('І', 8)] {
            glyphs.insert(ch, ProportionalGlyph {
                width, height: 20, horiz_advance: width, baseline: 18,
                s2: 1.0, t2: 1.0, ..Default::default()
            });
        }
        let _scope = UnicodeFallbackScope::enter(Some(std::sync::Arc::new(
            super::super::UnicodeFallbackFont { glyphs, colored: std::collections::HashSet::new(), cap_heights: std::collections::HashMap::new(), failed: std::collections::HashSet::new(), height: 32, cap_height: 16.0 },
        )));
        let mut out = Vec::new();
        fixed_charset_text(&mut out, "ЖІ", 10.0, 20.0, 8.0, 16.0,
            8.0, [1.0; 4], false, 640, 480);
        assert_eq!(out.len(), 12);
        let bounds = |vertices: &[UiVertex]| {
            let min_x = vertices.iter().map(|v| v.position[0]).fold(f32::INFINITY, f32::min);
            let max_x = vertices.iter().map(|v| v.position[0]).fold(f32::NEG_INFINITY, f32::max);
            let min_y = vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            let max_y = vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
            ((max_x - min_x) * 320.0, (max_y - min_y) * 240.0)
        };
        let wide = bounds(&out[..6]);
        let narrow = bounds(&out[6..]);
        assert!(wide.0 <= 8.001 && narrow.0 <= 8.001);
        assert!((wide.1 - 14.0).abs() < 0.001);
        assert!((wide.1 - narrow.1).abs() < 0.001);
    }

    #[test]
    fn jura_fallback_preserves_every_stock_glyph_vertex() {
        let legacy = test_font();
        let value: String = (0..=255u8).map(char::from).collect();
        let _empty = UnicodeFallbackScope::enter(None);
        let mut before = Vec::new();
        proportional_text_uniform(&mut before, &value, &legacy, 10.0, 24.0,
            1.0, [1.0; 4], true, 1920, 1080);
        let glyphs = value.chars().map(|ch| (ch, ProportionalGlyph::default())).collect();
        let _loaded = UnicodeFallbackScope::enter(Some(std::sync::Arc::new(
            super::super::UnicodeFallbackFont { glyphs, colored: std::collections::HashSet::new(), cap_heights: std::collections::HashMap::new(), failed: std::collections::HashSet::new(), height: 64, cap_height: 40.0 },
        )));
        let mut after = Vec::new();
        proportional_text_uniform(&mut after, &value, &legacy, 10.0, 24.0,
            1.0, [1.0; 4], true, 1920, 1080);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&before), bytemuck::cast_slice::<_, u8>(&after));
    }

    #[test]
    fn jura_fallback_measurement_wrapping_color_and_scope_agree() {
        let legacy = test_font();
        let mut glyphs = std::collections::HashMap::new();
        glyphs.insert('Ж', ProportionalGlyph {
            width: 16, height: 22, horiz_advance: 36, baseline: 20,
            s2: 1.0, t2: 1.0, ..Default::default()
        });
        let _empty = UnicodeFallbackScope::enter(None);
        {
            let _loaded = UnicodeFallbackScope::enter(Some(std::sync::Arc::new(
                super::super::UnicodeFallbackFont { glyphs, colored: std::collections::HashSet::new(), cap_heights: std::collections::HashMap::new(), failed: std::collections::HashSet::new(), height: 24, cap_height: 18.0 },
            )));
            assert_eq!(proportional_text_width("^1AЖ", &legacy, 1.0), 27.0);
            assert_eq!(wrap_proportional_text("A ЖA", &legacy, 1.0, 27.0), "A\nЖA\n");
            assert_eq!(truncate_jka_text("^1AЖ你", 2), "^1AЖ");
            assert_eq!(crate::ui::chat::wrap_fixed_chat_text("^1ЖЖЖ", 2), "^1ЖЖ\nЖ");
            let mut out = Vec::new();
            fixed_charset_text(&mut out, "^1AЖ", 0.0, 0.0, 8.0, 16.0,
                8.0, [1.0; 4], true, 640, 480);
            assert_eq!(out.len(), 24);
            assert!(out[18..].iter().all(|v| v.textured == JURA_FIXED_GLYPH_SOURCE
                && v.color == [255, 0, 0, 255]));
        }
        assert_eq!(proportional_text_width("Ж", &legacy, 1.0), 9.0);
        let mut missing = Vec::new();
        let mut question = Vec::new();
        text(&mut missing, "你", 0.0, 0.0, 1.0, [1.0; 4], 640, 480);
        text(&mut question, "?", 0.0, 0.0, 1.0, [1.0; 4], 640, 480);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&missing), bytemuck::cast_slice::<_, u8>(&question));
    }

    #[test]
    fn lazy_glyph_requests_skip_stock_text_and_cache_failed_unicode() {
        let queue = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let failed_font = std::sync::Arc::new(super::super::UnicodeFallbackFont {
            glyphs: std::collections::HashMap::new(),
            colored: std::collections::HashSet::new(), cap_heights: std::collections::HashMap::new(),
            failed: std::collections::HashSet::from(['\u{1F642}']),
            height: 64, cap_height: 42.0,
        });
        let _scope = UnicodeFallbackScope::enter_with_requests(Some(failed_font), Some(queue.clone()));
        let stock = test_font();
        let _ = proportional_glyph(&stock, 'A');
        assert!(queue.lock().unwrap().is_empty(), "stock ASCII never queues a font");
        let _ = proportional_glyph(&stock, '你');
        let _ = proportional_glyph(&stock, '你');
        assert_eq!(queue.lock().unwrap().len(), 1, "duplicate requests are deduplicated");
        let _ = proportional_glyph(&stock, '\u{1F642}');
        assert_eq!(queue.lock().unwrap().len(), 1, "known missing font never retries");
    }

    #[test]
    fn jura_fallback_never_replaces_a_stock_byte_glyph() {
        let legacy = test_font();
        let mut glyphs = std::collections::HashMap::new();
        glyphs.insert('Ж', ProportionalGlyph {
            width: 8, height: 12, horiz_advance: 9, baseline: 10,
            s2: 1.0, t2: 1.0, ..Default::default()
        });
        let _scope = UnicodeFallbackScope::enter(Some(std::sync::Arc::new(
            super::super::UnicodeFallbackFont { glyphs, colored: std::collections::HashSet::new(), cap_heights: std::collections::HashMap::new(), failed: std::collections::HashSet::new(), height: 12, cap_height: 8.0 },
        )));
        let mut out = Vec::new();
        proportional_text_uniform(&mut out, "AЖA", &legacy, 10.0, 24.0,
            1.0, [1.0; 4], false, 640, 480);
        assert_eq!(out.len(), 18);
        assert!(out[..6].iter().all(|v| v.textured == 2.0));
        assert!(out[6..12].iter().all(|v| v.textured == JURA_GLYPH_SOURCE));
        assert!(out[12..].iter().all(|v| v.textured == 2.0));
        assert_eq!(proportional_text_width("AЖA", &legacy, 1.0), 27.0);
    }

    #[test]
    fn missing_wide_character_keeps_legacy_question_mark_fallback() {
        let legacy = test_font();
        let _scope = UnicodeFallbackScope::enter(None);
        let mut out = Vec::new();
        proportional_text_uniform(&mut out, "A你", &legacy, 10.0, 24.0,
            1.0, [1.0; 4], false, 640, 480);
        assert_eq!(out.len(), 12);
        assert!(out.iter().all(|v| v.textured == 2.0));
    }

    #[test]
    fn wide_text_is_split_from_byte_text_and_keeps_its_colour() {
        assert_eq!(wide_runs("Bob^7: ^2ты *** да"), [(false, "Bob^7: ^2"), (true, "ты"), (false, " *** "), (true, "да")]);
        assert_eq!(wide_runs("plain"), [(false, "plain")]);
        assert!(wide_runs("").is_empty());
        let white = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(colour_after("Bob^7: ^2", white, 1.0), color_code('2', 1.0).unwrap());
        assert_eq!(colour_after("no codes ^", white, 1.0), white);

        let mut font = test_font();
        assert!(font.wide_glyph('Ж').is_none());
        font.cyrillic = vec![font.glyphs[0]; CYRILLIC_COUNT];
        assert_eq!(font.wide_glyph('Ж').unwrap().horiz_advance, 9);
        // Outside the block: drawn as `?`, never as a wrong byte glyph.
        assert!(font.wide_glyph('你').is_none());
        assert_eq!(UiText::clean_width(&font, "^1Жук 你", 12.0), 5.0 * 9.0);
    }

    fn test_font() -> ProportionalFont {
        let mut glyphs = [ProportionalGlyph::default(); 256];
        for glyph in &mut glyphs {
            glyph.width = 8;
            glyph.height = 12;
            glyph.horiz_advance = 9;
            glyph.baseline = 10;
            glyph.s2 = 1.0;
            glyph.t2 = 1.0;
        }
        ProportionalFont { glyphs, point_size: 12, height: 12, cyrillic: Vec::new() }
    }

    fn x_bounds(out: &[UiVertex], w: u32) -> (f32, f32) {
        let x = |v: &UiVertex| (v.position[0] + 1.0) * w as f32 * 0.5;
        (out.iter().map(x).fold(f32::INFINITY, f32::min),
         out.iter().map(x).fold(f32::NEG_INFINITY, f32::max))
    }

    #[test]
    fn jka_hud_font_width_tracks_height_not_aspect_ratio() {
        let font = test_font();
        // The same glyph at the same height is equally wide on 4:3, 16:9
        // and ultrawide screens; only its NDC coordinates differ.
        for (w, h) in [(1280, 960), (1920, 1080), (2560, 1080), (1920, 700)] {
            let mut out = Vec::new();
            proportional_text_uniform(&mut out, "A", &font, 100.0, 80.0, 0.75,
                [1.0; 4], false, w, h);
            let (x0, x1) = x_bounds(&out, w);
            let expected = 8.0 * 0.75 * h as f32 / 480.0;
            assert!(((x1 - x0) - expected).abs() < 0.002, "{w}x{h}");
        }
    }

    #[test]
    fn nwh_hex_names_are_zero_width_codes_only_in_nwh_scope() {
        let font = test_font();
        let name = "^XFF8000Bo^x0F0b^7!";
        // The same bytes are ordinary text outside a verified NWH build.
        assert_eq!(proportional_text_width(name, &font, 1.0), 17.0 * 9.0);
        {
            let _scope = NwhColorScope::enter(true);
            assert_eq!(proportional_text_width(name, &font, 1.0), 4.0 * 9.0);
            assert_eq!(UiText::clean_width(&font, name, 12.0), 4.0 * 9.0);
            assert_eq!(crate::ui::visible_jka_chars(name), 4);
            assert_eq!(crate::ui::console::visible_text_len(name), 4);
            assert_eq!(truncate_jka_text(name, 3), "^XFF8000Bo^x0F0b");
            let mut vertices = Vec::new();
            proportional_text_uniform(&mut vertices, name, &font, 10.0, 30.0, 1.0,
                [1.0, 1.0, 1.0, 0.6], true, 640, 480);
            assert_eq!(vertices.len(), 4 * 6 * 2);
            assert_eq!(vertices[24].color, UiVertex::pack_color([1.0, 128.0 / 255.0, 0.0, 0.6]));
            assert_eq!(vertices[36].color, UiVertex::pack_color([0.0, 1.0, 0.0, 0.6]));
            assert_eq!(vertices[42].color, UiVertex::pack_color([1.0, 1.0, 1.0, 0.6]));
        }
        assert_eq!(proportional_text_width(name, &font, 1.0), 17.0 * 9.0);
    }

    #[test]
    fn projected_player_name_stays_centered_in_widescreen() {
        let font = test_font();
        let label = UiWorldPlayerLabel {
            anchor: [0.0; 3], text: "AA".into(), health_fraction: None,
            health_color: [1.0; 4], crowned: false,
        };
        for (w, h) in [(640, 480), (1920, 1080), (2560, 1080)] {
            let mut out = Vec::new();
            build_world_player_label_vertices(&mut out, &label, w as f32 / 2.0, 120.0,
                0.5, Some(&font), w, h, false);
            // The glyph width is slightly smaller than advance. Shadow adds
            // one pixel to the right; the computed center is advance-based.
            let (x0, x1) = x_bounds(&out, w);
            let center = (x0 + x1) / 2.0;
            assert!((center - w as f32 / 2.0).abs() < 4.0, "{w}x{h}");
        }
    }
}

#[cfg(test)]
mod centerprint_parity_tests {
    use super::*;

    #[test]
    fn color_escapes_match_taystjk_palette() {
        assert_eq!(color_code('8', 0.4), Some([1.0, 0.5, 0.0, 0.4]));
        assert_eq!(color_code('9', 0.4), Some([0.5, 0.5, 0.5, 0.4]));
        assert_eq!(color_code('7', 0.4), Some([1.0, 1.0, 1.0, 0.4]));
    }

    #[test]
    fn centerprint_shadow_uses_font_point_size_and_fade_alpha() {
        let mut font = ProportionalFont {
            glyphs: [ProportionalGlyph::default(); 256],
            point_size: 24,
            height: 24,
            cyrillic: Vec::new(),
        };
        font.glyphs[b'A' as usize] = ProportionalGlyph {
            width: 8,
            height: 12,
            horiz_advance: 9,
            baseline: 12,
            ..Default::default()
        };
        let mut out = Vec::new();
        proportional_centerprint_text(
            &mut out, "^8A^9A", &font, 100.0, 150.0, 1.0,
            [1.0, 1.0, 1.0, 0.4], 640, 480,
        );
        assert_eq!(out.len(), 24); // two glyphs, two passes, six vertices each
        assert_eq!(out[0].color, UiVertex::pack_color([0.15, 0.15, 0.15, 0.4]));
        assert_eq!(out[12].color, UiVertex::pack_color([1.0, 0.5, 0.0, 0.4]));
        assert_eq!(out[18].color, UiVertex::pack_color([0.5, 0.5, 0.5, 0.4]));
        // Round(24 * 1.0 * .075) = 2 virtual pixels.
        assert!((out[0].position[0] - out[12].position[0] - (2.0 / 640.0 * 2.0)).abs() < 0.0001);
        assert!((out[12].position[1] - out[0].position[1] - (2.0 / 480.0 * 2.0)).abs() < 0.0001);
    }
}
