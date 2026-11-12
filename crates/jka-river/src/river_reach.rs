//! `r_riverCaptureMode 1` (Option A): BSP-aware input for WaterAdvanced's
//! five depth captures. Not an Unreal system; it supplies what Unreal's
//! river is given by hand. `ShallowWaterRiverComponent` captures authored
//! landscape actors (`BottomContourLandscapeActors`) from above, and
//! authored bridges/obstacles (`BottomContourActors`, tags) from above and
//! below; Grid2D_SW_River stage 6 (`capture.wgsl`) then passes water under an
//! actor whose underside is above the river spline's WaterInfo height. A BSP
//! has neither the groups nor a spline, so both come from collision:
//!
//! 1. Columns. Each cell centre's vertical line through the solid brushes
//!    (exact plane intervals) and solid mesh triangles splits into open gaps.
//! 2. Reach. A breadth-first flood from the source's gap moves water into one
//!    gap of each neighbouring column: a continuous floor within a cell-sized
//!    step, else a drop into the gap below, else (ponding) the lowest floor it
//!    can rise to. No floor above the source level plus a run-up margin is
//!    reachable, and a sealed lower floor is never entered.
//! 3. Groups. Anything starting above the lowest reached floor of some column
//!    (bridge decks, roofs, arches, tunnel ceilings) is an Actor, captured from
//!    above and below. Everything else that can hold water is Landscape.
//! 4. Reference. The lowest reached floor of each column is the WaterInfo
//!    height for stage 6: an actor above it is passed under; one resting on
//!    it (a footing, a crate, a deck end on its abutment) stays solid.
//!
//! The source level bounds reach only; it never clips geometry, so a bridge
//! below the source but above the local river is passed under however far
//! downstream it is. Stacked waterways (two reachable floors in one column)
//! keep the lowest: the one-layer solver cannot carry both. They're counted.
//! All work happens once at spawn; nothing runs per solver step.

use crate::river_capture::wall_slab;
use jka_assets::bsp::river::{CaptureBrush, CaptureGeometry, MeshFace};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};

static MODE: AtomicU8 = AtomicU8::new(1);

/// 0 legacy source-level capture (all Landscape); 1 this classification.
/// Read when a river is placed; existing rivers keep their bed.
pub fn capture_mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

pub fn set_capture_mode(mode: u8) {
    MODE.store(mode.min(1), Ordering::Relaxed);
}

/// Reachable floors may sit this far (map units) above the source level.
pub const LEVEL_MARGIN: f32 = 16.0;
/// Narrower openings (map units) are closed: touching or near-touching solids.
const CLEARANCE: f32 = 2.0;
/// A drop into a lower gap needs this much headroom over the upper floor, so
/// the underside of a thin sloped slab isn't taken for a waterfall.
const PASSAGE: f32 = 64.0;
/// Zero-thickness mesh actors get this thickness: stage 6 needs top > under.
const MESH_THICKNESS: f32 = 1.0;
/// An item is above a floor when it starts more than this over it.
const ABOVE: f32 = 1.0;
/// The reference sits this far over the floor, so an actor resting on the
/// floor stays an obstacle (stage 6 passes under only when under >= height).
const REFERENCE_LIFT: f32 = 1.0;
/// Unreached cells take a reached neighbour's reference this many cells out:
/// the flood samples cell centres, the capture rasterizes finer.
const REFERENCE_DILATION: u32 = 2;
/// Unreached and undilated: no water expected, every actor is solid.
pub const NO_REFERENCE_CM: f32 = 1.0e30;
const BLOCK: u32 = 4;
const CHUNK: u32 = 64;
const UNSET: u32 = u32::MAX;
const UNREACHED: u32 = u32::MAX;

/// A cell lattice in map units: cell (x, y) spans `min + [x, x+1) * cell`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub min: [f64; 2],
    pub cell: f64,
    pub dims: [u32; 2],
}

impl Region {
    fn validate(self) -> Result<(), String> {
        if !(self.cell.is_finite() && self.cell > 0.0)
            || self.min.iter().any(|v| !v.is_finite())
            || self.dims.contains(&0)
        {
            return Err("invalid river capture classification region".into());
        }
        Ok(())
    }

