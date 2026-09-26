use crate::{
    camera::Camera,
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicModelVertex},
    scene,
};
use glam::{DVec3, Vec3};
use jka_assets::map::{BrushStyle, MapBrush, MapDocument, MapFace};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MapEditTool {
    MoveBrush,
    MoveFace,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BrushSelection {
    pub entity: usize,
    pub brush: usize,
    pub face: usize,
}

#[derive(Debug, Clone)]
enum DragKind {
    Brush {
        plane_point: DVec3,
        plane_normal: DVec3,
        start_point: DVec3,
        original_distances: Vec<f64>,
        applied: DVec3,
    },
    Face {
        axis_origin: DVec3,
        axis: DVec3,
        start_axis_t: f64,
        original_distance: f64,
        applied: f64,
    },
}

#[derive(Debug, Clone)]
struct MapEditDrag {
    selection: BrushSelection,
    kind: DragKind,
    changed: bool,
}

#[derive(Debug, Clone)]
pub(super) struct EditorBrushGeometry {
    pub faces: Vec<EditorFaceGeometry>,
}

#[derive(Debug, Clone)]
pub(super) struct EditorFaceGeometry {
    pub face_index: usize,
    pub vertices: Vec<[f64; 3]>,
}

#[derive(Debug, Clone)]
pub(super) struct MapEditor {
    pub source_path: PathBuf,
    pub source_text: String,
    pub document: MapDocument,
    baseline_document: MapDocument,
    pub tool: MapEditTool,
    pub grid: f64,
    pub pending_changes: usize,
    pub selected: Option<BrushSelection>,
    pub changed_brushes: BTreeSet<(usize, usize)>,
    pub status: String,
    drag: Option<MapEditDrag>,
    preview_visible: bool,
}

impl MapEditor {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let source_text = fs::read_to_string(path)
            .map_err(|error| format!("Could not read editable source map {}: {error}", path.display()))?;
        let document = jka_assets::map::parse(&source_text)
            .map_err(|error| format!("Could not parse editable source map {}: {error}", path.display()))?;
        Ok(Self {
            source_path: path.to_owned(),
            baseline_document: document.clone(),
            document,
            source_text,
            tool: MapEditTool::MoveBrush,
            grid: 8.0,
            pending_changes: 0,
            selected: None,
            changed_brushes: BTreeSet::new(),
            status: "Click a brush to select and drag it.".into(),
            drag: None,
            preview_visible: false,
        })
    }

    pub(super) fn has_pending_changes(&self) -> bool {
        self.pending_changes != 0
    }

    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(super) fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.restore_drag_original(&drag);
        }
        self.preview_visible = false;
    }

    pub(super) fn revert_all(&mut self) {
        self.document = self.baseline_document.clone();
        self.pending_changes = 0;
        self.changed_brushes.clear();
        self.drag = None;
        self.preview_visible = true;
        self.status = "Pending brush edits reverted; updating textured preview...".into();
    }

    pub(super) fn save(&mut self) -> Result<(), String> {
        if !self.has_pending_changes() {
            self.status = "No pending changes.".into();
            return Ok(());
        }
        let rendered = self.render_source_text()?;
        // Validate the exact text that will hit disk before replacing the source,
        // and retain the reparsed document so future edits use byte spans from
        // the newly-written source rather than stale offsets from the old file.
        let reparsed = jka_assets::map::parse(&rendered)
            .map_err(|error| format!("Edited map failed validation and was not saved: {error}"))?;

        let tmp = self.source_path.with_extension("map.dinurdojk.tmp");
        fs::write(&tmp, rendered.as_bytes())
            .map_err(|error| format!("Could not write {}: {error}", tmp.display()))?;
        fs::rename(&tmp, &self.source_path)
            .or_else(|_| {
                // Windows cannot always replace an existing destination with rename.
                fs::write(&self.source_path, rendered.as_bytes())?;
                let _ = fs::remove_file(&tmp);
                Ok(())
            })
            .map_err(|error: std::io::Error| format!("Could not replace {}: {error}", self.source_path.display()))?;

        self.source_text = rendered;
        self.document = reparsed.clone();
        self.baseline_document = reparsed;
        self.pending_changes = 0;
        self.changed_brushes.clear();
        self.status = format!("Saved {}", self.source_path.display());
        Ok(())
    }

    pub(super) fn begin_drag(
        &mut self,
        camera: &Camera,
        width: u32,
        height: u32,
        cursor: (f64, f64),
    ) -> bool {
        let Some((selection, hit)) = self.pick(camera, width, height, cursor) else {
            self.selected = None;
            self.status = "No brush under cursor.".into();
            self.drag = None;
            return false;
        };
        self.selected = Some(selection);
        self.preview_visible = true;
        let Some(brush) = self.brush(selection) else { return false };
        let Some((ray_origin, ray_dir)) = cursor_ray_jka(camera, width, height, cursor) else { return false };

        let kind = match self.tool {
            MapEditTool::MoveBrush => {
                let render_forward = camera.forward();
                let plane_normal = DVec3::from_array(scene::jka_position(render_forward.to_array()).map(f64::from))
                    .normalize_or_zero();
                let plane_point = hit;
                let start_point = intersect_ray_plane(ray_origin, ray_dir, plane_point, plane_normal)
                    .unwrap_or(hit);
                DragKind::Brush {
                    plane_point,
                    plane_normal,
                    start_point,
                    original_distances: brush.faces.iter().map(|face| face.plane.distance).collect(),
                    applied: DVec3::ZERO,
                }
            }
            MapEditTool::MoveFace => {
                let face = &brush.faces[selection.face];
                let axis = DVec3::from_array(face.plane.normal).normalize_or_zero();
                let start_axis_t = closest_axis_parameter(hit, axis, ray_origin, ray_dir).unwrap_or(0.0);
                DragKind::Face {
                    axis_origin: hit,
                    axis,
                    start_axis_t,
                    original_distance: face.plane.distance,
                    applied: 0.0,
                }
            }
        };
        self.drag = Some(MapEditDrag { selection, kind, changed: false });
        self.status = match self.tool {
            MapEditTool::MoveBrush => "Dragging brush. Movement snaps to the editor grid.".into(),
            MapEditTool::MoveFace => "Dragging face along its normal. Positive expands; negative shrinks.".into(),
        };
        true
    }

    pub(super) fn update_drag(
        &mut self,
        camera: &Camera,
        width: u32,
        height: u32,
        cursor: (f64, f64),
    ) -> bool {
        let Some(mut drag) = self.drag.take() else { return false };
        let Some((ray_origin, ray_dir)) = cursor_ray_jka(camera, width, height, cursor) else {
            self.drag = Some(drag);
            return false;
        };
        let grid = self.grid.max(0.125);
        let selection = drag.selection;
        let mut changed_now = false;
        match &mut drag.kind {
            DragKind::Brush { plane_point, plane_normal, start_point, original_distances, applied } => {
                let Some(current) = intersect_ray_plane(ray_origin, ray_dir, *plane_point, *plane_normal) else {
                    self.drag = Some(drag);
                    return false;
                };
                let raw = current - *start_point;
                let snapped = DVec3::new(
                    snap(raw.x, grid),
                    snap(raw.y, grid),
                    snap(raw.z, grid),
                );
                if snapped.distance_squared(*applied) > 1e-12 {
                    if let Some(brush) = self.brush_mut(selection) {
                        for (face, original) in brush.faces.iter_mut().zip(original_distances.iter().copied()) {
                            let n = DVec3::from_array(face.plane.normal);
                            face.plane.distance = original + n.dot(snapped);
                        }
                    }
                    *applied = snapped;
                    drag.changed = snapped.length_squared() > 1e-12;
                    self.status = format!("Brush Δ {:.0} {:.0} {:.0}", snapped.x, snapped.y, snapped.z);
                    changed_now = true;
                }
            }
            DragKind::Face { axis_origin, axis, start_axis_t, original_distance, applied } => {
                let Some(current_axis_t) = closest_axis_parameter(*axis_origin, *axis, ray_origin, ray_dir) else {
                    self.drag = Some(drag);
                    return false;
                };
                let snapped = snap(current_axis_t - *start_axis_t, grid);
                if (snapped - *applied).abs() > 1e-9 {
                    if let Some(brush) = self.brush_mut(selection) {
                        brush.faces[selection.face].plane.distance = *original_distance + snapped;
                        let (accepted, clamped) = if scene::reconstruct_brush(brush).is_ok() {
                            (snapped, false)
                        } else {
                            // NetRadiant-custom's component transform path searches
                            // back to the largest grid-aligned move that still
                            // leaves a contributing brush. Do the same in our
                            // one-dimensional face-normal drag.
                            let sign = if snapped < 0.0 { -1.0 } else { 1.0 };
                            let mut low = 0i64;
                            let mut high = (snapped.abs() / grid).round() as i64;
                            while low < high {
                                let mid = low + (high - low + 1) / 2;
                                let candidate = sign * mid as f64 * grid;
                                brush.faces[selection.face].plane.distance = *original_distance + candidate;
                                if scene::reconstruct_brush(brush).is_ok() {
                                    low = mid;
                                } else {
                                    high = mid - 1;
                                }
                            }
                            (sign * low as f64 * grid, true)
                        };
                        brush.faces[selection.face].plane.distance = *original_distance + accepted;
                        if (accepted - *applied).abs() > 1e-9 {
                            *applied = accepted;
                            drag.changed = accepted.abs() > 1e-9;
                            changed_now = true;
                        }
                        self.status = if clamped {
                            format!("Face offset {accepted:+.0} (clamped to keep brush valid)")
                        } else {
                            format!("Face offset {accepted:+.0}")
                        };
                    }
                }
            }
        }
        self.drag = Some(drag);
        changed_now
    }

    pub(super) fn finish_drag(&mut self) -> bool {
        let Some(drag) = self.drag.take() else { return false };
        if !drag.changed {
            return false;
        }
        self.pending_changes = self.pending_changes.saturating_add(1);
        self.changed_brushes.insert((drag.selection.entity, drag.selection.brush));
        self.preview_visible = true;
        self.status = format!(
            "{} pending change{}; updating textured preview...",
            self.pending_changes,
            if self.pending_changes == 1 { "" } else { "s" }
        );
        true
    }

    pub(super) fn working_source_text(&self) -> Result<String, String> {
        self.render_source_text()
    }

    pub(super) fn mark_preview_synced(&mut self) {
        self.preview_visible = false;
        if self.has_pending_changes() {
            self.status = format!(
                "{} pending change{}; textured preview up to date.",
                self.pending_changes,
                if self.pending_changes == 1 { "" } else { "s" }
            );
        }
    }

    /// Temporary depth-tested solid representation of the working brush. The
    /// source-map worker replaces the real textured world after the gesture;
    /// this keeps mouse motion visually solid instead of showing only lines.
    pub(super) fn preview_surface(&self) -> Option<DynamicModelSurface> {
        if !self.preview_visible {
            return None;
        }
        let selection = self.selected?;
        let brush = self.brush(selection)?;
        let reconstructed = scene::reconstruct_brush(brush).ok()?;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for polygon in reconstructed.faces {
            if polygon.vertices.len() < 3 {
                continue;
            }
            let face = brush.faces.get(polygon.face_index)?;
            let normal = scene::render_position(face.plane.normal.map(|value| value as f32));
            // This is only a lag-hiding editor ghost while the real textured
            // source-map world rebuild follows in the background. Keep it very
            // translucent so it never paints over the face/material the user is
            // trying to judge.
            let color = if polygon.face_index == selection.face {
                [1.0, 0.72, 0.18, 0.16]
            } else {
                [0.22, 0.72, 1.0, 0.07]
            };
            let base = u32::try_from(vertices.len()).ok()?;
            for point in &polygon.vertices {
                vertices.push(DynamicModelVertex {
                    position: scene::render_position(point.to_array().map(|value| value as f32)),
                    normal,
                    uv: [0.0, 0.0],
                    color,
                });
            }
            for triangle in 1..polygon.vertices.len() - 1 {
                indices.extend_from_slice(&[
                    base,
                    base + u32::try_from(triangle).ok()?,
                    base + u32::try_from(triangle + 1).ok()?,
                ]);
            }
        }
        (!indices.is_empty()).then(|| DynamicModelSurface {
            entity_num: u16::MAX,
            vertices: Arc::new(vertices),
            indices: Arc::new(indices),
            lighting_origin: None,
            ghoul2_gpu: None,
            texture: None,
            alpha_mode: DynamicModelAlphaMode::BlendUnlit,
        })
    }

    pub(super) fn pick(
        &self,
        camera: &Camera,
        width: u32,
        height: u32,
        cursor: (f64, f64),
    ) -> Option<(BrushSelection, DVec3)> {
        let (origin, direction) = cursor_ray_jka(camera, width, height, cursor)?;
        let mut best: Option<(f64, BrushSelection, DVec3)> = None;
        for (entity_index, entity) in self.document.entities.iter().enumerate() {
            if !matches!(entity.classname(), Some("worldspawn") | Some("func_group")) {
                continue;
            }
            for (brush_index, brush) in entity.brushes.iter().enumerate() {
                let Some((distance, face)) = ray_brush(origin, direction, brush) else { continue };
                if distance < 0.0 {
                    continue;
                }
                if best.as_ref().is_none_or(|(best_distance, _, _)| distance < *best_distance) {
                    best = Some((
                        distance,
                        BrushSelection { entity: entity_index, brush: brush_index, face },
                        origin + direction * distance,
                    ));
                }
            }
        }
        best.map(|(_, selection, point)| (selection, point))
    }

    pub(super) fn selected_geometry(&self) -> Option<EditorBrushGeometry> {
        let selection = self.selected?;
        let brush = self.brush(selection)?;
        let reconstructed = scene::reconstruct_brush(brush).ok()?;
        Some(EditorBrushGeometry {
            faces: reconstructed.faces.iter().map(|face| EditorFaceGeometry {
                face_index: face.face_index,
                vertices: face.vertices.iter().map(|v| v.to_array()).collect(),
            }).collect(),
        })
    }

    pub(super) fn changed_geometries(&self) -> Vec<EditorBrushGeometry> {
        self.changed_brushes.iter().filter_map(|&(entity, brush)| {
            let brush = self.document.entities.get(entity)?.brushes.get(brush)?;
            let reconstructed = scene::reconstruct_brush(brush).ok()?;
            Some(EditorBrushGeometry {
                faces: reconstructed.faces.iter().map(|face| EditorFaceGeometry {
                    face_index: face.face_index,
                    vertices: face.vertices.iter().map(|v| v.to_array()).collect(),
                }).collect(),
            })
        }).collect()
    }

    pub(super) fn selected_shader(&self) -> Option<&str> {
        let selection = self.selected?;
        Some(&self.brush(selection)?.faces.get(selection.face)?.shader)
    }

    fn brush(&self, selection: BrushSelection) -> Option<&MapBrush> {
        self.document.entities.get(selection.entity)?.brushes.get(selection.brush)
    }

    fn brush_mut(&mut self, selection: BrushSelection) -> Option<&mut MapBrush> {
        self.document.entities.get_mut(selection.entity)?.brushes.get_mut(selection.brush)
    }

    fn restore_drag_original(&mut self, drag: &MapEditDrag) {
        match &drag.kind {
            DragKind::Brush { original_distances, .. } => {
                if let Some(brush) = self.brush_mut(drag.selection) {
                    for (face, distance) in brush.faces.iter_mut().zip(original_distances.iter().copied()) {
                        face.plane.distance = distance;
                    }
                }
            }
            DragKind::Face { original_distance, .. } => {
                if let Some(brush) = self.brush_mut(drag.selection) {
                    brush.faces[drag.selection.face].plane.distance = *original_distance;
                }
            }
        }
    }

    fn render_source_text(&self) -> Result<String, String> {
        // Replace only the parser-recorded plane byte ranges. Unlike the first
        // editor prototype, this is not line-based: Radiant permits a face's
        // three point tuples (and brushDef3 planes) to span arbitrary lines.
        // Everything outside the exact plane declaration stays byte-for-byte.
        let mut replacements = Vec::<(usize, usize, String, usize)>::new();
        for &(entity_index, brush_index) in &self.changed_brushes {
            let brush = self.document.entities.get(entity_index)
                .and_then(|entity| entity.brushes.get(brush_index))
                .ok_or_else(|| "Edited brush disappeared from source document".to_owned())?;
            for face in &brush.faces {
                let [start, end] = face.plane_span;
                if start >= end || end > self.source_text.len() {
                    return Err(format!(
                        "Edited face near source line {} has an invalid source span {start}..{end}; it was not saved.",
                        face.line
                    ));
                }
                if !self.source_text.is_char_boundary(start) || !self.source_text.is_char_boundary(end) {
                    return Err(format!(
                        "Edited face near source line {} has a non-UTF-8 source boundary; it was not saved.",
                        face.line
                    ));
                }
                replacements.push((start, end, serialize_plane_prefix(brush.style, face), face.line));
            }
        }

        replacements.sort_by_key(|(start, _, _, _)| *start);
        let mut previous_end = 0usize;
        for &(start, end, _, line) in &replacements {
            if start < previous_end {
                return Err(format!(
                    "Edited face near source line {line} overlaps another source span; it was not saved."
                ));
            }
            previous_end = end;
        }

        let mut out = String::with_capacity(self.source_text.len() + replacements.len() * 32);
        let mut cursor = 0usize;
        for (start, end, plane_prefix, _) in replacements {
            out.push_str(&self.source_text[cursor..start]);
            out.push_str(&plane_prefix);
            cursor = end;
        }
        out.push_str(&self.source_text[cursor..]);
        Ok(out)
    }
}

