//! Local, map-scoped MD3 placement from the developer wheel.
//! Placements use the same CGame MD3 material path as map props; no protocol
//! entity is created and nothing is submitted to the game server.
use super::*;
use super::egui_theme as theme;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelPlacement {
    pub qpath: String,
    /// Native JKA (Z-up) coordinates.
    pub origin: [f32; 3],
    /// Native JKA pitch/yaw/roll, in degrees.
    pub angles: [f32; 3],
    pub scale: f32,
    pub bounds: ([f32; 3], [f32; 3]),
}

/// This BSP model is already authored in world coordinates, often hundreds of
/// units away from (0,0,0). `pivot` captures its original map-space origin.
/// Repositioning must subtract the *transformed* pivot before adding the hit.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BmodelPlacement {
    pub model: u32,
    pub pivot: [f32; 3],
    pub hit: [f32; 3],
    pub angles: [f32; 3],
    pub bounds: ([f32; 3], [f32; 3]),
}

impl BmodelPlacement {
    pub fn instance(&self) -> crate::renderer::InlineModelInstance {
        let axis = model_axis(self.angles, 1.0);
        let rotated_pivot: [f32; 3] = std::array::from_fn(|j| {
            axis[0][j] * self.pivot[0]
                + axis[1][j] * self.pivot[1]
                + axis[2][j] * self.pivot[2]
        });
        crate::renderer::InlineModelInstance {
            model: self.model,
            origin: std::array::from_fn(|j| self.hit[j] - rotated_pivot[j]),
            axis,
            is_portal_entity: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlacementKind { Md3, Bmodel, Fx }

/// Editing never removes an object from its collection. Index identities remain
/// stable for the lifetime of an interaction; structural changes clear selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditSelection { Md3(usize), Bmodel(usize), Fx(usize) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditDragMode { Move, YawPitch, Roll }

#[derive(Clone, Debug)]
pub(super) struct PlacementSnapshot {
    pub md3: Vec<ModelPlacement>,
    pub bmodels: Vec<BmodelPlacement>,
    pub fx: Vec<FxPlacement>,
    pub remaps: std::collections::BTreeMap<String, String>,
    pub disabled_remaps: std::collections::BTreeMap<String, String>,
    pub ent_dirty: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PlacementDrag {
    pub mode: EditDragMode,
    pub selected: EditSelection,
    pub changed: bool,
    pub before: PlacementSnapshot,
    /// Accumulated local-roll orientation; yaw/pitch use direct Euler edits.
    pub orientation: Option<glam::Quat>,
}


/// A placed effect, exported as an `fx_runner` entity: `effect` is the
/// `fxFile` value (`env/small_fire`), `angles` aim it (JKA pitch/yaw/roll;
/// `-90 0 0` is straight up) and it replays every `delay_ms` like OpenJK's runner.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FxPlacement {
    pub effect: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub delay_ms: i32,
}

/// Live playback of the placed effects while editing (the placement tool's
/// own FX scheduler, separate from CGame's).
pub(super) struct PlacementFx {
    system: crate::fx::system::FxSystem,
    started: Instant,
    ids: HashMap<String, crate::fx::system::EffectId>,
    /// Next replay time per placed effect, and for the one in hand.
    next: Vec<i32>,
    ghost_next: i32,
}

/// Effect path as a runner's `fxFile` wants it: no `effects/` prefix or `.efx`.
fn fx_runner_name(qpath: &str) -> String {
    let lower = qpath.replace('\\', "/");
    let trimmed = lower.strip_prefix("effects/").unwrap_or(&lower);
    trimmed.strip_suffix(".efx").unwrap_or(trimmed).to_owned()
}

/// Distance along a ray to a sphere (used to pick effect gizmos).
fn ray_hits_sphere(start: [f32; 3], direction: [f32; 3], center: [f32; 3], radius: f32) -> Option<f32> {
    let offset: [f32; 3] = std::array::from_fn(|j| start[j] - center[j]);
    let b = offset[0] * direction[0] + offset[1] * direction[1] + offset[2] * direction[2];
    let c = offset.iter().map(|v| v * v).sum::<f32>() - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 { return None; }
    let distance = -b - discriminant.sqrt();
    (distance > 0.0 || c < 0.0).then_some(distance.max(0.0))
}

/// Radius of the pick sphere around a placed effect.
const FX_PICK_RADIUS: f32 = 28.0;

pub(super) struct ModelPlacementUi {
    pub kind: PlacementKind,
    pub bmodel_search: String,
    pub bmodel_sort: usize,
    pub bmodel_selection: Option<u32>,
    pub bmodel_pivot: [f32; 3],
    pub bmodel_bounds: Option<([f32; 3], [f32; 3])>,
    pub bmodel_placed: Vec<BmodelPlacement>,
    /// Invalidate only on edits/imports or a transition into/out of local play.
    pub bmodel_collision_dirty: bool,
    pub bmodel_collision_local: bool,
    pub ent_filename: String,
    pub ent_overwrite: bool,
    pub ent_status: String,
    pub ent_dirty: bool,
    /// Persistent captured-camera map editing; no model is removed for dragging.
    pub edit_mode: bool,
    /// An asset was chosen while editing; the next LMB stamps exactly one
    /// object, then returns to selection without ending the editor session.
    pub editor_placing: bool,
    /// Placement under the crosshair in edit mode.
    pub hover: Option<usize>,
    /// Legacy placement-mode field (editing uses PlacementDrag snapshots).
    pub held: Option<ModelPlacement>,
    /// Whole-map undo snapshots including local remaps and BSP instances.
    pub edit_undo: Vec<PlacementSnapshot>,
    pub edit_redo: Vec<PlacementSnapshot>,
    /// Explicit map-local remaps only; never serialize server shader state.
    pub authored_remaps: std::collections::BTreeMap<String, String>,
    pub editor_selected: Option<EditSelection>,
    pub bmodel_hover: Option<usize>,
    pub editor_drag: Option<PlacementDrag>,
    pub editor_mouse_owned: Option<crate::app::MouseButton>,
    pub editor_keys_owned: HashSet<crate::app::KeyCode>,
    pub hover_since: Option<Instant>,
    pub fx_placed: Vec<FxPlacement>,
    /// Effect in hand (placing a new one, or a picked-up one) and its delay.
    pub fx_selection: Option<String>,
    pub fx_delay: f32,
    pub fx_held: Option<FxPlacement>,
    pub fx_hover: Option<usize>,
    pub fx_runtime: Option<PlacementFx>,
    pub fx_draws: Arc<Vec<DynamicModelSurface>>,
    /// The browser lists effects instead of models.
    pub picker_effects: bool,
    /// Highlighted browser asset, rendered with real CGame MD3/FX materials in
    /// the map behind the browser without starting a placement operation.
    pub picker_preview: Option<(AssetKind, String)>,
    pub picker_hit: Option<[f32; 3]>,
    pub picker_draws: Arc<Vec<DynamicModelSurface>>,
    pub picker_draw_key: Option<(String, [f32; 3], i32, i32)>,
    pub picker_yaw: f32,
    pub picker_scale: f32,
    /// Own FX scheduler, reset per preview asset like Asset Viewer. Browsing
    /// never consumes the live placement scheduler's 256-effect registry.
    pub picker_fx_runtime: Option<PlacementFx>,
    pub search: String,
    pub sort: usize,
    pub active: bool,
    pub rotating: bool,
    /// Cached local roll for the placement preview (Alt+RMB only).
    pub preview_rotation: Option<glam::Quat>,
    pub hit_valid: bool,
    pub selection: Option<String>,
    pub scale: f32,
    pub angles: [f32; 3],
    pub origin: [f32; 3],
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub placed: Vec<ModelPlacement>,
    pub placed_draws: Arc<Vec<DynamicModelSurface>>,
    pub preview_draws: Arc<Vec<DynamicModelSurface>>,
    pub placed_dirty: bool,
    pub preview_key: Option<ModelPlacement>,
    pub map_label: String,
    pub status: String,
    pub thumbnails: HashMap<String, egui::TextureHandle>,
    pub thumbnails_pending: HashSet<String>,
    pub thumbnails_missing: HashSet<String>,
}

impl Default for ModelPlacementUi {
    fn default() -> Self {
        Self {
            kind: PlacementKind::Md3, bmodel_search: String::new(),
            bmodel_sort: 0, bmodel_selection: None,
            bmodel_pivot: [0.0; 3], bmodel_bounds: None,
            bmodel_placed: Vec::new(),
            bmodel_collision_dirty: true, bmodel_collision_local: false,
            ent_filename: String::new(), ent_overwrite: false, ent_status: String::new(), ent_dirty: false,
            edit_mode: false, editor_placing: false, hover: None, held: None, edit_undo: Vec::new(),
            edit_redo: Vec::new(), authored_remaps: Default::default(),
            editor_selected: None, bmodel_hover: None, editor_drag: None,
            editor_mouse_owned: None, editor_keys_owned: HashSet::new(), hover_since: None,
            fx_placed: Vec::new(), fx_selection: None, fx_delay: 200.0, fx_held: None,
            fx_hover: None, fx_runtime: None, fx_draws: Arc::new(Vec::new()),
            picker_effects: false, picker_preview: None, picker_hit: None,
            picker_draws: Arc::new(Vec::new()), picker_draw_key: None,
            picker_yaw: 0.0, picker_scale: 1.0,
            picker_fx_runtime: None,
            search: String::new(), sort: 0, active: false, rotating: false,
            preview_rotation: None,
            hit_valid: false, selection: None, scale: 1.0, angles: [0.0; 3],
            origin: [0.0; 3], bounds: None, placed: Vec::new(),
            placed_draws: Arc::new(Vec::new()), preview_draws: Arc::new(Vec::new()),
            placed_dirty: false, preview_key: None, map_label: String::new(),
            status: String::new(), thumbnails: HashMap::new(),
            thumbnails_pending: HashSet::new(), thumbnails_missing: HashSet::new(),
        }
    }
}

/// This is the exact MD3 `angles_to_axis` transform from the CGame presenter.
// JKA's model_axis is Rz(yaw) * Ry(pitch) * Rx(roll), with columns
// forward, left and up. Only ROLL composes on the current object's local
// forward axis. Yaw and pitch intentionally keep v1's direct Euler edits.
// Cache the quaternion for the duration of a roll drag to avoid unnecessary
// Euler roundtrips between mouse events.
pub(in crate::app) fn placement_orientation(angles: [f32; 3]) -> glam::Quat {
    glam::Quat::from_rotation_z(angles[1].to_radians())
        * glam::Quat::from_rotation_y(angles[0].to_radians())
        * glam::Quat::from_rotation_x(angles[2].to_radians())
}

/// Match v1: screen X edits the stored yaw; screen Y edits pitch.
/// Unlike local quaternion composition, these two axes remain intuitive
/// even after the object has been rolled or pitched.
pub(in crate::app) fn placement_rotate_yaw_pitch(angles: [f32; 3], pitch: f32, yaw: f32) -> [f32; 3] {
    let mut result = angles;
    result[0] = (result[0] + pitch).rem_euclid(360.0);
    result[1] = (result[1] + yaw).rem_euclid(360.0);
    result
}

pub(in crate::app) fn placement_rotate_local(orientation: glam::Quat, pitch: f32, yaw: f32, roll: f32) -> glam::Quat {
    (orientation
        * glam::Quat::from_rotation_z(yaw.to_radians())
        * glam::Quat::from_rotation_y(pitch.to_radians())
        * glam::Quat::from_rotation_x(roll.to_radians())).normalize()
}

pub(in crate::app) fn placement_rotation_angles(orientation: glam::Quat) -> [f32; 3] {
    let (yaw, pitch, roll) = orientation.to_euler(glam::EulerRot::ZYX);
    [pitch, yaw, roll].map(|a| a.to_degrees().rem_euclid(360.0))
}

pub(super) fn model_axis(angles: [f32; 3], scale: f32) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, [-right[0], -right[1], -right[2]], up].map(|v| v.map(|c| c * scale))
}

/// Distance along a world ray to its first entry into a placement's oriented
/// bounds (the ray is carried into model space, where the bounds are an AABB).
fn ray_hits_placement(start: [f32; 3], direction: [f32; 3], placement: &ModelPlacement) -> Option<f32> {
    let scale_sq = placement.scale * placement.scale;
    if !(scale_sq > 1.0e-12) { return None; }
    let axis = model_axis(placement.angles, placement.scale);
    let relative: [f32; 3] = std::array::from_fn(|j| start[j] - placement.origin[j]);
    let dot = |k: usize, v: [f32; 3]| (axis[k][0] * v[0] + axis[k][1] * v[1] + axis[k][2] * v[2]) / scale_sq;
    let origin: [f32; 3] = std::array::from_fn(|k| dot(k, relative));
    let delta: [f32; 3] = std::array::from_fn(|k| dot(k, direction));
    let (mins, maxs) = placement.bounds;
    let (mut near, mut far) = (0.0_f32, f32::INFINITY);
    for k in 0..3 {
        if delta[k].abs() < 1.0e-9 {
            if origin[k] < mins[k] || origin[k] > maxs[k] { return None; }
        } else {
            let (a, b) = ((mins[k] - origin[k]) / delta[k], (maxs[k] - origin[k]) / delta[k]);
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far { return None; }
        }
    }
    Some(near)
}

fn placement_corners(placement: &ModelPlacement) -> [[f32; 3]; 8] {
    let (mins, maxs) = placement.bounds;
    let axis = model_axis(placement.angles, placement.scale);
    std::array::from_fn(|i| {
        let v = [if i & 1 == 0 { mins[0] } else { maxs[0] },
                 if i & 2 == 0 { mins[1] } else { maxs[1] },
                 if i & 4 == 0 { mins[2] } else { maxs[2] }];
        std::array::from_fn(|j| placement.origin[j]
            + axis[0][j] * v[0] + axis[1][j] * v[1] + axis[2][j] * v[2])
    })
}

fn bmodel_corners(placement: &BmodelPlacement) -> [[f32; 3]; 8] {
    let (mins, maxs) = placement.bounds;
    let pose = placement.instance();
    std::array::from_fn(|i| {
        let vertex = [if i & 1 == 0 { mins[0] } else { maxs[0] },
            if i & 2 == 0 { mins[1] } else { maxs[1] },
            if i & 4 == 0 { mins[2] } else { maxs[2] }];
        let r = pose.rotate(vertex);
        std::array::from_fn(|j| pose.origin[j] + r[j])
    })
}

/// BSP entity `origin` (if authored) is the preferred pivot. Otherwise use
/// the source model's compiled bounds midpoint, not the world's origin.
pub(super) fn bmodel_pivot(map: &scene::PreparedMap, model: u32) -> Option<([f32; 3], ([f32; 3], [f32; 3]))> {
    let bounds = *map.bsp_stats.as_ref()?.inline_model_bounds.get(model as usize)?;
    let midpoint = std::array::from_fn(|i| (bounds.0[i] + bounds.1[i]) * 0.5);
    let pivot = map.brush_entities.iter().find(|entity| entity.model == model)
        .and_then(|entity| entity.spawn_vars.iter().find(|(key, _)| key.eq_ignore_ascii_case("origin")))
        .and_then(|(_, value)| {
            let parts = value.split_whitespace()
                .map(str::parse::<f32>).collect::<Result<Vec<_>, _>>().ok()?;
            (parts.len() == 3 && parts.iter().all(|x| x.is_finite()))
                .then(|| [parts[0], parts[1], parts[2]])
        }).unwrap_or(midpoint);
    Some((pivot, bounds))
}

/// Project BSP triangles in a fixed isometric camera to paint small, cheap
/// geometry-derived thumbnails directly from the prepared inline mesh.
fn paint_bmodel_thumbnail(ui: &mut egui::Ui, map: &scene::PreparedMap,
    model: &scene::InlineModelGeometry, bounds: ([f32; 3], [f32; 3])) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 42.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, theme::CONTROL);
    let midpoint: [f32; 3] = std::array::from_fn(|i| (bounds.0[i] + bounds.1[i]) * 0.5);
    let span = (0..3).map(|i| (bounds.1[i] - bounds.0[i]).abs())
        .fold(1.0_f32, f32::max);
    let project = |v: [f32; 3]| {
        let p = scene::jka_position(v);
        let x = (p[0] - midpoint[0]) / span;
        let y = (p[1] - midpoint[1]) / span;
        let z = (p[2] - midpoint[2]) / span;
        rect.center() + egui::vec2((x - y * 0.70) * 34.0,
            (-z * 0.80 - x * 0.30 - y * 0.28) * 34.0)
    };
    let start = model.vertices.start as usize;
    let end = (model.vertices.end as usize).min(map.inline_vertices.len());
    if start >= end { return; }
    let vertices = &map.inline_vertices[start..end];
    let tri_count = vertices.len() / 3;
    let step = (tri_count / 55).max(1);
    for tri in vertices.chunks_exact(3).step_by(step).take(55) {
        let points = [project(tri[0].position), project(tri[1].position), project(tri[2].position)];
        for k in 0..3 {
            ui.painter().line_segment([points[k], points[(k + 1) % 3]],
                egui::Stroke::new(0.65, theme::TEXT_DIM));
        }
    }
}