    fn centre(self, c: [u32; 2]) -> [f32; 2] {
        std::array::from_fn(|a| (self.min[a] + (f64::from(c[a]) + 0.5) * self.cell) as f32)
    }

    fn cell_of(self, p: [f32; 2]) -> Option<[u32; 2]> {
        let c: [f64; 2] = std::array::from_fn(|a| ((f64::from(p[a]) - self.min[a]) / self.cell).floor());
        (0..2)
            .all(|a| c[a] >= 0.0 && c[a] < f64::from(self.dims[a]))
            .then(|| c.map(|v| v as u32))
    }

    /// Cells whose squares meet the XY box, clamped; None when disjoint.
    fn cell_range(self, lo: [f32; 2], hi: [f32; 2]) -> Option<([u32; 2], [u32; 2])> {
        let mut first = [0; 2];
        let mut last = [0; 2];
        for a in 0..2 {
            let l = ((f64::from(lo[a]) - self.min[a]) / self.cell).floor();
            let h = ((f64::from(hi[a]) - self.min[a]) / self.cell).floor();
            if h < 0.0 || l >= f64::from(self.dims[a]) {
                return None;
            }
            first[a] = l.max(0.0) as u32;
            last[a] = h.min(f64::from(self.dims[a] - 1)) as u32;
        }
        Some((first, last))
    }

