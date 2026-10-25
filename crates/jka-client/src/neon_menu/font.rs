//! Letterforms for the neon signs.
//!
//! Real neon lettering is one bent glass tube of constant diameter, so the menu
//! does not rasterise a fill font: each glyph is a handful of centre-line
//! primitives and the renderer shades a tube around their distance field.
//!
//! Two sources:
//! * an outline font (`fonts/starjedi.ttf`, Star Jedi): the tube traces each
//!   glyph's outline, exactly how Star Wars logo neon signs are bent. Loaded
//!   at runtime by `load_outline_font`; labels use its lowercase letters, which
//!   are the logo capitals.
//! * built-in single-stroke caps (condensed geometric, round terminals), used
//!   until/unless the outline font is present.
//!
//! Units: cap height = 1, baseline y = 0, +y up.
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Prim {
    Line { a: [f32; 2], b: [f32; 2] },
    /// Degrees, counter-clockwise; `sweep` may be negative.
    Arc { c: [f32; 2], r: f32, start: f32, sweep: f32 },
}

const fn l(x0: f32, y0: f32, x1: f32, y1: f32) -> Prim {
    Prim::Line { a: [x0, y0], b: [x1, y1] }
}
const fn a(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Prim {
    Prim::Arc { c: [cx, cy], r, start: from, sweep: to - from }
}

pub(crate) struct Glyph {
    pub width: f32,
    pub prims: Cow<'static, [Prim]>,
}

/// Gap between tubes of adjacent letters, in cap heights (single-stroke caps).
const TRACKING: f32 = 0.24;
const SPACE: f32 = 0.46;
/// Outline-font advances already include side bearings; the tube still needs
/// a little air between neighbouring outlines.
const OUTLINE_TRACKING: f32 = 0.05;

struct OutlineFont {
    glyphs: HashMap<char, (f32, Arc<[Prim]>)>,
    space: f32,
}

static OUTLINE: RwLock<Option<Arc<OutlineFont>>> = RwLock::new(None);
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn outline() -> Option<Arc<OutlineFont>> {
    OUTLINE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Bumped whenever the letterforms change, so cached tube geometry rebuilds.
pub(crate) fn generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}

/// Tube radius as a fraction of the cap height. Outlines carry two tube
/// lengths per stroke, so their tube is thinner.
pub(crate) fn tube_ratio() -> f32 {
    if outline().is_some() { 0.034 } else { 0.052 }
}

macro_rules! glyph {
    ($w:expr, [$($p:expr),* $(,)?]) => {{
        const PRIMS: &[Prim] = &[$($p),*];
        Some(Glyph { width: $w, prims: Cow::Borrowed(PRIMS) })
    }};
}

pub(crate) fn glyph(c: char) -> Option<Glyph> {
    if let Some(font) = outline() {
        // Star Jedi's lowercase letters are the logo capitals.
        let key = c.to_ascii_lowercase();
        return font.glyphs.get(&key).or_else(|| font.glyphs.get(&c))
            .map(|(width, prims)| Glyph { width: *width, prims: Cow::Owned(prims.to_vec()) });
    }
    stroke_glyph(c)
}

fn stroke_glyph(c: char) -> Option<Glyph> {
    match c.to_ascii_uppercase() {
        'A' => glyph!(0.62, [l(0.0, 0.0, 0.31, 1.0), l(0.31, 1.0, 0.62, 0.0), l(0.12, 0.36, 0.50, 0.36)]),
        'B' => glyph!(0.56, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.27, 1.0), a(0.27, 0.76, 0.24, 90.0, -90.0),
            l(0.0, 0.52, 0.30, 0.52), a(0.30, 0.26, 0.26, 90.0, -90.0), l(0.30, 0.0, 0.0, 0.0)]),
        'C' => glyph!(0.57, [a(0.30, 0.70, 0.30, 28.0, 180.0), l(0.0, 0.70, 0.0, 0.30), a(0.30, 0.30, 0.30, 180.0, 332.0)]),
        'D' => glyph!(0.58, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.28, 1.0), a(0.28, 0.70, 0.30, 90.0, 0.0),
            l(0.58, 0.70, 0.58, 0.30), a(0.28, 0.30, 0.30, 0.0, -90.0), l(0.28, 0.0, 0.0, 0.0)]),
        'E' => glyph!(0.50, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.50, 1.0), l(0.0, 0.52, 0.42, 0.52), l(0.0, 0.0, 0.50, 0.0)]),
        'F' => glyph!(0.48, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.48, 1.0), l(0.0, 0.52, 0.40, 0.52)]),
        'G' => glyph!(0.60, [a(0.30, 0.70, 0.30, 25.0, 180.0), l(0.0, 0.70, 0.0, 0.30), a(0.30, 0.30, 0.30, 180.0, 360.0),
            l(0.60, 0.30, 0.60, 0.46), l(0.60, 0.46, 0.34, 0.46)]),
        'H' => glyph!(0.58, [l(0.0, 0.0, 0.0, 1.0), l(0.58, 0.0, 0.58, 1.0), l(0.0, 0.52, 0.58, 0.52)]),
        'I' => glyph!(0.0, [l(0.0, 0.0, 0.0, 1.0)]),
        'J' => glyph!(0.50, [l(0.50, 1.0, 0.50, 0.25), a(0.25, 0.25, 0.25, 0.0, -165.0)]),
        'K' => glyph!(0.55, [l(0.0, 0.0, 0.0, 1.0), l(0.55, 1.0, 0.0, 0.40), l(0.19, 0.607, 0.55, 0.0)]),
        'L' => glyph!(0.46, [l(0.0, 1.0, 0.0, 0.0), l(0.0, 0.0, 0.46, 0.0)]),
        'M' => glyph!(0.72, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.36, 0.36), l(0.36, 0.36, 0.72, 1.0), l(0.72, 1.0, 0.72, 0.0)]),
        'N' => glyph!(0.58, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.58, 0.0), l(0.58, 0.0, 0.58, 1.0)]),
        'O' => glyph!(0.60, [a(0.30, 0.70, 0.30, 0.0, 180.0), l(0.0, 0.70, 0.0, 0.30), a(0.30, 0.30, 0.30, 180.0, 360.0),
            l(0.60, 0.30, 0.60, 0.70)]),
        'P' => glyph!(0.53, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.27, 1.0), a(0.27, 0.74, 0.26, 90.0, -90.0), l(0.27, 0.48, 0.0, 0.48)]),
        'Q' => glyph!(0.62, [a(0.30, 0.70, 0.30, 0.0, 180.0), l(0.0, 0.70, 0.0, 0.30), a(0.30, 0.30, 0.30, 180.0, 360.0),
            l(0.60, 0.30, 0.60, 0.70), l(0.40, 0.20, 0.66, -0.10)]),
        'R' => glyph!(0.56, [l(0.0, 0.0, 0.0, 1.0), l(0.0, 1.0, 0.27, 1.0), a(0.27, 0.74, 0.26, 90.0, -90.0), l(0.27, 0.48, 0.0, 0.48),
            l(0.24, 0.48, 0.56, 0.0)]),
        'S' => glyph!(0.53, [a(0.28, 0.75, 0.25, 12.0, 270.0), a(0.28, 0.25, 0.25, 90.0, -168.0)]),
        'T' => glyph!(0.54, [l(0.0, 1.0, 0.54, 1.0), l(0.27, 1.0, 0.27, 0.0)]),
        'U' => glyph!(0.56, [l(0.0, 1.0, 0.0, 0.28), a(0.28, 0.28, 0.28, 180.0, 360.0), l(0.56, 0.28, 0.56, 1.0)]),
        'V' => glyph!(0.60, [l(0.0, 1.0, 0.30, 0.0), l(0.30, 0.0, 0.60, 1.0)]),
        'W' => glyph!(0.86, [l(0.0, 1.0, 0.20, 0.0), l(0.20, 0.0, 0.43, 0.68), l(0.43, 0.68, 0.66, 0.0), l(0.66, 0.0, 0.86, 1.0)]),
        'X' => glyph!(0.58, [l(0.0, 0.0, 0.58, 1.0), l(0.0, 1.0, 0.58, 0.0)]),
        'Y' => glyph!(0.58, [l(0.0, 1.0, 0.29, 0.48), l(0.58, 1.0, 0.29, 0.48), l(0.29, 0.48, 0.29, 0.0)]),
        'Z' => glyph!(0.54, [l(0.0, 1.0, 0.54, 1.0), l(0.54, 1.0, 0.0, 0.0), l(0.0, 0.0, 0.54, 0.0)]),
        _ => None,
    }
}