fn snap(value: f64, grid: f64) -> f64 {
    (value / grid).round() * grid
}

fn cursor_ray_jka(camera: &Camera, width: u32, height: u32, cursor: (f64, f64)) -> Option<(DVec3, DVec3)> {
    if width == 0 || height == 0 { return None; }
    let width_f = width as f32;
    let height_f = height as f32;
    let x = ((cursor.0 as f32 / width_f) * 2.0 - 1.0).clamp(-2.0, 2.0);
    let y = (1.0 - (cursor.1 as f32 / height_f) * 2.0).clamp(-2.0, 2.0);
    let fov_y = camera.fov_y_for_viewport(width, height);
    let aspect = width_f / height_f;
    let tan_y = (fov_y * 0.5).tan();
    let forward = camera.forward();
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    let direction_render = (forward + right * (x * aspect * tan_y) + up * (y * tan_y)).normalize_or_zero();
    if direction_render.length_squared() <= 1e-12 { return None; }
    let origin = DVec3::from_array(scene::jka_position(camera.position.to_array()).map(f64::from));
    let direction = DVec3::from_array(scene::jka_position(direction_render.to_array()).map(f64::from)).normalize_or_zero();
    Some((origin, direction))
}

fn ray_brush(origin: DVec3, direction: DVec3, brush: &MapBrush) -> Option<(f64, usize)> {
    let mut enter = f64::NEG_INFINITY;
    let mut exit = f64::INFINITY;
    let mut enter_face = 0usize;
    let mut exit_face = 0usize;
    for (face_index, face) in brush.faces.iter().enumerate() {
        let normal = DVec3::from_array(face.plane.normal);
        let denom = normal.dot(direction);
        let numer = face.plane.distance - normal.dot(origin);
        if denom.abs() < 1e-10 {
            if numer < 0.0 { return None; }
            continue;
        }
        let t = numer / denom;
        if denom < 0.0 {
            if t > enter {
                enter = t;
                enter_face = face_index;
            }
        } else if t < exit {
            exit = t;
            exit_face = face_index;
        }
        if enter > exit { return None; }
    }
    if exit < 0.0 { return None; }
    if enter >= 0.0 { Some((enter, enter_face)) } else { Some((exit, exit_face)) }
}

