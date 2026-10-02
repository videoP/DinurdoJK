//! Blueprint data for the in-game entity graph (`entities` console command).
//!
//! Built once per map on the prepare worker from the BSP entity lump: every
//! entity with a plot position, the target/targetname links between them, and a
//! rasterised top-down floor plan of the world so the 2D view reads like a
//! blueprint instead of floating dots. All coordinates are JKA units, Z up.

use jka_assets::bsp::{Bsp, SurfaceKind, SURF_NODRAW, SURF_SKY};
use std::collections::HashMap;

/// Spawn keys whose value names another entity's `targetname`. Ordered by
/// how commonly maps use them; `target` is the primary flow edge.
pub const LINK_KEYS: [&str; 10] = [
    "target",
    "target2",
    "target3",
    "target4",
    "killtarget",
    "paintarget",
    "npc_target",
    "npc_target2",
    "truetarget",
    "falsetarget",
];

/// Longest side of the floor-plan raster. Layers are one byte per pixel.
const FOOTPRINT_MAX_DIM: f32 = 1536.0;
/// Floor bands (by floor-triangle height quantile) on top of the "all" layer.
const FOOTPRINT_BANDS: usize = 4;
const FOOTPRINT_MIN_BAND_FLOORS: usize = 64;
const FLOOR_NORMAL_Z: f32 = 0.7;

pub const FOOTPRINT_EMPTY: u8 = 0;
pub const FOOTPRINT_FLOOR: u8 = 1;
pub const FOOTPRINT_WALL: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityCategory {
    Spawn,
    Trigger,
    Target,
    Func,
    Light,
    Item,
    Fx,
    Npc,
    Misc,
    Info,
    Other,
}

