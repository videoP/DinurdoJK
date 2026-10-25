//! Bitmap-font quads for the lightbox (Aurebesh) and the CRT's source image
//! (Aurebesh + JKA's ergoec), from the stock fontdat metrics.
use crate::ui::ProportionalFont;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct TextVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

pub(super) const SOLID: f32 = 0.0;
pub(super) const AUREBESH: f32 = 1.0;
pub(super) const LATIN: f32 = 2.0;
pub(super) const PANEL: f32 = 3.0;

pub(super) fn quad(out: &mut Vec<TextVertex>, rect: [f32; 4], uv: [f32; 4], color: [f32; 4], params: [f32; 4]) {
    let [x0, y0, x1, y1] = rect;
    let [u0, v0, u1, v1] = uv;
    let v = |x: f32, y: f32, u: f32, w: f32| TextVertex { position: [x, y], uv: [u, w], color, params };
    out.extend_from_slice(&[
        v(x0, y0, u0, v0), v(x1, y0, u1, v0), v(x0, y1, u0, v1),
        v(x0, y1, u0, v1), v(x1, y0, u1, v0), v(x1, y1, u1, v1),
    ]);
}

pub(super) fn rect(out: &mut Vec<TextVertex>, rect: [f32; 4], color: [f32; 4]) {
    quad(out, rect, [0.0; 4], color, [SOLID, 0.0, 0.0, 0.0]);
}

/// Advance width of `text` at `scale` (pixels per font pixel).
pub(super) fn width(font: &ProportionalFont, text: &str, scale: f32) -> f32 {
    text.bytes().map(|b| font.glyphs[b as usize].horiz_advance.max(0) as f32 * scale).sum()
}

/// Draw `text` on a baseline. Returns the x after the last glyph.
pub(super) fn line(
    out: &mut Vec<TextVertex>,
    font: &ProportionalFont,
    mode: f32,
    text: &str,
    x: f32,
    baseline: f32,
    scale: f32,
    color: [f32; 4],
) -> f32 {
    let mut cursor = x;
    for byte in text.bytes() {
        let glyph = font.glyphs[byte as usize];
        if glyph.width > 0 && glyph.height > 0 && byte != b' ' {
            let gx = cursor + glyph.horiz_offset as f32 * scale;
            let gy = baseline - glyph.baseline as f32 * scale;
            quad(out, [gx, gy, gx + glyph.width as f32 * scale, gy + glyph.height as f32 * scale],
                [glyph.s, glyph.t, glyph.s2, glyph.t2], color, [mode, 0.0, 0.0, 0.0]);
        }
        cursor += glyph.horiz_advance.max(0) as f32 * scale;
    }
    cursor
}

/// A vertical sign: one glyph per row, centred, top to bottom (tategaki).
pub(super) fn vertical(
    out: &mut Vec<TextVertex>,
    font: &ProportionalFont,
    text: &str,
    centre_x: f32,
    top: f32,
    bottom: f32,
    cell_width: f32,
    color: [f32; 4],
) {
    let widest = text.bytes().map(|b| font.glyphs[b as usize].width).max().unwrap_or(1).max(1) as f32;
    let tallest = text.bytes().map(|b| font.glyphs[b as usize].height).max().unwrap_or(1).max(1) as f32;
    let rows = text.len().max(1) as f32;
    let mut scale = cell_width / widest;
    let mut row = tallest * scale * 1.3;
    if row * rows > bottom - top {
        row = (bottom - top) / rows;
        scale = row / (tallest * 1.3);
    }
    let mut y = (top + bottom) * 0.5 - row * rows * 0.5;
    for byte in text.bytes() {
        let glyph = font.glyphs[byte as usize];
        if glyph.width > 0 && glyph.height > 0 && byte != b' ' {
            let w = glyph.width as f32 * scale;
            let h = glyph.height as f32 * scale;
            let x0 = centre_x - w * 0.5;
            let y0 = y + (row - h) * 0.5;
            quad(out, [x0, y0, x0 + w, y0 + h], [glyph.s, glyph.t, glyph.s2, glyph.t2], color, [AUREBESH, 0.0, 0.0, 0.0]);
        }
        y += row;
    }
}

/// Greedy word wrap by advance width.
pub(super) fn wrap(font: &ProportionalFont, text: &str, scale: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() { word.to_owned() } else { format!("{current} {word}") };
        if width(font, &candidate, scale) > max_width && !current.is_empty() {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}