fn intersect_ray_plane(origin: DVec3, direction: DVec3, point: DVec3, normal: DVec3) -> Option<DVec3> {
    let denom = normal.dot(direction);
    if denom.abs() < 1e-10 { return None; }
    let t = normal.dot(point - origin) / denom;
    (t.is_finite() && t >= 0.0).then_some(origin + direction * t)
}

fn closest_axis_parameter(axis_origin: DVec3, axis: DVec3, ray_origin: DVec3, ray_dir: DVec3) -> Option<f64> {
    // Closest points between infinite axis and mouse ray. The ray is only used
    // as a direction carrier here; this remains stable when the cursor crosses
    // the selected face and matches a normal-axis manipulator naturally.
    let w0 = axis_origin - ray_origin;
    let a = axis.dot(axis);
    let b = axis.dot(ray_dir);
    let c = ray_dir.dot(ray_dir);
    let d = axis.dot(w0);
    let e = ray_dir.dot(w0);
    let denom = a * c - b * b;
    if denom.abs() < 1e-10 { return None; }
    Some((b * e - c * d) / denom)
}

fn serialize_plane_prefix(style: BrushStyle, face: &MapFace) -> String {
    if style == BrushStyle::BrushDef3 {
        return format!(
            "( {} {} {} {} )",
            fmt_num(face.plane.normal[0]),
            fmt_num(face.plane.normal[1]),
            fmt_num(face.plane.normal[2]),
            fmt_num(-face.plane.distance),
        );
    }

    let [p0, p1, p2] = plane_points(face);
    format!(
        "( {} {} {} ) ( {} {} {} ) ( {} {} {} )",
        fmt_num(p0[0]), fmt_num(p0[1]), fmt_num(p0[2]),
        fmt_num(p1[0]), fmt_num(p1[1]), fmt_num(p1[2]),
        fmt_num(p2[0]), fmt_num(p2[1]), fmt_num(p2[2]),
    )
}

