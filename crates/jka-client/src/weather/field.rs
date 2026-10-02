//! CPU weather field: an immutable map-wide RGBA32F grid built once per map on a
//! worker thread. One texel per XZ cell:
//!
//! * R = highest rain-blocking Y (rain cover, exposure, splash placement)
//! * G = highest physical top-facing rendered surface Y
//! * B = flat-area score: how good a place this exposed surface is for scattered
//!   puddles (large, level, paved, away from its own edge)
//! * A = basin score: coherent terrace enclosed by higher rim (major puddles)
//!
//! "Blocks rain" and "can hold standing water" are deliberately separate. Many
//! authored JKA planar shader faces are real visible floors without being part of
//! the collision subset used for rain cover.

use super::rain::{WeatherOcclusionInfo, WEATHER_NO_SURFACE_HEIGHT};
use crate::scene::{WeatherOcclusionSource, WeatherOcclusionTriangle};
use glam::Vec3;
use std::ops::Range;

const TARGET_TEXEL: f32 = 8.0;
const MIN_AXIS: u32 = 64;
const MAX_AXIS: u32 = 1024;

// Multi-layer topography. A JKA BSP is not a single terrain heightfield: floors,
// catwalks, roofs and trims overlap at the same X/Z. Keeping only the highest
// surface fragments lower-but-exposed terraces whenever a small raised insert
// crosses them, so keep the highest few TOP-facing layers per texel and classify
// connected same-height terraces as a unit.
const LAYER_CAP: usize = 4;
const LAYER_HEIGHT_TOLERANCE: f32 = 0.75;
const MIN_PHYSICAL_UPNESS: f32 = 0.985;

// Basin thresholds were verified offline against ffa5.bsp: landing_pad forms one
// coherent -16-unit terrace with a 0-unit spill rim instead of sparse islands.
const BOUNDARY_HEIGHT_EPSILON: f32 = 1.5;
const MAX_RELATED_STEP: f32 = 96.0;
const MIN_COMPONENT_TEXELS: usize = 8;
const BASIN_MIN_HIGHER_FRACTION: f32 = 0.78;
const BASIN_MAX_DRAIN_FRACTION: f32 = 0.05;
const BASIN_MIN_SPILL_DEPTH: f32 = 2.0;

// Flat-area (scattered puddle) scoring, in world units. A surface qualifies when
// it is a big level piece of ground: the area ramps in between MIN and FULL, and
// a strip narrower than a footpath or a ramp that climbs more than a few units
// is rejected no matter how large it is.
const FLAT_AREA_MIN: f32 = 16_000.0;
const FLAT_AREA_FULL: f32 = 90_000.0;
const FLAT_THICKNESS_MIN: f32 = 24.0;
const FLAT_THICKNESS_FULL: f32 = 64.0;
const FLAT_HEIGHT_RANGE_FULL: f32 = 2.5;
const FLAT_HEIGHT_RANGE_NONE: f32 = 8.0;
// Both scores thin out towards the boundary of their terrace, so standing water
// recedes from a kerb, ramp or drop-off in an irregular shoreline instead of
// stopping on a hard straight line where the ground stops being flat.
const EDGE_FADE_START: f32 = 2.0;
const EDGE_FADE_END: f32 = 28.0;

type Field = Vec<[f32; 4]>;

