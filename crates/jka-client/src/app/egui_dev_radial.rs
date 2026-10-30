//! Hold the `/dev` bind to open a relative-mouse developer wheel.
//! Mouse movement chooses the closest sector by angle. Exactly one sector is
//! always highlighted, even at the center or beyond the rim. Left click
//! activates the selected tool; releasing the dev bind cancels unconditionally.

use super::egui_theme as theme;
use super::*;
use std::f32::consts::{FRAC_PI_2, PI};

const WHEEL_GAP: f32 = 0.045; // visual-only separation; no input dead zones
const SEGMENTS_PER_SECTOR: usize = 24;
const INNER_RADIUS: f32 = 0.32;
const OUTER_RADIUS: f32 = 1.0;
const SECTOR_ANGLE: f32 = 2.0 * PI / DevTool::ALL.len() as f32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DevTool {
    Trace,
    DrawEntities,
    EntityViewer,
    RemapShader,
    ShaderComplexity,
    ModelPlacement,
    BmodelPlacement,
    MapMods,
}

impl DevTool {
    const ALL: [Self; 8] = [
        Self::Trace,
        Self::DrawEntities,
        Self::EntityViewer,
        Self::RemapShader,
        Self::ShaderComplexity,
        Self::ModelPlacement,
        Self::BmodelPlacement,
        Self::MapMods,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::DrawEntities => "3D ENTITIES",
            Self::EntityViewer => "ENTITY VIEWER",
            Self::RemapShader => "REMAP SHADER",
            Self::ShaderComplexity => "SHADER COST",
            Self::ModelPlacement => "PLACE MODEL",
            Self::BmodelPlacement => "PLACE BMODEL",
            Self::MapMods => "MAP MODS",
        }
    }

    fn subtitle(self, entities_enabled: bool, complexity_enabled: bool) -> &'static str {
        match self {
            Self::Trace => "INSPECT SURFACE",
            Self::DrawEntities if entities_enabled => "OVERLAY ON",
            Self::DrawEntities => "OVERLAY OFF",
            Self::EntityViewer => "GRAPH EDITOR",
            Self::RemapShader => "MAP MATERIAL EDITOR",
            Self::ShaderComplexity if complexity_enabled => "HEATMAP ON",
            Self::ShaderComplexity => "HEATMAP OFF",
            Self::ModelPlacement => "MD3 WORLD EDITOR",
            Self::BmodelPlacement => "INLINE BSP EDITOR",
            Self::MapMods => ".ENT IMPORT/EXPORT",
        }
    }
}

/// The raw mouse deltas accumulate into a virtual stick; only its angle
/// matters. Unlike the painted annulus, selection has no center, rim, or gap
/// dead zones. On first open (zero offset), default to the top/Trace sector.
fn tool_at(offset: egui::Vec2) -> DevTool {
    if offset == egui::Vec2::ZERO {
        return DevTool::Trace;
    }
    // Center sector zero on 12 o'clock and proceed clockwise in screen space.
    let start = -FRAC_PI_2 - SECTOR_ANGLE * 0.5;
    let angle = offset.y.atan2(offset.x);
    let sector = ((angle - start).rem_euclid(2.0 * PI) / SECTOR_ANGLE).floor() as usize;
    DevTool::ALL[sector % DevTool::ALL.len()]
}

impl App {
    /// Invoked only by a left mouse press. Release of the dev bind instead
    /// calls `close_dev_radial()` and never executes a tool.
    pub(in crate::app) fn activate_dev_radial_selection(&mut self) {
        if self.overlay != OverlayMode::DevRadial {
            return;
        }
        let selected = tool_at(egui::vec2(self.dev_radial_offset[0], self.dev_radial_offset[1]));
        self.close_dev_radial();
        match selected {
            DevTool::Trace => self.trace_surface_center(),
            DevTool::DrawEntities => {
                self.set_draw_entities(!self.video.draw_entities);
            }
            DevTool::EntityViewer => self.open_entity_graph(None),
            DevTool::ShaderComplexity => {
                // Use the normal cvar setter so UI, renderer, and session-only
                // state stay in sync; do not add a second rendering path.
                let value = if self.video.shader_complexity { "0" } else { "1" };
                if let Err(error) = self.set_console_cvar("r_shaderComplexity", value) {
                    self.console_status = error;
                }
            }
            DevTool::RemapShader => self.open_shader_remap(None),
            DevTool::ModelPlacement => self.open_model_placement_picker(),
            DevTool::BmodelPlacement => self.open_bmodel_placement_picker(),
            DevTool::MapMods => self.open_map_mods(),
        }
    }