/// A glyph placed on a line, in cap-height units from the line origin.
pub(crate) struct Placed {
    pub x: f32,
    pub glyph: Glyph,
}

/// Lay out `text` from x = 0. Unknown characters advance like a space.
pub(crate) fn place(text: &str) -> Vec<Placed> {
    let (tracking, space) = outline().map_or((TRACKING, SPACE), |font| (OUTLINE_TRACKING, font.space));
    let mut out = Vec::new();
    let mut x = 0.0;
    for c in text.chars() {
        match glyph(c) {
            Some(glyph) => {
                let width = glyph.width;
                out.push(Placed { x, glyph });
                x += width + tracking;
            }
            None => x += space,
        }
    }
    out
}

/// Flattens TrueType/CFF outlines into line primitives (font units).
struct Flatten {
    prims: Vec<Prim>,
    start: [f32; 2],
    pen: [f32; 2],
}

impl Flatten {
    fn segment(&mut self, to: [f32; 2]) {
        if to != self.pen {
            self.prims.push(Prim::Line { a: self.pen, b: to });
        }
        self.pen = to;
    }
}

impl ttf_parser::OutlineBuilder for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = [x, y];
        self.pen = [x, y];
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segment([x, y]);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = self.pen;
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            self.segment([
                u * u * p0[0] + 2.0 * u * t * x1 + t * t * x,
                u * u * p0[1] + 2.0 * u * t * y1 + t * t * y,
            ]);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.pen;
        for i in 1..=10 {
            let t = i as f32 / 10.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.segment([
                a * p0[0] + b * x1 + c * x2 + d * x,
                a * p0[1] + b * y1 + c * y2 + d * y,
            ]);
        }
    }
    fn close(&mut self) {
        let start = self.start;
        self.segment(start);
    }
}