fn smooth_unit(value: f32, low: f32, high: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn axis_size(extent: f32) -> u32 {
    if !extent.is_finite() || extent <= 1.0 {
        return MIN_AXIS;
    }
    ((extent / TARGET_TEXEL).ceil() as u32).clamp(MIN_AXIS, MAX_AXIS)
}

/// Builds the weather field for a map, or `None` when there is no usable footprint.
pub(super) fn build_heightfield(
    source: &WeatherOcclusionSource,
) -> Option<(WeatherOcclusionInfo, Vec<[f32; 4]>)> {
    let grid = Grid::for_source(source)?;

    let mut field: Field = vec![
        [WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0];
        grid.width * grid.height
    ];
    rasterize_rain_blockers(&grid, source, &mut field);

    let layers = build_topography(&grid, source);
    for (texel, stack) in field.iter_mut().zip(&layers) {
        if stack[0].is_valid() {
            texel[1] = stack[0].height;
        }
    }
    classify_terraces(&grid, &layers, &mut field);

    let info = WeatherOcclusionInfo {
        min_xz: source.min_xz,
        inv_extent_xz: [
            1.0 / (source.max_xz[0] - source.min_xz[0]),
            1.0 / (source.max_xz[1] - source.min_xz[1]),
        ],
        width: grid.width as u32,
        height: grid.height as u32,
    };
    Some((info, field))
}

/// Uniform XZ raster the field is generated on. Render-space X/Z, matching the
/// shaders: render Z is JKA -Y.
#[derive(Clone, Copy)]
struct Grid {
    min: [f32; 2],
    step: [f32; 2],
    width: usize,
    height: usize,
}

impl Grid {
    fn for_source(source: &WeatherOcclusionSource) -> Option<Self> {
        let extent_x = source.max_xz[0] - source.min_xz[0];
        let extent_z = source.max_xz[1] - source.min_xz[1];
        if !extent_x.is_finite()
            || !extent_z.is_finite()
            || extent_x <= 1.0
            || extent_z <= 1.0
            || (source.brushes.is_empty()
                && source.triangles.is_empty()
                && source.topography_triangles.is_empty())
        {
            return None;
        }
        let width = axis_size(extent_x);
        let height = axis_size(extent_z);
        Some(Self {
            min: source.min_xz,
            step: [extent_x / width as f32, extent_z / height as f32],
            width: width as usize,
            height: height as usize,
        })
    }

    fn texel_area(&self) -> f32 {
        self.step[0] * self.step[1]
    }

    fn mean_step(&self) -> f32 {
        0.5 * (self.step[0] + self.step[1])
    }

    fn offset(&self, x: usize, z: usize) -> usize {
        z * self.width + x
    }

    fn centre(&self, x: usize, z: usize) -> [f32; 2] {
        [
            self.min[0] + (x as f32 + 0.5) * self.step[0],
            self.min[1] + (z as f32 + 0.5) * self.step[1],
        ]
    }

    /// Texel range along `axis` (0 = X, 1 = Z) touched by the interval `lo..=hi`.
    fn span(&self, axis: usize, lo: f32, hi: f32) -> Range<usize> {
        let count = if axis == 0 { self.width } else { self.height };
        let first = (((lo - self.min[axis]) / self.step[axis]).floor() as i32)
            .clamp(0, count as i32 - 1) as usize;
        let last = (((hi - self.min[axis]) / self.step[axis]).ceil() as i32)
            .clamp(0, count as i32) as usize;
        first..last
    }

    /// Calls `visit(texel offset, surface height)` for every texel centre covered
    /// by the triangle's XZ footprint. Positions are JKA-space (Z up).
    fn rasterize_triangle(&self, positions: &[[f32; 3]; 3], mut visit: impl FnMut(usize, f32)) {
        let render_xz = |p: [f32; 3]| [p[0], -p[1]];
        let a = render_xz(positions[0]);
        let b = render_xz(positions[1]);
        let c = render_xz(positions[2]);
        let xs = self.span(0, a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
        let zs = self.span(1, a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
        if xs.is_empty() || zs.is_empty() {
            return;
        }
        let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denom.abs() <= 1.0e-8 {
            return;
        }
        for z_index in zs {
            for x_index in xs.clone() {
                let [x, z] = self.centre(x_index, z_index);
                let w0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (z - c[1])) / denom;
                let w1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (z - c[1])) / denom;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let surface_y =
                    w0 * positions[0][2] + w1 * positions[1][2] + w2 * positions[2][2];
                visit(self.offset(x_index, z_index), surface_y);
            }
        }
    }
}

/// R channel: solid/terrain brushes plus tessellated patch/triangle blockers.
fn rasterize_rain_blockers(grid: &Grid, source: &WeatherOcclusionSource, field: &mut Field) {
    for brush in &source.brushes {
        let xs = grid.span(0, brush.mins_xy[0], brush.maxs_xy[0]);
        let zs = grid.span(1, -brush.maxs_xy[1], -brush.mins_xy[1]);
        if xs.is_empty() || zs.is_empty() {
            continue;
        }
        for z_index in zs {
            let jka_y = -grid.centre(0, z_index)[1];
            for x_index in xs.clone() {
                let jka_x = grid.centre(x_index, 0)[0];
                let mut lower = f32::NEG_INFINITY;
                let mut upper = f32::INFINITY;
                let mut inside = true;
                for side in &brush.sides {
                    let [nx, ny, nz] = side.normal;
                    let remaining = side.distance - nx * jka_x - ny * jka_y;
                    if nz.abs() <= 1.0e-6 {
                        if remaining < -0.05 {
                            inside = false;
                            break;
                        }
                    } else {
                        let bound = remaining / nz;
                        if nz > 0.0 {
                            upper = upper.min(bound);
                        } else {
                            lower = lower.max(bound);
                        }
                    }
                }
                if !inside || !upper.is_finite() || lower > upper + 0.05 {
                    continue;
                }
                let texel = &mut field[grid.offset(x_index, z_index)][0];
                *texel = texel.max(upper);
            }
        }
    }

    for triangle in &source.triangles {
        grid.rasterize_triangle(&triangle.positions, |offset, surface_y| {
            field[offset][0] = field[offset][0].max(surface_y);
        });
    }
}

#[derive(Clone, Copy)]
struct Layer {
    height: f32,
    upness: f32,
    holds_puddles: bool,
}

const EMPTY_LAYER: Layer = Layer {
    height: WEATHER_NO_SURFACE_HEIGHT,
    upness: 0.0,
    holds_puddles: false,
};

impl Layer {
    fn is_valid(self) -> bool {
        self.height > WEATHER_NO_SURFACE_HEIGHT + 1.0 && self.height.is_finite()
    }
}

type LayerStack = [Layer; LAYER_CAP];

fn insert_layer(stack: &mut LayerStack, candidate: Layer) {
    // Coplanar triangles merge into one layer.
    for layer in stack.iter_mut() {
        if layer.is_valid() && (layer.height - candidate.height).abs() <= LAYER_HEIGHT_TOLERANCE {
            layer.height = layer.height.max(candidate.height);
            layer.upness = layer.upness.max(candidate.upness);
            layer.holds_puddles &= candidate.holds_puddles;
            return;
        }
    }
    // Layers stay ordered highest to lowest. A surface below the retained
    // capacity is irrelevant to any rain-exposed puddle and is dropped.
    let Some(insert_at) = stack.iter().position(|layer| candidate.height > layer.height) else {
        return;
    };
    stack.copy_within(insert_at..LAYER_CAP - 1, insert_at + 1);
    stack[insert_at] = candidate;
}

/// Rasterizes every visible non-sky world surface into the layer stacks. Physical
/// winding is used rather than `abs(z)`: on RBSP world faces a physical
/// upward-facing floor has negative raw-Z winding. That rejects ceiling undersides
/// while still allowing explicit planar shader faces such as taspir/landing_pad.
fn build_topography(grid: &Grid, source: &WeatherOcclusionSource) -> Vec<LayerStack> {
    let mut layers = vec![[EMPTY_LAYER; LAYER_CAP]; grid.width * grid.height];
    for triangle in &source.topography_triangles {
        let physical_upness = physical_upness(triangle);
        if physical_upness < MIN_PHYSICAL_UPNESS {
            continue;
        }
        grid.rasterize_triangle(&triangle.positions, |offset, height| {
            insert_layer(
                &mut layers[offset],
                Layer { height, upness: physical_upness, holds_puddles: triangle.holds_puddles },
            );
        });
    }
    layers
}

fn physical_upness(triangle: &WeatherOcclusionTriangle) -> f32 {
    let ab = Vec3::from_array(triangle.positions[1]) - Vec3::from_array(triangle.positions[0]);
    let ac = Vec3::from_array(triangle.positions[2]) - Vec3::from_array(triangle.positions[0]);
    let face = ab.cross(ac);
    (-face.z / face.length().max(1.0e-6)).clamp(0.0, 1.0)
}

/// Connected same-height terraces over the layer stacks.
struct Terraces<'a> {
    grid: &'a Grid,
    layers: &'a [LayerStack],
}