impl EntityCategory {
    pub const ALL: [Self; 11] = [
        Self::Spawn,
        Self::Trigger,
        Self::Target,
        Self::Func,
        Self::Light,
        Self::Item,
        Self::Fx,
        Self::Npc,
        Self::Misc,
        Self::Info,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Spawn => "SPAWN",
            Self::Trigger => "TRIGGER",
            Self::Target => "TARGET",
            Self::Func => "FUNC",
            Self::Light => "LIGHT",
            Self::Item => "ITEM",
            Self::Fx => "FX",
            Self::Npc => "NPC",
            Self::Misc => "MISC",
            Self::Info => "INFO",
            Self::Other => "OTHER",
        }
    }

    /// RGBA color used to draw this category's entities. Shared by the 2D
    /// blueprint overlay and the 3D world-space marker overlay so both agree.
    pub fn color(self) -> [u8; 4] {
        match self {
            Self::Spawn => [0x5F, 0xE0, 0x8A, 0xFF],
            Self::Trigger => [0xFF, 0x9A, 0x3C, 0xFF],
            Self::Target => [0xD0, 0x7C, 0xFF, 0xFF],
            Self::Func => [0x4F, 0xD6, 0xE8, 0xFF],
            Self::Light => [0xFF, 0xE0, 0x66, 0xFF],
            Self::Item => [0x6F, 0xA8, 0xFF, 0xFF],
            Self::Fx => [0xFF, 0x7F, 0xB0, 0xFF],
            Self::Npc => [0xFF, 0x6B, 0x6B, 0xFF],
            Self::Misc => [0x9C, 0xC8, 0xC0, 0xFF],
            Self::Info => [0xB8, 0xC4, 0xD0, 0xFF],
            Self::Other => [0x8B, 0x99, 0xAA, 0xFF],
        }
    }

    fn from_classname(classname: &str) -> Self {
        let lower = classname.to_ascii_lowercase();
        let prefix = lower.split('_').next().unwrap_or("");
        match prefix {
            "trigger" => Self::Trigger,
            "target" => Self::Target,
            "func" => Self::Func,
            "light" => Self::Light,
            "item" | "weapon" | "ammo" | "team" if lower != "team_ctf_redspawn" && lower != "team_ctf_bluespawn" => {
                Self::Item
            }
            "fx" => Self::Fx,
            "npc" => Self::Npc,
            "misc" => Self::Misc,
            "info" if lower.starts_with("info_player") || lower == "info_jedimaster_start" => Self::Spawn,
            "info" => Self::Info,
            _ if lower.contains("spawn") || lower.starts_with("team_ctf_") => Self::Spawn,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphEntity {
    /// Index in the BSP entity lump (worldspawn is 0 and not listed here).
    pub bsp_index: usize,
    pub classname: String,
    /// Empty when the entity has no targetname.
    pub targetname: String,
    /// Plot position: `origin`, or the centre of an inline model's bounds.
    pub origin: [f32; 3],
    /// Inline-model bounds for brush entities (triggers, doors, ...).
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Inline BSP model number (`*N`) for brush entities, used to correlate
    /// this static entity with its live networked counterpart (brush-model
    /// entities carry the same number as `entityState_t.modelindex`).
    pub inline_model: Option<u32>,
    /// False when the entity has neither an origin nor a brush model.
    pub positioned: bool,
    pub category: EntityCategory,
    pub properties: Vec<(String, String)>,
    /// Link keys whose value matches no entity's targetname.
    pub dangling: Vec<(&'static str, String)>,
}

impl GraphEntity {
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .rev()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    pub fn size(&self) -> Option<[f32; 3]> {
        self.bounds.map(|(mins, maxs)| std::array::from_fn(|axis| maxs[axis] - mins[axis]))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GraphLink {
    /// Indices into `EntityGraph::entities`.
    pub from: usize,
    pub to: usize,
    pub key: &'static str,
}

/// Top-down raster of the world. `layers[0]` is every height; layers 1.. are
/// height bands of roughly equal floor area, lowest first.
#[derive(Debug, Clone)]
pub struct Footprint {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub width: usize,
    pub height: usize,
    pub layers: Vec<Vec<u8>>,
    /// Z range each band layer covers (`layers[i + 1]` uses `bands[i]`).
    pub bands: Vec<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct EntityGraph {
    pub entities: Vec<GraphEntity>,
    pub links: Vec<GraphLink>,
    /// Link indices leaving / entering each entity.
    pub outgoing: Vec<Vec<usize>>,
    pub incoming: Vec<Vec<usize>>,
    pub footprint: Option<Footprint>,
    /// XY extent of everything plottable (entities and floor plan).
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl EntityGraph {
    pub fn build(bsp: &Bsp) -> Self {
        let mut entities = Vec::new();
        for (bsp_index, entity) in bsp.entities.iter().enumerate() {
            let properties: Vec<(String, String)> = entity
                .properties
                .iter()
                .map(|(key, value)| {
                    (String::from_utf8_lossy(key).into_owned(), String::from_utf8_lossy(value).into_owned())
                })
                .collect();
            let lookup = |key: &str| {
                properties
                    .iter()
                    .rev()
                    .find(|(name, _)| name.eq_ignore_ascii_case(key))
                    .map(|(_, value)| value.as_str())
            };
            let classname = lookup("classname").unwrap_or("").trim().to_owned();
            if classname.eq_ignore_ascii_case("worldspawn") {
                continue;
            }
            let inline_model = lookup("model")
                .and_then(|model| model.trim().strip_prefix('*')?.parse::<u32>().ok())
                .filter(|&model| model > 0);
            let bounds = inline_model
                .and_then(|model| bsp.models.get(model as usize))
                .map(|model| (model.mins, model.maxs));
            let origin_key = lookup("origin").and_then(parse_vec3);
            let (origin, positioned) = match (bounds, origin_key) {
                (Some((mins, maxs)), _) => (std::array::from_fn(|axis| (mins[axis] + maxs[axis]) * 0.5), true),
                (None, Some(origin)) => (origin, true),
                (None, None) => ([0.0; 3], false),
            };
            entities.push(GraphEntity {
                bsp_index,
                targetname: lookup("targetname").unwrap_or("").trim().to_owned(),
                category: EntityCategory::from_classname(&classname),
                classname,
                origin,
                bounds,
                inline_model,
                positioned,
                properties,
                dangling: Vec::new(),
            });
        }

        // G_Find matches targetnames with Q_stricmp.
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, entity) in entities.iter().enumerate() {
            if !entity.targetname.is_empty() {
                by_name.entry(entity.targetname.to_ascii_lowercase()).or_default().push(index);
            }
        }
        let mut links = Vec::new();
        for from in 0..entities.len() {
            let mut dangling = Vec::new();
            for key in LINK_KEYS {
                let Some(value) = entities[from].property(key).map(str::trim).filter(|v| !v.is_empty()) else {
                    continue;
                };
                match by_name.get(&value.to_ascii_lowercase()) {
                    Some(targets) => {
                        links.extend(targets.iter().map(|&to| GraphLink { from, to, key }));
                    }
                    None => dangling.push((key, value.to_owned())),
                }
            }
            entities[from].dangling = dangling;
        }
        let mut outgoing = vec![Vec::new(); entities.len()];
        let mut incoming = vec![Vec::new(); entities.len()];
        for (index, link) in links.iter().enumerate() {
            outgoing[link.from].push(index);
            incoming[link.to].push(index);
        }

        let footprint = build_footprint(bsp);
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        let mut extend = |point: [f32; 2]| {
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        };
        for entity in entities.iter().filter(|entity| entity.positioned) {
            extend([entity.origin[0], entity.origin[1]]);
            if let Some((mins, maxs)) = entity.bounds {
                extend([mins[0], mins[1]]);
                extend([maxs[0], maxs[1]]);
            }
        }
        if let Some(footprint) = &footprint {
            extend(footprint.min);
            extend(footprint.max);
        }
        if min[0] > max[0] {
            min = [-1024.0; 2];
            max = [1024.0; 2];
        }
        Self { entities, links, outgoing, incoming, footprint, min, max }
    }

    /// Entities connected to `root` through links in either direction, as
    /// `(entity, column)`. Columns follow link direction from `root` (column 0):
    /// following a link forwards is +1, backwards -1, so siblings that share a
    /// trigger line up in one column. Breadth-first and capped at `limit` so a
    /// hub entity cannot explode the flow view.
    pub fn neighborhood(&self, root: usize, limit: usize) -> Vec<(usize, i32)> {
        let mut column = HashMap::from([(root, 0i32)]);
        let mut order = vec![root];
        let mut cursor = 0;
        while cursor < order.len() && order.len() < limit {
            let entity = order[cursor];
            cursor += 1;
            let here = column[&entity];
            let forwards = self.outgoing[entity].iter().map(|&link| (self.links[link].to, here + 1));
            let backwards = self.incoming[entity].iter().map(|&link| (self.links[link].from, here - 1));
            for (other, other_column) in forwards.chain(backwards) {
                if order.len() >= limit {
                    break;
                }
                if let std::collections::hash_map::Entry::Vacant(slot) = column.entry(other) {
                    slot.insert(other_column);
                    order.push(other);
                }
            }
        }
        order.into_iter().map(|entity| (entity, column[&entity])).collect()
    }
}

fn parse_vec3(text: &str) -> Option<[f32; 3]> {
    let mut parts = text.split_whitespace().map(|part| part.parse::<f32>().ok().filter(|v| v.is_finite()));
    Some([parts.next()??, parts.next()??, parts.next()??])
}

type Triangle = [[f32; 3]; 3];

/// World triangles that would draw (no sky / nodraw), patches by control grid.
fn world_triangles(bsp: &Bsp) -> Vec<Triangle> {
    let Some(world) = bsp.models.first() else {
        return Vec::new();
    };
    let mut triangles = Vec::new();
    for surface in bsp.surfaces.get(world.surfaces.clone()).unwrap_or(&[]) {
        if bsp
            .shaders
            .get(surface.shader)
            .is_some_and(|shader| shader.surface_flags & (SURF_SKY | SURF_NODRAW) != 0)
        {
            continue;
        }
        let Some(vertices) = bsp.vertices.get(surface.vertices.clone()) else {
            continue;
        };
        let position = |index: usize| vertices.get(index).map(|vertex| vertex.position);
        match surface.kind {
            SurfaceKind::Planar | SurfaceKind::Triangles => {
                let Some(indices) = bsp.indices.get(surface.indices.clone()) else {
                    continue;
                };
                for tri in indices.chunks_exact(3) {
                    if let (Some(a), Some(b), Some(c)) =
                        (position(tri[0] as usize), position(tri[1] as usize), position(tri[2] as usize))
                    {
                        triangles.push([a, b, c]);
                    }
                }
            }
            SurfaceKind::Patch => {
                let [width, height] = surface.patch_size;
                for row in 0..height.saturating_sub(1) {
                    for column in 0..width.saturating_sub(1) {
                        let at = |r: usize, c: usize| position(r * width + c);
                        if let (Some(a), Some(b), Some(c), Some(d)) =
                            (at(row, column), at(row, column + 1), at(row + 1, column + 1), at(row + 1, column))
                        {
                            triangles.push([a, b, c]);
                            triangles.push([a, c, d]);
                        }
                    }
                }
            }
            SurfaceKind::Flare => {}
        }
    }
    triangles
}

fn build_footprint(bsp: &Bsp) -> Option<Footprint> {
    let triangles = world_triangles(bsp);
    if triangles.is_empty() {
        return None;
    }
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for triangle in &triangles {
        for vertex in triangle {
            for axis in 0..2 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
    }
    let extent = [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)];
    let scale = FOOTPRINT_MAX_DIM / extent[0].max(extent[1]);
    let width = ((extent[0] * scale).ceil() as usize).clamp(1, FOOTPRINT_MAX_DIM as usize);
    let height = ((extent[1] * scale).ceil() as usize).clamp(1, FOOTPRINT_MAX_DIM as usize);

    // Classify once: (is_floor, is_wall, centroid z).
    struct Classified {
        floor: bool,
        wall: bool,
        z: f32,
    }
    let classified: Vec<Classified> = triangles
        .iter()
        .map(|[a, b, c]| {
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let normal = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            let nz = if length > 1e-6 { normal[2] / length } else { 0.0 };
            Classified {
                floor: length > 1e-6 && nz > FLOOR_NORMAL_Z,
                wall: length > 1e-6 && nz.abs() < FLOOR_NORMAL_Z,
                z: (a[2] + b[2] + c[2]) / 3.0,
            }
        })
        .collect();

    // Band edges: quantiles of floor-triangle height so each band holds a
    // similar amount of floor, whatever the map's vertical layout.
    let mut floor_z: Vec<f32> = classified.iter().filter(|t| t.floor).map(|t| t.z).collect();
    floor_z.sort_by(f32::total_cmp);
    let mut bands: Vec<[f32; 2]> = Vec::new();
    if floor_z.len() >= FOOTPRINT_MIN_BAND_FLOORS {
        let mut edges = vec![f32::MIN];
        for step in 1..FOOTPRINT_BANDS {
            edges.push(floor_z[floor_z.len() * step / FOOTPRINT_BANDS]);
        }
        edges.push(f32::MAX);
        for window in edges.windows(2) {
            if window[1] > window[0] {
                bands.push([window[0], window[1]]);
            }
        }
        if bands.len() < 2 {
            bands.clear();
        }
    }

    let layer_count = 1 + bands.len();
    let mut layers = vec![vec![FOOTPRINT_EMPTY; width * height]; layer_count];
    let to_pixel = |vertex: &[f32; 3]| -> [f32; 2] {
        // Screen rows run north-to-south, so Y is flipped.
        [(vertex[0] - min[0]) * scale, (max[1] - vertex[1]) * scale]
    };
    for (triangle, class) in triangles.iter().zip(&classified) {
        if !class.floor && !class.wall {
            continue;
        }
        let points = [to_pixel(&triangle[0]), to_pixel(&triangle[1]), to_pixel(&triangle[2])];
        let band = bands.iter().position(|[low, high]| class.z >= *low && class.z < *high);
        for layer in std::iter::once(0).chain(band.map(|band| band + 1)) {
            let pixels = &mut layers[layer];
            if class.floor {
                fill_triangle(pixels, width, height, points);
            } else {
                for edge in 0..3 {
                    draw_line(pixels, width, height, points[edge], points[(edge + 1) % 3]);
                }
            }
        }
    }
    Some(Footprint { min, max, width, height, layers, bands })
}

fn fill_triangle(pixels: &mut [u8], width: usize, height: usize, [a, b, c]: [[f32; 2]; 3]) {
    let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
    let max_x = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(width - 1);
    let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
    let max_y = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as usize).min(height - 1);
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 1e-6 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = ((b[0] - p[0]) * (c[1] - p[1]) - (b[1] - p[1]) * (c[0] - p[0])) / area;
            let w1 = ((c[0] - p[0]) * (a[1] - p[1]) - (c[1] - p[1]) * (a[0] - p[0])) / area;
            let w2 = 1.0 - w0 - w1;
            // Slightly generous so sub-pixel slivers still leave a mark.
            if w0 >= -0.02 && w1 >= -0.02 && w2 >= -0.02 {
                let pixel = &mut pixels[y * width + x];
                if *pixel == FOOTPRINT_EMPTY {
                    *pixel = FOOTPRINT_FLOOR;
                }
            }
        }
    }
    // Triangles thinner than a pixel can miss every centre; mark the vertices.
    for point in [a, b, c] {
        let (x, y) = (point[0] as usize, point[1] as usize);
        if x < width && y < height && pixels[y * width + x] == FOOTPRINT_EMPTY {
            pixels[y * width + x] = FOOTPRINT_FLOOR;
        }
    }
}

fn draw_line(pixels: &mut [u8], width: usize, height: usize, from: [f32; 2], to: [f32; 2]) {
    let steps = (to[0] - from[0]).abs().max((to[1] - from[1]).abs()).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = from[0] + (to[0] - from[0]) * t;
        let y = from[1] + (to[1] - from[1]) * t;
        if x >= 0.0 && y >= 0.0 && (x as usize) < width && (y as usize) < height {
            pixels[y as usize * width + x as usize] = FOOTPRINT_WALL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_follow_classname_prefix() {
        assert_eq!(EntityCategory::from_classname("trigger_multiple"), EntityCategory::Trigger);
        assert_eq!(EntityCategory::from_classname("info_player_deathmatch"), EntityCategory::Spawn);
        assert_eq!(EntityCategory::from_classname("info_null"), EntityCategory::Info);
        assert_eq!(EntityCategory::from_classname("weapon_bryar_pistol"), EntityCategory::Item);
        assert_eq!(EntityCategory::from_classname("func_door"), EntityCategory::Func);
        assert_eq!(EntityCategory::from_classname("NPC_Stormtrooper"), EntityCategory::Npc);
    }

    #[test]
    fn vec3_parsing_rejects_short_and_bad_values() {
        assert_eq!(parse_vec3("1 -2.5 300"), Some([1.0, -2.5, 300.0]));
        assert_eq!(parse_vec3("1 2"), None);
        assert_eq!(parse_vec3("a b c"), None);
    }

    #[test]
    fn fill_and_lines_mark_pixels() {
        let mut pixels = vec![FOOTPRINT_EMPTY; 16 * 16];
        fill_triangle(&mut pixels, 16, 16, [[1.0, 1.0], [14.0, 1.0], [1.0, 14.0]]);
        assert_eq!(pixels[3 * 16 + 3], FOOTPRINT_FLOOR);
        assert_eq!(pixels[14 * 16 + 14], FOOTPRINT_EMPTY);
        draw_line(&mut pixels, 16, 16, [0.0, 15.0], [15.0, 15.0]);
        assert_eq!(pixels[15 * 16 + 7], FOOTPRINT_WALL);
    }
}