/// Parse an outline font and make it the sign lettering.
pub(crate) fn install_outline_font(bytes: &[u8]) -> Result<(), String> {
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|error| format!("{error}"))?;
    let mut raw = HashMap::new();
    for c in ('a'..='z').chain('A'..='Z').chain('0'..='9') {
        let Some(id) = face.glyph_index(c) else { continue };
        let mut flatten = Flatten { prims: Vec::new(), start: [0.0; 2], pen: [0.0; 2] };
        let Some(bbox) = face.outline_glyph(id, &mut flatten) else { continue };
        let advance = face.glyph_hor_advance(id).unwrap_or(0) as f32;
        raw.insert(c, (advance, bbox.y_max as f32, flatten.prims));
    }
    // Normalise to the height of the letters the labels use (lowercase,
    // which are the logo capitals), not the font's nominal cap height.
    let mut heights: Vec<f32> = ('a'..='z').filter_map(|c| raw.get(&c).map(|g| g.1)).collect();
    if heights.is_empty() {
        heights = ('A'..='Z').filter_map(|c| raw.get(&c).map(|g| g.1)).collect();
    }
    heights.sort_by(f32::total_cmp);
    let cap = heights.get(heights.len() / 2).copied().filter(|h| *h > 0.0)
        .ok_or_else(|| "font has no letter outlines".to_owned())?;
    let scale = |p: [f32; 2]| [p[0] / cap, p[1] / cap];
    let glyphs = raw
        .into_iter()
        .map(|(c, (advance, _, prims))| {
            let prims: Vec<Prim> = prims
                .into_iter()
                .map(|prim| match prim {
                    Prim::Line { a, b } => Prim::Line { a: scale(a), b: scale(b) },
                    arc => arc,
                })
                .collect();
            (c, (advance / cap, Arc::from(prims)))
        })
        .collect();
    let space = face.glyph_index(' ').and_then(|id| face.glyph_hor_advance(id)).map_or(0.4, |a| a as f32 / cap);
    *OUTLINE.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(OutlineFont { glyphs, space }));
    GENERATION.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