/// Everything the basin and flat-area rules need to know about one terrace.
#[derive(Default)]
struct TerraceStats {
    texels: usize,
    edges: u32,
    lower_edges: u32,
    open_edges: u32,
    higher_deltas: Vec<f32>,
    min_height: f32,
    max_height: f32,
}

impl TerraceStats {
    /// Score for a terrace ringed by a higher rim with almost no drainage.
    /// Depth controls how soon the basin floods as accumulation rises: a 16-unit
    /// landing_pad depression reaches full score, a very shallow lip only appears
    /// under heavy rain.
    fn basin_score(&mut self) -> Option<f32> {
        if self.edges == 0 || self.higher_deltas.is_empty() {
            return None;
        }
        let higher_fraction = self.higher_deltas.len() as f32 / self.edges as f32;
        let drain_fraction = (self.lower_edges + self.open_edges) as f32 / self.edges as f32;
        self.higher_deltas.sort_by(|a, b| a.total_cmp(b));
        let spill_index = (((self.higher_deltas.len() - 1) as f32) * 0.10).floor() as usize;
        let spill_depth = self.higher_deltas[spill_index];
        if higher_fraction < BASIN_MIN_HIGHER_FRACTION
            || drain_fraction > BASIN_MAX_DRAIN_FRACTION
            || spill_depth < BASIN_MIN_SPILL_DEPTH
        {
            return None;
        }
        Some(smooth_unit(spill_depth, 1.5, 10.0))
    }

