//! Shader Remaps page: native Taffy geometry, WGPU vertices, winit hit
//! testing (`app::shader_remap_editor`). No egui widgets, painter or windows.
//!
//! Drawn in the native console's look (`ui::kit`): indigo gradient body,
//! cyan neon rim and edge, magenta title ticks, amber carets and warnings.
//! Three panes under a header: the map's materials as a thumbnail grid, a
//! replacement browser (folder rail + thumbnail grid over every known shader
//! and image name) and an inspector comparing original and replacement with
//! the list of active redirects. The app sends only the visible tiles; grids
//! scroll by whole rows. Thumbnails sample a renderer-owned slot atlas
//! (`ui.wgsl` binding 10) filled by the app from the ui-catalog worker.
use super::{
    flex::{Bounds, Container, Flex},
    kit::{self, alpha, mix, size, FieldView, Painter, Thumb, Tone},
    UiSnapshot, UiVertex,
};
use taffy::prelude::{AlignItems, NodeId};

// ------------------------------------------------------- thumbnail atlas --

/// `UiVertex::textured` value that samples the remap thumbnail atlas.
pub const THUMB_SOURCE: f32 = 10.0;
pub const ATLAS_W: u32 = 2560;
pub const ATLAS_H: u32 = 3072;
/// Grid thumbnails: 16 x 16 cells of 160 px.
pub const CELL: u32 = 160;
const CELL_COLS: u32 = ATLAS_W / CELL;
pub const SMALL_SLOTS: usize = 256;
/// Inspector previews: two 512 px cells under the grid cells.
pub const LARGE: u32 = 512;
pub const LARGE_SLOTS: usize = 2;
pub const SLOTS: usize = SMALL_SLOTS + LARGE_SLOTS;

pub fn slot_max_px(slot: usize) -> u32 {
    if slot < SMALL_SLOTS { CELL } else { LARGE }
}

pub fn slot_origin(slot: usize) -> [u32; 2] {
    if slot < SMALL_SLOTS {
        [(slot as u32 % CELL_COLS) * CELL, (slot as u32 / CELL_COLS) * CELL]
    } else {
        [(slot - SMALL_SLOTS) as u32 * LARGE, (SMALL_SLOTS as u32 / CELL_COLS) * CELL]
    }
}

/// UVs of an image of `size` written at the slot origin, inset half a texel
/// so linear filtering never reads a neighbouring cell.
pub fn slot_thumb(slot: usize, size: [u32; 2]) -> Thumb {
    let [x, y] = slot_origin(slot);
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    Thumb::Ready {
        uv0: [(x as f32 + 0.5) / ATLAS_W as f32, (y as f32 + 0.5) / ATLAS_H as f32],
        uv1: [(x as f32 + w - 0.5) / ATLAS_W as f32, (y as f32 + h - 0.5) / ATLAS_H as f32],
        aspect: w / h,
        source: THUMB_SOURCE,
    }
}

// ------------------------------------------------------------ view model --

