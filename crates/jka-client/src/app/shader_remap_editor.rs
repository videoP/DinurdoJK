//! Shader Remaps editor: state, winit input and the view model for the
//! native Taffy/WGPU page in `ui::shader_remap`. No egui.
//!
//! The page receives only what is on screen: visible tiles, rail rows and
//! remap rows, with thumbnails as atlas UVs. Hit testing reuses the page's
//! own `layout`/`hit`, so drawing and input share one set of rectangles.
//!
//! Remaps go through the existing local /remapShader path, which rebuilds the
//! world on change, so browsing never touches the renderer: only APPLY, a
//! double-click, Enter, a toggle or DELETE does. Thumbnails are static (editor
//! image or first texture stage) decoded on the ui-catalog worker; only
//! visible tiles request them, in-flight requests are capped, and the app
//! assigns the renderer's fixed atlas slots least-recently-used.
use super::*;
use crate::ui::kit::{EditResult, FieldEdit, FieldView, Thumb, Tone};
use crate::ui::shader_remap::{
    self as page, ActiveRowView, Field, Hit, InspectorView, Pane, RailRowView, ShaderRemapView, TileView,
};
use std::time::{Duration, Instant};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

const LARGE_SUFFIX: &str = "#large";
/// Grid decodes in flight at once. The worker is FIFO and shared, so fast
/// scrolling must not bury other catalog jobs under stale thumbnails.
const THUMB_INFLIGHT_CAP: usize = 32;
/// Results can be dropped (catalog generation bump); ask again after this.
const THUMB_RETRY: Duration = Duration::from_secs(4);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Pointer-driven republish cap; a full UI rebuild per raw mouse event is waste.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum SourceFilter {
    #[default]
    All,
    Remapped,
    Original,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum TargetKind {
    #[default]
    All,
    Scripted,
    Images,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum FolderScope {
    #[default]
    All,
    Map,
}

/// Index into `page::TILE_SIZES`; medium by default.
#[derive(Clone, Copy)]
struct TileSize(usize);

impl Default for TileSize {
    fn default() -> Self {
        Self(1)
    }
}

// ------------------------------------------------------------ thumbnails --

struct ThumbRequest {
    key: String,
    image_path: String,
    max_px: u32,
}

/// App-side owner of the renderer's thumbnail atlas slots: which shader is
/// in which slot, when it was last drawn, and which decodes are in flight.
#[derive(Default)]
pub(in crate::app) struct ThumbSlots {
    resident: HashMap<String, (usize, [u32; 2], u64)>,
    slots: Vec<Option<String>>,
    pending: HashMap<String, Instant>,
    missing: HashSet<String>,
    frame: u64,
    uploads: Vec<(usize, [u32; 2], Vec<u8>)>,
    /// The renderer holds an atlas that must be released with the page.
    allocated: bool,
}

impl ThumbSlots {
    fn begin_frame(&mut self) {
        self.frame += 1;
        let now = Instant::now();
        self.pending.retain(|_, asked| now.duration_since(*asked) < THUMB_RETRY);
    }

    fn get(&mut self, name: &str, large: bool, image_for: &dyn Fn(&str) -> String,
        out: &mut Vec<ThumbRequest>) -> Thumb {
        let key = if large { format!("{name}{LARGE_SUFFIX}") } else { name.to_owned() };
        if let Some((slot, size, used)) = self.resident.get_mut(&key) {
            *used = self.frame;
            return page::slot_thumb(*slot, *size);
        }
        if self.missing.contains(&key) {
            return Thumb::Missing;
        }
        if self.pending.contains_key(&key) || (!large && self.pending.len() >= THUMB_INFLIGHT_CAP) {
            return Thumb::Pending;
        }
        self.pending.insert(key.clone(), Instant::now());
        out.push(ThumbRequest {
            key,
            image_path: image_for(name),
            max_px: if large { page::LARGE } else { page::CELL },
        });
        Thumb::Pending
    }

    /// Place a decoded image in a free or least-recently-drawn slot. Slots
    /// drawn in the latest view are never reused, so an on-screen tile can
    /// not briefly show someone else's pixels.
    fn install(&mut self, key: String, image: Option<ui_catalog::DecodedImage>) -> bool {
        self.pending.remove(&key);
        let Some(image) = image else {
            return self.missing.insert(key);
        };
        let size = [image.size[0] as u32, image.size[1] as u32];
        let range = if key.ends_with(LARGE_SUFFIX) {
            page::SMALL_SLOTS..page::SLOTS
        } else {
            0..page::SMALL_SLOTS
        };
        if self.slots.len() < page::SLOTS {
            self.slots.resize(page::SLOTS, None);
        }
        let reuse = self.resident.get(&key).map(|(slot, _, _)| *slot);
        let slot = reuse
            .or_else(|| range.clone().find(|&i| self.slots[i].is_none()))
            .or_else(|| range.clone().filter_map(|i| {
                let used = self.slots[i].as_ref().and_then(|k| self.resident.get(k)).map_or(0, |r| r.2);
                (used < self.frame).then_some((used, i))
            }).min().map(|(_, i)| i));
        let Some(slot) = slot else { return false };
        let max = page::slot_max_px(slot);
        if size[0] == 0 || size[1] == 0 || size[0] > max || size[1] > max {
            return self.missing.insert(key);
        }
        if let Some(old) = self.slots[slot].take() {
            self.resident.remove(&old);
        }
        self.uploads.push((slot, size, image.rgba));
        self.resident.insert(key.clone(), (slot, size, self.frame));
        self.slots[slot] = Some(key);
        true
    }
}

// --------------------------------------------------------- target index --

fn parent_folder(name: &str) -> &str {
    name.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn leaf_name(name: &str) -> &str {
    name.rsplit_once('/').map_or(name, |(_, leaf)| leaf)
}

fn in_folder(name: &str, folder: &str) -> bool {
    name.len() > folder.len() && name.starts_with(folder) && name.as_bytes()[folder.len()] == b'/'
}

fn for_each_ancestor<'a>(folder: &'a str, mut visit: impl FnMut(&'a str)) {
    if folder.is_empty() {
        return;
    }
    for (slash, _) in folder.match_indices('/') {
        visit(&folder[..slash]);
    }
    visit(folder);
}

fn search_terms(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_ascii_lowercase).collect()
}

fn matches_terms(name: &str, terms: &[String]) -> bool {
    terms.iter().all(|term| name.contains(term.as_str()))
}

struct FolderRow {
    path: String,
    depth: usize,
    /// Targets in this folder or any descendant.
    count: u32,
    /// Holds (or is an ancestor of) one of this map's materials.
    in_map: bool,
}

/// Folder tree and per-target kind, rebuilt only when the name lists change.
#[derive(Default)]
struct TargetIndex {
    built_for: Option<(usize, usize)>,
    folders: Vec<FolderRow>,
    scripted: Vec<bool>,
}

impl TargetIndex {
    fn build(targets: &[String], sources: &[String], is_scripted: impl Fn(&str) -> bool) -> Self {
        let mut counts: HashMap<&str, u32> = HashMap::new();
        for name in targets {
            for_each_ancestor(parent_folder(name), |folder| *counts.entry(folder).or_default() += 1);
        }
        let mut used: HashSet<&str> = HashSet::new();
        for source in sources {
            for_each_ancestor(parent_folder(source), |folder| {
                used.insert(folder);
            });
        }
        let mut folders: Vec<FolderRow> = counts.into_iter().map(|(path, count)| FolderRow {
            depth: path.matches('/').count(),
            in_map: used.contains(path),
            path: path.to_owned(),
            count,
        }).collect();
        // Component order keeps every subtree contiguous ("a/b", "a/b/c",
        // "a/b-x"), which plain string order does not.
        folders.sort_by(|a, b| a.path.split('/').cmp(b.path.split('/')));
        Self {
            built_for: Some((targets.len(), sources.len())),
            folders,
            scripted: targets.iter().map(|name| is_scripted(name)).collect(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct ViewKey {
    search: String,
    folder: Option<String>,
    kind: TargetKind,
    len: usize,
}

/// Filtered target indices (sorted by name), rebuilt only on filter change.
#[derive(Default)]
struct TargetView {
    key: Option<ViewKey>,
    items: Vec<u32>,
}

#[derive(Clone)]
enum RailRow {
    All,
    Source(String),
    Folder(usize),
}

// ------------------------------------------------------------------ state --

#[derive(Default)]
pub(in crate::app) struct ShaderRemapUi {
    pub map_label: String,
    pub sources: Vec<String>,
    pub targets: Vec<String>,
    /// Implicit material names scanned from game-wide image assets.
    pub implicit_targets: Vec<String>,
    pub catalog_requested: bool,
    pub selected: Option<String>,
    /// Replacement staged for `selected`; only APPLY/double-click/Enter
    /// sends it to the renderer.
    pub target: String,
    /// Switching a redirect off removes it from the live renderer but keeps
    /// its target here, so switching it back on needs no re-picking.
    pub disabled: BTreeMap<String, String>,
    pub thumbs: ThumbSlots,
    status: String,
    status_tone: Tone,
    source_search: String,
    source_filter: SourceFilter,
    target_search: String,
    target_kind: TargetKind,
    /// Exact folder or any descendant; `None` = every folder.
    target_folder: Option<String>,
    folder_search: String,
    folder_scope: FolderScope,
    tile_size: TileSize,
    /// Header-only mode, to look at the remapped world behind the panel.
    hide_body: bool,
    scroll_to_selected: bool,
    index: TargetIndex,
    view: TargetView,
    source_first_row: usize,
    target_first_row: usize,
    folder_first: usize,
    active_first: usize,
    focus: Option<Field>,
    edit: FieldEdit,
    pointer: [f32; 2],
    hover: Option<Hit>,
    drag: Option<Pane>,
    last_target_click: Option<(String, Instant)>,
    /// What the page currently shows, and the lists its rows index into.
    native: Option<ShaderRemapView>,
    source_visible: Vec<usize>,
    rail: Vec<RailRow>,
    active_rows: Vec<(String, String, bool)>,
    remap_signature: u64,
    /// Window size of the last view; the layout depends on it.
    view_size: (u32, u32),
    dirty: bool,
    last_publish: Option<Instant>,
}

impl ShaderRemapUi {
    fn field_text(&mut self, field: Field) -> &mut String {
        match field {
            Field::SourceSearch => &mut self.source_search,
            Field::TargetSearch => &mut self.target_search,
            Field::FolderSearch => &mut self.folder_search,
            Field::Target => &mut self.target,
        }
    }

    fn field_view(&self, field: Field) -> FieldView {
        let text = match field {
            Field::SourceSearch => &self.source_search,
            Field::TargetSearch => &self.target_search,
            Field::FolderSearch => &self.folder_search,
            Field::Target => &self.target,
        };
        if self.focus == Some(field) {
            self.edit.view(text)
        } else {
            FieldView { text: text.clone(), caret: None, all_selected: false }
        }
    }

    fn set_status(&mut self, text: impl Into<String>, tone: Tone) {
        self.status = text.into();
        self.status_tone = tone;
    }
}

fn first_row_for(total_rows: usize, visible_rows: usize, first: usize) -> usize {
    first.min(total_rows.saturating_sub(visible_rows))
}

#[derive(Debug)]
enum RemapAction {
    Select(String),
    Stage(String),
    StageAndApply(String),
    Toggle(String, bool),
    Apply,
    Delete(String),
    Close,
}

impl App {
    pub(in crate::app) fn shader_remap_visible(&self) -> bool {
        self.overlay == OverlayMode::ShaderRemap
            || (self.overlay == OverlayMode::Console && self.overlay_before_console == OverlayMode::ShaderRemap)
    }

    /// The page for the UI snapshot; `None` while it is not showing.
    pub(in crate::app) fn shader_remap_snapshot(&self) -> Option<ShaderRemapView> {
        if self.shader_remap_visible() { self.shader_remap_ui.native.clone() } else { None }
    }

    /// Open from the Dev wheel or from one Trace record. Selecting an exact
    /// traced material works even when it is not in the BSP shader table
    /// (e.g. an MD3 surface registered by the entity presenter).
    pub(in crate::app) fn open_shader_remap(&mut self, source: Option<String>) {
        let map_label = self.prepared_map_cache.as_ref()
            .map(|cache| cache.label.clone()).unwrap_or_default();
        if self.shader_remap_ui.map_label != map_label {
            self.reset_shader_remap_ui();
            self.shader_remap_ui.map_label = map_label;
        }
        if let Some(cache) = self.prepared_map_cache.as_ref() {
            let ui = &mut self.shader_remap_ui;
            ui.sources = cache.map.material_debug.entries.iter()
                .map(|entry| scene::canonical_shader_name(&entry.name))
                .filter(|name| !name.is_empty()).collect();
            // Authored game shader definitions are parsed during map load;
            // this includes definitions unused by the current BSP.
            ui.targets = cache.map.shader_library.keys().cloned().collect();
            ui.targets.extend(ui.sources.iter().cloned());
            ui.targets.extend(ui.implicit_targets.iter().cloned());
        }
        for (name, target) in self.shader_remaps.redirects() {
            self.shader_remap_ui.sources.push(name.to_owned());
            self.shader_remap_ui.targets.push(target.to_owned());
        }
        for (name, target) in &self.shader_remap_ui.disabled {
            self.shader_remap_ui.sources.push(name.clone());
            self.shader_remap_ui.targets.push(target.clone());
        }
        let ui = &mut self.shader_remap_ui;
        ui.hide_body = false;
        ui.focus = None;
        ui.hover = None;
        ui.drag = None;
        // Sources may differ at an equal count; rebuild the folder index.
        ui.index = TargetIndex::default();
        if let Some(source) = source {
            let source = scene::canonical_shader_name(&source);
            if !source.is_empty() {
                ui.sources.push(source.clone());
                ui.source_search.clear();
                ui.source_filter = SourceFilter::All;
                ui.scroll_to_selected = true;
                self.select_shader_remap(source);
            }
        }
        let ui = &mut self.shader_remap_ui;
        ui.sources.sort_unstable();
        ui.sources.dedup();
        ui.targets.sort_unstable();
        ui.targets.dedup();
        // Scan implicit image-based shader names on the UI worker, not while
        // rendering. The compiled definition library above is usable at once.
        if !ui.catalog_requested {
            ui.catalog_requested = true;
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ShaderNames);
        }
        if self.overlay == OverlayMode::Trace {
            self.render_command(RenderCommand::ClearSurfaceInspection);
        }
        self.set_overlay(OverlayMode::ShaderRemap);
        self.refresh_shader_remap_view();
        self.publish_shader_remap();
    }

    /// Drop all page state (map change) and free the renderer's atlas.
    pub(in crate::app) fn reset_shader_remap_ui(&mut self) {
        if self.shader_remap_ui.thumbs.allocated {
            self.render_command(RenderCommand::RemapThumbsRelease);
        }
        self.shader_remap_ui = ShaderRemapUi::default();
    }

    /// Implicit image names arrived from the catalog worker.
    pub(in crate::app) fn install_remap_shader_names(&mut self, names: Vec<String>) {
        let ui = &mut self.shader_remap_ui;
        ui.implicit_targets = names;
        ui.targets.extend(ui.implicit_targets.iter().cloned());
        ui.targets.sort_unstable();
        ui.targets.dedup();
        ui.dirty = true;
    }

    /// One decoded thumbnail from the catalog worker.
    pub(in crate::app) fn install_remap_thumb(&mut self, map_label: String, key: String,
        image: Option<ui_catalog::DecodedImage>) {
        if map_label != self.shader_remap_ui.map_label || !self.shader_remap_visible() {
            self.shader_remap_ui.thumbs.pending.remove(&key);
            return;
        }
        if self.shader_remap_ui.thumbs.install(key, image) {
            self.shader_remap_ui.dirty = true;
        }
    }

    /// Per-tick upkeep: coalesce thumbnail arrivals and external remap
    /// changes into one republish, and free the atlas once the page closes.
    pub(in crate::app) fn tick_shader_remap(&mut self) {
        if !self.shader_remap_visible() {
            if self.shader_remap_ui.thumbs.allocated {
                let ui = &mut self.shader_remap_ui;
                ui.thumbs = ThumbSlots::default();
                ui.native = None;
                ui.focus = None;
                self.render_command(RenderCommand::RemapThumbsRelease);
            }
            return;
        }
        let signature = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for (source, target) in self.shader_remaps.redirects() {
                source.hash(&mut hasher);
                target.hash(&mut hasher);
            }
            self.shader_remap_ui.disabled.len().hash(&mut hasher);
            hasher.finish()
        };
        let size = self.window.as_ref().map(|w| w.inner_size()).map(|s| (s.width, s.height));
        let ui = &mut self.shader_remap_ui;
        if size.is_some_and(|size| size != ui.view_size) {
            ui.view_size = size.unwrap_or_default();
            ui.hover = None;
            ui.dirty = true;
        }
        if ui.remap_signature != signature {
            ui.remap_signature = signature;
            ui.dirty = true;
        }
        let now = Instant::now();
        if !ui.thumbs.pending.is_empty()
            && ui.thumbs.pending.values().any(|asked| now.duration_since(*asked) >= THUMB_RETRY) {
            ui.dirty = true;
        }
        if ui.dirty && ui.last_publish.is_none_or(|at| now.duration_since(at) >= PUBLISH_INTERVAL * 2) {
            self.refresh_shader_remap_view();
            self.publish_shader_remap();
        }
    }

    fn publish_shader_remap(&mut self) {
        self.shader_remap_ui.dirty = false;
        self.shader_remap_ui.last_publish = Some(Instant::now());
        self.publish_ui();
    }

    fn select_shader_remap(&mut self, source: String) {
        let source = scene::canonical_shader_name(&source);
        if source.is_empty() { return; }
        let target = self.shader_remap_ui.disabled.get(&source).cloned()
            .or_else(|| self.shader_remaps.redirects().find(|(name, _)| *name == source)
                .map(|(_, value)| value.to_owned()))
            .unwrap_or_else(|| source.clone());
        let ui = &mut self.shader_remap_ui;
        ui.selected = Some(source);
        ui.target = target;
        ui.status.clear();
        if ui.focus == Some(Field::Target) {
            ui.edit = FieldEdit::at_end(&ui.target);
        }
    }

    fn apply_remap_editor_action(&mut self, action: RemapAction) {
        if matches!(&action, RemapAction::Toggle(..) | RemapAction::Apply | RemapAction::Delete(..)
            | RemapAction::StageAndApply(..)) {
            self.placement_checkpoint();
        }
        match action {
            RemapAction::Close => self.set_overlay(OverlayMode::None),
            RemapAction::Select(source) => self.select_shader_remap(source),
            RemapAction::Stage(target) => {
                self.shader_remap_ui.target = target;
                self.shader_remap_ui.status.clear();
            }
            RemapAction::StageAndApply(target) => {
                self.shader_remap_ui.target = target;
                self.apply_staged_remap();
            }
            RemapAction::Toggle(source, enabled) => {
                if enabled {
                    if let Some(target) = self.shader_remap_ui.disabled.remove(&source) {
                        self.local_remap_shader(&source, &target);
                        self.model_placement.authored_remaps.insert(source.clone(), target);
                    }
                } else {
                    // Own the replacement before mutating `self`: the iterator
                    // otherwise keeps `shader_remaps` immutably borrowed.
                    let live_target = self.shader_remaps.redirects()
                        .find(|(name, _)| *name == source)
                        .map(|(_, target)| target.to_owned());
                    if let Some(target) = live_target {
                        self.shader_remap_ui.disabled.insert(source.clone(), target);
                        // old == new clears the live redirect in the existing
                        // renderer, preserving the original material unchanged.
                        self.local_remap_shader(&source, &source);
                        self.model_placement.authored_remaps.remove(&source);
                    }
                }
                if self.shader_remap_ui.selected.as_deref() == Some(&source) {
                    self.select_shader_remap(source);
                }
            }
            RemapAction::Apply => self.apply_staged_remap(),
            RemapAction::Delete(source) => {
                // Delete = no redirect at all: neither live nor remembered.
                self.shader_remap_ui.disabled.remove(&source);
                self.local_remap_shader(&source, &source);
                self.model_placement.authored_remaps.remove(&source);
                if self.shader_remap_ui.selected.as_deref() == Some(&source) {
                    self.shader_remap_ui.target = source.clone();
                }
                self.shader_remap_ui.set_status(format!("Deleted remap of {}", leaf_name(&source)), Tone::Dim);
            }
        }
    }

    fn apply_staged_remap(&mut self) {
        let Some(source) = self.shader_remap_ui.selected.clone() else { return; };
        let target = scene::canonical_shader_name(&self.shader_remap_ui.target);
        let ui = &mut self.shader_remap_ui;
        if target.is_empty() {
            ui.set_status("Pick a replacement tile or type a shader name.", Tone::Warning);
            return;
        }
        if ui.targets.binary_search(&target).is_err() && ui.sources.binary_search(&target).is_err() {
            ui.set_status(format!("Unknown shader: {target}"), Tone::Danger);
            return;
        }
        ui.target = target.clone();
        if ui.disabled.contains_key(&source) && source != target {
            ui.disabled.insert(source.clone(), target.clone());
            self.model_placement.authored_remaps.remove(&source);
            self.shader_remap_ui.set_status("Saved. Switch the redirect ON to apply it.", Tone::Warning);
            return;
        }
        ui.disabled.remove(&source);
        self.local_remap_shader(&source, &target);
        if source == target {
            self.model_placement.authored_remaps.remove(&source);
            self.shader_remap_ui.set_status("Original material restored.", Tone::Dim);
        } else {
            self.model_placement.authored_remaps.insert(source.clone(), target.clone());
            self.shader_remap_ui.set_status(format!("Applied » {}", leaf_name(&target)), Tone::Accent);
        }
    }

    fn remap_layout(&self) -> Option<page::Layout> {
        let size = self.window.as_ref()?.inner_size();
        Some(page::layout(size.width, size.height, self.shader_remap_ui.tile_size.0))
    }

    /// Rebuild the visible view model and queue any missing thumbnails.
    pub(in crate::app) fn refresh_shader_remap_view(&mut self) {
        let Some(l) = self.remap_layout() else { return };
        let remaps: BTreeMap<String, String> = self.shader_remaps.redirects()
            .map(|(source, target)| (source.to_owned(), target.to_owned())).collect();
        let map = self.prepared_map_cache.as_ref().map(|cache| cache.map.as_ref());
        // Candidate images per shader name, identical for map sources and
        // replacement targets, newline-separated for the worker to try in
        // order: editor image, every stage's map and first animMap frame,
        // then the name itself (implicit shaders are bare images).
        let image_for = |name: &str| -> String {
            let mut candidates: Vec<&str> = Vec::new();
            if let Some(shader) = map.and_then(|map| map.shader_library.get(name)) {
                if let Some(editor) = shader.definition_text.as_deref().and_then(|text| text.lines().find_map(|line| {
                    let mut parts = line.split_whitespace();
                    if parts.next()?.eq_ignore_ascii_case("qer_editorimage") { parts.next() } else { None }
                })) {
                    candidates.push(editor);
                }
                for stage in &shader.stages {
                    candidates.push(stage.image.as_str());
                    if let Some(frame) = stage.animation.as_ref().and_then(|anim| anim.frames.first()) {
                        candidates.push(frame.as_str());
                    }
                }
            }
            candidates.push(name);
            let mut seen = HashSet::new();
            candidates.into_iter()
                .filter(|path| !path.is_empty() && !path.starts_with('$') && seen.insert(*path))
                .take(8)
                .collect::<Vec<_>>()
                .join("
")
        };
        let mut requests = Vec::new();
        let state = &mut self.shader_remap_ui;
        state.thumbs.begin_frame();
        let map_name = if state.map_label.is_empty() { "No map" } else { state.map_label.as_str() };
        let mut v = ShaderRemapView {
            map_name: map_name.to_owned(),
            pointer: state.pointer,
            tile_size: state.tile_size.0,
            hide_body: state.hide_body,
            no_map: map.is_none(),
            stats: format!("{} materials   ·   {} active   ·   {} off   ·   {} replacements",
                state.sources.len(), remaps.len(), state.disabled.len(), state.targets.len()),
            source_search: state.field_view(Field::SourceSearch),
            target_search: state.field_view(Field::TargetSearch),
            folder_search: state.field_view(Field::FolderSearch),
            source_filter: state.source_filter as usize,
            target_kind: state.target_kind as usize,
            folder_scope: state.folder_scope as usize,
            ..ShaderRemapView::default()
        };
        if !state.hide_body && map.is_some() {
            if state.index.built_for != Some((state.targets.len(), state.sources.len())) {
                state.index = TargetIndex::build(&state.targets, &state.sources,
                    |name| map.is_some_and(|map| map.shader_library.contains_key(name)));
            }

            // ------------------------------------------- map materials --
            let terms = search_terms(&state.source_search);
            state.source_visible = state.sources.iter().enumerate().filter(|(_, source)| {
                let mapped = remaps.contains_key(*source) || state.disabled.contains_key(*source);
                let wanted = match state.source_filter {
                    SourceFilter::All => true,
                    SourceFilter::Remapped => mapped,
                    SourceFilter::Original => !mapped,
                };
                wanted && (matches_terms(source, &terms)
                    || remaps.get(*source).or_else(|| state.disabled.get(*source))
                        .is_some_and(|to| matches_terms(to, &terms)))
            }).map(|(index, _)| index).collect();
            let g = l.s_grid;
            let total_rows = state.source_visible.len().div_ceil(g.cols);
            if std::mem::take(&mut state.scroll_to_selected) {
                if let Some(position) = state.selected.as_ref().and_then(|selected|
                    state.source_visible.iter().position(|&i| &state.sources[i] == selected)) {
                    state.source_first_row = (position / g.cols).saturating_sub(g.rows / 2);
                }
            }
            state.source_first_row = first_row_for(total_rows, g.rows, state.source_first_row);
            v.source_first_row = state.source_first_row;
            v.source_rows = total_rows;
            v.source_detail = format!("{} / {}", state.source_visible.len(), state.sources.len());
            v.source_empty = "No materials match.".to_owned();
            let start = state.source_first_row * g.cols;
            for &index in state.source_visible.iter().skip(start).take(g.capacity()) {
                let name = &state.sources[index];
                let live = remaps.get(name);
                let off = state.disabled.get(name);
                let thumb = state.thumbs.get(name, false, &image_for, &mut requests);
                let inset = live.map(|to| (to, Tone::Accent)).or_else(|| off.map(|to| (to, Tone::Warning)))
                    .map(|(to, tone)| (state.thumbs.get(to, false, &image_for, &mut requests), tone));
                let (detail, detail_tone) = match (live, off) {
                    (Some(to), _) => (format!("» {}", leaf_name(to)), Tone::Accent),
                    (None, Some(to)) => (format!("OFF · {}", leaf_name(to)), Tone::Warning),
                    (None, None) => (parent_folder(name).to_owned(), Tone::Faint),
                };
                v.source_tiles.push(TileView {
                    name: leaf_name(name).to_owned(),
                    detail,
                    detail_tone,
                    selected: state.selected.as_deref() == Some(name.as_str()),
                    tag: None,
                    corner_tag: None,
                    thumb,
                    inset,
                });
            }

            // -------------------------------------------- folder rail --
            let folder_query = state.folder_search.trim().to_ascii_lowercase();
            state.rail = vec![RailRow::All];
            if let Some(folder) = state.selected.as_deref().map(parent_folder).filter(|f| !f.is_empty()) {
                state.rail.push(RailRow::Source(folder.to_owned()));
            }
            let scope = state.folder_scope;
            state.rail.extend(state.index.folders.iter().enumerate()
                .filter(|(_, row)| (scope == FolderScope::All || row.in_map)
                    && (folder_query.is_empty() || row.path.contains(&folder_query)))
                .map(|(index, _)| RailRow::Folder(index)));
            state.folder_first = first_row_for(state.rail.len(), l.rail_rows(), state.folder_first);
            v.folder_first = state.folder_first;
            v.folder_total = state.rail.len();
            for row in state.rail.iter().skip(state.folder_first).take(l.rail_rows()) {
                v.folders.push(match row {
                    RailRow::All => RailRowView {
                        label: "All folders".to_owned(),
                        count: state.targets.len() as u32,
                        selected: state.target_folder.is_none(),
                        ..RailRowView::default()
                    },
                    RailRow::Source(folder) => RailRowView {
                        label: format!("Source · {}", leaf_name(folder)),
                        count: state.index.folders.iter().find(|r| &r.path == folder).map_or(0, |r| r.count),
                        selected: state.target_folder.as_deref() == Some(folder.as_str()),
                        source: true,
                        ..RailRowView::default()
                    },
                    RailRow::Folder(f) => {
                        let row = &state.index.folders[*f];
                        RailRowView {
                            label: if folder_query.is_empty() { leaf_name(&row.path) } else { row.path.as_str() }
                                .to_owned(),
                            count: row.count,
                            depth: if folder_query.is_empty() { row.depth as u16 } else { 0 },
                            in_map: row.in_map,
                            selected: state.target_folder.as_deref() == Some(row.path.as_str()),
                            source: false,
                        }
                    }
                });
            }

            // -------------------------------------------- replacements --
            let key = ViewKey {
                search: state.target_search.trim().to_ascii_lowercase(),
                folder: state.target_folder.clone(),
                kind: state.target_kind,
                len: state.targets.len(),
            };
            if state.view.key.as_ref() != Some(&key) {
                let terms = search_terms(&key.search);
                let scripted = &state.index.scripted;
                state.view.items = state.targets.iter().enumerate().filter(|(i, name)| {
                    let is_scripted = scripted.get(*i).copied().unwrap_or(false);
                    let kind_ok = match key.kind {
                        TargetKind::All => true,
                        TargetKind::Scripted => is_scripted,
                        TargetKind::Images => !is_scripted,
                    };
                    kind_ok
                        && key.folder.as_deref().is_none_or(|folder| in_folder(name, folder))
                        && matches_terms(name, &terms)
                }).map(|(i, _)| i as u32).collect();
                state.view.key = Some(key);
                state.target_first_row = 0;
            }
            let staged = scene::canonical_shader_name(&state.target);
            let selected = state.selected.clone();
            let current = selected.as_ref().and_then(|s| remaps.get(s).or_else(|| state.disabled.get(s))).cloned();
            let g = l.t_grid;
            let total_rows = state.view.items.len().div_ceil(g.cols);
            state.target_first_row = first_row_for(total_rows, g.rows, state.target_first_row);
            v.target_first_row = state.target_first_row;
            v.target_rows = total_rows;
            v.target_detail = format!("{} shown · {}", state.view.items.len(),
                state.target_folder.as_deref().unwrap_or("all folders"));
            v.target_empty = if state.targets.is_empty() { "Scanning shader names..." }
                else { "No replacements match." }.to_owned();
            let start = state.target_first_row * g.cols;
            for &item in state.view.items.iter().skip(start).take(g.capacity()) {
                let index = item as usize;
                let name = &state.targets[index];
                let scripted = state.index.scripted.get(index).copied().unwrap_or(false);
                let tag = if current.as_deref() == Some(name.as_str()) {
                    Some(("CURRENT".to_owned(), Tone::Accent))
                } else if selected.as_deref() == Some(name.as_str()) {
                    Some(("ORIGINAL".to_owned(), Tone::Dim))
                } else {
                    None
                };
                let thumb = state.thumbs.get(name, false, &image_for, &mut requests);
                v.target_tiles.push(TileView {
                    name: leaf_name(name).to_owned(),
                    detail: parent_folder(name).to_owned(),
                    detail_tone: Tone::Faint,
                    selected: staged == *name,
                    tag,
                    corner_tag: scripted.then(|| "SHADER".to_owned()),
                    thumb,
                    inset: None,
                });
            }

            // ----------------------------------------------- inspector --
            let known = !staged.is_empty()
                && (state.targets.binary_search(&staged).is_ok() || state.sources.binary_search(&staged).is_ok());
            let active = selected.as_ref().is_some_and(|s| remaps.contains_key(s));
            let off = selected.as_ref().is_some_and(|s| state.disabled.contains_key(s));
            let mut ins = InspectorView {
                has_selection: selected.is_some(),
                folder: selected.as_deref().map_or("", parent_folder).to_owned(),
                field: state.field_view(Field::Target),
                field_invalid: !staged.is_empty() && !known,
                redirect: (active || off).then_some(active),
                can_delete: active || off,
                ..InspectorView::default()
            };
            if let Some(source) = selected.as_deref() {
                ins.original = state.thumbs.get(source, true, &image_for, &mut requests);
                ins.original_name = leaf_name(source).to_owned();
                ins.original_folder = parent_folder(source).to_owned();
                if known {
                    ins.replacement = Some(state.thumbs.get(&staged, true, &image_for, &mut requests));
                }
                ins.replacement_name = leaf_name(&staged).to_owned();
                ins.replacement_folder = parent_folder(&staged).to_owned();
                let live_is_staged = active && current.as_deref() == Some(staged.as_str());
                let untouched = !active && !off && source == staged;
                ins.can_apply = known && !live_is_staged && !untouched;
                ins.badge = if staged.is_empty() {
                    None
                } else if !known {
                    Some(("UNKNOWN".to_owned(), Tone::Danger))
                } else if live_is_staged {
                    Some(("LIVE".to_owned(), Tone::Accent))
                } else if off && current.as_deref() == Some(staged.as_str()) {
                    Some(("SAVED · OFF".to_owned(), Tone::Warning))
                } else if untouched {
                    Some(("ORIGINAL".to_owned(), Tone::Faint))
                } else {
                    Some(("STAGED".to_owned(), Tone::Warning))
                };
            }
            if !state.status.is_empty() {
                ins.status = Some((state.status.clone(), state.status_tone));
            }
            v.inspector = ins;

            // ------------------------------------------ active remaps --
            state.active_rows = remaps.iter().map(|(s, t)| (s.clone(), t.clone(), true))
                .chain(state.disabled.iter().map(|(s, t)| (s.clone(), t.clone(), false))).collect();
            state.active_rows.sort_by(|a, b| a.0.cmp(&b.0));
            state.active_first = first_row_for(state.active_rows.len(), l.active_rows(), state.active_first);
            v.active_first = state.active_first;
            v.active_total = state.active_rows.len();
            v.active_detail = format!("{} on · {} off", remaps.len(), state.disabled.len());
            for (source, target, on) in state.active_rows.iter().skip(state.active_first).take(l.active_rows()) {
                v.active.push(ActiveRowView {
                    source: leaf_name(source).to_owned(),
                    target: leaf_name(target).to_owned(),
                    on: *on,
                    current: selected.as_deref() == Some(source.as_str()),
                    source_thumb: state.thumbs.get(source, false, &image_for, &mut requests),
                    target_thumb: state.thumbs.get(target, false, &image_for, &mut requests),
                });
            }
        }
        v.tooltip = remap_tooltip(state, state.hover, (l.s_grid.cols, l.t_grid.cols));
        state.native = Some(v);

        let uploads = std::mem::take(&mut state.thumbs.uploads);
        if !uploads.is_empty() {
            state.thumbs.allocated = true;
        }
        let map_label = state.map_label.clone();
        for (slot, size, rgba) in uploads {
            self.render_command(RenderCommand::RemapThumb { slot, size, rgba });
        }
        for request in requests {
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ShaderThumbnail {
                    map_label: map_label.clone(),
                    source: request.key,
                    image_path: request.image_path,
                    max_px: request.max_px,
                });
        }
    }

    /// Native winit input for the page. Returns `true` when consumed. Keys
    /// the page does not use (Escape without a focused field, the Dev bind)
    /// fall through to the overlay's normal handling.
    pub(in crate::app) fn native_shader_remap_event(&mut self, event: &WindowEvent) -> bool {
        if self.overlay != OverlayMode::ShaderRemap {
            return false;
        }
        let Some(l) = self.remap_layout() else { return false };
        match event {
            WindowEvent::Resized(_) => {
                self.shader_remap_ui.dirty = true;
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
                let p = [position.x as f32, position.y as f32];
                let ui = &mut self.shader_remap_ui;
                ui.pointer = p;
                if let Some(pane) = ui.drag {
                    self.remap_scroll_to(pane, &l, p);
                    self.refresh_shader_remap_view();
                    self.publish_shader_remap();
                    return true;
                }
                let hit = ui.native.as_ref().and_then(|v| page::hit(&l, v, p));
                if hit != ui.hover {
                    ui.hover = hit;
                    let tooltip = remap_tooltip(ui, hit, (l.s_grid.cols, l.t_grid.cols));
                    if let Some(view) = ui.native.as_mut() { view.tooltip = tooltip; }
                }
                if let Some(view) = ui.native.as_mut() { view.pointer = p; }
                if ui.last_publish.is_none_or(|at| at.elapsed() >= PUBLISH_INTERVAL) {
                    ui.last_publish = Some(Instant::now());
                    self.publish_ui();
                } else {
                    ui.dirty = true;
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let p = self.shader_remap_ui.pointer;
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(d) => (d.y / 60.0) as f32,
                };
                if notches.abs() < f32::EPSILON || self.shader_remap_ui.hide_body {
                    return true;
                }
                let steps = notches.abs().ceil() as usize;
                let down = notches < 0.0;
                let ui = &mut self.shader_remap_ui;
                let scroll = |value: &mut usize, by: usize| {
                    *value = if down { value.saturating_add(by) } else { value.saturating_sub(by) };
                };
                // Rows clamp on refresh; scroll whatever the pointer is over.
                if kit_contains(l.sources, p) {
                    scroll(&mut ui.source_first_row, steps);
                } else if kit_contains(l.f_list, p) {
                    scroll(&mut ui.folder_first, steps * 3);
                } else if kit_contains(l.targets, p) {
                    scroll(&mut ui.target_first_row, steps);
                } else if kit_contains(l.a_list, p) {
                    scroll(&mut ui.active_first, steps);
                } else {
                    return true;
                }
                ui.hover = None;
                self.refresh_shader_remap_view();
                self.publish_shader_remap();
                true
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                self.shader_remap_ui.drag = None;
                true
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                self.remap_click(&l);
                true
            }
            WindowEvent::KeyboardInput { event: key, .. } => self.remap_key(key, &l),
            _ => false,
        }
    }

    fn remap_scroll_to(&mut self, pane: Pane, l: &page::Layout, p: [f32; 2]) {
        let ui = &mut self.shader_remap_ui;
        let (area, total, visible) = match pane {
            Pane::Sources => (l.s_grid.area, ui.source_visible.len().div_ceil(l.s_grid.cols), l.s_grid.rows),
            Pane::Targets => (l.t_grid.area, ui.view.items.len().div_ceil(l.t_grid.cols), l.t_grid.rows),
            Pane::Folders => (l.f_list, ui.rail.len(), l.rail_rows()),
            Pane::Active => (l.a_list, ui.active_rows.len(), l.active_rows()),
        };
        let fraction = ((p[1] - area[1]) / area[3].max(1.0)).clamp(0.0, 1.0);
        let first = (fraction * total.saturating_sub(visible) as f32).round() as usize;
        match pane {
            Pane::Sources => ui.source_first_row = first,
            Pane::Targets => ui.target_first_row = first,
            Pane::Folders => ui.folder_first = first,
            Pane::Active => ui.active_first = first,
        }
    }

    fn remap_click(&mut self, l: &page::Layout) {
        let p = self.shader_remap_ui.pointer;
        let hit = self.shader_remap_ui.native.as_ref().and_then(|v| page::hit(l, v, p));
        let mut action = None;
        let ui = &mut self.shader_remap_ui;
        if !matches!(hit, Some(Hit::Field(_) | Hit::Clear(_))) {
            ui.focus = None;
        }
        match hit {
            None | Some(Hit::Panel) => {}
            Some(Hit::TileSize(i)) => ui.tile_size = TileSize(i.min(2)),
            Some(Hit::Hide) => ui.hide_body = !ui.hide_body,
            Some(Hit::Close) => action = Some(RemapAction::Close),
            Some(Hit::Field(field)) => {
                if ui.focus != Some(field) {
                    ui.focus = Some(field);
                    let text = ui.field_text(field).clone();
                    ui.edit = FieldEdit::at_end(&text);
                }
            }
            Some(Hit::Clear(field)) => {
                ui.field_text(field).clear();
                ui.focus = Some(field);
                ui.edit = FieldEdit::default();
            }
            Some(Hit::SourceFilter(i)) => {
                ui.source_filter = [SourceFilter::All, SourceFilter::Remapped, SourceFilter::Original][i.min(2)];
                ui.source_first_row = 0;
            }
            Some(Hit::TargetKind(i)) => {
                ui.target_kind = [TargetKind::All, TargetKind::Scripted, TargetKind::Images][i.min(2)];
            }
            Some(Hit::FolderScope(i)) => {
                ui.folder_scope = [FolderScope::All, FolderScope::Map][i.min(1)];
                ui.folder_first = 0;
            }
            Some(Hit::SourceTile(i)) => {
                let index = ui.source_first_row * l.s_grid.cols + i;
                if let Some(&source) = ui.source_visible.get(index) {
                    action = Some(RemapAction::Select(ui.sources[source].clone()));
                }
            }
            Some(Hit::TargetTile(i)) => {
                let index = ui.target_first_row * l.t_grid.cols + i;
                if let Some(&item) = ui.view.items.get(index) {
                    let name = ui.targets[item as usize].clone();
                    let double = ui.last_target_click.as_ref()
                        .is_some_and(|(last, at)| *last == name && at.elapsed() <= DOUBLE_CLICK);
                    ui.last_target_click = (!double).then(|| (name.clone(), Instant::now()));
                    action = Some(if double && ui.selected.is_some() {
                        RemapAction::StageAndApply(name)
                    } else {
                        RemapAction::Stage(name)
                    });
                }
            }
            Some(Hit::Folder(i)) => {
                if let Some(row) = ui.rail.get(ui.folder_first + i).cloned() {
                    let path = match row {
                        RailRow::All => None,
                        RailRow::Source(folder) => Some(folder),
                        RailRow::Folder(f) => Some(ui.index.folders[f].path.clone()),
                    };
                    // Clicking the selected folder again returns to all folders.
                    ui.target_folder = if path.is_some() && ui.target_folder == path { None } else { path };
                }
            }
            Some(Hit::Apply) => action = Some(RemapAction::Apply),
            Some(Hit::Delete) => action = ui.selected.clone().map(RemapAction::Delete),
            Some(Hit::Redirect) => {
                if let (Some(source), Some(on)) = (ui.selected.clone(), ui.native.as_ref().and_then(|v| v.inspector.redirect)) {
                    action = Some(RemapAction::Toggle(source, !on));
                }
            }
            Some(Hit::ActiveRow(i)) => {
                action = ui.active_rows.get(ui.active_first + i).map(|row| RemapAction::Select(row.0.clone()));
            }
            Some(Hit::ActiveToggle(i)) => {
                action = ui.active_rows.get(ui.active_first + i).map(|row| RemapAction::Toggle(row.0.clone(), !row.2));
            }
            Some(Hit::ActiveDelete(i)) => {
                action = ui.active_rows.get(ui.active_first + i).map(|row| RemapAction::Delete(row.0.clone()));
            }
            Some(Hit::Scroll(pane, _)) => {
                ui.drag = Some(pane);
                self.remap_scroll_to(pane, l, p);
            }
        }
        if let Some(action) = action {
            self.apply_remap_editor_action(action);
        }
        if self.overlay == OverlayMode::ShaderRemap {
            self.refresh_shader_remap_view();
            self.publish_shader_remap();
        }
    }

    fn remap_key(&mut self, key: &KeyEvent, l: &page::Layout) -> bool {
        let focus = self.shader_remap_ui.focus;
        if key.state != ElementState::Pressed {
            return focus.is_some();
        }
        let PhysicalKey::Code(code) = key.physical_key else { return focus.is_some() };
        let control = self.modifiers.control_key();
        if let Some(field) = focus {
            let ui = &mut self.shader_remap_ui;
            match code {
                KeyCode::Escape => ui.focus = None,
                KeyCode::Enter | KeyCode::NumpadEnter => {
                    ui.focus = None;
                    if field == Field::Target {
                        self.apply_remap_editor_action(RemapAction::Apply);
                    }
                }
                KeyCode::Tab => {}
                _ => {
                    let mut edit = ui.edit;
                    let text = ui.field_text(field);
                    let mut result = edit.key(text, code, control);
                    if result == EditResult::Ignored && !control && !self.modifiers.alt_key() {
                        if let Some(typed) = key.text.as_ref() {
                            edit.insert(text, typed);
                            result = EditResult::Changed;
                        }
                    }
                    ui.edit = edit;
                    if result == EditResult::Changed {
                        match field {
                            Field::SourceSearch => ui.source_first_row = 0,
                            Field::FolderSearch => ui.folder_first = 0,
                            Field::TargetSearch => {}
                            Field::Target => ui.status.clear(),
                        }
                    }
                }
            }
            self.refresh_shader_remap_view();
            self.publish_shader_remap();
            // A focused field owns every key, including the console and Dev
            // binds, so typing a path can never close the page.
            return true;
        }
        if self.shader_remap_ui.hide_body {
            return false;
        }
        let can_apply = self.shader_remap_ui.native.as_ref().is_some_and(|v| v.inspector.can_apply);
        let selected = self.shader_remap_ui.selected.clone();
        let action = match code {
            KeyCode::Enter | KeyCode::NumpadEnter if can_apply => Some(RemapAction::Apply),
            KeyCode::Delete if selected.is_some()
                && self.shader_remap_ui.native.as_ref().is_some_and(|v| v.inspector.can_delete) =>
                selected.clone().map(RemapAction::Delete),
            KeyCode::KeyF if control => {
                let ui = &mut self.shader_remap_ui;
                ui.focus = Some(Field::TargetSearch);
                ui.edit = FieldEdit::at_end(&ui.target_search);
                ui.edit.all_selected = !ui.target_search.is_empty();
                None
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown
                if selected.is_some() => self.remap_step_target(code, l),
            _ => return false,
        };
        if let Some(action) = action {
            self.apply_remap_editor_action(action);
        }
        self.refresh_shader_remap_view();
        self.publish_shader_remap();
        true
    }

    /// Arrow keys walk the replacement grid, staging each tile and keeping
    /// it in view.
    fn remap_step_target(&mut self, code: KeyCode, l: &page::Layout) -> Option<RemapAction> {
        let ui = &mut self.shader_remap_ui;
        let items = &ui.view.items;
        if items.is_empty() {
            return None;
        }
        let cols = l.t_grid.cols as isize;
        let staged = scene::canonical_shader_name(&ui.target);
        let current = items.binary_search_by(|&i| ui.targets[i as usize].as_str().cmp(staged.as_str())).ok();
        let next = match current {
            None => ui.target_first_row * l.t_grid.cols,
            Some(at) => {
                let delta = match code {
                    KeyCode::ArrowLeft => -1,
                    KeyCode::ArrowRight => 1,
                    KeyCode::ArrowUp => -cols,
                    _ => cols,
                };
                (at as isize + delta).clamp(0, items.len() as isize - 1) as usize
            }
        };
        let row = next / l.t_grid.cols;
        if row < ui.target_first_row {
            ui.target_first_row = row;
        } else if row >= ui.target_first_row + l.t_grid.rows {
            ui.target_first_row = row + 1 - l.t_grid.rows;
        }
        Some(RemapAction::Stage(ui.targets[items[next] as usize].clone()))
    }
}

fn kit_contains(b: crate::ui::flex::Bounds, p: [f32; 2]) -> bool {
    crate::ui::flex::contains(b, p)
}

/// Hover card: only the full names that captions truncate (shader path,
/// remap target, folder path). No generic hints or control explanations.
/// `cols` = (map material grid columns, replacement grid columns).
fn remap_tooltip(ui: &ShaderRemapUi, hit: Option<Hit>, cols: (usize, usize)) -> Vec<(String, Tone)> {
    let Some(hit) = hit else { return Vec::new() };
    match hit {
        Hit::SourceTile(i) => {
            let index = ui.source_first_row * cols.0 + i;
            let Some(name) = ui.source_visible.get(index).map(|&s| &ui.sources[s]) else { return Vec::new() };
            let mut lines = vec![(name.clone(), Tone::Text)];
            if let Some(to) = ui.disabled.get(name) {
                lines.push((format!("» {to}"), Tone::Warning));
            } else if let Some(row) = ui.active_rows.iter().find(|row| &row.0 == name && row.2) {
                lines.push((format!("» {}", row.1), Tone::Accent));
            }
            lines
        }
        Hit::TargetTile(i) => ui.view.items.get(ui.target_first_row * cols.1 + i)
            .map(|&item| vec![(ui.targets[item as usize].clone(), Tone::Text)])
            .unwrap_or_default(),
        Hit::Folder(i) => match ui.rail.get(ui.folder_first + i) {
            Some(RailRow::Folder(f)) => {
                let row = &ui.index.folders[*f];
                vec![(row.path.clone(), if row.in_map { Tone::Magenta } else { Tone::Text })]
            }
            Some(RailRow::Source(folder)) => vec![(folder.clone(), Tone::Warning)],
            _ => Vec::new(),
        },
        Hit::ActiveRow(i) => ui.active_rows.get(ui.active_first + i)
            .map(|(s, t, on)| vec![(s.clone(), Tone::Text),
                (format!("» {t}"), if *on { Tone::Accent } else { Tone::Warning })])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}