    pub(super) fn egui_dev_radial(&mut self, root: &mut egui::Ui) {
        let rect = root.max_rect();
        let center = rect.center();
        let outer = (rect.width().min(rect.height()) * 0.37).clamp(125.0, 205.0);
        let inner = outer * INNER_RADIUS;
        let label_radius = inner + (outer - inner) * 0.62;
        let painter = root.painter();
        let selected = tool_at(egui::vec2(self.dev_radial_offset[0], self.dev_radial_offset[1]));

        // The OS cursor is hidden and locked while held. Eguis absolute pointer
        // position never influences the highlight; a left click commits it.
        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_black_alpha(92));
        painter.circle_filled(center, outer + 14.0, egui::Color32::from_black_alpha(54));
        painter.circle_filled(center, outer + 5.0, egui::Color32::from_black_alpha(90));

        for (index, tool) in DevTool::ALL.iter().copied().enumerate() {
            let angle = -FRAC_PI_2 + index as f32 * SECTOR_ANGLE;
            let start = angle - SECTOR_ANGLE * 0.5 + WHEEL_GAP;
            let end = angle + SECTOR_ANGLE * 0.5 - WHEEL_GAP;
            let highlighted = selected == tool;
            let planned = false;
            let fill = if highlighted {
                egui::Color32::from_rgba_unmultiplied(70, 102, 68, 240)
            } else if planned {
                egui::Color32::from_rgba_unmultiplied(36, 33, 28, 219)
            } else {
                egui::Color32::from_rgba_unmultiplied(27, 24, 21, 234)
            };
            paint_annular_sector(painter, center, inner, outer, start, end, fill);
            let arc_color = if highlighted {
                theme::ACCENT
            } else if planned {
                theme::LINE
            } else {
                theme::LINE_STRONG
            };
            paint_arc(painter, center, outer, start, end, egui::Stroke::new(1.5, arc_color));
            paint_arc(painter, center, inner, start, end, egui::Stroke::new(1.0, theme::LINE));

            let anchor = center + egui::vec2(angle.cos(), angle.sin()) * label_radius;
            let foreground = if highlighted {
                egui::Color32::WHITE
            } else if planned {
                theme::TEXT_DISABLED
            } else {
                theme::TEXT
            };
            let muted = if planned { theme::TEXT_DISABLED } else { theme::TEXT_FAINT };
            paint_tool_icon(painter, anchor + egui::vec2(0.0, -23.0), tool, foreground);
            painter.text(
                anchor + egui::vec2(0.0, 8.0),
                egui::Align2::CENTER_CENTER,
                tool.label(),
                egui::FontId::proportional(if outer < 165.0 { 10.0 } else { 12.0 }),
                foreground,
            );
            painter.text(
                anchor + egui::vec2(0.0, 25.0),
                egui::Align2::CENTER_CENTER,
                tool.subtitle(self.video.draw_entities, self.video.shader_complexity),
                egui::FontId::proportional(if outer < 165.0 { 8.0 } else { 9.5 }),
                muted,
            );
        }

        painter.circle_filled(center, inner - 3.0, theme::CHROME);
        painter.circle_stroke(center, inner - 2.0, egui::Stroke::new(1.0, theme::LINE_STRONG));
        painter.text(
            center + egui::vec2(0.0, -8.0),
            egui::Align2::CENTER_CENTER,
            "DEV",
            egui::FontId::proportional(25.0),
            theme::TEXT,
        );
        painter.text(
            center + egui::vec2(0.0, 17.0),
            egui::Align2::CENTER_CENTER,
            "TOOLS",
            egui::FontId::proportional(11.0),
            theme::ACCENT,
        );
        painter.text(
            center + egui::vec2(0.0, outer + 28.0),
            egui::Align2::CENTER_CENTER,
            "LEFT CLICK TO SELECT · RELEASE DEV BIND TO CANCEL",
            egui::FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }
}