pub const TILE_SIZES: [(&str, f32); 3] = [("S", 92.0), ("M", 122.0), ("L", 156.0)];
pub const SOURCE_FILTERS: [&str; 3] = ["ALL", "REMAPPED", "ORIGINAL"];
const SOURCE_FILTER_W: [f32; 3] = [54.0, 100.0, 94.0];
pub const TARGET_KINDS: [&str; 3] = ["ALL", "SHADERS", "IMAGES"];
const TARGET_KIND_W: [f32; 3] = [54.0, 92.0, 84.0];
pub const FOLDER_SCOPES: [&str; 2] = ["ALL", "USED BY MAP"];
const FOLDER_SCOPE_W: [f32; 2] = [54.0, 118.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    SourceSearch,
    TargetSearch,
    FolderSearch,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Sources,
    Targets,
    Folders,
    Active,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileView {
    pub name: String,
    pub detail: String,
    pub detail_tone: Tone,
    pub selected: bool,
    pub tag: Option<(String, Tone)>,
    pub corner_tag: Option<String>,
    pub thumb: Thumb,
    /// Remapped materials show their replacement in the corner.
    pub inset: Option<(Thumb, Tone)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RailRowView {
    pub label: String,
    pub count: u32,
    pub depth: u16,
    pub in_map: bool,
    pub selected: bool,
    pub source: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActiveRowView {
    pub source: String,
    pub target: String,
    pub on: bool,
    pub current: bool,
    pub source_thumb: Thumb,
    pub target_thumb: Thumb,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectorView {
    pub has_selection: bool,
    pub folder: String,
    pub original: Thumb,
    pub replacement: Option<Thumb>,
    pub original_name: String,
    pub original_folder: String,
    pub replacement_name: String,
    pub replacement_folder: String,
    pub badge: Option<(String, Tone)>,
    pub field: FieldView,
    pub field_invalid: bool,
    /// `Some(on)` when the selected material has a redirect (live or off).
    pub redirect: Option<bool>,
    pub can_apply: bool,
    pub can_delete: bool,
    pub status: Option<(String, Tone)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderRemapView {
    pub pointer: [f32; 2],
    pub tile_size: usize,
    pub hide_body: bool,
    pub no_map: bool,
    pub map_name: String,
    pub stats: String,
    pub source_search: FieldView,
    pub source_filter: usize,
    pub source_detail: String,
    pub source_tiles: Vec<TileView>,
    pub source_first_row: usize,
    pub source_rows: usize,
    pub source_empty: String,
    pub target_search: FieldView,
    pub target_kind: usize,
    pub target_detail: String,
    pub folder_search: FieldView,
    pub folder_scope: usize,
    pub folders: Vec<RailRowView>,
    pub folder_first: usize,
    pub folder_total: usize,
    pub target_tiles: Vec<TileView>,
    pub target_first_row: usize,
    pub target_rows: usize,
    pub target_empty: String,
    pub inspector: InspectorView,
    pub active: Vec<ActiveRowView>,
    pub active_first: usize,
    pub active_total: usize,
    pub active_detail: String,
    pub tooltip: Vec<(String, Tone)>,
}

// ---------------------------------------------------------------- layout --

/// Uniform thumbnail grid showing only whole rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub area: Bounds,
    pub cols: usize,
    pub rows: usize,
    pub cell: f32,
    pub gap: f32,
    pub label_h: f32,
    pub row_h: f32,
    scroll_w: f32,
}

impl Grid {
    fn new(area: Bounds, tile: f32, s: f32) -> Self {
        let gap = 10.0 * s;
        let scroll_w = 14.0 * s;
        let label_h = 40.0 * s;
        let usable = (area[2] - scroll_w).max(1.0);
        let cols = (((usable + gap) / (tile + gap)).floor() as usize).max(1);
        let cell = ((usable - gap * (cols - 1) as f32) / cols as f32).max(1.0);
        let row_h = cell + label_h + gap;
        let rows = (((area[3] + gap) / row_h).floor() as usize).max(1);
        Self { area, cols, rows, cell, gap, label_h, row_h, scroll_w }
    }

    pub fn capacity(&self) -> usize {
        self.cols * self.rows
    }

    /// Tile `i` of the visible window (image square + caption strip).
    pub fn tile(&self, i: usize) -> Bounds {
        let (row, col) = (i / self.cols, i % self.cols);
        [self.area[0] + col as f32 * (self.cell + self.gap), self.area[1] + row as f32 * self.row_h,
            self.cell, self.cell + self.label_h]
    }

    pub fn hit(&self, p: [f32; 2], count: usize) -> Option<usize> {
        let col = ((p[0] - self.area[0]) / (self.cell + self.gap)).floor();
        let row = ((p[1] - self.area[1]) / self.row_h).floor();
        if col < 0.0 || row < 0.0 || col as usize >= self.cols || row as usize >= self.rows {
            return None;
        }
        let i = row as usize * self.cols + col as usize;
        (i < count && kit::contains(self.tile(i), p)).then_some(i)
    }

    pub fn track(&self) -> Bounds {
        [self.area[0] + self.area[2] - self.scroll_w, self.area[1], self.scroll_w, self.area[3]]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub scale: f32,
    pub panel: Bounds,
    pub header: Bounds,
    pub title: Bounds,
    pub stats: Bounds,
    pub sizes: [Bounds; 3],
    pub hide: Bounds,
    pub close: Bounds,
    pub sources: Bounds,
    pub s_title: Bounds,
    pub s_search: Bounds,
    pub s_filters: [Bounds; 3],
    pub s_grid: Grid,
    pub targets: Bounds,
    pub t_title: Bounds,
    pub t_search: Bounds,
    pub t_kinds: [Bounds; 3],
    pub f_search: Bounds,
    pub f_scopes: [Bounds; 2],
    pub f_list: Bounds,
    pub t_grid: Grid,
    pub inspector: Bounds,
    pub i_title: Bounds,
    pub compare: [Bounds; 2],
    pub captions: [Bounds; 2],
    pub field: Bounds,
    pub state: Bounds,
    pub apply: Bounds,
    pub delete: Bounds,
    pub status: Bounds,
    pub a_title: Bounds,
    pub a_list: Bounds,
    pub foot: Bounds,
    pub rail_row_h: f32,
    pub active_row_h: f32,
}

impl Layout {
    pub fn rail_rows(&self) -> usize {
        ((self.f_list[3] / self.rail_row_h).floor() as usize).max(1)
    }

    pub fn rail_row(&self, i: usize) -> Bounds {
        [self.f_list[0], self.f_list[1] + i as f32 * self.rail_row_h,
            self.f_list[2] - 12.0 * self.scale, self.rail_row_h]
    }

    pub fn active_rows(&self) -> usize {
        ((self.a_list[3] / self.active_row_h).floor() as usize).max(1)
    }

    /// Row, ON/OFF toggle and delete box of active-remap row `i`.
    pub fn active_parts(&self, i: usize) -> (Bounds, Bounds, Bounds) {
        let s = self.scale;
        let row = [self.a_list[0], self.a_list[1] + i as f32 * self.active_row_h,
            self.a_list[2] - 12.0 * s, self.active_row_h - 4.0 * s];
        let y = row[1] + (row[3] - 24.0 * s) * 0.5;
        let delete = [row[0] + row[2] - 30.0 * s, y, 24.0 * s, 24.0 * s];
        let toggle = [delete[0] - 54.0 * s, y, 46.0 * s, 24.0 * s];
        (row, toggle, delete)
    }

    pub fn redirect_switch(&self) -> Bounds {
        let s = self.scale;
        [self.state[0] + 92.0 * s, self.state[1] + (self.state[3] - 24.0 * s) * 0.5, 58.0 * s, 24.0 * s]
    }

    /// Header-only panel while the body is hidden to look at the world.
    pub fn header_panel(&self) -> Bounds {
        self.header
    }
}

fn at(origin: [f32; 2], b: Bounds) -> Bounds {
    [origin[0] + b[0], origin[1] + b[1], b[2], b[3]]
}

fn leaves<const N: usize>(f: &mut Flex, widths: [f32; N], h: f32) -> [NodeId; N] {
    widths.map(|w| f.leaf(w, h))
}

/// Physical-pixel layout shared by the WGPU build and winit hit testing.
/// Every pane uses the same title/search/toolbar rows, so the two grids, the
/// folder rail and the inspector's comparison start on common lines.
pub fn layout(width: u32, height: u32, tile_size: usize) -> Layout {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let s = (h / 1125.0).clamp(0.6, 2.4);
    // Extra bottom margin leaves room for the neon edge glow under the panel.
    let panel = [18.0 * s, 16.0 * s, (w - 36.0 * s).max(1.0), (h - 46.0 * s).max(1.0)];
    let (pw, ph) = (panel[2], panel[3]);
    let compact = pw < 1500.0 * s;
    let inspector_w = if compact { 320.0 * s } else { (pw * 0.22).clamp(360.0 * s, 480.0 * s) };
    let rail_w = if compact { 180.0 * s } else { 232.0 * s };
    let (pad, gap, row_gap) = (12.0 * s, 12.0 * s, 8.0 * s);
    let (title_h, field_h, chip_h, header_h) = (30.0 * s, 32.0 * s, 26.0 * s, 56.0 * s);
    let mut f = Flex::new();

    let title = f.leaf(240.0 * s, 34.0 * s);
    let stats = f.leaf(0.0, 24.0 * s);
    f.set_grow(stats, 1.0);
    let sizes = leaves(&mut f, [34.0 * s; 3], chip_h);
    let size_row = f.node(Container::row().gap(3.0 * s), &sizes);
    let hide = f.leaf(140.0 * s, 32.0 * s);
    let close = f.leaf(96.0 * s, 32.0 * s);
    let header = f.node(Container::row().height(header_h).pad(18.0 * s, 0.0).gap(gap),
        &[title, stats, size_row, hide, close]);

    let s_title = f.band(title_h);
    let s_search = f.band(field_h);
    let s_filters = leaves(&mut f, SOURCE_FILTER_W.map(|x| x * s), chip_h);
    let s_filter_row = f.node(Container::row().height(chip_h).gap(4.0 * s), &s_filters);
    let s_grid = f.spring();
    let sources = f.node(Container::column().pad(pad, pad).gap(row_gap),
        &[s_title, s_search, s_filter_row, s_grid]);
    f.set_share(sources, 1.0);

    let t_title = f.band(title_h);
    let t_search = f.band(field_h);
    // Toolbar: folder scope chips over the rail, kind chips over the grid.
    let f_scopes = leaves(&mut f, FOLDER_SCOPE_W.map(|x| x * s), chip_h);
    let f_scope_row = f.node(Container::row().width(rail_w).height(chip_h).gap(4.0 * s), &f_scopes);
    let t_kinds = leaves(&mut f, TARGET_KIND_W.map(|x| x * s), chip_h);
    let t_kind_row = f.node(Container::row().height(chip_h).gap(4.0 * s), &t_kinds);
    let t_tools = f.node(Container::row().height(chip_h).gap(gap), &[f_scope_row, t_kind_row]);
    let f_search = f.band(field_h);
    let f_list = f.spring();
    let rail = f.node(Container::column().width(rail_w).gap(row_gap), &[f_search, f_list]);
    let t_grid = f.spring();
    let t_body = f.node(Container::row().gap(gap).align(AlignItems::STRETCH), &[rail, t_grid]);
    f.set_grow(t_body, 1.0);
    let targets = f.node(Container::column().pad(pad, pad).gap(row_gap),
        &[t_title, t_search, t_tools, t_body]);
    f.set_share(targets, 1.8);

    let inspector = f.node(Container::column().width(inspector_w), &[]);
    let body = f.node(Container::row().pad(gap, gap).gap(gap).align(AlignItems::STRETCH),
        &[sources, targets, inspector]);
    f.set_grow(body, 1.0);
    let root = f.node(Container::column().width(pw).height(ph), &[header, body]);
    f.compute(root, pw, ph);
    let o = [panel[0], panel[1]];
    let r = |node| at(o, f.bounds(node));
    let tile = TILE_SIZES[tile_size.min(2)].1 * s;
    let inspector_b = r(inspector);

    // Inspector: comparison squares sized to the width (the arrow sits in a
    // wider gap between them), shrunk on short screens so the active-remap
    // list keeps at least ~120 pt of rows.
    let (iw, ih) = (inspector_b[2].max(1.0), inspector_b[3].max(1.0));
    let compare_gap = 28.0 * s;
    let fixed = (title_h * 2.0 + 44.0 * s + field_h + 28.0 * s + 34.0 * s + 20.0 * s + 40.0 * s
        + 120.0 * s + 10.0 * 10.0 * s + 2.0 * pad).min(ih);
    let square = ((iw - 2.0 * pad - compare_gap) * 0.5).min(ih - fixed).clamp(48.0 * s, 250.0 * s);
    let mut g = Flex::new();
    let i_title = g.band(title_h);
    let compare = [g.leaf(square, square), g.leaf(square, square)];
    let compare_row = g.node(Container::row().height(square).gap(compare_gap), &compare);
    let captions = [g.leaf(square, 44.0 * s), g.leaf(square, 44.0 * s)];
    let caption_row = g.node(Container::row().height(44.0 * s).gap(compare_gap), &captions);
    let field = g.band(field_h);
    let state = g.band(28.0 * s);
    let apply = g.leaf(0.0, 34.0 * s);
    let delete = g.leaf(0.0, 34.0 * s);
    g.set_share(apply, 1.0);
    g.set_share(delete, 1.0);
    let buttons = g.node(Container::row().height(34.0 * s).gap(row_gap), &[apply, delete]);
    let status = g.band(20.0 * s);
    let a_title = g.band(title_h);
    let a_list = g.spring();
    let foot = g.band(40.0 * s);
    let i_root = g.node(Container::column().width(iw).height(ih).pad(pad, pad).gap(10.0 * s),
        &[i_title, compare_row, caption_row, field, state, buttons, status, a_title, a_list, foot]);
    g.compute(i_root, iw, ih);
    let io = [inspector_b[0], inspector_b[1]];
    let ri = |node| at(io, g.bounds(node));

    Layout {
        scale: s,
        panel,
        header: [panel[0], panel[1], pw, header_h],
        title: r(title),
        stats: r(stats),
        sizes: sizes.map(r),
        hide: r(hide),
        close: r(close),
        sources: r(sources),
        s_title: r(s_title),
        s_search: r(s_search),
        s_filters: s_filters.map(r),
        s_grid: Grid::new(r(s_grid), tile, s),
        targets: r(targets),
        t_title: r(t_title),
        t_search: r(t_search),
        t_kinds: t_kinds.map(r),
        f_search: r(f_search),
        f_scopes: f_scopes.map(r),
        f_list: r(f_list),
        t_grid: Grid::new(r(t_grid), tile, s),
        inspector: inspector_b,
        i_title: ri(i_title),
        compare: compare.map(ri),
        captions: captions.map(ri),
        field: ri(field),
        state: ri(state),
        apply: ri(apply),
        delete: ri(delete),
        status: ri(status),
        a_title: ri(a_title),
        a_list: ri(a_list),
        foot: ri(foot),
        rail_row_h: 25.0 * s,
        active_row_h: 44.0 * s,
    }
}

// ------------------------------------------------------------ hit testing --

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    TileSize(usize),
    Hide,
    Close,
    Field(Field),
    Clear(Field),
    SourceFilter(usize),
    TargetKind(usize),
    FolderScope(usize),
    SourceTile(usize),
    TargetTile(usize),
    Folder(usize),
    Apply,
    Delete,
    Redirect,
    ActiveRow(usize),
    ActiveToggle(usize),
    ActiveDelete(usize),
    /// Scroll track click/drag: fraction of the scrollable range.
    Scroll(Pane, f32),
    /// Inside the panel but on no control.
    Panel,
}

fn track_fraction(track: Bounds, p: [f32; 2]) -> f32 {
    ((p[1] - track[1]) / track[3].max(1.0)).clamp(0.0, 1.0)
}

pub fn hit(l: &Layout, v: &ShaderRemapView, p: [f32; 2]) -> Option<Hit> {
    let inside = |b: Bounds| kit::contains(b, p);
    if inside(l.close) { return Some(Hit::Close); }
    if inside(l.hide) { return Some(Hit::Hide); }
    if v.hide_body {
        return inside(l.header_panel()).then_some(Hit::Panel);
    }
    if !inside(l.panel) { return None; }
    if let Some(i) = l.sizes.iter().position(|b| inside(*b)) { return Some(Hit::TileSize(i)); }
    if v.no_map { return Some(Hit::Panel); }
    let s = l.scale;
    let field = |b: Bounds, text: &str, f: Field| -> Option<Hit> {
        if !inside(b) { return None; }
        if !text.is_empty() && inside(Painter::field_clear_rect(b, s)) {
            return Some(Hit::Clear(f));
        }
        Some(Hit::Field(f))
    };
    // Map materials.
    if let Some(hit) = field(l.s_search, &v.source_search.text, Field::SourceSearch) { return Some(hit); }
    if let Some(i) = l.s_filters.iter().position(|b| inside(*b)) { return Some(Hit::SourceFilter(i)); }
    if inside(l.s_grid.track()) && v.source_rows > l.s_grid.rows {
        return Some(Hit::Scroll(Pane::Sources, track_fraction(l.s_grid.track(), p)));
    }
    if let Some(i) = l.s_grid.hit(p, v.source_tiles.len()) { return Some(Hit::SourceTile(i)); }
    // Replacement browser.
    if let Some(hit) = field(l.t_search, &v.target_search.text, Field::TargetSearch) { return Some(hit); }
    if let Some(i) = l.t_kinds.iter().position(|b| inside(*b)) { return Some(Hit::TargetKind(i)); }
    if let Some(hit) = field(l.f_search, &v.folder_search.text, Field::FolderSearch) { return Some(hit); }
    if let Some(i) = l.f_scopes.iter().position(|b| inside(*b)) { return Some(Hit::FolderScope(i)); }
    if inside(l.f_list) {
        let track = [l.f_list[0] + l.f_list[2] - 12.0 * s, l.f_list[1], 12.0 * s, l.f_list[3]];
        if inside(track) && v.folder_total > l.rail_rows() {
            return Some(Hit::Scroll(Pane::Folders, track_fraction(track, p)));
        }
        return (0..v.folders.len()).find(|&i| inside(l.rail_row(i))).map(Hit::Folder).or(Some(Hit::Panel));
    }
    if inside(l.t_grid.track()) && v.target_rows > l.t_grid.rows {
        return Some(Hit::Scroll(Pane::Targets, track_fraction(l.t_grid.track(), p)));
    }
    if let Some(i) = l.t_grid.hit(p, v.target_tiles.len()) { return Some(Hit::TargetTile(i)); }
    // Inspector.
    let ins = &v.inspector;
    if ins.has_selection {
        if let Some(hit) = field(l.field, &ins.field.text, Field::Target) { return Some(hit); }
        if ins.redirect.is_some() && inside(l.redirect_switch()) { return Some(Hit::Redirect); }
        if inside(l.apply) && ins.can_apply { return Some(Hit::Apply); }
        if inside(l.delete) && ins.can_delete { return Some(Hit::Delete); }
    }
    if inside(l.a_list) {
        let track = [l.a_list[0] + l.a_list[2] - 12.0 * s, l.a_list[1], 12.0 * s, l.a_list[3]];
        if inside(track) && v.active_total > l.active_rows() {
            return Some(Hit::Scroll(Pane::Active, track_fraction(track, p)));
        }
        for i in 0..v.active.len() {
            let (row, toggle, delete) = l.active_parts(i);
            if inside(delete) { return Some(Hit::ActiveDelete(i)); }
            if inside(toggle) { return Some(Hit::ActiveToggle(i)); }
            if inside(row) { return Some(Hit::ActiveRow(i)); }
        }
    }
    Some(Hit::Panel)
}

// ----------------------------------------------------------------- build --

fn tile(p: &mut Painter, g: &Grid, b: Bounds, t: &TileView) {
    let c = p.c;
    let s = p.scale;
    let hovered = p.hovered(b);
    let cell = g.cell;
    let image = [b[0], b[1], cell, cell];
    let caption = [b[0], b[1] + cell, b[2], g.label_h];
    p.thumb(image, t.thumb, "NO IMAGE");
    if let Some((inset, tone)) = t.inset {
        let side = (cell * 0.40).round();
        let r = [image[0] + cell - side - 6.0 * s, image[1] + cell - side - 6.0 * s, side, side];
        let frame = kit::inset(r, -2.0 * s, -2.0 * s);
        p.fill(frame, c.card);
        p.thumb(r, inset, "");
        p.outline(frame, (1.5 * s).max(1.0), c.tone(tone));
    }
    if let Some((label, tone)) = &t.tag {
        p.tag([image[0] + 5.0 * s, image[1] + 5.0 * s], false, label, c.tone(*tone));
    }
    if let Some(label) = &t.corner_tag {
        p.tag([image[0] + cell - 5.0 * s, image[1] + 5.0 * s], true, label, c.dim);
    }
    p.fill(caption, if t.selected { mix(c.bar, c.cyan, 0.18) } else if hovered { mix(c.bar, c.cyan, 0.08) }
        else { alpha(c.bar, 0.90) });
    let text_w = caption[2] - 16.0 * s;
    p.text([caption[0] + 8.0 * s, caption[1] + 3.0 * s, text_w, 19.0 * s], &t.name, size::BODY,
        if t.selected || hovered { c.bright } else { c.text });
    p.text([caption[0] + 8.0 * s, caption[1] + 21.0 * s, text_w, 16.0 * s], &t.detail, size::MICRO,
        c.tone(t.detail_tone));
    if t.selected {
        p.glow(b, c.cyan);
    } else {
        p.outline(b, 1.0, if hovered { alpha(c.cyan, 0.50) } else { alpha(c.faint, 0.22) });
    }
}

fn grid(p: &mut Painter, g: &Grid, tiles: &[TileView], first_row: usize, total_rows: usize, empty: &str) {
    if tiles.is_empty() {
        let c = p.c;
        p.text_center([g.area[0], g.area[1] + 12.0 * p.scale, g.area[2], 40.0 * p.scale], empty, size::BODY, c.faint);
        return;
    }
    for (i, t) in tiles.iter().enumerate().take(g.capacity()) {
        tile(p, g, g.tile(i), t);
    }
    p.scrollbar(g.area, first_row, total_rows, g.rows);
}

pub(crate) fn build(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(v) = ui.shader_remap.as_ref() else { return };
    let l = layout(w, h, v.tile_size);
    let mut p = Painter::new(out, ui, l.scale, v.pointer, w, h);
    let c = p.c;
    let s = l.scale;

    // Panel, header band and title, like the console.
    p.panel(if v.hide_body { l.header_panel() } else { l.panel });
    p.header_band(l.header);
    p.title(l.title, "SHADER REMAPS");
    let map_w = p.width(&v.map_name, size::BODY);
    p.text([l.stats[0], l.stats[1], l.stats[2], l.stats[3]], &v.map_name, size::BODY, c.cyan);
    let x = l.stats[0] + map_w + 18.0 * s;
    p.text([x, l.stats[1], (l.stats[0] + l.stats[2] - x).max(0.0), l.stats[3]], &v.stats, size::SMALL, c.dim);
    if !v.hide_body {
        for (i, b) in l.sizes.iter().enumerate() {
            p.chip(*b, TILE_SIZES[i].0, v.tile_size == i);
        }
    }
    p.button(l.hide, if v.hide_body { "SHOW PANEL" } else { "HIDE PANEL" }, false, true, c.cyan);
    p.button(l.close, "CLOSE", false, true, c.magenta);
    if v.hide_body {
        p.tooltip(&v.tooltip);
        return;
    }
    if v.no_map {
        p.text_center(l.targets, "Load a map to browse its shader materials.", size::BODY, c.dim);
        return;
    }

    // Map materials.
    p.pane(l.sources);
    p.pane_title(l.s_title, "MAP MATERIALS", &v.source_detail);
    p.field(l.s_search, &v.source_search, "Search materials or replacements", false);
    for (i, b) in l.s_filters.iter().enumerate() {
        p.chip(*b, SOURCE_FILTERS[i], v.source_filter == i);
    }
    grid(&mut p, &l.s_grid, &v.source_tiles, v.source_first_row, v.source_rows, &v.source_empty);

    // Replacement browser.
    p.pane(l.targets);
    p.pane_title(l.t_title, "REPLACEMENTS", &v.target_detail);
    p.field(l.t_search, &v.target_search, "Search every shader and image name (space separates terms)", false);
    for (i, b) in l.t_kinds.iter().enumerate() {
        p.chip(*b, TARGET_KINDS[i], v.target_kind == i);
    }
    for (i, b) in l.f_scopes.iter().enumerate() {
        p.chip(*b, FOLDER_SCOPES[i], v.folder_scope == i);
    }
    p.field(l.f_search, &v.folder_search, "Filter folders", false);
    p.fill(l.f_list, alpha(c.card, 0.70));
    p.outline(l.f_list, 1.0, alpha(c.faint, 0.22));
    for (i, row) in v.folders.iter().enumerate().take(l.rail_rows()) {
        let b = l.rail_row(i);
        if row.selected {
            p.fill(b, alpha(c.cyan, 0.13));
            p.fill([b[0], b[1], 3.0 * s, b[3]], c.cyan);
        } else if p.hovered(b) {
            p.fill(b, alpha(c.cyan, 0.06));
        }
        let x = b[0] + 14.0 * s + row.depth as f32 * 12.0 * s;
        if row.in_map {
            p.dot([x - 6.0 * s, b[1] + b[3] * 0.5], 2.2 * s, c.magenta);
        }
        let count = row.count.to_string();
        let count_w = p.width(&count, size::MICRO) + 14.0 * s;
        let color = if row.source { c.amber } else if row.selected { c.bright } else { c.text };
        p.text([x, b[1], (b[0] + b[2] - count_w - x).max(0.0), b[3]], &row.label, size::SMALL, color);
        p.text_right([b[0], b[1], b[2] - 8.0 * s, b[3]], &count, size::MICRO, c.faint);
    }
    p.scrollbar(l.f_list, v.folder_first, v.folder_total, l.rail_rows());
    grid(&mut p, &l.t_grid, &v.target_tiles, v.target_first_row, v.target_rows, &v.target_empty);

    // Inspector.
    let ins = &v.inspector;
    p.pane(l.inspector);
    p.pane_title(l.i_title, "REMAP", &ins.folder);
    if ins.has_selection {
        p.thumb(l.compare[0], ins.original, "NO IMAGE");
    } else {
        p.fill(l.compare[0], c.card);
        p.text_center(l.compare[0], "SELECT A MATERIAL", size::MICRO, c.faint);
    }
    match ins.replacement {
        Some(thumb) => p.thumb(l.compare[1], thumb, "NO IMAGE"),
        None => {
            p.fill(l.compare[1], c.card);
            if ins.has_selection {
                p.text_center(l.compare[1], "PICK A REPLACEMENT", size::MICRO, c.faint);
            }
        }
    }
    p.outline(l.compare[0], 1.0, alpha(c.faint, 0.35));
    p.outline(l.compare[1], 1.0, if ins.replacement.is_some() { alpha(c.cyan, 0.55) } else { alpha(c.faint, 0.35) });
    let gap = [l.compare[0][0] + l.compare[0][2], l.compare[0][1],
        l.compare[1][0] - (l.compare[0][0] + l.compare[0][2]), l.compare[0][3]];
    p.text_center(gap, "\u{bb}", 18.0, if ins.replacement.is_some() { c.cyan } else { c.faint });
    for (i, (label, name, folder)) in [
        ("ORIGINAL", &ins.original_name, &ins.original_folder),
        ("REPLACEMENT", &ins.replacement_name, &ins.replacement_folder),
    ].into_iter().enumerate() {
        let b = l.captions[i];
        p.text([b[0], b[1], b[2], 14.0 * s], label, size::MICRO, c.faint);
        if i == 1 {
            if let Some((badge, tone)) = &ins.badge {
                p.tag([b[0] + b[2], b[1] - 1.0 * s], true, badge, c.tone(*tone));
            }
        }
        p.text([b[0], b[1] + 14.0 * s, b[2], 18.0 * s], name, size::BODY, c.bright);
        p.text([b[0], b[1] + 31.0 * s, b[2], 13.0 * s], folder, size::MICRO, c.faint);
    }
    if ins.has_selection {
        p.field(l.field, &ins.field, "Exact shader name", ins.field_invalid);
        match ins.redirect {
            Some(on) => {
                p.text([l.state[0], l.state[1], 88.0 * s, l.state[3]], "Redirect", size::BODY, c.dim);
                p.chip(l.redirect_switch(), if on { "ON" } else { "OFF" }, on);
                if !on {
                    let sw = l.redirect_switch();
                    p.outline(sw, 1.0, alpha(c.amber, 0.70));
                }
            }
            None => p.text(l.state, "Not remapped: drawing the original material.", size::SMALL, c.faint),
        }
        p.button(l.apply, "APPLY", true, ins.can_apply, c.cyan);
        p.button(l.delete, "DELETE", false, ins.can_delete, c.danger);
    } else {
        p.fill(l.field, c.field);
        p.outline(l.field, 1.0, alpha(c.faint, 0.40));
        p.text(kit::inset(l.field, 10.0 * s, 0.0), "Select a material on the left to start.", size::BODY, c.faint);
    }
    if let Some((status, tone)) = &ins.status {
        p.text(l.status, status, size::SMALL, c.tone(*tone));
    }

    p.pane_title(l.a_title, "ACTIVE REMAPS", &v.active_detail);
    if v.active.is_empty() {
        p.text_center([l.a_list[0], l.a_list[1] + 8.0 * s, l.a_list[2], 32.0 * s], "No redirects yet.", size::SMALL, c.faint);
    }
    for (i, row) in v.active.iter().enumerate().take(l.active_rows()) {
        let (b, toggle, delete) = l.active_parts(i);
        let hovered = p.hovered(b);
        p.fill(b, if row.current { mix(c.bar, c.cyan, 0.16) } else if hovered { mix(c.bar, c.cyan, 0.07) }
            else { alpha(c.bar, 0.75) });
        if row.current {
            p.fill([b[0], b[1], 3.0 * s, b[3]], c.cyan);
        }
        let side = b[3] - 8.0 * s;
        let a = [b[0] + 8.0 * s, b[1] + 4.0 * s, side, side];
        let bb = [a[0] + side + 20.0 * s, a[1], side, side];
        p.thumb(a, row.source_thumb, "");
        p.thumb(bb, row.target_thumb, "");
        let tone = if row.on { c.cyan } else { c.amber };
        p.text_center([a[0] + side, a[1], 20.0 * s, side], "\u{bb}", size::BODY, tone);
        let x = bb[0] + side + 10.0 * s;
        let text_w = (toggle[0] - x - 8.0 * s).max(0.0);
        p.text([x, b[1] + 3.0 * s, text_w, 18.0 * s], &row.source, size::SMALL,
            if row.current { c.bright } else { c.text });
        p.text([x, b[1] + 20.0 * s, text_w, 16.0 * s], &row.target, size::MICRO, tone);
        p.chip(toggle, if row.on { "ON" } else { "OFF" }, row.on);
        if !row.on {
            p.outline(toggle, 1.0, alpha(c.amber, 0.70));
        }
        let hot = p.hovered(delete);
        p.fill(delete, if hot { mix(c.chip, c.danger, 0.18) } else { c.chip });
        p.outline(delete, 1.0, if hot { c.danger } else { alpha(c.faint, 0.45) });
        p.text_center(delete, "\u{d7}", 15.0, if hot { c.bright } else { c.dim });
    }
    p.scrollbar(l.a_list, v.active_first, v.active_total, l.active_rows());
    p.paragraph(l.foot, "Double-click a replacement or press Enter to apply. Arrow keys step through \
        replacements. Thumbnails are static (editor image or first stage). Remaps change appearance only.",
        size::MICRO, c.faint);

    p.tooltip(&v.tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disjoint(rects: &[Bounds]) {
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let overlap = a[0] + 0.5 < b[0] + b[2] && b[0] + 0.5 < a[0] + a[2]
                    && a[1] + 0.5 < b[1] + b[3] && b[1] + 0.5 < a[1] + a[3];
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    fn within(outer: Bounds, inner: Bounds) -> bool {
        inner[0] >= outer[0] - 0.5 && inner[1] >= outer[1] - 0.5
            && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
            && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
    }

    #[test]
    fn panes_fit_align_and_do_not_overlap() {
        for (w, h) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
            for size in 0..3 {
                let l = layout(w, h, size);
                for b in [l.close, l.sources, l.targets, l.inspector, l.s_grid.area, l.t_grid.area, l.f_list, l.foot] {
                    assert!(within(l.panel, b), "{w}x{h}: {b:?} escapes");
                }
                disjoint(&[l.sources, l.targets, l.inspector]);
                disjoint(&[l.f_list, l.t_grid.area]);
                assert!(l.targets[2] > l.sources[2]);
                // Shared rows: titles, searches and toolbars line up, and
                // both grids start on the same line as the folder filter.
                assert!((l.s_title[1] - l.t_title[1]).abs() < 0.5 && (l.t_title[1] - l.i_title[1]).abs() < 0.5);
                assert!((l.s_search[1] - l.t_search[1]).abs() < 0.5);
                assert!((l.s_filters[0][1] - l.t_kinds[0][1]).abs() < 0.5);
                assert!((l.s_grid.area[1] - l.t_grid.area[1]).abs() < 0.5);
                assert!((l.t_grid.area[1] - l.f_search[1]).abs() < 0.5);
                assert!((l.f_scopes[0][0] - l.f_search[0]).abs() < 0.5);
                assert!(l.a_list[3] >= 90.0 * l.scale, "{w}x{h}: active list {}", l.a_list[3]);
                for g in [l.s_grid, l.t_grid] {
                    let last = g.tile(g.capacity() - 1);
                    assert!(within(g.area, last), "{w}x{h}/{size}: tile escapes grid");
                }
                let (row, toggle, delete) = l.active_parts(0);
                assert!(within(row, toggle) && within(row, delete) && toggle[0] + toggle[2] < delete[0]);
            }
        }
    }

    #[test]
    fn hit_maps_tiles_and_slots_stay_in_atlas() {
        let l = layout(2560, 1440, 1);
        let v = ShaderRemapView { source_tiles: vec![TileView::default(); 5], ..ShaderRemapView::default() };
        let b = l.s_grid.tile(3);
        assert_eq!(hit(&l, &v, [b[0] + 4.0, b[1] + 4.0]), Some(Hit::SourceTile(3)));
        assert_eq!(hit(&l, &v, [l.close[0] + 2.0, l.close[1] + 2.0]), Some(Hit::Close));
        for slot in [0, 15, 16, 255, 256, 257] {
            let [x, y] = slot_origin(slot);
            assert!(x + slot_max_px(slot) <= ATLAS_W && y + slot_max_px(slot) <= ATLAS_H, "slot {slot}");
        }
        assert_ne!(slot_origin(255), slot_origin(256));
    }
}