impl App {
    pub(in crate::app) fn open_model_placement_picker(&mut self) {
        if self.prepared_map_cache.is_none() || self.map_collision.is_none() {
            self.console_status = "MODEL PLACEMENT: LOAD A MAP FIRST".into();
            self.publish_ui();
            return;
        }
        self.model_placement.map_label = self.prepared_map_cache.as_ref()
            .map(|map| map.label.clone()).unwrap_or_default();
        if self.model_placement.editor_placing { self.finish_editor_add(); }
        self.model_placement.kind = PlacementKind::Md3;
        self.model_placement.active = false;
        self.model_placement.rotating = false;
        self.model_placement.preview_rotation = None;
        self.model_placement.preview_draws = Arc::new(Vec::new());
        self.model_placement.preview_key = None;
        self.ensure_asset_viewer_catalog();
        self.set_overlay(OverlayMode::ModelPicker);
    }

    pub(in crate::app) fn open_bmodel_placement_picker(&mut self) {
        if self.prepared_map_cache.is_none() || self.map_collision.is_none() {
            self.console_status = "BMODEL PLACEMENT: LOAD A BSP MAP FIRST".into();
            self.publish_ui();
            return;
        }
        self.model_placement.map_label = self.prepared_map_cache.as_ref()
            .map(|map| map.label.clone()).unwrap_or_default();
        if self.model_placement.editor_placing { self.finish_editor_add(); }
        self.model_placement.kind = PlacementKind::Bmodel;
        self.model_placement.picker_preview = None;
        self.model_placement.picker_hit = None;
        self.model_placement.picker_draws = Arc::new(Vec::new());
        self.model_placement.picker_draw_key = None;
        self.model_placement.picker_fx_runtime = None;
        self.model_placement.active = false;
        self.model_placement.rotating = false;
        self.model_placement.preview_rotation = None;
        self.model_placement.preview_draws = Arc::new(Vec::new());
        self.model_placement.preview_key = None;
        self.model_placement.hit_valid = false;
        self.set_overlay(OverlayMode::BmodelPicker);
    }

    pub(in crate::app) fn choose_placement_bmodel(&mut self, model: u32) {
        let Some(cache) = self.prepared_map_cache.as_ref() else { return; };
        let Some((pivot, bounds)) = bmodel_pivot(&cache.map, model) else {
            self.model_placement.status = format!("*{model}: missing bounds");
            return;
        };
        if !cache.map.inline_models.iter().any(|entry| entry.model == model) {
            self.model_placement.status = format!("*{model}: no drawable surfaces");
            return;
        }
        self.model_placement.kind = PlacementKind::Bmodel;
        self.model_placement.editor_placing = self.model_placement.edit_mode;
        self.model_placement.editor_selected = None;
        self.model_placement.hover = None;
        self.model_placement.bmodel_hover = None;
        self.model_placement.fx_hover = None;
        self.model_placement.hover_since = None;
        self.model_placement.bmodel_selection = Some(model);
        self.model_placement.bmodel_pivot = pivot;
        self.model_placement.bmodel_bounds = Some(bounds);
        self.model_placement.scale = 1.0;
        self.model_placement.angles = [0.0; 3];
        self.model_placement.hit_valid = false;
        self.model_placement.rotating = false;
        self.model_placement.preview_rotation = None;
        self.model_placement.active = true;
        self.model_placement.status.clear();
        // Grow the model's private GPU slot once at selection, not every frame.
        self.ensure_inline_instance_capacity();
        self.set_overlay(OverlayMode::None);
    }

