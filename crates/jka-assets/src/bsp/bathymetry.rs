//! Static, Z-up BSP geometry for a future coastal solver. No movement or GPU state.
//! Water volumes are never seabed. Unknown columns and ambiguous height fields
//! remain explicit; they must not be initialized with a guessed water depth.
use super::{
    brush_side_polygon, patch_vertex, Bsp, Plane, SurfaceKind, CONTENTS_SOLID, CONTENTS_TERRAIN,
    SURF_SKY,
};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

mod cache;
mod bands;
mod upstream_shore;
mod promotion;
mod report;
pub use cache::fingerprint;
pub use promotion::{
    build_water_footprints, cluster_water_faces, WaterCluster, WaterFace, WaterFootprint,
    WaterFootprintTile, WaterTriangle,
};

#[cfg(test)]
mod tests;

// OpenJK codemp/game/surfaceflags.h, NOT Quake 3's content bit assignments.
pub const CONTENTS_WATER: u32 = 0x0000_0004;
const LIQUIDS: u32 = CONTENTS_WATER | 0x0000_0002 | 0x0002_0000;
const EPS: f64 = 0.001;
pub const CACHE_VERSION: u32 = 5;

/// BSP shader indices that are the surface of a real body of water: a shader
/// applied to a `CONTENTS_WATER` brush of the world model (the brush's own
/// shader and every shader on its sides), or any water-contents shader with a
/// face lying on such a brush's top plane (a visible sheet over a `caulk_water`
/// volume). `surfaceparm water` alone is not enough: costadelsol's shore-surf
/// patches `seph_surf` and waterfall sheets carry water contents but are neither
/// on a brush nor on a volume's top plane, so they must not become the FFT ocean.
pub fn water_volume_shaders(bsp: &Bsp) -> std::collections::BTreeSet<usize> {
    /// Same order as the renderer's flat-water test: the sheet and the brush top
    /// must coincide, otherwise a sheet a unit above or below a pool is an overlay.
    const TOP_TOLERANCE: f32 = 0.5;
    let mut shaders = std::collections::BTreeSet::new();
    let Some(world) = bsp.models.first() else {
        return shaders;
    };
    // (min, max) of each water brush from its axial bounding planes.
    let mut volumes = Vec::<([f32; 3], [f32; 3])>::new();
    for id in world.brushes.clone() {
        let Some(brush) = bsp.brushes.get(id) else { continue };
        if bsp.shaders.get(brush.shader).is_none_or(|s| s.contents & CONTENTS_WATER == 0) {
            continue;
        }
        shaders.insert(brush.shader);
        let mut low = [f32::NEG_INFINITY; 3];
        let mut high = [f32::INFINITY; 3];
        for side in brush.sides.clone() {
            let Some(side) = bsp.brush_sides.get(side) else { continue };
            shaders.insert(side.shader);
            let Some(plane) = bsp.planes.get(side.plane) else { continue };
            for axis in 0..3 {
                if plane.normal[axis].abs() > 0.999 {
                    if plane.normal[axis] > 0.0 {
                        high[axis] = high[axis].min(plane.distance);
                    } else {
                        low[axis] = low[axis].max(-plane.distance);
                    }
                }
            }
        }
        volumes.push((low, high));
    }
    for surface in &bsp.surfaces {
        if shaders.contains(&surface.shader)
            || bsp.shaders.get(surface.shader).is_none_or(|s| s.contents & CONTENTS_WATER == 0)
        {
            continue;
        }
        let Some(points) = bsp.vertices.get(surface.vertices.clone()) else { continue };
        if points.is_empty() {
            continue;
        }
        let mut low = [f32::INFINITY; 3];
        let mut high = [f32::NEG_INFINITY; 3];
        for point in points {
            for axis in 0..3 {
                low[axis] = low[axis].min(point.position[axis]);
                high[axis] = high[axis].max(point.position[axis]);
            }
        }
        let on_top = volumes.iter().any(|(volume_low, volume_high)| {
            (high[2] - volume_high[2]).abs() <= TOP_TOLERANCE
                && (0..2).all(|axis| {
                    low[axis] >= volume_low[axis] - 1.0 && high[axis] <= volume_high[axis] + 1.0
                })
        });
        if on_top {
            shaders.insert(surface.shader);
        }
    }
    shaders
}

pub type FootprintTriangle = [[f32; 2]; 3];