    /// Score for a big level piece of ground. Thickness is the area/perimeter
    /// ratio: it equals the width of a strip and half the side of a square, so
    /// footpaths and catwalks fail while plazas and roads pass.
    fn flat_area_score(&self, grid: &Grid) -> f32 {
        if self.edges == 0 {
            return 0.0;
        }
        let area = self.texels as f32 * grid.texel_area();
        let thickness = 2.0 * self.texels as f32 * grid.texel_area()
            / (self.edges as f32 * grid.mean_step());
        let height_range = self.max_height - self.min_height;
        smooth_unit(area, FLAT_AREA_MIN, FLAT_AREA_FULL)
            * smooth_unit(thickness, FLAT_THICKNESS_MIN, FLAT_THICKNESS_FULL)
            * (1.0 - smooth_unit(height_range, FLAT_HEIGHT_RANGE_FULL, FLAT_HEIGHT_RANGE_NONE))
    }
}

impl Terraces<'_> {
    fn layer(&self, cell: usize, layer_index: usize) -> Layer {
        self.layers[cell][layer_index]
    }

    /// The four edge-adjacent texels; `None` where the grid ends.
    fn neighbours(&self, cell: usize) -> [Option<usize>; 4] {
        let width = self.grid.width;
        let x = cell % width;
        let z = cell / width;
        [
            (x > 0).then(|| cell - 1),
            (x + 1 < width).then(|| cell + 1),
            (z > 0).then(|| cell - width),
            (z + 1 < self.grid.height).then(|| cell + width),
        ]
    }

    /// The layer of `cell` that continues a terrace at `target_height`.
    fn matching_layer(&self, cell: usize, target_height: f32) -> Option<usize> {
        let mut best = None;
        let mut best_delta = f32::INFINITY;
        for (index, layer) in self.layers[cell].iter().enumerate() {
            if !layer.is_valid() {
                continue;
            }
            let delta = (layer.height - target_height).abs();
            if delta < best_delta {
                best_delta = delta;
                best = Some(index);
            }
        }
        if best_delta <= LAYER_HEIGHT_TOLERANCE {
            best
        } else {
            None
        }
    }

    fn measure(&self, members: &[usize], stats: &mut TerraceStats) {
        stats.texels = members.len();
        stats.min_height = f32::INFINITY;
        stats.max_height = f32::NEG_INFINITY;
        for &member in members {
            let (cell, layer_index) = (member / LAYER_CAP, member % LAYER_CAP);
            let terrace_height = self.layer(cell, layer_index).height;
            stats.min_height = stats.min_height.min(terrace_height);
            stats.max_height = stats.max_height.max(terrace_height);

            for neighbour in self.neighbours(cell) {
                let Some(neighbour) = neighbour else {
                    stats.edges += 1;
                    stats.open_edges += 1;
                    continue;
                };
                if self.matching_layer(neighbour, terrace_height).is_some() {
                    // Same terrace continues through this edge, including
                    // beneath a small higher insert/trim layer.
                    continue;
                }
                stats.edges += 1;
                let closest = self.layers[neighbour]
                    .iter()
                    .filter(|layer| layer.is_valid())
                    .map(|layer| layer.height)
                    .filter(|height| (height - terrace_height).abs() <= MAX_RELATED_STEP)
                    .min_by(|a, b| {
                        (a - terrace_height).abs().total_cmp(&(b - terrace_height).abs())
                    });
                let Some(neighbour_height) = closest else {
                    stats.open_edges += 1;
                    continue;
                };
                let delta = neighbour_height - terrace_height;
                if delta > BOUNDARY_HEIGHT_EPSILON {
                    stats.higher_deltas.push(delta);
                } else if delta < -BOUNDARY_HEIGHT_EPSILON {
                    stats.lower_edges += 1;
                }
            }
        }
    }
}

