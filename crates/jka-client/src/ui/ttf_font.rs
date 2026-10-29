//! TrueType/OpenType fonts rasterized into the same glyph-atlas layout as a
//! JKA `.fontdat`, so overlays can draw them through the proportional-font
//! path. Byte codes 32-126 and 160-255 map to Latin-1, as JKA text does.

use super::{ProportionalFont, ProportionalGlyph};

/// Atlas line height in pixels. Large enough for 4K overlays; mips cover
/// small text.
const LINE_PIXELS: f32 = 64.0;
/// Empty texels around each glyph bitmap.
const PAD: i32 = 2;
/// Glyph cells start on this grid so every mip level stays separated.
const CELL_ALIGN: u32 = 8;
const ATLAS_WIDTH: u32 = 1024;
const MIP_LEVELS: u32 = 4;

/// Overlay font choice for JKA's own ergoec bitmap font.
pub const STUDIO_FONT_CLASSIC: &str = "jka";
/// Picked automatically when present in the game filesystem (e.g. a pk3).
pub const STUDIO_PACKAGED_FONTS: [&str; 2] = ["fonts/studio_overlay.ttf", "fonts/studio_overlay.otf"];

/// Per-machine and per-user font folders.
pub fn system_font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        dirs.push(std::path::Path::new(&windows).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(std::path::Path::new(&local).join(r"Microsoft\Windows\Fonts"));
    }
    dirs
}

/// System UI fonts tried when no font is chosen, best first.
pub fn studio_default_system_fonts() -> Vec<std::path::PathBuf> {
    let Some(dir) = system_font_dirs().into_iter().next() else { return Vec::new() };
    ["segoeui.ttf", "arial.ttf", "seguisb.ttf"].iter().map(|f| dir.join(f)).collect()
}

/// (display name, path) of every installed TrueType/OpenType font, sorted.
/// Reads each file's name table, so call it off the UI thread.
pub fn list_system_fonts() -> Vec<(String, String)> {
    let mut fonts: Vec<(String, String)> = system_font_dirs()
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if !["ttf", "otf", "ttc"].contains(&ext.as_str()) {
                return None;
            }
            let bytes = std::fs::read(&path).ok()?;
            Some((font_display_name(&bytes)?, path.display().to_string()))
        })
        .collect();
    fonts.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.1.cmp(&b.1)));
    fonts.dedup_by(|a, b| a.0 == b.0);
    fonts
}

pub struct RasterizedFont {
    pub width: u32,
    pub height: u32,
    pub mip_level_count: u32,
    /// White RGB, coverage alpha; all mip levels, largest first.
    pub rgba: Vec<u8>,
    pub font: ProportionalFont,
}

/// "Family Style" from the name table, for font pickers.
pub fn font_display_name(bytes: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let name = |id: u16| {
        face.names()
            .into_iter()
            .filter(|n| n.name_id == id && n.is_unicode())
            .find_map(|n| n.to_string())
    };
    let family = name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| name(ttf_parser::name_id::FAMILY))?;
    let style = name(ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
        .or_else(|| name(ttf_parser::name_id::SUBFAMILY))
        .filter(|s| !s.eq_ignore_ascii_case("regular"));
    Some(match style {
        Some(style) => format!("{family} {style}"),
        None => family,
    })
}

/// Flattened outline in atlas pixels (y down).
struct Outline {
    scale: f32,
    offset: [f32; 2],
    start: [f32; 2],
    last: [f32; 2],
    lines: Vec<([f32; 2], [f32; 2])>,
}

impl Outline {
    fn point(&self, x: f32, y: f32) -> [f32; 2] {
        [x * self.scale + self.offset[0], self.offset[1] - y * self.scale]
    }
    fn push(&mut self, to: [f32; 2]) {
        self.lines.push((self.last, to));
        self.last = to;
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.point(x, y);
        self.last = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let to = self.point(x, y);
        self.push(to);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.last, self.point(x1, y1), self.point(x, y));
        let steps = 8;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            self.push(std::array::from_fn(|k| u * u * p0[k] + 2.0 * u * t * p1[k] + t * t * p2[k]));
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (self.last, self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        let steps = 12;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            self.push(std::array::from_fn(|k| {
                u * u * u * p0[k] + 3.0 * u * u * t * p1[k] + 3.0 * u * t * t * p2[k] + t * t * t * p3[k]
            }));
        }
    }
    fn close(&mut self) {
        if self.last != self.start {
            let start = self.start;
            self.push(start);
        }
    }
}