    pub(in crate::app) fn choose_placement_model(&mut self, qpath: String) {
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.model_placement.status = error;
            return;
        }
        let bounds = self.asset_preview_entity_presenter.as_mut()
            .expect("presenter initialized").md3_frame_bounds(&qpath);
        let bounds = match bounds {
            Ok(bounds) => bounds,
            Err(error) => {
                self.model_placement.status = format!("{qpath}: {error}");
                return;
            }
        };
        self.model_placement.kind = PlacementKind::Md3;
        // During map editing, choose a one-shot ghost without exiting edit mode.
        // The existing dev-wheel placement tool still supports repeat stamping.
        self.model_placement.editor_placing = self.model_placement.edit_mode;
        self.model_placement.editor_selected = None;
        self.model_placement.hover = None;
        self.model_placement.bmodel_hover = None;
        self.model_placement.fx_hover = None;
        self.model_placement.hover_since = None;
        self.model_placement.held = None;
        self.model_placement.fx_selection = None;
        self.model_placement.fx_held = None;
        self.model_placement.selection = Some(qpath);
        self.model_placement.scale = 1.0;
        self.model_placement.angles = [0.0; 3];
        self.model_placement.bounds = Some(bounds);
        self.model_placement.active = true;
        self.model_placement.rotating = false;
        self.model_placement.preview_rotation = None;
        self.model_placement.preview_key = None;
        self.model_placement.status.clear();
        self.set_overlay(OverlayMode::None);
    }

    /// Edit mode: move / copy / delete the placements already in the list
    /// (staged by the placement tool, `menuent`, or a Map Mods import).
    pub(in crate::app) fn enter_placement_edit(&mut self) {
        if self.prepared_map_cache.is_none() || self.map_collision.is_none() {
            self.console_status = "PLACEMENT EDIT: LOAD A MAP FIRST".into();
            self.publish_ui();
            return;
        }
        let state = &mut self.model_placement;
        state.map_label = self.prepared_map_cache.as_ref()
            .map(|map| map.label.clone()).unwrap_or_default();
        state.kind = PlacementKind::Md3;
        state.edit_mode = true;
        state.editor_placing = false;
        state.active = true;
        state.editor_drag = None;
        state.editor_mouse_owned = None;
        state.editor_selected = None;
        state.bmodel_hover = None;
        state.rotating = false;
        state.preview_rotation = None;
        state.selection = None;
        state.bounds = None;
        state.held = None;
        state.hover = None;
        state.fx_selection = None;
        state.fx_held = None;
        state.fx_hover = None;
        state.hit_valid = false;
        state.preview_key = None;
        state.preview_draws = Arc::new(Vec::new());
        state.status = format!("Edit mode: {} model(s), {} fx", state.placed.len(), state.fx_placed.len());
        self.set_overlay(OverlayMode::None);
        self.egui_repaint_requested = true;
    }

    fn placement_snapshot(&self) -> PlacementSnapshot {
        PlacementSnapshot {
            md3: self.model_placement.placed.clone(),
            bmodels: self.model_placement.bmodel_placed.clone(),
            fx: self.model_placement.fx_placed.clone(),
            remaps: self.model_placement.authored_remaps.clone(),
            disabled_remaps: self.shader_remap_ui.disabled.clone(),
            ent_dirty: self.model_placement.ent_dirty,
        }
    }

    pub(in crate::app) fn placement_checkpoint(&mut self) {
        let before = self.placement_snapshot();
        let state = &mut self.model_placement;
        if state.edit_undo.len() >= 128 { state.edit_undo.remove(0); }
        state.edit_undo.push(before);
        state.edit_redo.clear();
        state.ent_dirty = true;
    }

    fn apply_placement_snapshot(&mut self, snapshot: PlacementSnapshot) {
        let previous = std::mem::replace(&mut self.model_placement.authored_remaps, snapshot.remaps);
        self.model_placement.placed = snapshot.md3;
        self.model_placement.bmodel_placed = snapshot.bmodels;
        self.model_placement.fx_placed = snapshot.fx;
        let previously_disabled = std::mem::replace(
            &mut self.shader_remap_ui.disabled, snapshot.disabled_remaps);
        self.model_placement.editor_selected = None;
        self.model_placement.hover = None;
        self.model_placement.fx_hover = None;
        self.model_placement.bmodel_hover = None;
        self.model_placement.placed_dirty = true;
        self.model_placement.bmodel_collision_dirty = true;
        self.model_placement.ent_dirty = true;
        self.model_placement.editor_drag = None;
        self.model_placement.preview_key = None;
        let new_remaps = self.model_placement.authored_remaps.clone();
        for source in previous.keys().filter(|key| !new_remaps.contains_key(*key)) {
            self.local_remap_shader(source, source);
        }
        for (source, target) in new_remaps.iter() {
            if previous.get(source) != Some(target) { self.local_remap_shader(source, target); }
        }
        // Disabling a non-authored (e.g. server-provided) redirect is a UI
        // change, not an exportable map mod. Restore its effective preview on
        // undo/redo without accidentally promoting it into authored_remaps.
        let disabled = self.shader_remap_ui.disabled.clone();
        for (source, target) in &previously_disabled {
            if !disabled.contains_key(source) && !new_remaps.contains_key(source) {
                self.local_remap_shader(source, target);
            }
        }
        for source in disabled.keys() {
            if !previously_disabled.contains_key(source) && !new_remaps.contains_key(source) {
                self.local_remap_shader(source, source);
            }
        }
    }

    pub(in crate::app) fn redo_model_placement(&mut self) {
        let Some(next) = self.model_placement.edit_redo.pop() else { return; };
        let current = self.placement_snapshot();
        self.model_placement.edit_undo.push(current);
        self.apply_placement_snapshot(next);
        self.model_placement.status = "Redo".into();
    }

    pub(in crate::app) fn placement_click(&mut self, copy: bool) -> bool {
        if self.model_placement.editor_placing {
            // An add preview is active *inside* the persistent editor session.
            // Never let a failed stamp trigger gameplay attacks or noclip binds.
            if !self.model_placement.hit_valid {
                self.model_placement.status = "Aim at a solid surface to place the asset".into();
                return true;
            }
            let kind = self.model_placement.kind;
            let index = match kind {
                PlacementKind::Md3 => self.model_placement.placed.len(),
                PlacementKind::Bmodel => self.model_placement.bmodel_placed.len(),
                PlacementKind::Fx => self.model_placement.fx_placed.len(),
            };
            self.stamp_placement();
            let added = match kind {
                PlacementKind::Md3 => self.model_placement.placed.len() > index,
                PlacementKind::Bmodel => self.model_placement.bmodel_placed.len() > index,
                PlacementKind::Fx => self.model_placement.fx_placed.len() > index,
            };
            if added {
                self.finish_editor_add();
                self.model_placement.editor_selected = Some(match kind {
                    PlacementKind::Md3 => EditSelection::Md3(index),
                    PlacementKind::Bmodel => EditSelection::Bmodel(index),
                    PlacementKind::Fx => EditSelection::Fx(index),
                });
            }
            // Consume button-up too; the game never received this press.
            self.model_placement.editor_mouse_owned = Some(crate::app::MouseButton::Left);
            return true;
        }
        if !self.model_placement.edit_mode {
            if !self.model_placement.hit_valid { return false; }
            self.stamp_placement();
            return true;
        }
        let picked = self.model_placement.bmodel_hover.map(EditSelection::Bmodel)
            .or_else(|| self.model_placement.fx_hover.map(EditSelection::Fx))
            .or_else(|| self.model_placement.hover.map(EditSelection::Md3));
        self.model_placement.editor_selected = picked;
        let Some(mut selected) = picked else { return false; };
        let before = self.placement_snapshot();
        if copy {
            let state = &mut self.model_placement;
            selected = match selected {
                EditSelection::Md3(i) => {
                    let Some(p) = state.placed.get(i).cloned() else { return false; };
                    state.placed.push(p);
                    state.placed_dirty = true;
                    EditSelection::Md3(state.placed.len() - 1)
                }
                EditSelection::Bmodel(i) => {
                    let Some(p) = state.bmodel_placed.get(i).cloned() else { return false; };
                    state.bmodel_placed.push(p);
                    state.bmodel_collision_dirty = true;
                    EditSelection::Bmodel(state.bmodel_placed.len() - 1)
                }
                EditSelection::Fx(i) => {
                    let Some(p) = state.fx_placed.get(i).cloned() else { return false; };
                    state.fx_placed.push(p);
                    EditSelection::Fx(state.fx_placed.len() - 1)
                }
            };
            state.editor_selected = Some(selected);
        }
        if copy && matches!(selected, EditSelection::Bmodel(_)) {
            self.ensure_inline_instance_capacity();
        }
        self.model_placement.editor_drag = Some(PlacementDrag {
            mode: EditDragMode::Move, selected, changed: copy, before,
            orientation: None,
        });
        self.model_placement.editor_mouse_owned = Some(crate::app::MouseButton::Left);
        true
    }

    /// Dismiss a one-shot Add ghost without leaving the editing session.
    pub(in crate::app) fn finish_editor_add(&mut self) {
        let state = &mut self.model_placement;
        state.editor_placing = false;
        state.kind = PlacementKind::Md3;
        state.selection = None;
        state.bounds = None;
        state.bmodel_selection = None;
        state.bmodel_bounds = None;
        state.fx_selection = None;
        state.hit_valid = false;
        state.rotating = false;
        state.preview_rotation = None;
        state.preview_key = None;
        state.preview_draws = Arc::new(Vec::new());
    }

    pub(in crate::app) fn placement_begin_rotation(&mut self, button: crate::app::MouseButton) -> bool {
        if !self.model_placement.edit_mode || self.model_placement.editor_placing { return false; }
        // Right/middle-clicking empty space must remain available to noclip
        // and normal bindings, even when a previous object is still selected.
        let selected = self.model_placement.bmodel_hover.map(EditSelection::Bmodel)
            .or_else(|| self.model_placement.fx_hover.map(EditSelection::Fx))
            .or_else(|| self.model_placement.hover.map(EditSelection::Md3));
        let Some(selected) = selected else { return false; };
        let before = self.placement_snapshot();
        let angles = match selected {
            EditSelection::Md3(i) => self.model_placement.placed.get(i).map(|p| p.angles),
            EditSelection::Bmodel(i) => self.model_placement.bmodel_placed.get(i).map(|p| p.angles),
            EditSelection::Fx(i) => self.model_placement.fx_placed.get(i).map(|p| p.angles),
        };
        let Some(angles) = angles else { return false; };
        self.model_placement.editor_selected = Some(selected);
        self.model_placement.editor_drag = Some(PlacementDrag {
            mode: if button == crate::app::MouseButton::Middle { EditDragMode::YawPitch }
                else { EditDragMode::Roll }, selected, changed: false, before,
            // Only local roll needs a running quaternion; middle mouse
            // keeps the direct yaw/pitch adjustments from v1.
            orientation: (button == crate::app::MouseButton::Right)
                .then(|| placement_orientation(angles)),
        });
        self.model_placement.editor_mouse_owned = Some(button);
        true
    }

    pub(in crate::app) fn placement_release_drag(&mut self, button: crate::app::MouseButton) -> bool {
        if self.model_placement.editor_mouse_owned != Some(button) { return false; }
        self.model_placement.editor_mouse_owned = None;
        if button == crate::app::MouseButton::Right {
            self.model_placement.rotating = false;
            self.model_placement.preview_rotation = None;
        }
        if let Some(drag) = self.model_placement.editor_drag.take() {
            if drag.changed {
                if self.model_placement.edit_undo.len() >= 128 { self.model_placement.edit_undo.remove(0); }
                self.model_placement.edit_undo.push(drag.before);
                self.model_placement.edit_redo.clear();
                self.model_placement.ent_dirty = true;
            }
        }
        true
    }

    pub(in crate::app) fn placement_drag_motion(&mut self, delta: (f64, f64)) {
        let Some(drag) = self.model_placement.editor_drag.as_ref() else { return; };
        let selection = drag.selected;
        let mode = drag.mode;
        if !delta.0.is_finite() || !delta.1.is_finite() { return; }
        let dx = (delta.0 as f32).clamp(-250.0, 250.0);
        let dy = (delta.1 as f32).clamp(-250.0, 250.0);
        if dx.abs() + dy.abs() < 0.0001 { return; }
        let (origin, angles) = match selection {
            EditSelection::Md3(i) => match self.model_placement.placed.get(i) {
                Some(p) => (p.origin, p.angles), None => return,
            },
            EditSelection::Bmodel(i) => match self.model_placement.bmodel_placed.get(i) {
                Some(p) => (p.hit, p.angles), None => return,
            },
            EditSelection::Fx(i) => match self.model_placement.fx_placed.get(i) {
                Some(p) => (p.origin, p.angles), None => return,
            },
        };
        let mut next_origin = origin;
        let mut next_angles = angles;
        match mode {
            EditDragMode::Move => {
                let eye = scene::jka_position(self.camera.position.to_array());
                let forward = scene::jka_position(self.camera.forward().to_array());
                let mut right = [forward[1], -forward[0], 0.0_f32];
                let len = (right[0]*right[0] + right[1]*right[1]).sqrt();
                if len < 0.0001 { right = [0.0, -1.0, 0.0]; }
                else { right[0] /= len; right[1] /= len; }
                // right × forward points toward world up with JKA Z-up.
                let up = [ right[1]*forward[2], -right[0]*forward[2],
                    right[0]*forward[1] - right[1]*forward[0] ];
                let distance = ((0..3).map(|i| (origin[i]-eye[i]).powi(2)).sum::<f32>()).sqrt();
                let (width, height) = self.window.as_ref().map_or((1280.0, 720.0), |w| {
                    let size = w.inner_size(); (size.width as f32, size.height as f32)
                });
                let height = height.max(1.0);
                let aspect = width.max(1.0) / height;
                let tan_half_vertical = (self.camera.cg_fov().clamp(1.0, 170.0)
                    * 0.5).to_radians().tan() / aspect;
                let fine = if self.modifiers.alt_key() { 0.15 } else { 1.0 };
                let units = (2.0 * distance.max(32.0) * tan_half_vertical / height)
                    .clamp(0.02, 20.0) * fine;
                if self.modifiers.control_key() {
                    next_origin[2] -= dy * units;
                } else if self.modifiers.shift_key() {
                    next_origin[0] += right[0] * dx * units - right[1] * dy * units;
                    next_origin[1] += right[1] * dx * units + right[0] * dy * units;
                } else {
                    for axis in 0..3 { next_origin[axis] += (right[axis]*dx - up[axis]*dy)*units; }
                }
            }
            EditDragMode::YawPitch => {
                // Restore the original middle-mouse feel: directly adjust
                // JKA pitch/yaw fields rather than rotating object-local axes.
                next_angles = placement_rotate_yaw_pitch(angles, -dy * 0.20, dx * 0.20);
            }
            EditDragMode::Roll => {
                // Retain v2's right-mouse roll about the model's *current*
                // forward axis (including any existing yaw and pitch).
                let orientation = drag.orientation.unwrap_or_else(|| placement_orientation(angles));
                let rotated = placement_rotate_local(orientation, 0.0, 0.0, (dx - dy) * 0.20);
                next_angles = placement_rotation_angles(rotated);
                if let Some(drag) = self.model_placement.editor_drag.as_mut() {
                    drag.orientation = Some(rotated);
                }
            }
        }
        match selection {
            EditSelection::Md3(i) => if let Some(p) = self.model_placement.placed.get_mut(i) {
                p.origin = next_origin; p.angles = next_angles;
                self.model_placement.placed_dirty = true;
            },
            EditSelection::Bmodel(i) => if let Some(p) = self.model_placement.bmodel_placed.get_mut(i) {
                p.hit = next_origin; p.angles = next_angles;
                self.model_placement.bmodel_collision_dirty = true;
            },
            EditSelection::Fx(i) => if let Some(p) = self.model_placement.fx_placed.get_mut(i) {
                p.origin = next_origin; p.angles = next_angles;
            },
        }
        if let Some(drag) = self.model_placement.editor_drag.as_mut() { drag.changed = true; }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn duplicate_selected_placement(&mut self) {
        let Some(selected) = self.model_placement.editor_selected else { return; };
        self.placement_checkpoint();
        let state = &mut self.model_placement;
        state.editor_selected = match selected {
            EditSelection::Md3(i) => state.placed.get(i).cloned().map(|p| {
                state.placed.push(p); state.placed_dirty = true; EditSelection::Md3(state.placed.len()-1)
            }),
            EditSelection::Bmodel(i) => state.bmodel_placed.get(i).cloned().map(|p| {
                state.bmodel_placed.push(p); state.bmodel_collision_dirty = true;
                EditSelection::Bmodel(state.bmodel_placed.len()-1)
            }),
            EditSelection::Fx(i) => state.fx_placed.get(i).cloned().map(|p| {
                state.fx_placed.push(p); EditSelection::Fx(state.fx_placed.len()-1)
            }),
        };
        if matches!(selected, EditSelection::Bmodel(_)) { self.ensure_inline_instance_capacity(); }
    }

    pub(in crate::app) fn delete_hovered_placement(&mut self) {
        if !self.model_placement.edit_mode { return; }
        let selected = self.model_placement.editor_selected
            .or_else(|| self.model_placement.bmodel_hover.map(EditSelection::Bmodel))
            .or_else(|| self.model_placement.fx_hover.map(EditSelection::Fx))
            .or_else(|| self.model_placement.hover.map(EditSelection::Md3));
        let Some(selected) = selected else { return; };
        self.placement_checkpoint();
        let state = &mut self.model_placement;
        match selected {
            EditSelection::Md3(i) if i < state.placed.len() => {
                state.placed.remove(i); state.placed_dirty = true;
            }
            EditSelection::Bmodel(i) if i < state.bmodel_placed.len() => {
                state.bmodel_placed.remove(i); state.bmodel_collision_dirty = true;
            }
            EditSelection::Fx(i) if i < state.fx_placed.len() => { state.fx_placed.remove(i); }
            _ => {}
        }
        state.editor_selected = None;
        state.hover = None;
        state.bmodel_hover = None;
        state.fx_hover = None;
        state.status = "Object deleted (Ctrl+Z to undo)".into();
    }

    /// E switches between explicitly enabled editing and asset placement.
    pub(in crate::app) fn toggle_placement_edit(&mut self) {
        if self.model_placement.edit_mode {
            self.model_placement.status = "Map editing stays active. Open Map Mods and choose EXIT MAP EDITING.".into();
        } else { self.enter_placement_edit(); }
    }

    pub(in crate::app) fn exit_map_editing(&mut self) {
        if let Some(drag) = self.model_placement.editor_drag.take() {
            if drag.changed {
                let was_dirty = drag.before.ent_dirty;
                self.apply_placement_snapshot(drag.before);
                self.model_placement.ent_dirty = was_dirty;
            }
        }
        self.model_placement.editor_mouse_owned = None;
        self.model_placement.editor_selected = None;
        self.model_placement.editor_placing = false;
        self.model_placement.edit_mode = false;
        self.model_placement.active = false;
        self.model_placement.rotating = false;
        self.model_placement.preview_rotation = None;
        self.model_placement.hit_valid = false;
        self.model_placement.preview_key = None;
        self.model_placement.preview_draws = Arc::new(Vec::new());
    }

    /// Esc only cancels the current interaction or deselects. The editing
    /// session stays active until Exit Map Editing is explicitly used.
    pub(in crate::app) fn cancel_placement(&mut self) {
        if self.model_placement.edit_mode {
            if let Some(drag) = self.model_placement.editor_drag.take() {
                let was_dirty = drag.before.ent_dirty;
                self.apply_placement_snapshot(drag.before);
                self.model_placement.ent_dirty = was_dirty;
            } else {
                self.model_placement.editor_selected = None;
            }
            self.model_placement.editor_mouse_owned = None;
            self.model_placement.status = "Editing — select another object, or exit explicitly".into();
            return;
        }
        self.exit_map_editing();
    }

    /// Choose an effect from the browser: it follows the crosshair as a live
    /// preview until placed.
    pub(in crate::app) fn choose_placement_effect(&mut self, qpath: String) {
        let state = &mut self.model_placement;
        state.kind = PlacementKind::Fx;
        state.editor_placing = state.edit_mode;
        state.editor_selected = None;
        state.hover = None;
        state.bmodel_hover = None;
        state.fx_hover = None;
        state.hover_since = None;
        state.held = None;
        state.fx_held = None;
        state.selection = None;
        state.bounds = None;
        state.fx_selection = Some(fx_runner_name(&qpath));
        state.fx_delay = 200.0;
        // Straight up, like an unaimed fx_runner.
        state.angles = [270.0, 0.0, 0.0];
        state.active = true;
        state.rotating = false;
        state.preview_rotation = None;
        state.hit_valid = false;
        state.preview_key = None;
        state.preview_draws = Arc::new(Vec::new());
        state.status.clear();
        self.set_overlay(OverlayMode::None);
    }

    pub(in crate::app) fn stamp_placement(&mut self) {
        if !self.model_placement.active || !self.model_placement.hit_valid { return; }
        self.placement_checkpoint();
        if self.model_placement.kind == PlacementKind::Fx {
            let state = &mut self.model_placement;
            let Some(effect) = state.fx_selection.clone() else { return; };
            state.fx_placed.push(FxPlacement {
                effect, origin: state.origin, angles: state.angles,
                delay_ms: state.fx_delay.round() as i32,
            });
            state.status = format!("{} effect(s) placed", state.fx_placed.len());
            if state.edit_mode {
                // Dropped: back to picking things up.
                state.kind = PlacementKind::Md3;
                state.fx_selection = None;
                state.fx_held = None;
                state.hit_valid = false;
            }
            return;
        }
        if self.model_placement.kind == PlacementKind::Bmodel {
            if let (Some(model), Some(bounds)) = (self.model_placement.bmodel_selection,
                self.model_placement.bmodel_bounds) {
                self.model_placement.bmodel_placed.push(BmodelPlacement {
                    model, bounds, pivot: self.model_placement.bmodel_pivot,
                    hit: self.model_placement.origin,
                    angles: self.model_placement.angles,
                });
                self.model_placement.bmodel_collision_dirty = true;
                self.model_placement.status = format!("{} BSP model(s) placed",
                    self.model_placement.bmodel_placed.len());
                self.ensure_inline_instance_capacity();
            }
            return;
        }
        let (Some(qpath), Some(bounds)) = (
            self.model_placement.selection.clone(), self.model_placement.bounds) else { return; };
        self.model_placement.placed.push(ModelPlacement {
            qpath, origin: self.model_placement.origin,
            angles: self.model_placement.angles, scale: self.model_placement.scale, bounds,
        });
        self.model_placement.placed_dirty = true;
        self.model_placement.status = format!("{} model(s) placed", self.model_placement.placed.len());
        if self.model_placement.edit_mode {
            // Dropped: back to picking things up.
            let state = &mut self.model_placement;
            state.selection = None;
            state.bounds = None;
            state.held = None;
            state.hit_valid = false;
            state.preview_key = None;
            state.preview_draws = Arc::new(Vec::new());
        }
    }

    pub(in crate::app) fn undo_model_placement(&mut self) {
        let Some(before) = self.model_placement.edit_undo.pop() else { return; };
        let current = self.placement_snapshot();
        self.model_placement.edit_redo.push(current);
        self.apply_placement_snapshot(before);
        self.model_placement.status = "Undo".into();
    }

    /// Keep exact CM_TransformedBoxTrace/CM_TransformedPointContents copies
    /// in the in-process server and the client's shared collision world.
    /// Never change collision on a remote server: a local visual map mod is
    /// not an authoritative server entity and would desync prediction.
    fn sync_bmodel_collision(&mut self) {
        let local = self.local_server.is_some();
        if !self.model_placement.bmodel_collision_dirty
            && self.model_placement.bmodel_collision_local == local { return; }
        let Some(world) = self.map_collision.as_ref() else { return; };
        let placed = if local {
            self.model_placement.bmodel_placed.iter().filter_map(|p| {
                let instance = p.instance();
                i32::try_from(p.model).ok().map(|index| jka_movement::PlacedInlineCollision {
                    index, origin: instance.origin, angles: p.angles,
                })
            }).collect()
        } else {
            Vec::new()
        };
        world.set_placed_inline_models(placed);
        self.model_placement.bmodel_collision_local = local;
        self.model_placement.bmodel_collision_dirty = false;
    }

    /// Changes whenever a staged model or effect is added, moved or removed, so
    /// the `r_drawEntities` markers rebuild exactly then.
    pub(in crate::app) fn placement_marker_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let state = &self.model_placement;
        for placement in &state.placed {
            placement.qpath.hash(&mut hasher);
            for value in placement.origin.iter().chain(&placement.angles) {
                value.to_bits().hash(&mut hasher);
            }
            placement.scale.to_bits().hash(&mut hasher);
        }
        for placement in &state.fx_placed {
            placement.effect.hash(&mut hasher);
            for value in placement.origin.iter().chain(&placement.angles) {
                value.to_bits().hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Adds the staged models (green oriented-bounds boxes) and effects
    /// (magenta box with an arrow along their direction) to the
    /// `r_drawEntities` marker mesh.
    pub(in crate::app) fn push_placement_markers(&self, mesh: &mut jka_assets::bsp::DebugVolumeMesh) {
        const MODEL_COLOR: [u8; 4] = [0x6C, 0xE0, 0x7A, 0xFF];
        const FX_COLOR: [u8; 4] = [0xFF, 0x7A, 0xD8, 0xFF];
        for placement in &self.model_placement.placed {
            let corners = placement_corners(placement);
            let mut mins = [f32::INFINITY; 3];
            let mut maxs = [f32::NEG_INFINITY; 3];
            for corner in corners {
                for axis in 0..3 {
                    mins[axis] = mins[axis].min(corner[axis]);
                    maxs[axis] = maxs[axis].max(corner[axis]);
                }
            }
            mesh.push_box(mins, maxs, MODEL_COLOR);
        }
        for placement in &self.model_placement.fx_placed {
            let origin = placement.origin;
            let half = 10.0;
            mesh.push_box(
                std::array::from_fn(|j| origin[j] - half),
                std::array::from_fn(|j| origin[j] + half),
                FX_COLOR,
            );
            let forward = model_axis(placement.angles, 1.0)[0];
            mesh.push_line(origin, std::array::from_fn(|j| origin[j] + forward[j] * 72.0), FX_COLOR);
        }
    }

    /// Labels for the staged placements in the `r_drawEntities` view.
    pub(in crate::app) fn placement_marker_labels(&self) -> Vec<([f32; 3], String, [u8; 3])> {
        let mut labels = Vec::new();
        for placement in &self.model_placement.placed {
            let leaf = placement.qpath.rsplit('/').next().unwrap_or(&placement.qpath);
            let leaf = leaf.strip_suffix(".md3").unwrap_or(leaf);
            labels.push((placement.origin, leaf.to_owned(), [0x6C, 0xE0, 0x7A]));
        }
        for placement in &self.model_placement.fx_placed {
            labels.push((placement.origin, format!("fx: {}", placement.effect), [0xFF, 0x7A, 0xD8]));
        }
        labels
    }

    /// Runs the placed effects (and the one in hand) through the placement
    /// tool's own FX scheduler and tessellates this frame's particles. Cheap
    /// when nothing is placed.
    fn update_placement_fx(&mut self) {
        let want_ghost = self.model_placement.kind == PlacementKind::Fx
            && self.model_placement.active && self.model_placement.hit_valid
            && self.model_placement.fx_selection.is_some();
        if self.model_placement.fx_placed.is_empty() && !want_ghost {
            if !self.model_placement.fx_draws.is_empty() {
                self.model_placement.fx_draws = Arc::new(Vec::new());
            }
            return;
        }
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.model_placement.status = error;
            return;
        }
        if self.model_placement.fx_runtime.is_none() {
            self.model_placement.fx_runtime = Some(PlacementFx {
                system: crate::fx::system::FxSystem::new(),
                started: Instant::now(),
                ids: HashMap::new(),
                next: Vec::new(),
                ghost_next: 0,
            });
            if let Some(runtime) = self.model_placement.fx_runtime.as_mut() {
                runtime.system.reset_time(0);
            }
        }
        // Effects not yet known to the scheduler are registered from the VFS.
        let mut wanted: Vec<String> = self.model_placement.fx_placed.iter()
            .map(|fx| fx.effect.clone()).collect();
        if let Some(effect) = self.model_placement.fx_selection.clone() {
            wanted.push(effect);
        }
        let mut assets = None;
        for effect in wanted {
            let known = self.model_placement.fx_runtime.as_ref()
                .is_some_and(|runtime| runtime.ids.contains_key(&effect));
            if known { continue; }
            if assets.is_none() {
                match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                    Ok(mut opened) => {
                        opened.set_allow_asset_overrides(self.video.allow_asset_overrides);
                        assets = Some(opened);
                    }
                    Err(error) => {
                        self.model_placement.status = format!("FX VFS ERROR: {error}");
                        return;
                    }
                }
            }
            let reader = assets.as_mut().expect("opened above");
            let runtime = self.model_placement.fx_runtime.as_mut().expect("created above");
            let id = runtime.system.register(&effect, &mut |qpath| {
                reader.read(qpath, 32 * 1024 * 1024).ok().flatten().map(|asset| asset.bytes)
            });
            if id == 0 {
                self.model_placement.status = format!("Could not load effect {effect}");
            }
            runtime.ids.insert(effect, id);
        }
        let state = &mut self.model_placement;
        let Some(runtime) = state.fx_runtime.as_mut() else { return; };
        let now_ms = (runtime.started.elapsed().as_millis().min(i32::MAX as u128) as i32).max(1);
        runtime.system.adjust_time(now_ms);
        runtime.next.resize(state.fx_placed.len(), 0);
        let forward = |angles: [f32; 3]| model_axis(angles, 1.0)[0];
        for (index, placement) in state.fx_placed.iter().enumerate() {
            let id = runtime.ids.get(&placement.effect).copied().unwrap_or(0);
            if id != 0 && now_ms >= runtime.next[index] {
                runtime.system.play_effect_dir(id, placement.origin, forward(placement.angles));
                runtime.next[index] = now_ms.saturating_add(placement.delay_ms.max(50));
            }
        }
        if want_ghost {
            if let Some(effect) = state.fx_selection.as_ref() {
                let id = runtime.ids.get(effect).copied().unwrap_or(0);
                if id != 0 && now_ms >= runtime.ghost_next {
                    runtime.system.play_effect_dir(id, state.origin, forward(state.angles));
                    runtime.ghost_next = now_ms.saturating_add((state.fx_delay as i32).max(50));
                }
            }
        }
        let frame = runtime.system.frame();
        let view = crate::fx::draw::FxView::from_camera(&self.camera);
        let Some(presenter) = self.asset_preview_entity_presenter.as_mut() else { return; };
        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.model_placement.fx_draws = Arc::new(surfaces);
    }

    /// Browser audition uses the real gameplay presentation path, not a fake
    /// raster FX icon. It stays independent of active placement/undo history.
    fn update_placement_picker_preview(&mut self) {
        if self.overlay != OverlayMode::ModelPicker {
            self.model_placement.picker_preview = None;
            self.model_placement.picker_hit = None;
            self.model_placement.picker_draw_key = None;
            if !self.model_placement.picker_draws.is_empty() {
                self.model_placement.picker_draws = Arc::new(Vec::new());
            }
            return;
        }
        let Some((kind, qpath)) = self.model_placement.picker_preview.clone() else {
            self.model_placement.picker_hit = None;
            self.model_placement.picker_draw_key = None;
            self.model_placement.picker_draws = Arc::new(Vec::new());
            return;
        };
        // Same camera ray/trace mask as actual placement. The world remains
        // fully rendered, with the browser anchored away from the crosshair.
        use jka_movement::TraceWorld;
        let start = scene::jka_position(self.camera.position.to_array());
        let direction = scene::jka_position(self.camera.forward().to_array());
        let end = std::array::from_fn(|i| start[i] + direction[i] * 131_072.0);
        let hit = self.map_collision.as_mut().and_then(|world| {
            let trace = world.trace(jka_movement::TraceQuery {
                start, end, mins: [0.0; 3], maxs: [0.0; 3],
                pass_entity: -1, mask: 0x1 | 0x1000,
            });
            (trace.fraction < 1.0 && trace.start_solid == 0).then_some(trace.end)
        });
        // The Asset Viewer does not require aiming at BSP, either. Keep the
        // browser usable if the camera faces sky or a very distant wall.
        let hit = hit.filter(|point| {
            let d2: f32 = (0..3).map(|i| (point[i] - start[i]).powi(2)).sum();
            d2 <= 600.0 * 600.0
        }).or_else(|| Some(std::array::from_fn(|i| start[i] + direction[i] * 220.0)));
        self.model_placement.picker_hit = hit;
        if kind != AssetKind::Md3 || hit.is_none() {
            self.model_placement.picker_draw_key = None;
            self.model_placement.picker_draws = Arc::new(Vec::new());
            return; // EFX gets its own isolated, Asset Viewer-style scheduler.
        }
        let key = (qpath.clone(), hit.unwrap(),
            (self.model_placement.picker_yaw * 10.0).round() as i32,
            (self.model_placement.picker_scale * 1000.0).round() as i32);
        if self.model_placement.picker_draw_key.as_ref() == Some(&key) { return; }
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.model_placement.status = error;
            return;
        }
        let presenter = self.asset_preview_entity_presenter.as_mut().unwrap();
        match presenter.present_static_md3(1023, &qpath, hit.unwrap(),
                model_axis([0.0, self.model_placement.picker_yaw, 0.0],
                    self.model_placement.picker_scale), [1.0; 4], 0) {
            Ok(draws) => {
                self.model_placement.picker_draws = Arc::new(draws);
                self.model_placement.picker_draw_key = Some(key);
            }
            Err(error) => {
                self.model_placement.picker_draws = Arc::new(Vec::new());
                self.model_placement.picker_draw_key = Some(key);
                self.model_placement.status = format!("Preview {qpath}: {error}");
            }
        }
    }

    /// Replay an EFX through the same authored scheduler and FX tessellation
    /// used by Asset Viewer, but in a *separate* map-browser runtime. Nothing
    /// in the placement scheduler, stamps, or undo history is touched.
    fn update_placement_picker_fx(&mut self) {
        let effect = if self.overlay == OverlayMode::ModelPicker {
            self.model_placement.picker_preview.as_ref().and_then(|(kind, qpath)|
                (*kind == AssetKind::Efx).then(|| fx_runner_name(qpath)))
        } else { None };
        let (Some(effect), Some(origin)) = (effect, self.model_placement.picker_hit) else {
            self.model_placement.picker_fx_runtime = None;
            return;
        };
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.model_placement.status = error;
            return;
        }
        let changed = self.model_placement.picker_fx_runtime.as_ref()
            .is_none_or(|state| !state.ids.contains_key(&effect));
        if changed {
            let mut assets = match jka_assets::pk3::AssetSearchPath::open_game(
                &self.base, self.game.as_deref()) {
                Ok(reader) => reader,
                Err(error) => {
                    self.model_placement.status = format!("EFX PREVIEW VFS: {error}");
                    return;
                }
            };
            assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
            let mut system = crate::fx::system::FxSystem::new();
            let id = system.register(&effect, &mut |path| {
                assets.read(path, 32 * 1024 * 1024).ok().flatten().map(|data| data.bytes)
            });
            if id == 0 {
                self.model_placement.status = format!("Could not preview {effect}");
                self.model_placement.picker_fx_runtime = None;
                return;
            }
            system.reset_time(0);
            let mut ids = HashMap::new();
            ids.insert(effect.clone(), id);
            self.model_placement.picker_fx_runtime = Some(PlacementFx {
                system, started: Instant::now(), ids, next: Vec::new(), ghost_next: 0,
            });
        }
        let runtime = self.model_placement.picker_fx_runtime.as_mut().unwrap();
        let now_ms = (runtime.started.elapsed().as_millis().min(i32::MAX as u128) as i32).max(1);
        runtime.system.adjust_time(now_ms);
        if now_ms >= runtime.ghost_next {
            let id = runtime.ids[&effect];
            runtime.system.play_effect_dir(id, origin, [0.0, 0.0, 1.0]);
            runtime.ghost_next = now_ms.saturating_add(1500);
        }
        let frame = runtime.system.frame();
        let view = crate::fx::draw::FxView::from_camera(&self.camera);
        let presenter = self.asset_preview_entity_presenter.as_mut().unwrap();
        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.model_placement.picker_draws = Arc::new(surfaces);
    }

    /// Called before the RenderSnapshot is published. Model vertices for fixed
    /// placements are built only on a stamp/undo, not once per game frame.
    pub(in crate::app) fn update_model_placement(&mut self) {
        let label = self.prepared_map_cache.as_ref().map(|map| map.label.as_str()).unwrap_or("");
        if self.model_placement.map_label != label {
            self.model_placement = ModelPlacementUi {
                map_label: label.to_owned(), ..ModelPlacementUi::default()
            };
        }
        self.sync_bmodel_collision();
        self.update_placement_picker_preview();
        self.update_placement_picker_fx();
        if self.model_placement.active && (!self.model_placement.edit_mode || self.model_placement.editor_placing) && !self.model_placement.rotating {
            use jka_movement::TraceWorld;
            let start = scene::jka_position(self.camera.position.to_array());
            // Use the center camera ray, never weapon muzzle/dynamic-crosshair
            // offsets: the visual gizmo and actual insertion point must agree.
            let direction = scene::jka_position(self.camera.forward().to_array());
            let end = std::array::from_fn(|i| start[i] + direction[i] * 131_072.0);
            self.model_placement.hit_valid = false;
            if let Some(world) = self.map_collision.as_mut() {
                let trace = world.trace(jka_movement::TraceQuery {
                    start, end, mins: [0.0; 3], maxs: [0.0; 3],
                    pass_entity: -1, mask: 0x1 | 0x1000,
                });
                if trace.fraction < 1.0 && trace.start_solid == 0 {
                    self.model_placement.hit_valid = true;
                    self.model_placement.origin = trace.end;
                }
            }
        }
        if self.model_placement.edit_mode && !self.model_placement.editor_placing {
            // Pick from the camera ray, respecting solid BSP geometry. Editing a
            // currently selected item never removes it from rendering.
            let start = scene::jka_position(self.camera.position.to_array());
            let direction = scene::jka_position(self.camera.forward().to_array());
            let (mut hit_limit, mut model, mut bmodel, mut fx) = (f32::INFINITY, None, None, None);
            if let Some(world) = self.map_collision.as_mut() {
                use jka_movement::TraceWorld;
                let end = std::array::from_fn(|i| start[i] + direction[i] * 131_072.0);
                let hit = world.trace(jka_movement::TraceQuery { start, end,
                    mins: [0.0; 3], maxs: [0.0; 3], pass_entity: -1,
                    mask: 0x1 | 0x1000 });
                if hit.fraction < 1.0 { hit_limit = hit.fraction * 131_072.0 + 4.0; }
            }
            let state = &self.model_placement;
            for (i,p) in state.placed.iter().enumerate() {
                if let Some(distance) = ray_hits_placement(start, direction, p) {
                    if distance < hit_limit { hit_limit = distance; model = Some(i); bmodel = None; fx = None; }
                }
            }
            for (i,p) in state.bmodel_placed.iter().enumerate() {
                let proxy = ModelPlacement { qpath: String::new(), origin: p.instance().origin,
                    angles: p.angles, scale: 1.0, bounds: p.bounds };
                if let Some(distance) = ray_hits_placement(start, direction, &proxy) {
                    if distance < hit_limit { hit_limit = distance; model = None; bmodel = Some(i); fx = None; }
                }
            }
            for (i,p) in state.fx_placed.iter().enumerate() {
                if let Some(distance) = ray_hits_sphere(start, direction, p.origin, FX_PICK_RADIUS) {
                    if distance < hit_limit + 32.0 {
                        hit_limit = distance; model = None; bmodel = None; fx = Some(i);
                    }
                }
            }
            let state = &mut self.model_placement;
            if (state.hover, state.bmodel_hover, state.fx_hover) != (model, bmodel, fx) {
                state.hover_since = if model.is_some() || bmodel.is_some() || fx.is_some() {
                    Some(Instant::now()) } else { None };
            }
            state.hover = model;
            state.bmodel_hover = bmodel;
            state.fx_hover = fx;
        }
        self.update_placement_fx();
        if self.model_placement.kind == PlacementKind::Bmodel {
            self.model_placement.preview_draws = Arc::new(Vec::new());
            self.model_placement.preview_key = None;
            if !self.model_placement.placed_dirty { return; }
        }
        if !self.model_placement.active || !self.model_placement.hit_valid {
            self.model_placement.preview_draws = Arc::new(Vec::new());
            self.model_placement.preview_key = None;
        }
        if !self.model_placement.placed_dirty
            && (!self.model_placement.active || !self.model_placement.hit_valid
                || self.model_placement.selection.is_none()) {
            if !self.model_placement.active || !self.model_placement.hit_valid {
                self.model_placement.preview_draws = Arc::new(Vec::new());
                self.model_placement.preview_key = None;
            }
            return;
        }
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.model_placement.status = error;
            return;
        }
        let presenter = self.asset_preview_entity_presenter.as_mut().unwrap();
        if self.model_placement.placed_dirty {
            let mut surfaces = Vec::new();
            for placement in &self.model_placement.placed {
                match presenter.present_static_md3(
                    1023, &placement.qpath, placement.origin,
                    model_axis(placement.angles, placement.scale), [1.0; 4], 0,
                ) {
                    Ok(draws) => surfaces.extend(draws),
                    Err(error) => self.model_placement.status = error,
                }
            }
            self.model_placement.placed_draws = Arc::new(surfaces);
            self.model_placement.placed_dirty = false;
        }
        if self.model_placement.kind == PlacementKind::Md3
            && self.model_placement.active && self.model_placement.hit_valid
            && self.model_placement.selection.is_some() && self.model_placement.bounds.is_some() {
            let placement = ModelPlacement {
                qpath: self.model_placement.selection.as_ref().unwrap().clone(),
                origin: self.model_placement.origin,
                angles: self.model_placement.angles, scale: self.model_placement.scale,
                bounds: self.model_placement.bounds.unwrap(),
            };
            if self.model_placement.preview_key.as_ref() != Some(&placement) {
                match presenter.present_static_md3(
                    1023, &placement.qpath, placement.origin,
                    model_axis(placement.angles, placement.scale), [1.0; 4], 0,
                ) {
                    Ok(draws) => self.model_placement.preview_draws = Arc::new(draws),
                    Err(error) => {
                        self.model_placement.preview_draws = Arc::new(Vec::new());
                        self.model_placement.status = error;
                    }
                }
                self.model_placement.preview_key = Some(placement);
            }
        }
    }

    pub(super) fn egui_model_picker(&mut self, root: &mut egui::Ui) {
        self.ensure_asset_viewer_catalog();
        let mut selected = None;
        let mut selected_effect = None;
        let mut close = false;
        let mut bmodels = false;
        let mut image_requests = Vec::new();
        let state = &mut self.model_placement;
        egui::Window::new("ADD TO MAP · ASSET BROWSER")
            .id(egui::Id::new("md3_model_placement_catalog"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(18.0, 72.0))
            .default_width(690.0).default_height(560.0)
            .resizable(true).collapsible(false)
            .show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.heading("ADD TO MAP");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("CLOSE").clicked() { close = true; }
                    });
                });
                ui.horizontal(|ui| {
                    let md3_clicked = ui.selectable_value(&mut state.picker_effects, false, "MD3 MODELS").clicked();
                    let fx_clicked = ui.selectable_value(&mut state.picker_effects, true, "EFFECTS (.efx)").clicked();
                    if md3_clicked || fx_clicked { state.picker_preview = None; }
                    if ui.button("BSP BMODELS (*N)").clicked() { bmodels = true; }
                });
                if state.picker_effects {
                    ui.label("Hover PREVIEW to play the authored EFX live at the crosshair (behind this pane). PLACE creates a movable fx_runner ghost; wheel sets its repeat delay.");
                } else {
                    ui.label("Hover PREVIEW to inspect the MD3 with its real materials in the map. PLACE starts placement; thumbnails show textured frame-zero geometry when available.");
                }
                if !state.picker_effects {
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut state.picker_yaw, 0.0..=360.0)
                            .text("Preview yaw"));
                        ui.add(egui::Slider::new(&mut state.picker_scale, 0.1..=5.0)
                            .text("Preview size"));
                        if ui.small_button("RESET VIEW").clicked() {
                            state.picker_yaw = 0.0;
                            state.picker_scale = 1.0;
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.search)
                        .hint_text("Filter by filename or folder...").desired_width(420.0));
                    egui::ComboBox::from_id_salt("model_sort")
                        .selected_text(match state.sort { 1 => "Z → A", 2 => "Folder", _ => "A → Z" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.sort, 0, "A → Z");
                            ui.selectable_value(&mut state.sort, 1, "Z → A");
                            ui.selectable_value(&mut state.sort, 2, "Folder");
                        });
                });
                if self.ui_catalog.pending.asset_viewer {
                    ui.label("Scanning loaded assets on the catalog worker...");
                    return;
                }
                if let Some(error) = &self.asset_viewer_catalog_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                    return;
                }
                let query = state.search.trim().to_ascii_lowercase();
                let wanted_kind = if state.picker_effects { AssetKind::Efx } else { AssetKind::Md3 };
                let mut models: Vec<_> = self.asset_viewer_entries.iter()
                    .filter(|entry| entry.kind == wanted_kind)
                    .filter(|entry| query.is_empty() || entry.qpath.to_ascii_lowercase().contains(&query))
                    .collect();
                match state.sort {
                    1 => models.sort_unstable_by(|a,b| b.qpath.cmp(&a.qpath)),
                    2 => models.sort_unstable_by(|a,b| a.folder.cmp(&b.folder).then(a.leaf.cmp(&b.leaf))),
                    _ => models.sort_unstable_by(|a,b| a.qpath.cmp(&b.qpath)),
                }
                if state.picker_effects {
                    ui.label(format!("{} matching effects   •   {} already placed", models.len(), state.fx_placed.len()));
                } else {
                    ui.label(format!("{} matching MD3s   •   {} already placed", models.len(), state.placed.len()));
                }
                ui.separator();
                egui::ScrollArea::vertical().id_salt("model_placement_results")
                    .auto_shrink([false, false]).show_rows(ui, 74.0, models.len(), |ui, rows| {
                        for i in rows {
                            let entry = models[i];
                            let kind = entry.kind;
                            let qpath = &entry.qpath;
                            let row = ui.horizontal(|ui| {
                                if kind == AssetKind::Efx {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(60.0, 60.0), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 4.0, theme::CONTROL);
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                        "▶ FX", egui::FontId::monospace(12.0), theme::TEXT_DIM);
                                } else if let Some(texture) = state.thumbnails.get(qpath) {
                                    ui.add(egui::Image::new(texture)
                                        .fit_to_exact_size(egui::vec2(60.0, 60.0)));
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(60.0, 60.0), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 4.0, theme::CONTROL);
                                    if !state.thumbnails_missing.contains(qpath)
                                        && state.thumbnails_pending.insert(qpath.clone()) {
                                        image_requests.push(qpath.clone());
                                    }
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                        "MD3", egui::FontId::monospace(12.0), theme::TEXT_DIM);
                                }
                                ui.vertical(|ui| {
                                    ui.label(qpath.as_str());
                                    ui.horizontal(|ui| {
                                        if ui.small_button("PREVIEW").clicked() {
                                            state.picker_preview = Some((kind, qpath.clone()));
                                        }
                                        if ui.small_button("PLACE").clicked() {
                                            if kind == AssetKind::Efx {
                                                selected_effect = Some(qpath.clone());
                                            } else {
                                                selected = Some(qpath.clone());
                                            }
                                        }
                                        if state.picker_preview.as_ref().is_some_and(|v|
                                            v.0 == kind && v.1.as_str() == qpath.as_str()) {
                                            ui.small("● LIVE AT CROSSHAIR");
                                        }
                                    });
                                });
                            });
                            // Hovering rows auditions immediately, without ever
                            // mutating saved or in-hand placements.
                            if row.response.hovered()
                                && state.picker_preview.as_ref().is_none_or(|v|
                                    v.0 != kind || v.1.as_str() != qpath.as_str()) {
                                state.picker_preview = Some((kind, qpath.clone()));
                            }
                        }
                    });
                if !state.status.is_empty() { ui.label(&state.status); }
                ui.separator();
                ui.label("Place: LMB · Size: wheel · Rotate: RMB · Ctrl+Z/Y undo/redo · E edit · Tab browser");
            });
        for qpath in image_requests {
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ModelThumbnail { qpath });
        }
        if close { self.set_overlay(OverlayMode::None); }
        if bmodels { self.open_bmodel_placement_picker(); }
        if let Some(qpath) = selected { self.choose_placement_model(qpath); }
        if let Some(qpath) = selected_effect { self.choose_placement_effect(qpath); }
    }

    /// Screen-space projected oriented bounds; coordinates originate in JKA
    /// model space, not a made-up AABB about the instance origin.
    pub(super) fn egui_model_placement_hud(&mut self, root: &mut egui::Ui) {
        if !self.model_placement.active { return; }
        let Some(window) = self.window.as_ref() else { return; };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 { return; }
        let state = &self.model_placement;
        let painter = root.painter();
        let pixels_per_point = root.ctx().pixels_per_point().max(0.001);
        let project = |point: [f32; 3]| {
            self.camera.project_to_screen(size.width, size.height,
                glam::Vec3::from_array(scene::render_position(point)))
                .map(|p| egui::pos2(p.x / pixels_per_point, p.y / pixels_per_point))
        };
        let draw_box = |corners: [[f32; 3]; 8], color: egui::Color32| {
            let projected: [Option<egui::Pos2>; 8] = std::array::from_fn(|i| project(corners[i]));
            for i in 0..8 {
                for bit in [1, 2, 4] {
                    if i & bit != 0 { continue; }
                    if let (Some(a), Some(b)) = (projected[i], projected[i | bit]) {
                        painter.line_segment([a, b], egui::Stroke::new(1.7, color));
                    }
                }
            }
        };
        // An effect: a ring at its origin and a line along the way it plays.
        let draw_fx = |origin: [f32; 3], angles: [f32; 3], color: egui::Color32| {
            let Some(a) = project(origin) else { return; };
            painter.circle_stroke(a, 7.0, egui::Stroke::new(1.7, color));
            let forward = model_axis(angles, 1.0)[0];
            let tip = std::array::from_fn(|j| origin[j] + forward[j] * 72.0);
            if let Some(b) = project(tip) {
                painter.line_segment([a, b], egui::Stroke::new(1.7, color));
                painter.circle_filled(b, 3.0, color);
            }
        };
        // Picking and placing use the exact screen-centre ray. In third person the
        // game's own crosshair follows the weapon aim and sits elsewhere, so mark
        // the real centre: aim with this reticle, not the game crosshair.
        {
            let center = egui::pos2(
                size.width as f32 / pixels_per_point * 0.5,
                size.height as f32 / pixels_per_point * 0.5,
            );
            let color = if state.editor_drag.is_some() {
                egui::Color32::from_rgb(92, 204, 235)
            } else if state.hover.is_some() || state.bmodel_hover.is_some() || state.fx_hover.is_some() {
                egui::Color32::from_rgb(255, 214, 90)
            } else {
                egui::Color32::from_rgb(255, 255, 255)
            };
            painter.circle_stroke(center, 10.0, egui::Stroke::new(1.5, color));
            for (dx, dy) in [(-16.0, 0.0), (16.0, 0.0), (0.0, -16.0), (0.0, 16.0)] {
                let near = center + egui::vec2(dx * 0.62, dy * 0.62);
                painter.line_segment([near, center + egui::vec2(dx, dy)], egui::Stroke::new(1.5, color));
            }
        }
        if state.hit_valid && (!state.edit_mode || state.editor_placing) {
            let color = if state.rotating { egui::Color32::from_rgb(255, 179, 73) }
                else { egui::Color32::from_rgb(76, 220, 191) };
            match state.kind {
                PlacementKind::Md3 => {
                    if let (Some(qpath), Some(bounds)) = (&state.selection, state.bounds) {
                        draw_box(placement_corners(&ModelPlacement {
                            qpath: qpath.clone(), origin: state.origin,
                            scale: state.scale, angles: state.angles, bounds,
                        }), color);
                    }
                }
                PlacementKind::Bmodel => {
                    if let (Some(model), Some(bounds)) = (state.bmodel_selection, state.bmodel_bounds) {
                        draw_box(bmodel_corners(&BmodelPlacement {
                            model, pivot: state.bmodel_pivot, hit: state.origin,
                            angles: state.angles, bounds,
                        }), color);
                    }
                }
                PlacementKind::Fx => {
                    if state.fx_selection.is_some() { draw_fx(state.origin, state.angles, color); }
                }
            }
            if state.kind != PlacementKind::Fx {
                if let Some(center) = project(state.origin) {
                    painter.circle_filled(center, 3.0, color);
                }
            }
        }
        let edit_idle = state.edit_mode && !state.editor_placing;
        if edit_idle {
            for placement in &state.fx_placed {
                draw_fx(placement.origin, placement.angles,
                    egui::Color32::from_rgba_unmultiplied(255, 122, 216, 150));
            }
        }
        let hovered = state.hover.and_then(|index| state.placed.get(index).map(|p| (index, p)));
        let hovered_fx = state.fx_hover.and_then(|index| state.fx_placed.get(index).map(|p| (index, p)));
        let gold = egui::Color32::from_rgb(255, 214, 90);
        if let Some((_, placement)) = hovered { draw_box(placement_corners(placement), gold); }
        if let Some((_, placement)) = hovered_fx { draw_fx(placement.origin, placement.angles, gold); }
        if let Some(index) = state.bmodel_hover {
            if let Some(p) = state.bmodel_placed.get(index) { draw_box(bmodel_corners(p), gold); }
        }
        let cyan = egui::Color32::from_rgb(92, 204, 235);
        match state.editor_selected {
            Some(EditSelection::Md3(i)) => if let Some(p) = state.placed.get(i) {
                draw_box(placement_corners(p), cyan);
            },
            Some(EditSelection::Bmodel(i)) => if let Some(p) = state.bmodel_placed.get(i) {
                draw_box(bmodel_corners(p), cyan);
            },
            Some(EditSelection::Fx(i)) => if let Some(p) = state.fx_placed.get(i) {
                draw_fx(p.origin, p.angles, cyan);
            },
            _ => {}
        }
        if state.edit_mode && state.hover_since.is_some_and(|since|
            since.elapsed().as_millis() >= 500) {
            let hovered_name = hovered.map(|(_, p)| p.qpath.clone())
                .or_else(|| hovered_fx.map(|(_, p)| p.effect.clone()))
                .or_else(|| state.bmodel_hover.and_then(|i| state.bmodel_placed.get(i))
                    .map(|p| format!("BMODEL *{}", p.model)));
            if let Some(name) = hovered_name {
                let screen_center = egui::pos2(size.width as f32 / pixels_per_point * 0.5,
                    size.height as f32 / pixels_per_point * 0.5);
                painter.text(screen_center + egui::vec2(20.0, 20.0),
                    egui::Align2::LEFT_TOP, name, egui::FontId::monospace(11.0), theme::ACCENT);
            }
        }
        let bounds = root.max_rect();
        let top = bounds.min + egui::vec2(20.0, 20.0);
        painter.rect_filled(egui::Rect::from_min_size(top, egui::vec2(690.0, 78.0)),
            5.0, egui::Color32::from_black_alpha(188));
        painter.text(top + egui::vec2(12.0, 11.0), egui::Align2::LEFT_TOP,
            match state.kind {
                _ if state.editor_placing => match state.kind {
                    PlacementKind::Md3 => format!("ADD MD3   {}", state.selection.as_deref().unwrap_or("")),
                    PlacementKind::Bmodel => format!("ADD BMODEL   *{}", state.bmodel_selection.unwrap_or(0)),
                    PlacementKind::Fx => format!("ADD FX   {}", state.fx_selection.as_deref().unwrap_or("")),
                },
                _ if edit_idle && state.editor_selected.is_some() => {
                    match state.editor_selected.unwrap() {
                        EditSelection::Md3(i) => state.placed.get(i)
                            .map(|p| format!("EDIT SELECTED   MD3 #{i}   {}", p.qpath))
                            .unwrap_or_else(|| "EDIT SELECTED".into()),
                        EditSelection::Bmodel(i) => state.bmodel_placed.get(i)
                            .map(|p| format!("EDIT SELECTED   BMODEL #{i} *{}", p.model))
                            .unwrap_or_else(|| "EDIT SELECTED".into()),
                        EditSelection::Fx(i) => state.fx_placed.get(i)
                            .map(|p| format!("EDIT SELECTED   FX #{i}   {}", p.effect))
                            .unwrap_or_else(|| "EDIT SELECTED".into()),
                    }
                }
                _ if edit_idle => match (hovered, hovered_fx) {
                    (_, Some((index, placement))) => format!("EDIT   FX #{index}   {}", placement.effect),
                    (Some((index, placement)), _) => format!("EDIT   MD3 #{index}   {}", placement.qpath),
                    _ if state.bmodel_hover.is_some() => format!("EDIT   BMODEL *{}", state.bmodel_placed[state.bmodel_hover.unwrap()].model),
                    _ => "EDIT   (aim at MD3, BMODEL or FX)".to_owned(),
                },
                PlacementKind::Md3 if state.edit_mode => format!("MD3 EDIT   holding   {}", state.selection.as_deref().unwrap_or("")),
                PlacementKind::Md3 => format!("MD3 PLACEMENT   {}", state.selection.as_deref().unwrap_or("")),
                PlacementKind::Bmodel => format!("BMODEL PLACEMENT   *{}", state.bmodel_selection.unwrap_or(0)),
                PlacementKind::Fx if state.edit_mode => format!("FX EDIT   holding   {}", state.fx_selection.as_deref().unwrap_or("")),
                PlacementKind::Fx => format!("FX PLACEMENT   {}", state.fx_selection.as_deref().unwrap_or("")),
            },
            egui::FontId::monospace(12.0), theme::TEXT);
        painter.text(top + egui::vec2(12.0, 34.0), egui::Align2::LEFT_TOP,
            if let Some((_, placement)) = hovered_fx {
                format!("Origin {:.0} {:.0} {:.0}   Pitch {:.0}°  Yaw {:.0}°  Roll {:.0}°   Delay {} ms   {} fx",
                    placement.origin[0], placement.origin[1], placement.origin[2],
                    placement.angles[0], placement.angles[1], placement.angles[2],
                    placement.delay_ms, state.fx_placed.len())
            } else if let Some((_, placement)) = hovered {
                format!("Origin {:.0} {:.0} {:.0}   Scale {:.3}×   Pitch {:.0}°  Yaw {:.0}°  Roll {:.0}°   {} placed",
                    placement.origin[0], placement.origin[1], placement.origin[2], placement.scale,
                    placement.angles[0], placement.angles[1], placement.angles[2], state.placed.len())
            } else if edit_idle {
                format!("{} models · {} fx · {} fx surfaces drawn · {} effects loaded   {}",
                    state.placed.len(), state.fx_placed.len(), state.fx_draws.len(),
                    state.fx_runtime.as_ref().map_or(0, |runtime| runtime.ids.values().filter(|&&id| id != 0).count()),
                    state.status)
            } else if state.kind == PlacementKind::Fx {
                format!("Delay {:.0} ms   Pitch {:.1}°   Yaw {:.1}°   Roll {:.1}°  {}",
                    state.fx_delay, state.angles[0], state.angles[1], state.angles[2],
                    if !state.hit_valid { "NO SURFACE" } else if state.rotating { "ROTATE" } else { "READY" })
            } else {
                format!("{}   Pitch {:.1}°   Yaw {:.1}°   Roll {:.1}°  {}",
                    if state.kind == PlacementKind::Md3 { format!("Scale {:.3}×", state.scale) } else { "BMODEL 1:1 (scale locked)".into() },
                    state.angles[0], state.angles[1], state.angles[2],
                    if !state.hit_valid { "NO SURFACE" } else if state.rotating { "ROTATE" } else { "READY" })
            },
            egui::FontId::monospace(11.0), theme::ACCENT);
        painter.text(top + egui::vec2(12.0, 56.0), egui::Align2::LEFT_TOP,
            if edit_idle {
                "Ctrl+LMB empty: ADD · LMB drag · Ctrl+drag vertical · MMB yaw/pitch · RMB local roll · Esc: Menu"
            } else if state.editor_placing {
                "LMB Place once · RMB orient (Alt local roll) · Wheel adjust · Backspace cancel · Esc menu"
            } else if state.kind == PlacementKind::Fx {
                "LMB Place · Wheel Repeat delay · RMB Rotate (Shift pitch, Alt local roll) · Tab Browser · Ctrl+Z Undo"
            } else if state.kind == PlacementKind::Md3 {
                "LMB Place · Wheel Size · RMB Yaw · Shift Pitch · Alt Local Roll · Tab Browser · Ctrl+Z Undo"
            } else {
                "LMB Place · RMB Yaw · Shift Pitch · Alt Local Roll · Tab Browser · Ctrl+Z Undo (scale locked)"
            },
            egui::FontId::monospace(9.0), theme::TEXT_DIM);
    }
}