/// Classifies coherent flat terraces instead of individual texels, writing the
/// basin score (A) and the flat-area score (B) for the highest exposed layer, each
/// faded towards the edge of its terrace.
fn classify_terraces(grid: &Grid, layers: &[LayerStack], field: &mut Field) {
    let terraces = Terraces { grid, layers };
    let mut visited = vec![false; layers.len() * LAYER_CAP];
    let mut stack = Vec::<usize>::new();
    let mut members = Vec::<usize>::new();
    let mut stats = TerraceStats::default();
    // Terrace id per texel for the edge fade; 0 = not a scored terrace.
    let mut label_of = vec![0_u32; layers.len()];
    let mut basin_score = vec![0.0_f32; layers.len()];
    let mut flat_score = vec![0.0_f32; layers.len()];
    let mut next_label = 1_u32;

    for cell in 0..layers.len() {
        for layer_index in 0..LAYER_CAP {
            let node = cell * LAYER_CAP + layer_index;
            if visited[node] || !terraces.layer(cell, layer_index).is_valid() {
                continue;
            }

            visited[node] = true;
            stack.clear();
            members.clear();
            stack.push(node);
            while let Some(current) = stack.pop() {
                members.push(current);
                let current_cell = current / LAYER_CAP;
                let current_height = terraces.layer(current_cell, current % LAYER_CAP).height;
                for neighbour in terraces.neighbours(current_cell).into_iter().flatten() {
                    let Some(neighbour_layer) = terraces.matching_layer(neighbour, current_height)
                    else {
                        continue;
                    };
                    let neighbour_node = neighbour * LAYER_CAP + neighbour_layer;
                    if !visited[neighbour_node] {
                        visited[neighbour_node] = true;
                        stack.push(neighbour_node);
                    }
                }
            }
            if members.len() < MIN_COMPONENT_TEXELS {
                continue;
            }

            stats.higher_deltas.clear();
            stats.edges = 0;
            stats.lower_edges = 0;
            stats.open_edges = 0;
            terraces.measure(&members, &mut stats);
            let basin = stats.basin_score();
            let flat = stats.flat_area_score(grid);
            if basin.is_none() && flat <= 0.0 {
                continue;
            }

            // Only the highest physical top-facing surface at an X/Z may receive
            // direct rainfall. Lower terraces stay in the solve so small overlying
            // inserts do not fragment the terrace, but never render as puddles
            // through cover. Rain exposure is deliberately NOT baked in: the shader
            // evaluates cover against the actual fragment height every frame.
            let label = next_label;
            next_label += 1;
            for &member in &members {
                if member % LAYER_CAP != 0 {
                    continue;
                }
                let member_cell = member / LAYER_CAP;
                label_of[member_cell] = label;
                if let Some(basin) = basin {
                    basin_score[member_cell] = basin;
                }
                if flat > 0.0 && terraces.layer(member_cell, 0).holds_puddles {
                    flat_score[member_cell] = flat;
                }
            }
        }
    }

    let edge_distance = distance_from_terrace_edge(grid, &label_of);
    for (cell, texel) in field.iter_mut().enumerate() {
        if label_of[cell] == 0 {
            continue;
        }
        let fade = smooth_unit(edge_distance[cell], EDGE_FADE_START, EDGE_FADE_END);
        texel[2] = flat_score[cell] * fade;
        texel[3] = basin_score[cell] * fade;
    }
}