/// Candidate VFS paths for the sign font (loose in `base/fonts` or in a pk3).
pub(crate) const OUTLINE_FONT_PATHS: [&str; 4] =
    ["fonts/starjedi.ttf", "fonts/Starjedi.ttf", "fonts/STARJEDI.TTF", "fonts/starjedi.otf"];

/// Ink width of `text` in cap heights (no trailing tracking).
pub(crate) fn measure(text: &str) -> f32 {
    place(text).last().map_or(0.0, |last| last.x + last.glyph.width)
}

/// Bounds of a primitive, before tube thickness.
pub(crate) fn bounds(prim: &Prim) -> [f32; 4] {
    match *prim {
        Prim::Line { a, b } => [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])],
        Prim::Arc { c, r, .. } => [c[0] - r, c[1] - r, c[0] + r, c[1] + r],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_label_has_glyphs() {
        for info in crate::neon_menu::ITEMS.iter() {
            for c in info.label.chars().filter(|c| *c != ' ') {
                assert!(stroke_glyph(c).is_some(), "missing neon glyph {c:?} in {}", info.label);
            }
        }
        for c in crate::neon_menu::TITLE.chars() {
            assert!(stroke_glyph(c).is_some(), "missing neon glyph {c:?}");
        }
    }

    #[test]
    fn outline_fonts_flatten_to_closed_tubes() {
        // Any system TrueType font exercises the parser and flattener.
        let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\arialbd.ttf") else { return };
        let face = ttf_parser::Face::parse(&bytes, 0).expect("parse");
        let id = face.glyph_index('o').expect("o");
        let mut flatten = Flatten { prims: Vec::new(), start: [0.0; 2], pen: [0.0; 2] };
        face.outline_glyph(id, &mut flatten).expect("outline");
        assert!(flatten.prims.len() > 16, "o should flatten to many segments");
        // Each contour closes: every segment end is some segment's start.
        for prim in &flatten.prims {
            let Prim::Line { b, .. } = prim else { panic!("only lines") };
            assert!(flatten.prims.iter().any(|p| matches!(p, Prim::Line { a, .. } if a == b)));
        }
    }

    #[test]
    fn glyphs_stay_inside_their_advance() {
        for c in 'A'..='Z' {
            let glyph = stroke_glyph(c).expect("A-Z defined");
            for prim in glyph.prims.iter() {
                let [x0, y0, x1, y1] = match *prim {
                    // Arcs: test the swept points, not the full circle.
                    Prim::Arc { c, r, start, sweep } => {
                        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                        for i in 0..=32 {
                            let t = (start + sweep * i as f32 / 32.0).to_radians();
                            let p = [c[0] + r * t.cos(), c[1] + r * t.sin()];
                            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                        }
                        b
                    }
                    _ => bounds(prim),
                };
                assert!(x0 >= -0.01 && x1 <= glyph.width + 0.07, "{c} x {x0}..{x1} width {}", glyph.width);
                assert!(y0 >= -0.11 && y1 <= 1.01, "{c} y {y0}..{y1}");
            }
        }
    }
}