fn replace_face_plane(
    original: &str,
    style: BrushStyle,
    plane_prefix: &str,
    line_no: usize,
) -> Result<String, String> {
    let newline_len = if original.ends_with("\r\n") {
        2
    } else if original.ends_with('\n') || original.ends_with('\r') {
        1
    } else {
        0
    };
    let body_end = original.len() - newline_len;
    let body = &original[..body_end];
    let indent_len = body
        .char_indices()
        .find_map(|(index, c)| (!matches!(c, ' ' | '\t')).then_some(index))
        .unwrap_or(body.len());
    let content = &body[indent_len..];
    let closes_needed = if style == BrushStyle::BrushDef3 { 1 } else { 3 };
    let mut close_count = 0usize;
    let mut prefix_end = None;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b')' {
            close_count += 1;
            if close_count == closes_needed {
                prefix_end = Some(index + 1);
                break;
            }
        }
    }
    let Some(prefix_end) = prefix_end else {
        return Err(format!(
            "Edited face on source line {line_no} is split across lines or has an unsupported plane layout; it was not saved."
        ));
    };

    let mut out = String::with_capacity(original.len() + 32);
    out.push_str(&body[..indent_len]);
    out.push_str(plane_prefix);
    out.push_str(&content[prefix_end..]);
    out.push_str(&original[body_end..]);
    Ok(out)
}