    fn neighbours(self, c: [u32; 2]) -> impl Iterator<Item = [u32; 2]> {
        let [w, h] = self.dims;
        [
            (c[0] > 0).then(|| [c[0] - 1, c[1]]),
            (c[0] + 1 < w).then(|| [c[0] + 1, c[1]]),
            (c[1] > 0).then(|| [c[0], c[1] - 1]),
            (c[1] + 1 < h).then(|| [c[0], c[1] + 1]),
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Brush(u32),
    /// Up/down mesh triangle; z = plane[0] x + plane[1] y + plane[2].
    Mesh { mesh: u32, plane: [f32; 3] },
    /// Steep mesh face, widened into the dense capture's wall slab.
    Wall { quad: [[f32; 3]; 4], bottom: f32 },
}

struct Item {
    lo: [f32; 2],
    hi: [f32; 2],
    zmin: f32,
    shape: Shape,
}

fn cross2(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Inside a convex polygon of either winding, edges included.
fn inside(polygon: &[[f32; 2]], p: [f32; 2]) -> bool {
    let Some(&last) = polygon.last() else { return false };
    let (mut negative, mut positive) = (false, false);
    let mut previous = last;
    for &current in polygon {
        let d = cross2(previous, current, p);
        negative |= d < -1e-4;
        positive |= d > 1e-4;
        previous = current;
    }
    !(negative && positive)
}

fn xy<const N: usize>(points: [[f32; 3]; N]) -> [[f32; 2]; N] {
    points.map(|v| [v[0], v[1]])
}

fn brush_interval(brush: &CaptureBrush, p: [f32; 2]) -> (f32, f32) {
    let (mut lo, mut hi) = (brush.mins[2], brush.maxs[2]);
    for &(n, d) in &brush.planes {
        if n[2].abs() > 1e-4 {
            let z = (d - n[0] * p[0] - n[1] * p[1]) / n[2];
            if n[2] > 0.0 {
                hi = hi.min(z);
            } else {
                lo = lo.max(z);
            }
        }
    }
    let lo = lo.clamp(brush.mins[2], brush.maxs[2]);
    let hi = hi.clamp(brush.mins[2], brush.maxs[2]);
    (lo.min(hi), lo.max(hi))
}

fn sample(item: &Item, geometry: &CaptureGeometry, p: [f32; 2]) -> Option<(f32, f32)> {
    if p[0] < item.lo[0] || p[0] > item.hi[0] || p[1] < item.lo[1] || p[1] > item.hi[1] {
        return None;
    }
    match item.shape {
        Shape::Brush(b) => {
            let brush = &geometry.brushes[b as usize];
            inside(&brush.footprint, p).then(|| brush_interval(brush, p))
        }
        Shape::Mesh { mesh, plane } => inside(&xy(geometry.meshes[mesh as usize].triangle), p).then(|| {
            let z = plane[0] * p[0] + plane[1] * p[1] + plane[2];
            (z, z)
        }),
        Shape::Wall { quad, bottom } => inside(&xy(quad), p).then_some((bottom, quad[0][2])),
    }
}

fn items(geometry: &CaptureGeometry, region: Region, cutoff: f32) -> Vec<Item> {
    let mut items = Vec::new();
    let rect_lo = region.min.map(|v| v as f32);
    let rect_hi: [f32; 2] = std::array::from_fn(|a| (region.min[a] + f64::from(region.dims[a]) * region.cell) as f32);
    let mut keep = |item: Item| {
        if item.zmin <= cutoff && (0..2).all(|a| item.hi[a] >= rect_lo[a] && item.lo[a] <= rect_hi[a]) {
            items.push(item);
        }
    };
    for (i, brush) in geometry.brushes.iter().enumerate() {
        keep(Item {
            lo: [brush.mins[0], brush.mins[1]],
            hi: [brush.maxs[0], brush.maxs[1]],
            zmin: brush.mins[2],
            shape: Shape::Brush(i as u32),
        });
    }
    for (i, mesh) in geometry.meshes.iter().enumerate() {
        let t = mesh.triangle;
        let zmin = t.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
        let shape = match mesh.face {
            MeshFace::Wall => {
                let quad = wall_slab(&t, 0.75 * region.cell as f32, None).expect("slab without a level");
                Shape::Wall { quad, bottom: zmin }
            }
            MeshFace::Up | MeshFace::Down => {
                let e1: [f32; 3] = std::array::from_fn(|a| t[1][a] - t[0][a]);
                let e2: [f32; 3] = std::array::from_fn(|a| t[2][a] - t[0][a]);
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                if n[2].abs() < 1e-6 {
                    continue;
                }
                let d = n[0] * t[0][0] + n[1] * t[0][1] + n[2] * t[0][2];
                Shape::Mesh { mesh: i as u32, plane: [-n[0] / n[2], -n[1] / n[2], d / n[2]] }
            }
        };
        let points: Vec<[f32; 3]> = match shape {
            Shape::Wall { quad, .. } => quad.to_vec(),
            _ => t.to_vec(),
        };
        let lo = [0, 1].map(|a| points.iter().map(|p| p[a]).fold(f32::INFINITY, f32::min));
        let hi = [0, 1].map(|a| points.iter().map(|p| p[a]).fold(f32::NEG_INFINITY, f32::max));
        keep(Item { lo, hi, zmin, shape });
    }
    items
}

/// Items per BLOCK x BLOCK cells, compressed rows.
struct Index {
    blocks: [u32; 2],
    offsets: Vec<u32>,
    entries: Vec<u32>,
}

impl Index {
    fn new(items: &[Item], region: Region) -> Result<Self, String> {
        let blocks = region.dims.map(|n| n.div_ceil(BLOCK));
        let count = blocks[0] as usize * blocks[1] as usize;
        let ranges: Vec<_> = items
            .iter()
            .map(|item| {
                region
                    .cell_range(item.lo, item.hi)
                    .map(|(first, last)| (first.map(|v| v / BLOCK), last.map(|v| v / BLOCK)))
            })
            .collect();
        let mut offsets = vec![0u32; count + 1];
        for (first, last) in ranges.iter().flatten() {
            for y in first[1]..=last[1] {
                for x in first[0]..=last[0] {
                    offsets[(y * blocks[0] + x) as usize + 1] += 1;
                }
            }
        }
        for i in 0..count {
            offsets[i + 1] = offsets[i]
                .checked_add(offsets[i + 1])
                .ok_or("river capture classification index overflow")?;
        }
        let mut fill = offsets.clone();
        let mut entries = vec![0u32; offsets[count] as usize];
        for (i, range) in ranges.iter().enumerate() {
            let Some((first, last)) = range else { continue };
            for y in first[1]..=last[1] {
                for x in first[0]..=last[0] {
                    let b = (y * blocks[0] + x) as usize;
                    entries[fill[b] as usize] = i as u32;
                    fill[b] += 1;
                }
            }
        }
        Ok(Self { blocks, offsets, entries })
    }

    fn at(&self, c: [u32; 2]) -> &[u32] {
        let b = ((c[1] / BLOCK) * self.blocks[0] + c[0] / BLOCK) as usize;
        &self.entries[self.offsets[b] as usize..self.offsets[b + 1] as usize]
    }
}

struct Context<'a> {
    geometry: &'a CaptureGeometry,
    items: Vec<Item>,
    index: Index,
    region: Region,
}

impl Context<'_> {
    /// Every item's (bottom, top, item) on the cell centre's vertical line.
    fn collect(&self, c: [u32; 2], out: &mut Vec<(f32, f32, u32)>) {
        out.clear();
        let p = self.region.centre(c);
        for &i in self.index.at(c) {
            if let Some((lo, hi)) = sample(&self.items[i as usize], self.geometry, p) {
                out.push((lo, hi, i));
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Gap {
    floor: f32,
    ceil: f32,
    dist: u32,
}

/// Open intervals between merged solid runs, ascending; the first floor is
/// -inf (void below everything) and the last ceiling +inf.
fn gaps(intervals: &mut [(f32, f32, u32)], into: &mut Vec<Gap>) {
    intervals.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut push = |floor: f32, ceil: f32| {
        if ceil - floor >= CLEARANCE {
            into.push(Gap { floor, ceil, dist: UNREACHED });
        }
    };
    let mut floor = f32::NEG_INFINITY;
    let mut run: Option<(f32, f32)> = None;
    for &(lo, hi, _) in intervals.iter() {
        run = match run {
            Some((start, end)) if lo <= end + CLEARANCE => Some((start, end.max(hi))),
            Some((start, end)) => {
                push(floor, start);
                floor = end;
                Some((lo, hi))
            }
            None => Some((lo, hi)),
        };
    }
    match run {
        Some((start, end)) => {
            push(floor, start);
            push(end, f32::INFINITY);
        }
        None => push(f32::NEG_INFINITY, f32::INFINITY),
    }
}

/// Lazily evaluated columns in CHUNK x CHUNK pages.
struct Columns {
    chunks_x: u32,
    chunks: Vec<Vec<u32>>,
    spans: Vec<(u32, u32)>,
    cells: Vec<[u32; 2]>,
    gaps: Vec<Gap>,
}

impl Columns {
    fn new(region: Region) -> Self {
        let chunks = region.dims.map(|n| n.div_ceil(CHUNK));
        Self {
            chunks_x: chunks[0],
            chunks: vec![Vec::new(); chunks[0] as usize * chunks[1] as usize],
            spans: Vec::new(),
            cells: Vec::new(),
            gaps: Vec::new(),
        }
    }

    fn slot(&self, c: [u32; 2]) -> (usize, usize) {
        (
            ((c[1] / CHUNK) * self.chunks_x + c[0] / CHUNK) as usize,
            ((c[1] % CHUNK) * CHUNK + c[0] % CHUNK) as usize,
        )
    }

    fn get(&self, c: [u32; 2]) -> Option<u32> {
        let (chunk, offset) = self.slot(c);
        self.chunks[chunk].get(offset).copied().filter(|&id| id != UNSET)
    }

    fn column(&mut self, c: [u32; 2], context: &Context<'_>, scratch: &mut Vec<(f32, f32, u32)>) -> u32 {
        let (chunk, offset) = self.slot(c);
        if self.chunks[chunk].is_empty() {
            self.chunks[chunk] = vec![UNSET; (CHUNK * CHUNK) as usize];
        }
        let id = self.chunks[chunk][offset];
        if id != UNSET {
            return id;
        }
        context.collect(c, scratch);
        let start = self.gaps.len() as u32;
        gaps(scratch, &mut self.gaps);
        let id = self.spans.len() as u32;
        self.spans.push((start, self.gaps.len() as u32 - start));
        self.cells.push(c);
        self.chunks[chunk][offset] = id;
        id
    }

    fn gaps_of(&self, id: u32) -> &[Gap] {
        let (start, count) = self.spans[id as usize];
        &self.gaps[start as usize..(start + count) as usize]
    }

    /// Lowest reached floor (-inf for a reached void), or None.
    fn lowest(&self, id: u32) -> Option<f32> {
        self.gaps_of(id).iter().find(|g| g.dist != UNREACHED).map(|g| g.floor)
    }
}

/// The one gap of a neighbouring column that water standing on `a` enters.
fn step(a: Gap, b: &[Gap], climb: f32, cap: f32) -> Option<usize> {
    let overlap = |g: &Gap| a.ceil.min(g.ceil) - a.floor.max(g.floor);
    // A continuous floor: slope, step, bank, the river under a bridge.
    let mut best: Option<(usize, f32)> = None;
    for (k, g) in b.iter().enumerate() {
        let rise = (g.floor - a.floor).abs();
        if g.floor.is_finite() && g.floor <= cap && rise <= climb && overlap(g) >= CLEARANCE
            && best.is_none_or(|(_, r)| rise < r)
        {
            best = Some((k, rise));
        }
    }
    if let Some((k, _)) = best {
        return Some(k);
    }
    // A drop: the gap holding the water's height, open well above it.
    let height = a.floor + CLEARANCE;
    if let Some(k) = b.iter().position(|g| {
        g.floor < a.floor - climb && g.floor <= height && height < g.ceil && g.ceil >= a.ceil.min(a.floor + PASSAGE)
    }) {
        return Some(k);
    }
    // Ponding: the lowest floor the water can rise onto under its ceiling.
    b.iter().position(|g| g.floor > a.floor + climb && g.floor <= cap && g.floor <= a.ceil - CLEARANCE)
}

/// Lowest reached floor per cell plus the reference handed to stage 6.
pub struct References {
    region: Region,
    chunks_x: u32,
    /// Map units; NaN unset, -inf a reached void.
    chunks: Vec<Vec<f32>>,
}

impl References {
    fn new(region: Region) -> Self {
        let chunks = region.dims.map(|n| n.div_ceil(CHUNK));
        Self { region, chunks_x: chunks[0], chunks: vec![Vec::new(); chunks[0] as usize * chunks[1] as usize] }
    }

    fn slot(&self, c: [u32; 2]) -> (usize, usize) {
        (
            ((c[1] / CHUNK) * self.chunks_x + c[0] / CHUNK) as usize,
            ((c[1] % CHUNK) * CHUNK + c[0] % CHUNK) as usize,
        )
    }

    fn set(&mut self, c: [u32; 2], value: f32) {
        let (chunk, offset) = self.slot(c);
        if self.chunks[chunk].is_empty() {
            self.chunks[chunk] = vec![f32::NAN; (CHUNK * CHUNK) as usize];
        }
        self.chunks[chunk][offset] = value;
    }

    /// The reference at a region cell in map units (-inf for a void).
    pub fn get(&self, c: [i64; 2]) -> Option<f32> {
        if (0..2).any(|a| c[a] < 0 || c[a] >= i64::from(self.region.dims[a])) {
            return None;
        }
        let (chunk, offset) = self.slot([c[0] as u32, c[1] as u32]);
        self.chunks[chunk].get(offset).copied().filter(|v| !v.is_nan())
    }

    /// Local WaterInfo height (cm over `origin_z_cm`): `void_cm` for a reached
    /// void (everything above passes), NO_REFERENCE_CM where unreached.
    pub fn local_cm(&self, c: [i64; 2], origin_z_cm: f64, void_cm: f32) -> f32 {
        match self.get(c) {
            Some(z) if z == f32::NEG_INFINITY => void_cm,
            Some(z) => (f64::from(z) * f64::from(jka_water::river::CENTIMETRES_PER_MAP_UNIT) - origin_z_cm) as f32,
            None => NO_REFERENCE_CM,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub items: usize,
    pub columns: usize,
    pub reached: usize,
    pub voids: usize,
    /// Columns with two or more reachable floors (stacked; lowest kept).
    pub stacked: usize,
    pub actor_items: usize,
    pub landscape_triangles: usize,
    pub actor_triangles: usize,
    pub millis: f64,
}

pub struct Classified {
    pub landscape: Vec<[[f32; 3]; 3]>,
    pub actors: Vec<[[f32; 3]; 3]>,
    pub references: References,
    pub stats: Stats,
}

/// Classify `geometry` for a river whose source stands on the floor below
/// `seed` (map units, a point just above the aimed-at ground) with surface
/// `level`. Deterministic: depends only on geometry, region, seed and level.
pub fn classify(geometry: &CaptureGeometry, region: Region, seed: [f32; 3], level: f32) -> Result<Classified, String> {
    region.validate()?;
    if seed.iter().any(|v| !v.is_finite()) || !level.is_finite() {
        return Err("invalid river capture classification seed".into());
    }
    let started = std::time::Instant::now();
    let cap = level + LEVEL_MARGIN;
    let climb = (region.cell as f32).max(8.0);
    let context = {
        let items = items(geometry, region, cap + PASSAGE);
        let index = Index::new(&items, region)?;
        Context { geometry, items, index, region }
    };
    let mut columns = Columns::new(region);
    let mut scratch = Vec::new();

    let seed_cell = region
        .cell_of([seed[0], seed[1]])
        .ok_or("river source is outside its capture region")?;
    let id = columns.column(seed_cell, &context, &mut scratch);
    let (start, _) = columns.spans[id as usize];
    let seed_gaps = columns.gaps_of(id);
    let seed_k = seed_gaps
        .iter()
        .position(|g| g.floor.is_finite() && g.floor <= seed[2] && seed[2] < g.ceil)
        .or_else(|| {
            seed_gaps
                .iter()
                .enumerate()
                .filter(|(_, g)| g.floor.is_finite() && g.floor <= cap)
                .min_by(|a, b| (a.1.floor - seed[2]).abs().total_cmp(&(b.1.floor - seed[2]).abs()))
                .map(|(k, _)| k)
        })
        .ok_or("no floor under the river source (r_riverCaptureMode 1)")?;
    let seed_gap = start + seed_k as u32;
    columns.gaps[seed_gap as usize].dist = 0;
    let mut queue = VecDeque::from([(seed_cell, seed_gap)]);
    while let Some((c, g)) = queue.pop_front() {
        let a = columns.gaps[g as usize];
        if !a.floor.is_finite() {
            continue; // water falling into the void goes no further
        }
        for n in region.neighbours(c) {
            let id = columns.column(n, &context, &mut scratch);
            let (start, _) = columns.spans[id as usize];
            if let Some(k) = step(a, columns.gaps_of(id), climb, cap) {
                let target = &mut columns.gaps[start as usize + k];
                if target.dist == UNREACHED {
                    target.dist = a.dist + 1;
                    queue.push_back((n, start + k as u32));
                }
            }
        }
    }

    let mut stats = Stats { items: context.items.len(), columns: columns.spans.len(), ..Stats::default() };
    let mut reached = Vec::new();
    let mut references = References::new(region);
    for id in 0..columns.spans.len() as u32 {
        let Some(floor) = columns.lowest(id) else { continue };
        let c = columns.cells[id as usize];
        reached.push((c, floor));
        stats.reached += 1;
        stats.voids += usize::from(floor == f32::NEG_INFINITY);
        stats.stacked +=
            usize::from(columns.gaps_of(id).iter().filter(|g| g.dist != UNREACHED && g.floor.is_finite()).count() > 1);
        references.set(c, floor + REFERENCE_LIFT);
    }
    dilate(&mut references, &reached, region);

    let (mut above, seen) = vote(&context, &reached);
    // Items too small to contain any cell centre (railing posts, finely
    // tessellated patch spans) vote at their centroid's column.
    let floor_at = |p: [f32; 2]| {
        let c = region.cell_of(p)?;
        Some((c, columns.lowest(columns.get(c)?)?))
    };
    for (i, item) in context.items.iter().enumerate() {
        if seen[i] {
            continue;
        }
        match item.shape {
            Shape::Brush(b) => {
                let brush = &geometry.brushes[b as usize];
                let n = brush.footprint.len() as f32;
                let p = [0, 1].map(|a| brush.footprint.iter().map(|v| v[a]).sum::<f32>() / n);
                if let Some((_, floor)) = floor_at(p) {
                    above[i] = brush_interval(brush, p).0 > floor + ABOVE;
                }
            }
            Shape::Mesh { mesh, plane } => {
                let t = geometry.meshes[mesh as usize].triangle;
                let p = [0, 1].map(|a| t.iter().map(|v| v[a]).sum::<f32>() / 3.0);
                if let Some((c, floor)) = floor_at(p) {
                    // Height extrapolated to the cell centre: a cell of slope tolerance.
                    let q = region.centre(c);
                    above[i] = plane[0] * q[0] + plane[1] * q[1] + plane[2] > floor + region.cell as f32;
                }
            }
            Shape::Wall { .. } => {}
        }
    }

    let mut landscape = Vec::new();
    let mut actors = Vec::new();
    let quad = |q: [[f32; 3]; 4]| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]];
    for (i, item) in context.items.iter().enumerate() {
        if item.zmin > cap {
            continue; // water never reaches it: as if absent
        }
        let actor = above[i];
        stats.actor_items += usize::from(actor);
        match item.shape {
            Shape::Brush(b) => {
                let faces = geometry.brushes[b as usize].faces.clone();
                for face in &geometry.brush_faces[faces.start as usize..faces.end as usize] {
                    if actor {
                        actors.push(face.triangle);
                    } else if face.up && !face.sky {
                        landscape.push(face.triangle);
                    }
                }
            }
            Shape::Mesh { mesh, .. } => {
                let m = geometry.meshes[mesh as usize];
                if actor {
                    actors.push(m.triangle);
                    actors.push(m.triangle.map(|p| [p[0], p[1], p[2] - MESH_THICKNESS]));
                } else if m.face == MeshFace::Up {
                    landscape.push(m.triangle);
                }
            }
            Shape::Wall { quad: top, bottom } => {
                if actor {
                    let low = bottom.min(top[0][2] - MESH_THICKNESS);
                    actors.extend(quad(top));
                    actors.extend(quad(top.map(|p| [p[0], p[1], low])));
                } else {
                    landscape.extend(quad(top));
                }
            }
        }
    }
    stats.landscape_triangles = landscape.len();
    stats.actor_triangles = actors.len();
    stats.millis = started.elapsed().as_secs_f64() * 1000.0;
    Ok(Classified { landscape, actors, references, stats })
}

/// Which items start above the lowest reached floor of a column they cross,
/// and which items any reached column sampled at all.
fn vote(context: &Context<'_>, reached: &[([u32; 2], f32)]) -> (Vec<bool>, Vec<bool>) {
    let count = context.items.len();
    let run = |part: &[([u32; 2], f32)]| {
        let mut above = vec![false; count];
        let mut seen = vec![false; count];
        let mut scratch = Vec::new();
        for &(c, floor) in part {
            context.collect(c, &mut scratch);
            for &(lo, _, i) in &scratch {
                seen[i as usize] = true;
                above[i as usize] |= lo > floor + ABOVE;
            }
        }
        (above, seen)
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    if threads < 2 || reached.len() < 8192 {
        return run(reached);
    }
    let parts: Vec<_> = std::thread::scope(|scope| {
        reached
            .chunks(reached.len().div_ceil(threads))
            .map(|part| scope.spawn(move || run(part)))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().expect("river capture vote worker"))
            .collect()
    });
    let mut parts = parts.into_iter();
    let (mut above, mut seen) = parts.next().unwrap_or_default();
    for (a, s) in parts {
        for i in 0..count {
            above[i] |= a[i];
            seen[i] |= s[i];
        }
    }
    (above, seen)
}

/// Unreached cells next to reached ones take the lowest neighbouring reference.
fn dilate(references: &mut References, reached: &[([u32; 2], f32)], region: Region) {
    let mut frontier: Vec<[u32; 2]> = reached.iter().map(|&(c, _)| c).collect();
    for _ in 0..REFERENCE_DILATION {
        let mut next: Vec<([u32; 2], f32)> = Vec::new();
        for &c in &frontier {
            let Some(value) = references.get(c.map(i64::from)) else { continue };
            for n in region.neighbours(c) {
                if references.get(n.map(i64::from)).is_none() {
                    next.push((n, value));
                }
            }
        }
        next.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        next.dedup_by(|later, earlier| later.0 == earlier.0);
        frontier.clear();
        for (c, value) in next {
            references.set(c, value);
            frontier.push(c);
        }
    }
}

#[cfg(test)]
#[path = "river_reach_tests.rs"]
mod tests;