/// Distance in world units from every labelled texel to the nearest texel with a
/// different label (or the edge of the grid), by 3-4 chamfer.
fn distance_from_terrace_edge(grid: &Grid, label: &[u32]) -> Vec<f32> {
    const AXIS: u32 = 3;
    const DIAGONAL: u32 = 4;
    let (width, height) = (grid.width, grid.height);
    let mut distance = vec![u32::MAX; label.len()];
    for z in 0..height {
        for x in 0..width {
            let cell = z * width + x;
            if label[cell] == 0 {
                distance[cell] = 0;
                continue;
            }
            let differs = |other: Option<usize>| other.is_none_or(|o| label[o] != label[cell]);
            let on_edge = differs((x > 0).then(|| cell - 1))
                || differs((x + 1 < width).then(|| cell + 1))
                || differs((z > 0).then(|| cell - width))
                || differs((z + 1 < height).then(|| cell + width));
            if on_edge {
                distance[cell] = AXIS;
            }
        }
    }

    let relax = |distance: &mut [u32], cell: usize, other: usize, cost: u32| {
        if distance[other] != u32::MAX {
            distance[cell] = distance[cell].min(distance[other] + cost);
        }
    };
    for z in 0..height {
        for x in 0..width {
            let cell = z * width + x;
            if x > 0 {
                relax(&mut distance, cell, cell - 1, AXIS);
            }
            if z > 0 {
                relax(&mut distance, cell, cell - width, AXIS);
                if x > 0 {
                    relax(&mut distance, cell, cell - width - 1, DIAGONAL);
                }
                if x + 1 < width {
                    relax(&mut distance, cell, cell - width + 1, DIAGONAL);
                }
            }
        }
    }
    for z in (0..height).rev() {
        for x in (0..width).rev() {
            let cell = z * width + x;
            if x + 1 < width {
                relax(&mut distance, cell, cell + 1, AXIS);
            }
            if z + 1 < height {
                relax(&mut distance, cell, cell + width, AXIS);
                if x > 0 {
                    relax(&mut distance, cell, cell + width - 1, DIAGONAL);
                }
                if x + 1 < width {
                    relax(&mut distance, cell, cell + width + 1, DIAGONAL);
                }
            }
        }
    }

    let units_per_step = grid.mean_step() / AXIS as f32;
    distance
        .into_iter()
        .map(|d| if d == u32::MAX { f32::MAX } else { d as f32 * units_per_step })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level up-facing quad in JKA space. Winding is the RBSP convention: an
    /// upward-facing floor has negative raw-Z winding.
    fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32, holds_puddles: bool) -> [WeatherOcclusionTriangle; 2] {
        [
            WeatherOcclusionTriangle {
                positions: [[x0, y0, z], [x0, y1, z], [x1, y0, z]],
                holds_puddles,
            },
            WeatherOcclusionTriangle {
                positions: [[x1, y1, z], [x1, y0, z], [x0, y1, z]],
                holds_puddles,
            },
        ]
    }

    fn source(triangles: Vec<WeatherOcclusionTriangle>, extent: f32) -> WeatherOcclusionSource {
        WeatherOcclusionSource {
            min_xz: [0.0, -extent],
            max_xz: [extent, 0.0],
            brushes: Vec::new(),
            triangles: Vec::new(),
            topography_triangles: triangles,
        }
    }

    fn texel_at(info: &WeatherOcclusionInfo, field: &[[f32; 4]], x: f32, jka_y: f32) -> [f32; 4] {
        let u = (x - info.min_xz[0]) * info.inv_extent_xz[0];
        let v = (-jka_y - info.min_xz[1]) * info.inv_extent_xz[1];
        let px = ((u * info.width as f32) as usize).min(info.width as usize - 1);
        let pz = ((v * info.height as f32) as usize).min(info.height as usize - 1);
        field[pz * info.width as usize + px]
    }

    #[test]
    fn large_level_plaza_scores_high_in_its_interior_and_fades_at_its_edge() {
        let source = source(quad(0.0, 0.0, 800.0, 800.0, 0.0, true).to_vec(), 800.0);
        let (info, field) = build_heightfield(&source).unwrap();
        let centre = texel_at(&info, &field, 400.0, 400.0);
        let edge = texel_at(&info, &field, 4.0, 400.0);
        assert!(centre[2] > 0.98, "interior flat-area score {}", centre[2]);
        assert!(edge[2] < 0.5, "edge flat-area score {}", edge[2]);
        assert_eq!(centre[3], 0.0, "an open plaza is not a basin");
        assert!((centre[1] - 0.0).abs() < 0.01, "top surface height is published");
    }

    #[test]
    fn small_pads_and_narrow_strips_never_score() {
        // 400x400 map with only a 60x60 pad and a 30-unit-wide 800-unit strip.
        let mut triangles = quad(0.0, 0.0, 60.0, 60.0, 0.0, true).to_vec();
        triangles.extend(quad(0.0, 100.0, 800.0, 130.0, 0.0, true));
        let source = source(triangles, 800.0);
        let (info, field) = build_heightfield(&source).unwrap();
        let best = field.iter().map(|texel| texel[2]).fold(0.0, f32::max);
        assert!(best < 0.01, "pad/strip scored {best}");
        assert!(texel_at(&info, &field, 30.0, 30.0)[1] > WEATHER_NO_SURFACE_HEIGHT + 1.0);
    }

    #[test]
    fn ramps_and_water_never_score() {
        // Gentle ramp: passes the upness gate but climbs faster than a terrace
        // tolerates, so every texel is its own tiny terrace.
        let ramp = |x: f32| x * 0.1;
        let a = [0.0, 0.0, ramp(0.0)];
        let b = [0.0, 800.0, ramp(0.0)];
        let c = [800.0, 0.0, ramp(800.0)];
        let d = [800.0, 800.0, ramp(800.0)];
        let ramp_triangles = vec![
            WeatherOcclusionTriangle { positions: [a, b, c], holds_puddles: true },
            WeatherOcclusionTriangle { positions: [d, c, b], holds_puddles: true },
        ];
        let (_, field) = build_heightfield(&source(ramp_triangles, 800.0)).unwrap();
        assert!(field.iter().all(|texel| texel[2] == 0.0), "ramp scored");

        let lake = quad(0.0, 0.0, 800.0, 800.0, 0.0, false).to_vec();
        let (info, field) = build_heightfield(&source(lake, 800.0)).unwrap();
        assert!(field.iter().all(|texel| texel[2] == 0.0), "water scored");
        assert!(texel_at(&info, &field, 400.0, 400.0)[1] > WEATHER_NO_SURFACE_HEIGHT + 1.0);
    }

    #[test]
    fn recessed_terrace_ringed_by_a_kerb_is_a_basin() {
        // 1000x1000 map: higher ground everywhere except a 400x400 pit 16 units
        // lower in the middle.
        let mut triangles = Vec::new();
        triangles.extend(quad(0.0, 0.0, 300.0, 1000.0, 0.0, true));
        triangles.extend(quad(700.0, 0.0, 1000.0, 1000.0, 0.0, true));
        triangles.extend(quad(300.0, 0.0, 700.0, 300.0, 0.0, true));
        triangles.extend(quad(300.0, 700.0, 700.0, 1000.0, 0.0, true));
        triangles.extend(quad(300.0, 300.0, 700.0, 700.0, -16.0, true));
        let (info, field) = build_heightfield(&source(triangles, 1000.0)).unwrap();
        let pit = texel_at(&info, &field, 500.0, 500.0);
        let rim = texel_at(&info, &field, 100.0, 500.0);
        assert!(pit[3] > 0.99, "pit basin score {}", pit[3]);
        assert_eq!(rim[3], 0.0, "raised ground is not a basin");
        assert!((pit[1] + 16.0).abs() < 0.01);
    }

    #[test]
    fn basin_and_flat_scores_thin_out_toward_their_terrace_edge() {
        let mut triangles = Vec::new();
        triangles.extend(quad(0.0, 0.0, 300.0, 1000.0, 0.0, true));
        triangles.extend(quad(700.0, 0.0, 1000.0, 1000.0, 0.0, true));
        triangles.extend(quad(300.0, 0.0, 700.0, 300.0, 0.0, true));
        triangles.extend(quad(300.0, 700.0, 700.0, 1000.0, 0.0, true));
        triangles.extend(quad(300.0, 300.0, 700.0, 700.0, -16.0, true));
        let (info, field) = build_heightfield(&source(triangles, 1000.0)).unwrap();
        // Walk from the middle of the pit toward its kerb: the score must fall
        // monotonically to near zero rather than stop on a hard line.
        let mut previous = f32::INFINITY;
        for x in [500.0, 340.0, 328.0, 316.0, 306.0] {
            let texel = texel_at(&info, &field, x, 500.0);
            assert!(texel[3] <= previous + 1.0e-6, "basin score rose toward the kerb at x={x}");
            previous = texel[3];
        }
        assert!(previous < 0.5, "basin score at the kerb {previous}");
        let flat = texel_at(&info, &field, 306.0, 500.0)[2];
        assert!(flat < 0.5, "flat-area score at the kerb {flat}");
    }

    #[test]
    fn chamfer_distance_measures_from_the_terrace_edge() {
        let grid = Grid { min: [0.0; 2], step: [8.0; 2], width: 9, height: 9 };
        let mut label = vec![1_u32; 81];
        // A different terrace in the right-hand column.
        for z in 0..9 {
            label[z * 9 + 8] = 2;
        }
        let distance = distance_from_terrace_edge(&grid, &label);
        // Column 0 touches the grid edge; column 7 touches terrace 2.
        assert_eq!(distance[4 * 9], 8.0);
        assert_eq!(distance[4 * 9 + 7], 8.0);
        assert_eq!(distance[4 * 9 + 4], 32.0);
    }
}