fn plane_points(face: &MapFace) -> [[f64; 3]; 3] {
    let n = DVec3::from_array(face.plane.normal).normalize_or_zero();
    let center = n * face.plane.distance;
    let helper = if n.z.abs() < 0.9 { DVec3::Z } else { DVec3::Y };
    let u = helper.cross(n).normalize_or_zero();
    let v = n.cross(u).normalize_or_zero();
    let size = 64.0;
    // parser uses cross(p2-p0, p1-p0), so p2=u and p1=v preserve +normal.
    [center.to_array(), (center + v * size).to_array(), (center + u * size).to_array()]
}

fn fmt_num(value: f64) -> String {
    if value.abs() < 0.0000005 { return "0".into(); }
    let rounded = value.round();
    if (value - rounded).abs() < 0.0000005 {
        return format!("{rounded:.0}");
    }
    let mut text = format!("{value:.6}");
    while text.ends_with('0') { text.pop(); }
    if text.ends_with('.') { text.pop(); }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_plane_rewrite_preserves_material_suffix() {
        let original = "    ( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) textures/test/wall 4 8 0 0.5 0.5 0 0 0\r\n";
        let rewritten = replace_face_plane(
            original,
            BrushStyle::Legacy,
            "( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 )",
            17,
        ).unwrap();
        assert_eq!(
            rewritten,
            "    ( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) textures/test/wall 4 8 0 0.5 0.5 0 0 0\r\n"
        );
    }

    #[test]
    fn translated_plane_points_round_trip_normal() {
        let face = MapFace {
            plane: jka_assets::map::Plane { normal: [1.0, 0.0, 0.0], distance: 128.0 },
            shader: "textures/test/wall".into(),
            projection: jka_assets::map::TextureProjection::Legacy { shift: [0.0, 0.0], rotate: 0.0, scale: [0.5, 0.5] },
            trailing: vec![0.0, 0.0, 0.0],
            plane_span: [0, 0],
            line: 1,
        };
        let [a, b, c] = plane_points(&face);
        let plane = jka_assets::map::plane_from_points(a, b, c).unwrap();
        assert!((plane.normal[0] - 1.0).abs() < 1e-9);
        assert!((plane.distance - 128.0).abs() < 1e-9);
    }
}
