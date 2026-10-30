//! Landing-menu look for the egui pages: neon tube lettering in the same
//! Star Jedi outline the Main page's signs use, lightsaber blades as
//! selection marks, kyber crystals as level pips, and frosted glass panels.
//!
//! Everything here is painter geometry, so it costs no textures and stays
//! sharp at every zoom. Colours come from `theme` (and so `crate::palette`).
use super::theme;
use crate::neon_menu::font::{self, Prim};
use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (f32::from(a) * alpha.clamp(0.0, 1.0)).round() as u8)
}

/// Mix toward white, keeping alpha.
fn whiten(color: Color32, t: f32) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let up = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(up(r), up(g), up(b), a)
}

/// The neon letterforms have no ampersand; letter it out instead.
fn neon_spelling(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('&') { text.replace('&', "AND").into() } else { text.into() }
}

/// Centre-line polylines of `text` in cap units (y up), one per primitive.
fn neon_paths(text: &str) -> Vec<Vec<[f32; 2]>> {
    let mut paths = Vec::new();
    for placed in font::place(&neon_spelling(text)) {
        for prim in placed.glyph.prims.iter() {
            match *prim {
                Prim::Line { a, b } => paths.push(vec![[placed.x + a[0], a[1]], [placed.x + b[0], b[1]]]),
                Prim::Arc { c, r, start, sweep } => {
                    let steps = ((sweep.abs() / 12.0).ceil() as usize).max(2);
                    paths.push(
                        (0..=steps)
                            .map(|i| {
                                let angle = (start + sweep * i as f32 / steps as f32).to_radians();
                                [placed.x + c[0] + r * angle.cos(), c[1] + r * angle.sin()]
                            })
                            .collect(),
                    );
                }
            }
        }
    }
    paths
}

/// Width of `text` lettered as neon at a `cap`-point cap height.
pub(in crate::app) fn neon_width(text: &str, cap: f32) -> f32 {
    font::measure(&neon_spelling(text)) * cap
}

/// Neon tube lettering with its left baseline at `origin`. `lit` 0..1 dims
/// the glow for idle states; the tube itself always stays readable.
pub(in crate::app) fn neon_text(painter: &Painter, origin: Pos2, text: &str, cap: f32, color: Color32, lit: f32) {
    let paths = neon_paths(text);
    let tube = (cap * font::tube_ratio() * 2.0).max(1.2);
    let to_screen = |p: &[f32; 2]| pos2(origin.x + p[0] * cap, origin.y - p[1] * cap);
    let passes = [
        (tube * 5.5, with_alpha(color, 0.07 * lit)),
        (tube * 3.0, with_alpha(color, 0.16 * lit)),
        (tube * 1.25, color),
        (tube * 0.45, whiten(color, 0.75)),
    ];
    for (width, stroke) in passes {
        if stroke.a() == 0 {
            continue;
        }
        for path in &paths {
            painter.add(Shape::line(path.iter().map(to_screen).collect(), Stroke::new(width, stroke)));
        }
    }
}

/// A lightsaber blade from `from` to `to`: wide soft glow, the blade, and a
/// white-hot core. Used as the selection mark under tabs and beside rails.
pub(in crate::app) fn blade(painter: &Painter, from: Pos2, to: Pos2, thickness: f32, color: Color32, lit: f32) {
    painter.line_segment([from, to], Stroke::new(thickness * 4.5, with_alpha(color, 0.10 * lit)));
    painter.line_segment([from, to], Stroke::new(thickness * 2.2, with_alpha(color, 0.35 * lit)));
    painter.line_segment([from, to], Stroke::new(thickness, with_alpha(color, lit.max(0.25))));
    painter.line_segment([from, to], Stroke::new(thickness * 0.45, with_alpha(whiten(color, 0.8), lit.max(0.25))));
}

/// A complete lightsaber, pommel at `hilt`, blade toward `tip`.
pub(in crate::app) fn saber(painter: &Painter, hilt: Pos2, tip: Pos2, thickness: f32, color: Color32, lit: f32) {
    let along = |t: f32| hilt + (tip - hilt) * t;
    let emitter = along(0.24);
    if lit > 0.0 {
        blade(painter, emitter, tip, thickness, color, lit);
    }
    painter.line_segment([hilt, emitter], Stroke::new(thickness * 1.6, Color32::from_rgb(0x9A, 0x98, 0x96)));
    for t in [0.05, 0.10, 0.15] {
        painter.line_segment([along(t), along(t + 0.025)], Stroke::new(thickness * 1.75, Color32::from_rgb(0x58, 0x55, 0x51)));
    }
    painter.line_segment([along(0.21), emitter], Stroke::new(thickness * 1.9, Color32::from_rgb(0xD8, 0xD6, 0xD2)));
}

