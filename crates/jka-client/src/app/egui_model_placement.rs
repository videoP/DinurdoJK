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
    /// Edit mode: with no model in hand the crosshair picks existing
    /// placements up (move / copy / delete) instead of stamping a new one.
    pub edit_mode: bool,
    /// Placement under the crosshair in edit mode.
    pub hover: Option<usize>,
    /// The original of a picked-up placement, restored by Esc.
    pub held: Option<ModelPlacement>,
    /// Whole-list snapshots (models, effects) taken before each edit-mode
    /// pickup or delete.
    pub edit_undo: Vec<(Vec<ModelPlacement>, Vec<FxPlacement>)>,
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
    pub search: String,
    pub sort: usize,
    pub active: bool,
    pub rotating: bool,
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
            ent_filename: String::new(), ent_overwrite: false, ent_status: String::new(),
            edit_mode: false, hover: None, held: None, edit_undo: Vec::new(),
            fx_placed: Vec::new(), fx_selection: None, fx_delay: 200.0, fx_held: None,
            fx_hover: None, fx_runtime: None, fx_draws: Arc::new(Vec::new()),
            picker_effects: false,
            search: String::new(), sort: 0, active: false, rotating: false,
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
        self.model_placement.kind = PlacementKind::Md3;
        self.model_placement.active = false;
        self.model_placement.rotating = false;
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
        self.model_placement.kind = PlacementKind::Bmodel;
        self.model_placement.active = false;
        self.model_placement.rotating = false;
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
        self.model_placement.bmodel_selection = Some(model);
        self.model_placement.bmodel_pivot = pivot;
        self.model_placement.bmodel_bounds = Some(bounds);
        self.model_placement.scale = 1.0;
        self.model_placement.angles = [0.0; 3];
        self.model_placement.hit_valid = false;
        self.model_placement.rotating = false;
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
        // Choosing a model from the browser is "place new", not "edit".
        self.model_placement.edit_mode = false;
        self.model_placement.held = None;
        self.model_placement.fx_selection = None;
        self.model_placement.fx_held = None;
        self.model_placement.selection = Some(qpath);
        self.model_placement.scale = 1.0;
        self.model_placement.angles = [0.0; 3];
        self.model_placement.bounds = Some(bounds);
        self.model_placement.active = true;
        self.model_placement.rotating = false;
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
        state.active = true;
        state.rotating = false;
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

    /// Primary click. In edit mode with nothing in hand it picks up the
    /// placement under the crosshair (`copy` keeps the original); otherwise it
    /// stamps the model in hand.
    pub(in crate::app) fn placement_click(&mut self, copy: bool) {
        let state = &mut self.model_placement;
        if !(state.edit_mode && state.selection.is_none() && state.fx_selection.is_none()
            && state.kind == PlacementKind::Md3) {
            self.stamp_placement();
            return;
        }
        // An effect under the crosshair: pick it up the same way.
        if let Some(index) = state.fx_hover.filter(|&index| index < state.fx_placed.len()) {
            state.edit_undo.push((state.placed.clone(), state.fx_placed.clone()));
            let picked = if copy { state.fx_placed[index].clone() } else { state.fx_placed.remove(index) };
            state.fx_held = (!copy).then(|| picked.clone());
            state.fx_selection = Some(picked.effect);
            state.fx_delay = picked.delay_ms as f32;
            state.angles = picked.angles;
            state.origin = picked.origin;
            state.kind = PlacementKind::Fx;
            state.fx_hover = None;
            state.hit_valid = false;
            state.status = if copy { "Effect copy picked up: click to place".into() }
                else { "Effect picked up: click to drop (Esc puts it back)".into() };
            return;
        }
        let Some(index) = state.hover.filter(|&index| index < state.placed.len()) else { return; };
        state.edit_undo.push((state.placed.clone(), state.fx_placed.clone()));
        let picked = if copy { state.placed[index].clone() } else { state.placed.remove(index) };
        state.held = (!copy).then(|| picked.clone());
        state.selection = Some(picked.qpath);
        state.bounds = Some(picked.bounds);
        state.scale = picked.scale;
        state.angles = picked.angles;
        state.origin = picked.origin;
        state.hover = None;
        state.hit_valid = false;
        state.preview_key = None;
        state.placed_dirty = true;
        state.status = if copy { "Copy picked up: click to place".into() }
            else { "Picked up: click to drop (Esc puts it back)".into() };
    }

    /// Delete the placement under the crosshair (edit mode, nothing in hand).
    pub(in crate::app) fn delete_hovered_placement(&mut self) {
        let state = &mut self.model_placement;
        if !(state.edit_mode && state.selection.is_none() && state.fx_selection.is_none()) { return; }
        if let Some(index) = state.fx_hover.filter(|&index| index < state.fx_placed.len()) {
            state.edit_undo.push((state.placed.clone(), state.fx_placed.clone()));
            let removed = state.fx_placed.remove(index);
            state.fx_hover = None;
            state.status = format!("Deleted effect {} ({} left; Ctrl+Z undoes)", removed.effect, state.fx_placed.len());
            return;
        }
        let Some(index) = state.hover.filter(|&index| index < state.placed.len()) else { return; };
        state.edit_undo.push((state.placed.clone(), state.fx_placed.clone()));
        let removed = state.placed.remove(index);
        state.hover = None;
        state.placed_dirty = true;
        state.status = format!("Deleted {} ({} left; Ctrl+Z undoes)", removed.qpath, state.placed.len());
    }

    /// `E` toggles between placing a model and editing placed ones.
    pub(in crate::app) fn toggle_placement_edit(&mut self) {
        if self.model_placement.edit_mode {
            self.model_placement.edit_mode = false;
            self.cancel_placement();
        } else {
            self.enter_placement_edit();
        }
    }

    pub(in crate::app) fn cancel_placement(&mut self) {
        // Edit mode with a picked-up model or effect: Esc puts it back and stays
        // in edit mode.
        if self.model_placement.edit_mode
            && (self.model_placement.selection.is_some() || self.model_placement.fx_selection.is_some()) {
            let state = &mut self.model_placement;
            if let Some(original) = state.held.take() {
                state.placed.push(original);
            }
            if let Some(original) = state.fx_held.take() {
                state.fx_placed.push(original);
            }
            state.kind = PlacementKind::Md3;
            state.fx_selection = None;
            state.selection = None;
            state.bounds = None;
            state.hit_valid = false;
            state.preview_key = None;
            state.preview_draws = Arc::new(Vec::new());
            state.placed_dirty = true;
            state.status = "Edit mode".into();
            self.egui_repaint_requested = true;
            return;
        }
        self.model_placement.edit_mode = false;
        self.model_placement.held = None;
        self.model_placement.hover = None;
        self.model_placement.fx_held = None;
        self.model_placement.fx_hover = None;
        self.model_placement.fx_selection = None;
        if self.model_placement.kind == PlacementKind::Fx {
            self.model_placement.kind = PlacementKind::Md3;
        }
        self.model_placement.active = false;
        self.model_placement.rotating = false;
        self.model_placement.hit_valid = false;
        self.model_placement.preview_key = None;
        self.model_placement.preview_draws = Arc::new(Vec::new());
        self.egui_repaint_requested = true;
    }

    /// Choose an effect from the browser: it follows the crosshair as a live
    /// preview until placed.
    pub(in crate::app) fn choose_placement_effect(&mut self, qpath: String) {
        let state = &mut self.model_placement;
        state.kind = PlacementKind::Fx;
        state.edit_mode = false;
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
        state.hit_valid = false;
        state.preview_key = None;
        state.preview_draws = Arc::new(Vec::new());
        state.status.clear();
        self.set_overlay(OverlayMode::None);
    }

    pub(in crate::app) fn stamp_placement(&mut self) {
        if !self.model_placement.active || !self.model_placement.hit_valid { return; }
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
        if self.model_placement.edit_mode {
            let state = &mut self.model_placement;
            if let Some((models, effects)) = state.edit_undo.pop() {
                state.placed = models;
                state.fx_placed = effects;
                state.fx_selection = None;
                state.fx_held = None;
                state.fx_hover = None;
                if state.kind == PlacementKind::Fx { state.kind = PlacementKind::Md3; }
                state.selection = None;
                state.bounds = None;
                state.held = None;
                state.hover = None;
                state.hit_valid = false;
                state.preview_key = None;
                state.preview_draws = Arc::new(Vec::new());
                state.placed_dirty = true;
                state.status = "Edit undone".into();
            }
            return;
        }
        if self.model_placement.kind == PlacementKind::Bmodel {
            if self.model_placement.bmodel_placed.pop().is_some() {
                self.model_placement.bmodel_collision_dirty = true;
                self.model_placement.status = "Last BMODEL placement undone".into();
            }
            return;
        }
        if self.model_placement.placed.pop().is_some() {
            self.model_placement.placed_dirty = true;
            self.model_placement.status = "Last placement undone".into();
        }
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
        if self.model_placement.active && !self.model_placement.rotating {
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
        if self.model_placement.edit_mode
            && self.model_placement.selection.is_none() && self.model_placement.fx_selection.is_none() {
            // Nothing in hand: find the model or effect under the crosshair
            // (whichever the ray meets first).
            let start = scene::jka_position(self.camera.position.to_array());
            let direction = scene::jka_position(self.camera.forward().to_array());
            let state = &mut self.model_placement;
            let model = state.placed.iter().enumerate()
                .filter_map(|(index, placement)| {
                    ray_hits_placement(start, direction, placement).map(|distance| (index, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let effect = state.fx_placed.iter().enumerate()
                .filter_map(|(index, placement)| {
                    ray_hits_sphere(start, direction, placement.origin, FX_PICK_RADIUS)
                        .map(|distance| (index, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            // Effect gizmos are small and often sit inside a model's bounds, so
            // an effect wins unless a model is clearly nearer.
            let effect_wins = match (model, effect) {
                (Some(m), Some(e)) => e.1 <= m.1 + 64.0,
                (None, Some(_)) => true,
                _ => false,
            };
            state.fx_hover = effect.filter(|_| effect_wins).map(|(index, _)| index);
            state.hover = model.filter(|_| !effect_wins).map(|(index, _)| index);
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
        let mut image_requests = Vec::new();
        let state = &mut self.model_placement;
        egui::Window::new("MD3 MODEL PLACEMENT")
            .id(egui::Id::new("md3_model_placement_catalog"))
            .default_width(690.0).default_height(560.0)
            .resizable(true).collapsible(false)
            .show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.heading("PLACE MODEL");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("CLOSE").clicked() { close = true; }
                    });
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut state.picker_effects, false, "MD3 MODELS");
                    ui.selectable_value(&mut state.picker_effects, true, "EFFECTS (.efx)");
                });
                if state.picker_effects {
                    ui.label("Choose an effect: it plays live at the crosshair. Place it as an fx_runner; aim with right-drag (Shift pitch, Alt roll), wheel sets its repeat delay.");
                } else {
                    ui.label("Choose any loaded MD3. Thumbnails show actual model geometry; the live world preview shows its authored materials.");
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
                    .auto_shrink([false, false]).show_rows(ui, 46.0, models.len(), |ui, rows| {
                        for i in rows {
                            let entry = models[i];
                            ui.horizontal(|ui| {
                                if state.picker_effects {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(34.0, 34.0), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 3.0, theme::CONTROL);
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                        "EFX", egui::FontId::monospace(10.0), theme::TEXT_DIM);
                                } else if let Some(texture) = state.thumbnails.get(&entry.qpath) {
                                    ui.add(egui::Image::new(texture)
                                        .fit_to_exact_size(egui::vec2(34.0, 34.0)));
                                } else {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(34.0, 34.0), egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 3.0, theme::CONTROL);
                                    if !state.thumbnails_missing.contains(&entry.qpath)
                                        && state.thumbnails_pending.insert(entry.qpath.clone()) {
                                        image_requests.push(entry.qpath.clone());
                                    }
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                        "MD3", egui::FontId::monospace(10.0), theme::TEXT_DIM);
                                }
                                if ui.button(&entry.qpath).clicked() {
                                    if state.picker_effects {
                                        selected_effect = Some(entry.qpath.clone());
                                    } else {
                                        selected = Some(entry.qpath.clone());
                                    }
                                }
                            });
                        }
                    });
                if !state.status.is_empty() { ui.label(&state.status); }
                ui.separator();
                ui.label("Place: left click · Size: wheel · Rotate: hold right click and move mouse · Undo: Ctrl+Z · Change model: Tab · Exit: Esc");
            });
        for qpath in image_requests {
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ModelThumbnail { qpath });
        }
        if close { self.set_overlay(OverlayMode::None); }
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
            let color = if state.hover.is_some() || state.fx_hover.is_some() {
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
        if state.hit_valid {
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
        // Edit mode, nothing in hand: show every effect, and outline the model or
        // effect under the crosshair.
        let edit_idle = state.edit_mode && state.selection.is_none()
            && state.fx_selection.is_none() && state.kind == PlacementKind::Md3;
        if edit_idle {
            for placement in &state.fx_placed {
                draw_fx(placement.origin, placement.angles,
                    egui::Color32::from_rgba_unmultiplied(255, 122, 216, 150));
            }
        }
        let hovered = state.hover.filter(|_| edit_idle)
            .and_then(|index| state.placed.get(index).map(|p| (index, p)));
        let hovered_fx = state.fx_hover.filter(|_| edit_idle)
            .and_then(|index| state.fx_placed.get(index).map(|p| (index, p)));
        let gold = egui::Color32::from_rgb(255, 214, 90);
        if let Some((_, placement)) = hovered { draw_box(placement_corners(placement), gold); }
        if let Some((_, placement)) = hovered_fx { draw_fx(placement.origin, placement.angles, gold); }
        let bounds = root.max_rect();
        let top = bounds.min + egui::vec2(20.0, 20.0);
        painter.rect_filled(egui::Rect::from_min_size(top, egui::vec2(690.0, 78.0)),
            5.0, egui::Color32::from_black_alpha(188));
        painter.text(top + egui::vec2(12.0, 11.0), egui::Align2::LEFT_TOP,
            match state.kind {
                _ if edit_idle => match (hovered, hovered_fx) {
                    (_, Some((index, placement))) => format!("EDIT   FX #{index}   {}", placement.effect),
                    (Some((index, placement)), _) => format!("EDIT   MD3 #{index}   {}", placement.qpath),
                    _ => "EDIT   (aim at a model or effect)".to_owned(),
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
                "LMB Pick up · Ctrl+LMB Copy · Del/X Delete · Ctrl+Z Undo · Tab Place new · E Place mode · Esc Exit"
            } else if state.kind == PlacementKind::Fx && state.edit_mode {
                "LMB Drop · Wheel Delay · RMB Aim · Shift Pitch · Alt Roll · Esc Put back · Ctrl+Z Undo edit"
            } else if state.kind == PlacementKind::Fx {
                "LMB Place · Wheel Repeat delay · RMB Aim (Shift pitch, Alt roll) · Tab Browser · E Edit placed · Ctrl+Z Undo · Esc Exit"
            } else if state.kind == PlacementKind::Md3 && state.edit_mode {
                "LMB Drop · Wheel Size · RMB Rotate · Shift Pitch · Alt Roll · Esc Put back · Ctrl+Z Undo edit"
            } else if state.kind == PlacementKind::Md3 {
                "LMB Place · Wheel Size · RMB Hold Pivot + Rotate · Shift Pitch · Alt Roll · Tab Browser · E Edit placed · Ctrl+Z Undo · Esc Exit"
            } else {
                "LMB Place · RMB Rotate · Shift Pitch · Alt Roll · Tab Browser · Ctrl+Z Undo · Esc Exit (scale locked)"
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
        let state = &mut self.model_placement;
        egui::Window::new("BMODEL PLACEMENT · BSP INLINE MODELS")
            .id(egui::Id::new("bsp_bmodel_placement_catalog"))
            .default_width(680.0).default_height(560.0)
            .resizable(true).collapsible(false).show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.heading("PLACE BMODEL");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("CLOSE").clicked() { close = true; }
                    });
                });
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
                ui.label("LMB Place · RMB Hold to Lock and Rotate · Tab Reopen · Ctrl+Z Undo · Esc Exit");
                if !state.status.is_empty() { ui.label(&state.status); }
            });
        if close { self.set_overlay(OverlayMode::None); }
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