#[derive(Clone, Debug)]
pub struct Region {
    /// Matches the renderer's ocean slot, including slots occupied by authored oceans.
    pub slot: u32,
    pub level: f32,
    pub triangles: Vec<FootprintTriangle>,
    /// Optional sparse allocation rectangle. The exact triangles remain the
    /// authority; this merely selects one occupied footprint tile to bake.
    pub bounds: Option<[[f32; 2]; 2]>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub cell_size: f32,
    pub padding_cells: u32,
    pub max_cells_per_region: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cell_size: 16.0,
            padding_cells: 2,
            max_cells_per_region: 262_144,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Class {
    Outside = 0,
    Wet = 1,
    Dry = 2,
    Outline = 3,
    NoFloor = 4,
    Overhang = 5,
    Mixed = 6,
}

impl Class {
    pub const ALL: [Self; 7] = [
        Self::Outside,
        Self::Wet,
        Self::Dry,
        Self::Outline,
        Self::NoFloor,
        Self::Overhang,
        Self::Mixed,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Outside => "outside footprint",
            Self::Wet => "wet",
            Self::Dry => "dry solid",
            Self::Outline => "partial footprint",
            Self::NoFloor => "missing seabed",
            Self::Overhang => "overhang / stacked floor",
            Self::Mixed => "unresolved geometry",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub class: Class,
    /// NaN means unknown, never zero depth. All lengths are JKA world units.
    pub bed: f32,
    pub depth: f32,
    pub ceiling: f32,
    pub bed_min: f32,
    pub bed_max: f32,
    /// Number of covered probes / 5; diagnostic only, not a volume fraction.
    pub coverage: u8,
    /// 0 = no supported wet component. Connectivity uses solid face barriers.
    pub component: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Boundary {
    Open = 0,
    Wall = 1,
    Outline = 2,
    Unsupported = 3,
    /// Overtoppable terrain at the still-water line. Unlike a wall, this face
    /// may become wet or dry as the future coastal free surface moves.
    Bank = 4,
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub boundary: Boundary,
    /// Highest supported sampled bed at this face. Wall faces remain sealed
    /// independently of depth; a vertical wall is not a deep-water open edge.
    pub crest: f32,
}

#[derive(Clone, Debug)]
pub struct Grid {
    pub slot: u32,
    pub level: f32,
    /// Lower XY corner; cell values are at centers. JKA XY, Z up.
    pub origin: [f32; 2],
    pub size: [u32; 2],
    pub cell_size: f32,
    pub cells: Vec<Cell>,
    /// Staggered faces: (nx+1)*ny and nx*(ny+1). Outer faces are closed.
    pub faces_x: Vec<Face>,
    pub faces_y: Vec<Face>,
    pub components: u32,
    /// Exact source triangles, including holes, retained independently of the
    /// sampled grid. Future surface reconstruction must use this geometry.
    pub footprint: Vec<FootprintTriangle>,
}

#[derive(Clone, Debug, Default)]
pub struct Atlas {
    pub grids: Vec<Grid>,
    pub warnings: Vec<String>,
    pub bake_ms: f64,
    /// Present only for pools whose offshore tiles were screened out. Records
    /// the depth proof and, when necessary, the explicit bank patch policy.
    /// Runtime wavelength overrides must not exceed the screened depth.
    pub shoreline_bands: Vec<ShorelineBand>,
}

#[derive(Clone, Debug)]
pub struct ShorelineBand {
    pub slot: u32,
    pub full_tiles: u32,
    pub retained_tiles: u32,
    pub max_depth: f32,
    /// Zero retains all screened shallows. Otherwise physics is restricted to
    /// this many cell steps from resolved banks, followed by an FFT reservoir.
    pub patch_radius_cells: u32,
}

impl Atlas {
    pub fn memory_bytes(&self) -> usize {
        self.grids
            .iter()
            .map(|g| {
                g.cells.len() * std::mem::size_of::<Cell>()
                    + (g.faces_x.len() + g.faces_y.len()) * std::mem::size_of::<Face>()
                    + g.footprint.len() * std::mem::size_of::<FootprintTriangle>()
            })
            .sum()
    }
}

struct Solid {
    planes: Vec<Plane>,
    sky: Vec<bool>,
    bounds: [[f32; 2]; 2],
}
struct Triangle {
    points: [[f32; 3]; 3],
    normal: [f64; 3],
    bounds: [[f32; 2]; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blocker {
    Bank,
    Wall,
}

/// Surfaces steeper than 75 degrees from horizontal are rigid walls. Shallower
/// faces are terrain banks which a wet/dry solver may overtop.
const WALL_NORMAL_Z_MAX: f64 = 0.258_819_045_1;

fn blocker_from_normal(normal: [f32; 3]) -> Blocker {
    let length = normal
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if length > 1e-9 && f64::from(normal[2]).abs() / length <= WALL_NORMAL_Z_MAX {
        Blocker::Wall
    } else {
        Blocker::Bank
    }
}

fn blocker_from_f64_normal(normal: [f64; 3]) -> Blocker {
    let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length > 1e-9 && normal[2].abs() / length <= WALL_NORMAL_Z_MAX {
        Blocker::Wall
    } else {
        Blocker::Bank
    }
}

fn strongest_blocker(a: Option<Blocker>, b: Blocker) -> Option<Blocker> {
    Some(if a == Some(Blocker::Wall) || b == Blocker::Wall {
        Blocker::Wall
    } else {
        Blocker::Bank
    })
}

/// Bake-only acceleration source. Inline/moving models are deliberately excluded.
pub struct Geometry {
    solids: Vec<Solid>,
    triangles: Vec<Triangle>,
    liquids: Vec<Solid>,
}

fn bounds(points: impl Iterator<Item = [f32; 2]>) -> [[f32; 2]; 2] {
    let mut b = [[f32::INFINITY; 2], [f32::NEG_INFINITY; 2]];
    for p in points {
        for a in 0..2 {
            b[0][a] = b[0][a].min(p[a]);
            b[1][a] = b[1][a].max(p[a]);
        }
    }
    b
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f64; 3] {
    std::array::from_fn(|i| f64::from(a[i]) - f64::from(b[i]))
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn solid_contents(contents: u32) -> bool {
    contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) != 0 && contents & LIQUIDS == 0
}

impl Geometry {
    pub fn from_bsp(bsp: &Bsp) -> Result<Self, String> {
        let world = bsp.models.first().ok_or("BSP has no world model")?;
        let extent = world
            .mins
            .iter()
            .chain(&world.maxs)
            .map(|v| v.abs())
            .fold(1024.0, f32::max)
            * 2.0;
        let mut result = Self {
            solids: Vec::new(),
            triangles: Vec::new(),
            liquids: Vec::new(),
        };
        for id in world.brushes.clone() {
            let brush = &bsp.brushes[id];
            let contents = bsp.shaders[brush.shader].contents;
            let liquid = contents & CONTENTS_WATER != 0;
            if !solid_contents(contents) && !liquid {
                continue;
            }
            let mut points = Vec::new();
            let mut planes = Vec::new();
            let mut sky = Vec::new();
            for side in brush.sides.clone() {
                planes.push(bsp.planes[bsp.brush_sides[side].plane]);
                sky.push(bsp.shaders[bsp.brush_sides[side].shader].surface_flags & SURF_SKY != 0);
            }
            // Compiled brushes normally carry axial bounding planes. Use those
            // exact conservative bounds, including for zero-volume compiler
            // remnants: column/segment clipping still tests every brush plane.
            let mut b = [[f32::NEG_INFINITY; 2], [f32::INFINITY; 2]];
            for p in &planes {
                for a in 0..2 {
                    if (0..3).all(|i| i == a || p.normal[i].abs() < 1e-9)
                        && p.normal[a].abs() > 1e-9
                    {
                        let v = p.distance / p.normal[a];
                        if p.normal[a] > 0.0 {
                            b[1][a] = b[1][a].min(v);
                        } else {
                            b[0][a] = b[0][a].max(v);
                        }
                    }
                }
            }
            if b.iter().flatten().any(|v| !v.is_finite()) {
                for side in brush.sides.clone() {
                    points.extend(
                        brush_side_polygon(bsp, brush, side, extent).map_err(|e| e.to_string())?,
                    );
                }
                if points.is_empty() {
                    return Err(format!(
                        "unbounded brush {id}; cannot safely classify its influence"
                    ));
                }
                b = bounds(points.iter().map(|p| [p[0], p[1]]));
            }
            if (0..2).any(|a| b[0][a] > b[1][a]) {
                continue;
            } // proven empty convex volume
            let source = Solid {
                planes,
                sky,
                bounds: b,
            };
            if liquid {
                result.liquids.push(source);
            } else {
                result.solids.push(source);
            }
        }
        // Planar detail, triangle terrain and curved patches can have no brush.
        // Include nodraw collision surfaces too. A patch is explicitly sampled
        // at eight segments per quadratic span, not claimed as an exact curve.
        for id in world.surfaces.clone() {
            let surface = &bsp.surfaces[id];
            let shader = &bsp.shaders[surface.shader];
            if !solid_contents(shader.contents) || shader.surface_flags & SURF_SKY != 0 {
                continue;
            }
            let verts = &bsp.vertices[surface.vertices.clone()];
            let mut append = |v: [super::Vertex; 3]| {
                let points = v.map(|p| p.position);
                let mut normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
                let authored: [f64; 3] =
                    std::array::from_fn(|a| v.iter().map(|v| f64::from(v.normal[a])).sum());
                if dot(normal, authored) < 0.0 {
                    normal = normal.map(|n| -n);
                }
                let len = dot(normal, normal).sqrt();
                if len < EPS {
                    return;
                }
                normal = normal.map(|n| n / len);
                result.triangles.push(Triangle {
                    points,
                    normal,
                    bounds: bounds(points.iter().map(|p| [p[0], p[1]])),
                });
            };
            match surface.kind {
                SurfaceKind::Patch => {
                    let [w, h] = surface.patch_size;
                    for y in (0..h - 2).step_by(2) {
                        for x in (0..w - 2).step_by(2) {
                            for j in 0..8 {
                                for i in 0..8 {
                                    let a = patch_vertex(
                                        verts,
                                        w,
                                        x,
                                        y,
                                        i as f32 / 8.0,
                                        j as f32 / 8.0,
                                    );
                                    let b = patch_vertex(
                                        verts,
                                        w,
                                        x,
                                        y,
                                        (i + 1) as f32 / 8.0,
                                        j as f32 / 8.0,
                                    );
                                    let c = patch_vertex(
                                        verts,
                                        w,
                                        x,
                                        y,
                                        i as f32 / 8.0,
                                        (j + 1) as f32 / 8.0,
                                    );
                                    let d = patch_vertex(
                                        verts,
                                        w,
                                        x,
                                        y,
                                        (i + 1) as f32 / 8.0,
                                        (j + 1) as f32 / 8.0,
                                    );
                                    append([a, c, b]);
                                    append([b, c, d]);
                                }
                            }
                        }
                    }
                }
                SurfaceKind::Planar | SurfaceKind::Triangles => {
                    for t in bsp.indices[surface.indices.clone()].chunks_exact(3) {
                        append([
                            verts[t[0] as usize],
                            verts[t[1] as usize],
                            verts[t[2] as usize],
                        ]);
                    }
                }
                SurfaceKind::Flare => {}
            }
        }
        Ok(result)
    }

    pub fn bake(&self, regions: &[Region], options: Options) -> Result<Atlas, String> {
        if !options.cell_size.is_finite()
            || options.cell_size < 1.0
            || options.padding_cells > 32
            || options.max_cells_per_region > 1_048_576
        {
            return Err("invalid bathymetry bake limits".into());
        }
        let started = Instant::now();
        let mut atlas = Atlas::default();
        for region in regions {
            match self.bake_region(region, options) {
                Ok(grid) => atlas.grids.push(grid),
                Err(e) => atlas
                    .warnings
                    .push(format!("slot {}: {e}; retain legacy water", region.slot)),
            }
        }
        self.stitch_tiles(&mut atlas.grids);
        atlas.bake_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(atlas)
    }

    fn seam_face(
        &self,
        p: [f32; 2],
        q: [f32; 2],
        level: f32,
        a: Cell,
        b: Cell,
        footprint: impl Iterator<Item = FootprintTriangle>,
        solids: &[usize],
        triangles: &[usize],
    ) -> Face {
        let mut mid = [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5];
        // Keep the midpoint explicitly finite even for malformed input; the
        // caller only supplies aligned neighboring tile cells.
        if mid.iter().any(|v| !v.is_finite()) {
            mid = p;
        }
        let middle = self.column(mid, level, solids, triangles);
        let blocker = solids
            .iter()
            .filter_map(|&s| self.solids[s].blocks(p, q, level))
            .chain(
                triangles
                    .iter()
                    .filter_map(|&t| self.triangles[t].blocks(p, q, level)),
            )
            .fold(None, strongest_blocker);
        let exact: Vec<_> = footprint.collect();
        let boundary = if blocker == Some(Blocker::Wall) {
            Boundary::Wall
        } else if blocker == Some(Blocker::Bank) || a.class == Class::Dry || b.class == Class::Dry {
            Boundary::Bank
        } else if a.class == Class::Outside
            || b.class == Class::Outside
            || a.class == Class::Outline
            || b.class == Class::Outline
            || !segment_covered(p, q, exact.into_iter())
        {
            Boundary::Outline
        } else if a.class != Class::Wet
            || b.class != Class::Wet
            || middle.bed.is_none()
            || middle.overhang
        {
            Boundary::Unsupported
        } else {
            Boundary::Open
        };
        Face {
            boundary,
            crest: middle.bed.map_or(f32::NAN, |v| v.max(a.bed).max(b.bed)),
        }
    }

    fn stitch_tiles(&self, grids: &mut [Grid]) {
        let key = |grid: &Grid, origin: [f32; 2]| {
            (
                grid.slot,
                grid.level.to_bits(),
                grid.cell_size.to_bits(),
                (origin[0] / grid.cell_size).round() as i64,
                (origin[1] / grid.cell_size).round() as i64,
            )
        };
        let lookup: HashMap<_, _> = grids
            .iter()
            .enumerate()
            .map(|(index, grid)| (key(grid, grid.origin), index))
            .collect();
        let candidates: Vec<(Vec<usize>, Vec<usize>)> = grids
            .iter()
            .map(|grid| {
                let a_max = [
                    grid.origin[0] + grid.size[0] as f32 * grid.cell_size,
                    grid.origin[1] + grid.size[1] as f32 * grid.cell_size,
                ];
                let overlaps = |bounds: [[f32; 2]; 2]| {
                    (0..2).all(|axis| {
                        bounds[1][axis] + EPS as f32 >= grid.origin[axis] - grid.cell_size
                            && bounds[0][axis] - EPS as f32 <= a_max[axis] + grid.cell_size
                    })
                };
                (
                    self.solids
                        .iter()
                        .enumerate()
                        .filter_map(|(i, value)| overlaps(value.bounds).then_some(i))
                        .collect(),
                    self.triangles
                        .iter()
                        .enumerate()
                        .filter_map(|(i, value)| overlaps(value.bounds).then_some(i))
                        .collect(),
                )
            })
            .collect();

        for i in 0..grids.len() {
            let right_origin = [
                grids[i].origin[0] + grids[i].size[0] as f32 * grids[i].cell_size,
                grids[i].origin[1],
            ];
            let top_origin = [
                grids[i].origin[0],
                grids[i].origin[1] + grids[i].size[1] as f32 * grids[i].cell_size,
            ];
            for (axis, neighbour) in [
                (0usize, lookup.get(&key(&grids[i], right_origin)).copied()),
                (1usize, lookup.get(&key(&grids[i], top_origin)).copied()),
            ] {
                let Some(j) = neighbour.filter(|&j| j != i) else { continue };
                let mut solid_candidates = candidates[i].0.clone();
                solid_candidates.extend(candidates[j].0.iter().copied());
                solid_candidates.sort_unstable();
                solid_candidates.dedup();
                let mut triangle_candidates = candidates[i].1.clone();
                triangle_candidates.extend(candidates[j].1.iter().copied());
                triangle_candidates.sort_unstable();
                triangle_candidates.dedup();
                let (a, b) = if i < j {
                    let (left, right) = grids.split_at_mut(j);
                    (&mut left[i], &mut right[0])
                } else {
                    let (left, right) = grids.split_at_mut(i);
                    (&mut right[0], &mut left[j])
                };
                if a.slot != b.slot
                    || (a.level - b.level).abs() > 0.01
                    || (a.cell_size - b.cell_size).abs() > 0.001
                {
                    continue;
                }
                if axis == 0 && a.size[1] == b.size[1] {
                    let a_max_x = a.origin[0] + a.size[0] as f32 * a.cell_size;
                    let (anx, bnx) = (a.size[0] as usize, b.size[0] as usize);
                    for y in 0..a.size[1] as usize {
                        let ak = y * anx + anx - 1;
                        let bk = y * bnx;
                        let p = [a_max_x - a.cell_size * 0.5, a.origin[1] + (y as f32 + 0.5) * a.cell_size];
                        let q = [b.origin[0] + b.cell_size * 0.5, p[1]];
                        let face = self.seam_face(
                            p,
                            q,
                            a.level,
                            a.cells[ak],
                            b.cells[bk],
                            a.footprint.iter().chain(&b.footprint).copied(),
                            &solid_candidates,
                            &triangle_candidates,
                        );
                        a.faces_x[y * (anx + 1) + anx] = face;
                        b.faces_x[y * (bnx + 1)] = face;
                    }
                } else if axis == 1 && a.size[0] == b.size[0] {
                    let a_max_y = a.origin[1] + a.size[1] as f32 * a.cell_size;
                    let (nx, any) = (a.size[0] as usize, a.size[1] as usize);
                    for x in 0..nx {
                        let ak = (any - 1) * nx + x;
                        let bk = x;
                        let p = [a.origin[0] + (x as f32 + 0.5) * a.cell_size, a_max_y - a.cell_size * 0.5];
                        let q = [p[0], b.origin[1] + b.cell_size * 0.5];
                        let face = self.seam_face(
                            p,
                            q,
                            a.level,
                            a.cells[ak],
                            b.cells[bk],
                            a.footprint.iter().chain(&b.footprint).copied(),
                            &solid_candidates,
                            &triangle_candidates,
                        );
                        a.faces_y[any * nx + x] = face;
                        b.faces_y[x] = face;
                    }
                }
            }
        }
    }

    fn bake_region(&self, region: &Region, options: Options) -> Result<Grid, String> {
        if !region.level.is_finite()
            || region.triangles.is_empty()
            || region
                .triangles
                .iter()
                .flatten()
                .flatten()
                .any(|v| !v.is_finite())
        {
            return Err("invalid water footprint".into());
        }
        let b = region
            .bounds
            .unwrap_or_else(|| bounds(region.triangles.iter().flatten().copied()));
        let dx = options.cell_size;
        let padding = if region.bounds.is_some() { 0 } else { options.padding_cells };
        let origin: [f32; 2] = std::array::from_fn(|a| {
            (b[0][a] / dx).floor() * dx - padding as f32 * dx
        });
        let dims: [f32; 2] = std::array::from_fn(|a| {
            ((b[1][a] - origin[a]) / dx).ceil() + padding as f32
        });
        if dims
            .iter()
            .any(|n| *n < 1.0 || *n > options.max_cells_per_region as f32)
        {
            return Err("grid dimensions exceed budget".into());
        }
        let size = dims.map(|d| d as u32);
        let n = size[0] as usize * size[1] as usize;
        if n > options.max_cells_per_region {
            return Err(format!(
                "{n} cells exceed fixed {}-cell budget at {dx} units",
                options.max_cells_per_region
            ));
        }
        let solid_bins = bins(self.solids.iter().map(|s| s.bounds), origin, size, dx)?;
        let triangle_bins = bins(self.triangles.iter().map(|t| t.bounds), origin, size, dx)?;
        let footprint_bins = bins(
            region.triangles.iter().map(|t| bounds(t.iter().copied())),
            origin,
            size,
            dx,
        )?;
        let sample =
            |p: [f32; 2], k: usize| self.column(p, region.level, &solid_bins[k], &triangle_bins[k]);
        let inside = |p: [f32; 2], k: usize| {
            footprint_bins[k]
                .iter()
                .any(|&t| contains(region.triangles[t], p))
        };
        let mut cells = Vec::with_capacity(n);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let k = (y * size[0] + x) as usize;
                let center = [
                    origin[0] + (x as f32 + 0.5) * dx,
                    origin[1] + (y as f32 + 0.5) * dx,
                ];
                let probes = [
                    center,
                    [center[0] - 0.49 * dx, center[1] - 0.49 * dx],
                    [center[0] + 0.49 * dx, center[1] - 0.49 * dx],
                    [center[0] - 0.49 * dx, center[1] + 0.49 * dx],
                    [center[0] + 0.49 * dx, center[1] + 0.49 * dx],
                ];
                let samples = probes.map(|p| sample(p, k));
                let coverage = probes.iter().filter(|&&p| inside(p, k)).count() as u8;
                let c = samples[0];
                let bed_min = samples
                    .iter()
                    .filter_map(|c| c.bed)
                    .fold(f32::INFINITY, f32::min);
                let bed_max = samples
                    .iter()
                    .filter_map(|c| c.bed)
                    .fold(f32::NEG_INFINITY, f32::max);
                let class = if c.bed.is_some_and(|b| b >= region.level - EPS as f32) {
                    Class::Dry
                } else if coverage == 0 {
                    Class::Outside
                } else if samples.iter().any(|s| s.bed.is_none()) {
                    Class::NoFloor
                } else if samples.iter().any(|s| s.overhang) {
                    Class::Overhang
                } else if coverage < 5 {
                    Class::Outline
                } else if samples
                    .iter()
                    .any(|s| s.bed.is_some_and(|b| b >= region.level - EPS as f32))
                {
                    Class::Mixed
                } else {
                    Class::Wet
                };
                cells.push(Cell {
                    class,
                    bed: c.bed.unwrap_or(f32::NAN),
                    depth: c.bed.map_or(f32::NAN, |b| (region.level - b).max(0.0)),
                    ceiling: c.ceiling.unwrap_or(f32::NAN),
                    bed_min: if bed_min.is_finite() {
                        bed_min
                    } else {
                        f32::NAN
                    },
                    bed_max: if bed_max.is_finite() {
                        bed_max
                    } else {
                        f32::NAN
                    },
                    coverage,
                    component: 0,
                });
            }
        }
        let sealed = Face {
            boundary: Boundary::Outline,
            crest: f32::NAN,
        };
        let mut grid = Grid {
            slot: region.slot,
            level: region.level,
            origin,
            size,
            cell_size: dx,
            cells,
            faces_x: vec![sealed; (size[0] as usize + 1) * size[1] as usize],
            faces_y: vec![sealed; size[0] as usize * (size[1] as usize + 1)],
            components: 0,
            footprint: region.triangles.clone(),
        };
        for y in 0..size[1] {
            for x in 0..size[0] {
                let k = (y * size[0] + x) as usize;
                let p = [
                    origin[0] + (x as f32 + 0.5) * dx,
                    origin[1] + (y as f32 + 0.5) * dx,
                ];
                for axis in 0..2 {
                    if (axis == 0 && x + 1 == size[0]) || (axis == 1 && y + 1 == size[1]) {
                        continue;
                    }
                    let qk = k + if axis == 0 { 1 } else { size[0] as usize };
                    let mut q = p;
                    q[axis] += dx;
                    let mut mid = p;
                    mid[axis] += dx * 0.5;
                    let solid_candidates: Vec<_> = solid_bins[k]
                        .iter()
                        .chain(&solid_bins[qk])
                        .copied()
                        .collect();
                    let triangle_candidates: Vec<_> = triangle_bins[k]
                        .iter()
                        .chain(&triangle_bins[qk])
                        .copied()
                        .collect();
                    let middle =
                        self.column(mid, region.level, &solid_candidates, &triangle_candidates);
                    let a = grid.cells[k];
                    let b = grid.cells[qk];
                    // Exact convex segment clipping catches walls thinner than a
                    // cell, even when every center/corner sample misses the wall.
                    let blocker = solid_bins[k]
                        .iter()
                        .chain(&solid_bins[qk])
                        .filter_map(|&s| self.solids[s].blocks(p, q, region.level))
                        .chain(
                            triangle_bins[k]
                                .iter()
                                .chain(&triangle_bins[qk])
                                .filter_map(|&t| self.triangles[t].blocks(p, q, region.level)),
                        )
                        .fold(None, strongest_blocker);
                    let boundary = if blocker == Some(Blocker::Wall) {
                        Boundary::Wall
                    } else if blocker == Some(Blocker::Bank)
                        || a.class == Class::Dry
                        || b.class == Class::Dry
                    {
                        Boundary::Bank
                    } else if a.class == Class::Outside
                        || b.class == Class::Outside
                        || a.class == Class::Outline
                        || b.class == Class::Outline
                        || !segment_covered(
                            p,
                            q,
                            footprint_bins[k]
                                .iter()
                                .chain(&footprint_bins[qk])
                                .map(|&t| region.triangles[t]),
                        )
                    {
                        Boundary::Outline
                    } else if a.class != Class::Wet
                        || b.class != Class::Wet
                        || middle.bed.is_none()
                        || middle.overhang
                    {
                        Boundary::Unsupported
                    } else {
                        Boundary::Open
                    };
                    let crest = middle.bed.map_or(f32::NAN, |v| v.max(a.bed).max(b.bed));
                    let face = Face { boundary, crest };
                    if axis == 0 {
                        grid.faces_x[(y * (size[0] + 1) + x + 1) as usize] = face;
                    } else {
                        grid.faces_y[((y + 1) * size[0] + x) as usize] = face;
                    }
                }
            }
        }
        grid.label_components();
        Ok(grid)
    }

    fn column(&self, p: [f32; 2], level: f32, solids: &[usize], triangles: &[usize]) -> Column {
        self.column_with_sky(p, level, solids, triangles, false)
    }

    fn column_with_sky(&self, p: [f32; 2], level: f32, solids: &[usize], triangles: &[usize], open_sky: bool) -> Column {
        let mut spans: Vec<_> = solids
            .iter()
            .filter_map(|&s| {
                let solid = &self.solids[s];
                let span = solid.column(p)?;
                // The BSP's sky shell closes collision/visibility. Its vertical sky face
                // at the water level is an opening to the extended sea, not a coastal bank.
                // Retain real seabed below the water and the existing solver's geometry.
                let sky_boundary = open_sky && span.0 <= f64::from(level) && span.1 >= f64::from(level)
                    && solid.sky.iter().zip(&solid.planes).any(|(sky,plane)| *sky && f64::from(plane.normal[2]).abs() < EPS);
                (!sky_boundary).then_some(span)
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64, bool)> = Vec::new();
        for span in spans {
            if let Some(last) = merged.last_mut().filter(|last| span.0 <= last.1 + EPS) {
                if span.1 > last.1 {
                    last.1 = span.1;
                    last.2 = span.2;
                }
            } else {
                merged.push(span);
            }
        }
        let level = f64::from(level);
        let containing = merged.iter().find(|s| s.0 <= level && s.1 >= level - EPS);
        let mut bed = containing.filter(|s| !s.2).map(|s| s.1);
        if containing.is_none() {
            bed = merged
                .iter()
                .filter(|s| s.1 < level && !s.2)
                .map(|s| s.1)
                .max_by(f64::total_cmp);
        }
        let mut ceiling = merged
            .iter()
            .filter(|s| s.0 > level)
            .map(|s| s.0)
            .min_by(f64::total_cmp);
        let mut below_ceiling = None::<f64>;
        for &t in triangles {
            let t = &self.triangles[t];
            let Some(z) = t.height(p) else {
                continue;
            };
            if t.normal[2] > EPS && z < level + EPS && bed.is_none_or(|b| z > b) {
                bed = Some(z);
            }
            if t.normal[2] < -EPS {
                if z > level && ceiling.is_none_or(|c| z < c) {
                    ceiling = Some(z);
                }
                if z < level && below_ceiling.is_none_or(|c| z > c) {
                    below_ceiling = Some(z);
                }
            }
        }
        // Multiple separated solid layers below the water surface are not a
        // single-valued bed. Reject rather than treating a submerged bridge as
        // a continuous filled mound and hiding the water below it.
        // A water volume identifies which vertical layer belongs to this body.
        // It never supplies a bed or limits the measured depth. Rooms below the
        // authored volume do not make an otherwise valid basin an overhang.
        let liquid_bottom = self
            .liquids
            .iter()
            .filter(|s| (0..2).all(|a| p[a] >= s.bounds[0][a] && p[a] <= s.bounds[1][a]))
            .filter_map(|s| s.column(p))
            .filter(|s| (s.1 - level).abs() < 1.01)
            .map(|s| s.0)
            .min_by(f64::total_cmp);
        let stacked = merged
            .iter()
            .filter(|s| s.1 < level - EPS && liquid_bottom.is_none_or(|bottom| s.1 > bottom + EPS))
            .count()
            > 1;
        // An overhead bridge is supported above this water layer: retain its
        // ceiling/headroom for the eventual solver and renderer. A submerged
        // underside above the selected bed is an ambiguous fluid column.
        let overhang = stacked || below_ceiling.is_some_and(|c| bed.is_some_and(|b| c > b + EPS));
        Column {
            bed: bed.map(|b| b as f32),
            ceiling: ceiling.map(|c| c as f32),
            overhang,
        }
    }
}

#[derive(Clone, Copy)]
struct Column {
    bed: Option<f32>,
    ceiling: Option<f32>,
    overhang: bool,
}

impl Solid {
    fn column(&self, p: [f32; 2]) -> Option<(f64, f64, bool)> {
        let (mut low, mut high, mut sky) = (f64::NEG_INFINITY, f64::INFINITY, false);
        for (i, plane) in self.planes.iter().enumerate() {
            let rhs = f64::from(plane.distance)
                - f64::from(plane.normal[0]) * f64::from(p[0])
                - f64::from(plane.normal[1]) * f64::from(p[1]);
            let nz = f64::from(plane.normal[2]);
            if nz.abs() < 1e-9 {
                if rhs < -EPS {
                    return None;
                }
            } else if nz > 0.0 {
                let h = rhs / nz;
                if h < high {
                    high = h;
                    sky = self.sky[i];
                }
            } else {
                low = low.max(rhs / nz);
            }
        }
        (low.is_finite() && high.is_finite() && low <= high + EPS).then_some((low, high, sky))
    }
    fn blocks(&self, a: [f32; 2], b: [f32; 2], z: f32) -> Option<Blocker> {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let (mut lo_blocker, mut hi_blocker) = (None, None);
        for plane in &self.planes {
            let d = f64::from(plane.normal[0]) * f64::from(b[0] - a[0])
                + f64::from(plane.normal[1]) * f64::from(b[1] - a[1]);
            let rhs = f64::from(plane.distance)
                - f64::from(plane.normal[0]) * f64::from(a[0])
                - f64::from(plane.normal[1]) * f64::from(a[1])
                - f64::from(plane.normal[2]) * f64::from(z - EPS as f32);
            if d.abs() < 1e-9 {
                if rhs < -EPS {
                    return None;
                }
            } else if d > 0.0 {
                let clipped = rhs / d;
                if clipped < hi {
                    hi = clipped;
                    hi_blocker = Some(blocker_from_normal(plane.normal));
                }
            } else {
                let clipped = rhs / d;
                if clipped > lo {
                    lo = clipped;
                    lo_blocker = Some(blocker_from_normal(plane.normal));
                }
            }
            if lo > hi + 1e-9 {
                return None;
            }
        }
        if hi < 0.0 || lo > 1.0 {
            return None;
        }
        let mut result = None;
        if (-1e-7..=1.0000001).contains(&lo) {
            if let Some(kind) = lo_blocker {
                result = strongest_blocker(result, kind);
            }
        }
        if (-1e-7..=1.0000001).contains(&hi) {
            if let Some(kind) = hi_blocker {
                result = strongest_blocker(result, kind);
            }
        }
        // A whole segment embedded in solid is dry terrain unless an actual
        // limiting side identifies a vertical obstruction.
        Some(result.unwrap_or(Blocker::Bank))
    }
}

impl Triangle {
    fn height(&self, p: [f32; 2]) -> Option<f64> {
        if self.normal[2].abs() < 1e-9 || !contains(self.points.map(|p| [p[0], p[1]]), p) {
            return None;
        }
        let a = self.points[0];
        Some(
            f64::from(a[2])
                - (self.normal[0] * f64::from(p[0] - a[0])
                    + self.normal[1] * f64::from(p[1] - a[1]))
                    / self.normal[2],
        )
    }
    fn blocks(&self, a: [f32; 2], b: [f32; 2], z: f32) -> Option<Blocker> {
        let origin = [a[0], a[1], z - EPS as f32];
        let end = [b[0], b[1], z - EPS as f32];
        let dir = sub(end, origin);
        let e1 = sub(self.points[1], self.points[0]);
        let e2 = sub(self.points[2], self.points[0]);
        let h = cross(dir, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-9 {
            return None;
        }
        let s = sub(origin, self.points[0]);
        let u = dot(s, h) / det;
        if !(-1e-7..=1.0000001).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(dir, q) / det;
        let t = dot(e2, q) / det;
        (v >= -1e-7 && u + v <= 1.0000001 && (-1e-7..=1.0000001).contains(&t))
            .then(|| blocker_from_f64_normal(self.normal))
    }
}

fn contains(t: FootprintTriangle, p: [f32; 2]) -> bool {
    let mut positive = false;
    let mut negative = false;
    let mut area = 0.0_f64;
    for i in 0..3 {
        let a = t[i];
        let b = t[(i + 1) % 3];
        let e = f64::from(b[0] - a[0]) * f64::from(p[1] - a[1])
            - f64::from(b[1] - a[1]) * f64::from(p[0] - a[0]);
        positive |= e > 1e-6;
        negative |= e < -1e-6;
        area += f64::from(a[0]) * f64::from(b[1]) - f64::from(a[1]) * f64::from(b[0]);
    }
    area.abs() > 1e-6 && !(positive && negative)
}

/// Union of exact triangle/segment intervals. Midpoint tests alone can miss a
/// narrow gap or island between two wet cells and falsely connect the water.
fn segment_covered(
    a: [f32; 2],
    b: [f32; 2],
    triangles: impl Iterator<Item = FootprintTriangle>,
) -> bool {
    let mut intervals = Vec::new();
    for t in triangles {
        let orientation = f64::from(t[1][0] - t[0][0]) * f64::from(t[2][1] - t[0][1])
            - f64::from(t[1][1] - t[0][1]) * f64::from(t[2][0] - t[0][0]);
        if orientation.abs() < 1e-9 {
            continue;
        }
        let sign = orientation.signum();
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut valid = true;
        for i in 0..3 {
            let p = t[i];
            let q = t[(i + 1) % 3];
            let dx = f64::from(q[0] - p[0]);
            let dy = f64::from(q[1] - p[1]);
            let start = sign * (dx * f64::from(a[1] - p[1]) - dy * f64::from(a[0] - p[0]));
            let slope = sign * (dx * f64::from(b[1] - a[1]) - dy * f64::from(b[0] - a[0]));
            if slope.abs() < 1e-12 {
                if start < -1e-8 {
                    valid = false;
                    break;
                }
            } else if slope > 0.0 {
                lo = lo.max(-start / slope);
            } else {
                hi = hi.min(-start / slope);
            }
        }
        if valid && lo <= hi {
            intervals.push((lo, hi));
        }
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut reached = 0.0;
    for (lo, hi) in intervals {
        if lo > reached + 1e-7 {
            return false;
        }
        reached = f64::max(reached, hi);
        if reached >= 1.0 - 1e-7 {
            return true;
        }
    }
    false
}

fn bins(
    bounds: impl Iterator<Item = [[f32; 2]; 2]>,
    origin: [f32; 2],
    size: [u32; 2],
    dx: f32,
) -> Result<Vec<Vec<usize>>, String> {
    let mut bins = vec![Vec::new(); size[0] as usize * size[1] as usize];
    let mut references = 0usize;
    for (id, b) in bounds.enumerate() {
        let low: [i64; 2] = std::array::from_fn(|a| ((b[0][a] - origin[a]) / dx).floor() as i64);
        let high: [i64; 2] = std::array::from_fn(|a| ((b[1][a] - origin[a]) / dx).floor() as i64);
        let low = low.map(|n| n.max(0));
        let high: [i64; 2] = std::array::from_fn(|a| high[a].min(i64::from(size[a]) - 1));
        if (0..2).any(|a| low[a] > high[a]) {
            continue;
        }
        references += ((high[0] - low[0] + 1) * (high[1] - low[1] + 1)) as usize;
        if references > 8_000_000 {
            return Err("geometry reference budget exceeded".into());
        }
        for y in low[1]..=high[1] {
            for x in low[0]..=high[0] {
                bins[y as usize * size[0] as usize + x as usize].push(id);
            }
        }
    }
    Ok(bins)
}

impl Grid {
    fn label_components(&mut self) {
        let nx = self.size[0] as usize;
        let ny = self.size[1] as usize;
        for start in 0..self.cells.len() {
            if self.cells[start].class != Class::Wet || self.cells[start].component != 0 {
                continue;
            }
            self.components += 1;
            self.cells[start].component = self.components;
            let mut queue = VecDeque::from([start]);
            while let Some(k) = queue.pop_front() {
                let x = k % nx;
                let y = k / nx;
                let neighbours = [
                    (x > 0, k.wrapping_sub(1), self.faces_x[y * (nx + 1) + x]),
                    (x + 1 < nx, k + 1, self.faces_x[y * (nx + 1) + x + 1]),
                    (y > 0, k.wrapping_sub(nx), self.faces_y[y * nx + x]),
                    (y + 1 < ny, k + nx, self.faces_y[(y + 1) * nx + x]),
                ];
                for (valid, q, face) in neighbours {
                    if valid
                        && face.boundary == Boundary::Open
                        && self.cells[q].class == Class::Wet
                        && self.cells[q].component == 0
                    {
                        self.cells[q].component = self.components;
                        queue.push_back(q);
                    }
                }
            }
        }
    }
}