impl App {
    pub(super) fn egui_bmodel_picker(&mut self, root: &mut egui::Ui) {
        let Some(map) = self.prepared_map_cache.as_ref().map(|cache| Arc::clone(&cache.map)) else {
            return;
        };
        let mut picked = None;
        let mut close = false;
        let mut models = false;
        let state = &mut self.model_placement;
        egui::Window::new("ADD TO MAP · BSP BMODELS")
            .id(egui::Id::new("bsp_bmodel_placement_catalog"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(18.0, 72.0))
            .default_width(680.0).default_height(560.0)
            .resizable(true).collapsible(false).show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.heading("ADD BSP BMODEL");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("CLOSE").clicked() { close = true; }
                    });
                });
                if ui.button("MD3 MODELS / EFFECTS (.efx)").clicked() { models = true; }
                ui.label("Re-use any drawable BSP inline *N model from this map. Thumbnails are sampled wireframes of its actual geometry.");
                ui.small("Native brush collision is active in the local game. On remote servers these placements remain visual-only until loaded server-side.");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.bmodel_search)
                        .hint_text("Filter by *number or entity classname...").desired_width(420.0));
                    egui::ComboBox::from_id_salt("bmodel_sort")
                        .selected_text(if state.bmodel_sort == 1 { "Reverse number" } else { "Model number" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.bmodel_sort, 0, "Model number");
                            ui.selectable_value(&mut state.bmodel_sort, 1, "Reverse number");
                        });
                });
                let query = state.bmodel_search.trim().to_ascii_lowercase();
                let mut available: Vec<_> = map.inline_models.iter()
                    .filter(|model| model.model > 0)
                    .filter(|model| bmodel_pivot(&map, model.model).is_some())
                    .collect();
                available.sort_unstable_by_key(|model| model.model);
                available.dedup_by_key(|model| model.model);
                if state.bmodel_sort == 1 { available.reverse(); }
                let filtered: Vec<_> = available.into_iter().filter(|model| {
                    let label = format!("*{}", model.model);
                    label.contains(&query) || map.brush_entities.iter()
                        .filter(|entity| entity.model == model.model)
                        .flat_map(|entity| entity.spawn_vars.iter())
                        .any(|(key, value)| key.eq_ignore_ascii_case("classname")
                            && value.to_ascii_lowercase().contains(&query))
                }).collect();
                ui.label(format!("{} matching submodels   •   {} placed",
                    filtered.len(), state.bmodel_placed.len()));
                ui.separator();
                egui::ScrollArea::vertical().id_salt("bmodel_placement_results")
                    .auto_shrink([false, false]).show_rows(ui, 52.0, filtered.len(), |ui, rows| {
                        for index in rows {
                            let model = filtered[index];
                            let Some((pivot, bounds)) = bmodel_pivot(&map, model.model) else { continue; };
                            let classname = map.brush_entities.iter()
                                .find(|entity| entity.model == model.model)
                                .and_then(|entity| entity.spawn_vars.iter().find(|(key, _)| key.eq_ignore_ascii_case("classname")))
                                .map_or("(unclassified)", |(_, value)| value.as_str());
                            ui.horizontal(|ui| {
                                paint_bmodel_thumbnail(ui, &map, model, bounds);
                                ui.vertical(|ui| {
                                    if ui.button(format!("*{} · {classname}", model.model)).clicked() {
                                        picked = Some(model.model);
                                    }
                                    ui.small(format!("Origin: {:.0}, {:.0}, {:.0}  ·  {:.0} × {:.0} × {:.0} u",
                                        pivot[0], pivot[1], pivot[2],
                                        bounds.1[0] - bounds.0[0],
                                        bounds.1[1] - bounds.0[1],
                                        bounds.1[2] - bounds.0[2]));
                                });
                            });
                        }
                    });
                ui.separator();
                ui.label("Original BSP pivot is preserved. Translation/rotation use the pivot; BMODEL scaling is disabled for server compatibility.");
                ui.label("Choose a BSP inline model, aim at a surface, and left-click to place. Esc closes the browser.");
                if !state.status.is_empty() { ui.label(&state.status); }
            });
        if close { self.set_overlay(OverlayMode::None); }
        if models { self.open_model_placement_picker(); }
        if let Some(model) = picked { self.choose_placement_bmodel(model); }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmodel_placement_cancels_original_world_pivot() {
        let placed = BmodelPlacement { model: 6,
            pivot: [1000.0, 100.0, 50.0], hit: [20.0, 30.0, 40.0],
            angles: [0.0; 3],
            bounds: ([990.0, 90.0, 40.0], [1010.0, 110.0, 60.0]),
        };
        let pose = placed.instance();
        let rotated = pose.rotate(placed.pivot);
        for axis in 0..3 {
            assert!((pose.origin[axis] + rotated[axis] - placed.hit[axis]).abs() < 0.001);
        }
        let rotated_placed = BmodelPlacement { angles: [20.0, 65.0, 45.0], ..placed };
        let pose = rotated_placed.instance();
        let rotated = pose.rotate(rotated_placed.pivot);
        for axis in 0..3 {
            assert!((pose.origin[axis] + rotated[axis] - rotated_placed.hit[axis]).abs() < 0.001);
        }
    }

    #[test]
    fn quaternion_matches_jka_model_axis_in_arbitrary_pose() {
        for angles in [[0.0, 0.0, 0.0], [22.0, 107.0, 53.0],
            [-82.0, 221.0, 300.0], [90.0, 45.0, 10.0]] {
            let q = placement_orientation(angles);
            let axes = model_axis(angles, 1.0);
            for i in 0..3 {
                let basis = match i { 0 => glam::Vec3::X, 1 => glam::Vec3::Y, _ => glam::Vec3::Z };
                let rotated = (q * basis).to_array();
                for k in 0..3 {
                    assert!((rotated[k] - axes[i][k]).abs() < 1.0e-4,
                        "basis {i} component {k}, angles {angles:?}");
                }
            }
        }
    }

    #[test]
    fn yaw_pitch_keep_v1_euler_behavior_even_on_a_rolled_object() {
        let initial = [31.0, 123.0, 71.0];
        let result = placement_rotate_yaw_pitch(initial, -7.0, 15.0);
        assert_eq!(result, [24.0, 138.0, 71.0]);
        assert_eq!(placement_rotate_yaw_pitch([359.0, 357.0, 90.0], 5.0, 8.0),
            [4.0, 5.0, 90.0]);
    }

    #[test]
    fn local_roll_after_euler_yaw_pitch_keeps_forward_axis() {
        let initial = placement_rotate_yaw_pitch([34.0, 68.0, 24.0], -17.0, 23.0);
        let before = model_axis(initial, 1.0)[0];
        let q = placement_rotate_local(placement_orientation(initial), 0.0, 0.0, 32.0);
        let after = model_axis(placement_rotation_angles(q), 1.0)[0];
        for i in 0..3 {
            assert!((before[i] - after[i]).abs() < 0.0001);
        }
    }

    #[test]
    fn local_rotation_keeps_the_selected_object_axis_fixed() {
        let initial = [24.0, 116.0, 51.0];
        let start = model_axis(initial, 1.0);
        for (axis, pitch, yaw, roll) in [(0, 0.0, 0.0, 34.0),
            (1, 33.0, 0.0, 0.0), (2, 0.0, 40.0, 0.0)] {
            let q = placement_rotate_local(placement_orientation(initial), pitch, yaw, roll);
            let final_axes = model_axis(placement_rotation_angles(q), 1.0);
            for k in 0..3 {
                assert!((start[axis][k] - final_axes[axis][k]).abs() < 1.0e-4,
                    "local axis {axis} component {k} moved");
            }
        }
    }

    #[test]
    fn oriented_bounds_scale_about_model_origin() {
        let mut model = ModelPlacement {
            qpath: "models/map_objects/test.md3".into(),
            origin: [100.0, 20.0, -5.0], angles: [0.0; 3], scale: 1.0,
            bounds: ([-2.0, -4.0, -6.0], [2.0, 4.0, 6.0]),
        };
        let corners = placement_corners(&model);
        assert_eq!(corners[0], [98.0, 16.0, -11.0]);
        assert_eq!(corners[7], [102.0, 24.0, 1.0]);
        model.scale = 2.0;
        let doubled = placement_corners(&model);
        assert_eq!(doubled[0], [96.0, 12.0, -17.0]);
        assert_eq!(doubled[7], [104.0, 28.0, 7.0]);
    }
}