/// A kyber crystal (an elongated diamond) centred at `center`. Lit crystals
/// glow in `color`; dark ones are a faint outline.
pub(in crate::app) fn kyber(painter: &Painter, center: Pos2, size: f32, color: Color32, lit: bool, hovered: bool) {
    let (w, h) = (size * 0.42, size * 0.62);
    let points = vec![
        center + vec2(0.0, -h),
        center + vec2(w, -h * 0.18),
        center + vec2(w * 0.55, h),
        center + vec2(-w * 0.55, h),
        center + vec2(-w, -h * 0.18),
    ];
    if lit {
        painter.circle_filled(center, size * 0.95, with_alpha(color, 0.12));
        painter.circle_filled(center, size * 0.62, with_alpha(color, 0.18));
        painter.add(Shape::convex_polygon(points.clone(), color, Stroke::new(1.0, whiten(color, 0.6))));
        // A facet highlight down the left face.
        painter.line_segment([center + vec2(-w * 0.45, -h * 0.25), center + vec2(0.0, -h * 0.8)], Stroke::new(1.2, whiten(color, 0.85)));
    } else {
        let edge = if hovered { with_alpha(color, 0.9) } else { theme::LINE_STRONG };
        let fill = if hovered { with_alpha(color, 0.18) } else { with_alpha(theme::INSET, 0.8) };
        painter.add(Shape::convex_polygon(points, fill, Stroke::new(1.0, edge)));
    }
}

/// Frosted panel over the menu scene, like the landing page's footer glass.
pub(in crate::app) fn glass() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(0x16, 0x14, 0x11, 0xD8))
        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x5C, 0x53, 0x46, 0xB0)))
        .corner_radius(egui::CornerRadius::same(10))
}

/// Vertical fade behind a header: dark at the top edge, clear at the bottom,
/// like the landing page's glass bar seen from the other side.
pub(in crate::app) fn header_backdrop(painter: &Painter, rect: Rect) {
    let top = Color32::from_rgba_unmultiplied(0x10, 0x0E, 0x0C, 0xE6);
    let bottom = Color32::from_rgba_unmultiplied(0x10, 0x0E, 0x0C, 0x00);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
}

/// A text tab: no box, the label lights up, and the selected tab carries a
/// short Celadon blade under its label.
pub(in crate::app) fn tab(ui: &mut egui::Ui, text: &str, size: f32, height: f32, selected: bool) -> egui::Response {
    let font = egui::FontId::proportional(size);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), theme::TEXT);
    let width = galley.size().x + 30.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    let color = if selected {
        theme::ACCENT
    } else if hovered {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    let label_center = rect.center() - vec2(0.0, 3.0);
    painter.text(label_center + vec2(1.0, 1.0), egui::Align2::CENTER_CENTER, text, font.clone(), theme::SHADOW);
    painter.text(label_center, egui::Align2::CENTER_CENTER, text, font, color);
    let y = rect.bottom() - 7.0;
    let half = galley.size().x * 0.5;
    if selected {
        blade(painter, pos2(rect.center().x - half, y), pos2(rect.center().x + half, y), 2.0, theme::ACCENT, 1.0);
    } else if hovered {
        painter.line_segment([pos2(rect.center().x - half, y), pos2(rect.center().x + half, y)], Stroke::new(1.0, with_alpha(theme::TEXT_DIM, 0.6)));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// A rail entry: the selected one gets a vertical blade at its left edge.
pub(in crate::app) fn rail_item(ui: &mut egui::Ui, label: &str, selected: bool, inset: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), egui::Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    if selected {
        painter.rect_filled(rect.shrink2(vec2(6.0, 1.0)), 6.0, with_alpha(theme::ACCENT, 0.10));
        blade(painter, pos2(rect.left() + 9.0, rect.top() + 6.0), pos2(rect.left() + 9.0, rect.bottom() - 6.0), 2.0, theme::ACCENT, 1.0);
    } else if hovered {
        painter.rect_filled(rect.shrink2(vec2(6.0, 1.0)), 6.0, with_alpha(theme::TEXT, 0.05));
    }
    let color = if selected { theme::ACCENT } else if hovered { theme::TEXT } else { theme::TEXT_DIM };
    painter.text(pos2(rect.left() + inset, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.5), color);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neon_paths_cover_every_letter() {
        assert!(!neon_paths("JEDI").is_empty());
        assert!(neon_width("JEDI", 20.0) > neon_width("JE", 20.0));
        assert_eq!(neon_width("", 20.0), 0.0);
    }

    #[test]
    fn whiten_moves_toward_white_and_keeps_alpha() {
        let c = whiten(Color32::from_rgba_unmultiplied(0, 100, 200, 128), 0.5);
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        assert!(r > 100 && g > 150 && b > 200);
        assert_eq!(a, 128);
    }
}