/// Exact-area coverage (signed accumulation, as in font-rs): each edge adds
/// its covered area to the cells it crosses; a running sum fills the span.
fn rasterize_lines(lines: &[([f32; 2], [f32; 2])], width: usize, height: usize) -> Vec<f32> {
    // One spare column: an edge on the right border spills into it.
    let stride = width + 2;
    let mut area = vec![0.0f32; stride * height + 2];
    for &(a, b) in lines {
        if (a[1] - b[1]).abs() <= f32::EPSILON {
            continue;
        }
        let (dir, p0, p1) = if a[1] < b[1] { (1.0, a, b) } else { (-1.0, b, a) };
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let y_start = p0[1].max(0.0);
        if p0[1] < 0.0 {
            x -= p0[1] * dxdy;
        }
        let y_end = (p1[1].ceil().max(0.0) as usize).min(height);
        for y in (y_start as usize)..y_end {
            let row = y * stride;
            let dy = ((y + 1) as f32).min(p1[1]) - (y as f32).max(p0[1]);
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
            // Outlines are padded inside the bitmap; clamp only guards rounding.
            let (x0, x1) = (x0.clamp(0.0, width as f32), x1.clamp(0.0, width as f32));
            let x0_floor = x0.floor();
            let x0i = x0_floor as usize;
            let x1_ceil = x1.ceil();
            let x1i = x1_ceil as usize;
            if x1i <= x0i + 1 {
                let mid = 0.5 * (x0 + x1) - x0_floor;
                area[row + x0i] += d - d * mid;
                area[row + x0i + 1] += d * mid;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0_floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1_ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                area[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    area[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    area[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        area[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    area[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                area[row + x1i] += d * am;
            }
            x = x_next;
        }
    }
    let mut coverage = vec![0.0f32; width * height];
    for y in 0..height {
        let mut sum = 0.0f32;
        for x in 0..width {
            sum += area[y * stride + x];
            coverage[y * width + x] = sum.abs().min(1.0);
        }
    }
    coverage
}

struct GlyphBitmap {
    byte: usize,
    width: u32,
    height: u32,
    coverage: Vec<f32>,
}

pub fn rasterize(bytes: &[u8]) -> Result<RasterizedFont, String> {
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("{e}"))?;
    let line_units = f32::from(face.ascender()) - f32::from(face.descender());
    if line_units <= 0.0 {
        return Err("font has no vertical metrics".into());
    }
    let scale = LINE_PIXELS / line_units;
    let space = face
        .glyph_index(' ')
        .and_then(|id| face.glyph_hor_advance(id))
        .map_or(LINE_PIXELS * 0.25, |a| f32::from(a) * scale);
    let mut glyphs = [ProportionalGlyph::default(); 256];
    let mut bitmaps = Vec::new();
    for byte in (32u8..=126).chain(160..=255) {
        let Some(id) = face.glyph_index(char::from(byte)) else {
            glyphs[byte as usize].horiz_advance = space.round() as i16;
            continue;
        };
        let advance = face.glyph_hor_advance(id).map_or(space, |a| f32::from(a) * scale);
        let glyph = &mut glyphs[byte as usize];
        glyph.horiz_advance = advance.round().max(0.0) as i16;
        let mut outline = Outline { scale, offset: [0.0; 2], start: [0.0; 2], last: [0.0; 2], lines: Vec::new() };
        let Some(bounds) = face.outline_glyph(id, &mut NullBuilder) else {
            continue;
        };
        let left = (f32::from(bounds.x_min) * scale).floor() as i32 - PAD;
        let top = (f32::from(bounds.y_max) * scale).ceil() as i32 + PAD;
        let right = (f32::from(bounds.x_max) * scale).ceil() as i32 + PAD;
        let bottom = (f32::from(bounds.y_min) * scale).floor() as i32 - PAD;
        let (width, height) = ((right - left).max(1) as u32, (top - bottom).max(1) as u32);
        outline.offset = [-left as f32, top as f32];
        face.outline_glyph(id, &mut outline);
        glyph.width = width as i16;
        glyph.height = height as i16;
        glyph.horiz_offset = left as i16;
        glyph.baseline = top;
        bitmaps.push(GlyphBitmap {
            byte: byte as usize,
            width,
            height,
            coverage: rasterize_lines(&outline.lines, width as usize, height as usize),
        });
    }
    // Shelf packing, tallest first.
    bitmaps.sort_by_key(|b| std::cmp::Reverse(b.height));
    let align = |v: u32| v.div_ceil(CELL_ALIGN) * CELL_ALIGN;
    let mut places = Vec::with_capacity(bitmaps.len());
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for bitmap in &bitmaps {
        let (w, h) = (align(bitmap.width + 1), align(bitmap.height + 1));
        if w > ATLAS_WIDTH {
            return Err("glyph wider than the atlas".into());
        }
        if x + w > ATLAS_WIDTH {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        places.push((x, y));
        x += w;
        shelf = shelf.max(h);
    }
    let atlas_height = (y + shelf).max(CELL_ALIGN).next_power_of_two();
    let (aw, ah) = (ATLAS_WIDTH as usize, atlas_height as usize);
    let mut alpha = vec![0.0f32; aw * ah];
    for (bitmap, &(px, py)) in bitmaps.iter().zip(&places) {
        for row in 0..bitmap.height as usize {
            let source = &bitmap.coverage[row * bitmap.width as usize..][..bitmap.width as usize];
            alpha[(py as usize + row) * aw + px as usize..][..source.len()].copy_from_slice(source);
        }
        let glyph = &mut glyphs[bitmap.byte];
        glyph.s = px as f32 / aw as f32;
        glyph.t = py as f32 / ah as f32;
        glyph.s2 = (px + bitmap.width) as f32 / aw as f32;
        glyph.t2 = (py + bitmap.height) as f32 / ah as f32;
    }
    // Box-filtered mips of the coverage; RGB stays white.
    let mut rgba = Vec::new();
    let (mut level, mut lw, mut lh) = (alpha, aw, ah);
    let mut mip_level_count = 0;
    while mip_level_count < MIP_LEVELS && lw >= 1 && lh >= 1 {
        rgba.extend(level.iter().flat_map(|&a| [255, 255, 255, (a * 255.0).round() as u8]));
        mip_level_count += 1;
        if lw == 1 || lh == 1 {
            break;
        }
        let (nw, nh) = (lw / 2, lh / 2);
        level = (0..nw * nh)
            .map(|i| {
                let (x, y) = (i % nw * 2, i / nw * 2);
                0.25 * (level[y * lw + x] + level[y * lw + x + 1] + level[(y + 1) * lw + x] + level[(y + 1) * lw + x + 1])
            })
            .collect();
        (lw, lh) = (nw, nh);
    }
    Ok(RasterizedFont {
        width: ATLAS_WIDTH,
        height: atlas_height,
        mip_level_count,
        rgba,
        font: ProportionalFont { glyphs, point_size: LINE_PIXELS as i16, height: LINE_PIXELS as i16 },
    })
}

/// Bounds-only pass.
struct NullBuilder;
impl ttf_parser::OutlineBuilder for NullBuilder {
    fn move_to(&mut self, _: f32, _: f32) {}
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_coverage_is_exact_inside_and_partial_on_edges() {
        // A clockwise-in-y-down square from (1.5,1) to (4.5,4).
        let p = [[1.5, 1.0], [4.5, 1.0], [4.5, 4.0], [1.5, 4.0]];
        let lines: Vec<_> = (0..4).map(|i| (p[i], p[(i + 1) % 4])).collect();
        let c = rasterize_lines(&lines, 6, 5);
        assert!((c[2 * 6 + 2] - 1.0).abs() < 1e-5, "interior is solid");
        assert!((c[2 * 6 + 1] - 0.5).abs() < 1e-5, "half-covered left edge");
        assert!((c[2 * 6 + 4] - 0.5).abs() < 1e-5, "half-covered right edge");
        assert_eq!(c[2 * 6 + 5], 0.0);
        assert_eq!(c[0], 0.0);
    }

    #[test]
    #[ignore = "needs a Windows system font"]
    fn system_font_rasterizes_latin1() {
        let bytes = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").unwrap();
        assert_eq!(font_display_name(&bytes).as_deref(), Some("Segoe UI"));
        let font = rasterize(&bytes).unwrap();
        let m = font.font.glyphs[b'M' as usize];
        assert!(m.width > 20 && m.height > 30 && m.horiz_advance > 20, "{m:?}");
        assert!(font.font.glyphs[b' ' as usize].horiz_advance > 0);
        assert_eq!(font.rgba.len() % 4, 0);
        assert!(font.rgba.chunks_exact(4).any(|p| p[3] == 255));
    }
}