pub(super) fn paint_annular_sector(
    painter: &egui::Painter,
    center: egui::Pos2,
    inner: f32,
    outer: f32,
    start: f32,
    end: f32,
    color: egui::Color32,
) {
    let mut mesh = egui::Mesh::default();
    for segment in 0..SEGMENTS_PER_SECTOR {
        let a0 = start + (end - start) * segment as f32 / SEGMENTS_PER_SECTOR as f32;
        let a1 = start + (end - start) * (segment + 1) as f32 / SEGMENTS_PER_SECTOR as f32;
        let dir0 = egui::vec2(a0.cos(), a0.sin());
        let dir1 = egui::vec2(a1.cos(), a1.sin());
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(center + dir0 * inner, color);
        mesh.colored_vertex(center + dir0 * outer, color);
        mesh.colored_vertex(center + dir1 * outer, color);
        mesh.colored_vertex(center + dir1 * inner, color);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
}

pub(super) fn paint_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    start: f32,
    end: f32,
    stroke: egui::Stroke,
) {
    let points = (0..=SEGMENTS_PER_SECTOR)
        .map(|step| {
            let angle = start + (end - start) * step as f32 / SEGMENTS_PER_SECTOR as f32;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
}

fn paint_tool_icon(painter: &egui::Painter, center: egui::Pos2, tool: DevTool, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.7, color);
    match tool {
        DevTool::Trace => {
            painter.circle_stroke(center, 9.0, stroke);
            painter.circle_filled(center, 2.0, color);
            for (start, end) in [
                (egui::vec2(-15.0, 0.0), egui::vec2(-6.0, 0.0)),
                (egui::vec2(6.0, 0.0), egui::vec2(15.0, 0.0)),
                (egui::vec2(0.0, -15.0), egui::vec2(0.0, -6.0)),
                (egui::vec2(0.0, 6.0), egui::vec2(0.0, 15.0)),
            ] {
                painter.line_segment([center + start, center + end], stroke);
            }
        }
        DevTool::DrawEntities => {
            let pts = [
                egui::vec2(0.0, -13.0), egui::vec2(12.0, -6.0),
                egui::vec2(12.0, 8.0), egui::vec2(0.0, 15.0),
                egui::vec2(-12.0, 8.0), egui::vec2(-12.0, -6.0),
            ];
            for i in 0..6 {
                painter.line_segment([center + pts[i], center + pts[(i + 1) % 6]], stroke);
            }
            for (a, b) in [
                (egui::vec2(0.0, 0.0), pts[1]),
                (egui::vec2(0.0, 0.0), pts[5]),
                (egui::vec2(0.0, 0.0), pts[3]),
            ] {
                painter.line_segment([center + a, center + b], stroke);
            }
        }
        DevTool::EntityViewer => {
            for (a, b) in [
                (egui::vec2(-11.0, -9.0), egui::vec2(12.0, -7.0)),
                (egui::vec2(-11.0, -9.0), egui::vec2(-2.0, 12.0)),
                (egui::vec2(12.0, -7.0), egui::vec2(-2.0, 12.0)),
            ] {
                painter.line_segment([center + a, center + b], stroke);
            }
            for at in [egui::vec2(-11.0, -9.0), egui::vec2(12.0, -7.0), egui::vec2(-2.0, 12.0)] {
                painter.circle_filled(center + at, 4.0, color);
            }
        }
        DevTool::RemapShader => {
            let rect = egui::Rect::from_center_size(center, egui::vec2(24.0, 20.0));
            painter.rect_stroke(rect, egui::CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            painter.line_segment([center + egui::vec2(-9.0, 5.0), center + egui::vec2(9.0, -5.0)], stroke);
        }
        DevTool::MapMods => {
            painter.rect_stroke(egui::Rect::from_center_size(center, egui::vec2(22.0, 26.0)),
                egui::CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            for y in [-7.0, 0.0, 7.0] {
                painter.line_segment([center+egui::vec2(-7.0,y), center+egui::vec2(7.0,y)], stroke);
            }
        }
        DevTool::BmodelPlacement => {
            let half = 11.0;
            let v = [egui::vec2(-half, -half), egui::vec2(half, -half),
                egui::vec2(half, half), egui::vec2(-half, half)];
            for i in 0..4 {
                painter.line_segment([center + v[i], center + v[(i + 1) % 4]], stroke);
            }
            painter.line_segment([center + v[0], center + v[2]], stroke);
            painter.line_segment([center + v[1], center + v[3]], stroke);
        }
        DevTool::ModelPlacement => {
            let points = [
                egui::vec2(-12.0, -6.0), egui::vec2(0.0, -12.0),
                egui::vec2(12.0, -6.0), egui::vec2(12.0, 8.0),
                egui::vec2(0.0, 14.0), egui::vec2(-12.0, 8.0),
            ];
            for (a,b) in [(0,1),(1,2),(2,3),(3,4),(4,5),(5,0),(1,4),(0,3),(2,5)] {
                painter.line_segment([center + points[a], center + points[b]], stroke);
            }
        }
        DevTool::ShaderComplexity => {
            for (offset, height) in [(-12.0, 8.0), (-6.0, 15.0), (0.0, 21.0), (6.0, 12.0), (12.0, 17.0)] {
                painter.line_segment([
                    center + egui::vec2(offset, 11.0),
                    center + egui::vec2(offset, 11.0 - height),
                ], stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_wheel_always_selects_a_sector() {
        // One default choice exists even before the mouse moves.
        assert_eq!(tool_at(egui::Vec2::ZERO), DevTool::Trace);
        for (index, tool) in DevTool::ALL.iter().copied().enumerate() {
            let angle = -FRAC_PI_2 + index as f32 * SECTOR_ANGLE;
            let direction = egui::vec2(angle.cos(), angle.sin());
            for radius in [0.001, 0.15, 0.7, 1.15, 10.0] {
                assert_eq!(tool_at(direction * radius), tool);
            }
        }
        // The graphics still show small separators, but they are no longer
        // unselectable wedges. Each boundary belongs to a neighboring sector.
        for index in 0..DevTool::ALL.len() {
            let boundary = -FRAC_PI_2 + (index as f32 + 0.5) * SECTOR_ANGLE;
            let direction = egui::vec2(boundary.cos(), boundary.sin());
            assert!(DevTool::ALL.contains(&tool_at(direction * 0.7)));
        }
    }
}
