//! Sparse tile-pool river for long-distance flow. NOT an Unreal system: UE
//! 5.8.1 simulates one dense rectangle (see jka_water::river::sparse for the
//! storage contract and its CPU equivalence tests). The hydraulic stages are
//! the same shallow.wgsl bodies with the sparse addressing prelude, dispatched
//! once per stage over every resident tile (z = slot), so the dispatch count
//! per solver frame does not grow with river length.
//!
//! Residency grows from GPU occupancy read back asynchronously (no stalls):
//! every tile within `guard` tiles of a non-pristine tile is captured and made
//! resident. Solver frames are throttled so the wet front can never reach a
//! non-resident cell before its tile exists. A camera-centred dense window is
//! gathered from the pool for the existing presentation chain (material, foam,
//! Single Layer Water body), like upstream's pawn-following window.
use crate::river_capture::{bed_polygon, DepthCaptures, Mesh, MeshClass, Sides};
use crate::river_gpu::Kernel;
use bytemuck::{Pod, Zeroable};
use jka_assets::bsp::river::CollisionSurface;
use jka_water::river::boundary::{Controls, Drain, SourceBox};
use jka_water::river::capture::{Camera, Channel, CHANNELS};
use jka_water::river::control::{Frame, Grid};
use jka_water::river::sparse::{required_tiles, solver_frames_within_margin, Lattice, Residency};
use jka_water::river::{Parameters, Presentation, Transport, CENTIMETRES_PER_MAP_UNIT};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver};
use wgpu::util::DeviceExt;

#[path = "river_tile_schedule.rs"]
mod schedule;
pub use schedule::{mode as tile_schedule, mode_name as tile_schedule_name, set_mode as set_tile_schedule};
#[path = "river_storage.rs"]
mod storage;
pub use storage::{mode as storage_mode, mode_name as storage_mode_name, set_mode as set_storage_mode, unpack_normal, Storage};
#[cfg(test)]
pub(crate) use storage::override_for_thread as override_storage_for_thread;

/// river_tiles.wgsl without its storage prelude; see `Storage::tiles_shader`.
const TILES_BODY: &str = include_str!("river_tiles.wgsl");
/// PreSim (stage 5) moving-body collisions over the pool; see the file header.
pub const COLLISIONS_SHADER: &str = include_str!("river_collisions.wgsl");
/// Bodies stamped per solver frame (the ocean's interaction bound is 16 too).
pub const MAX_BODIES: usize = 16;
/// Largest footprint side in cells; a body wider than this at a tiny cell
/// size is clipped to its centre square.
const MAX_FOOTPRINT: u32 = 64;

/// A moving axis-aligned box for the PreSim collision stage, in lattice-local
/// centimetres (XY from the lattice corner, Z in the river's height frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionBody {
    pub centre: [f32; 3],
    /// Full box size (Unreal BoxExtent; half-size = 0.5 * extent).
    pub extent: [f32; 3],
    /// cm/s, lattice axes.
    pub velocity: [f32; 3],
}

/// Footprint bookkeeping for the collision stage. Upstream rewrites the whole
/// grid's collider attributes every PreSim; the pool keeps the static border
/// and void colliders in the bed, so only body footprints are rewritten and
/// restored (river_collisions.wgsl).
struct BodyCollisions {
    side: u32,
    /// [first solver frame of a render frame, later solver frames].
    uniforms: [wgpu::Buffer; 2],
    saved: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    restore: wgpu::ComputePipeline,
    save: wgpu::ComputePipeline,
    stamp: wgpu::ComputePipeline,
    /// (pool generation, [uniform variant][state parity]).
    groups: Option<(u64, [[wgpu::BindGroup; 2]; 2])>,
    /// Footprint corners written by the last encoded stamp (still in the bed).
    stamped: Vec<[i32; 2]>,
    /// Footprint corners of the bodies set for this render frame.
    current: Vec<[i32; 2]>,
    /// The first solver frame since `set_bodies` has been encoded.
    first_done: bool,
}

impl BodyCollisions {
    /// Footprints cover a 32-unit-wide body (a JKA player hull is 30) plus a
    /// cell of margin per side and the floor rounding, in whole workgroups.
    fn new(device: &wgpu::Device, dx_cm: f32) -> Self {
        let width_cells = (32.0 * CENTIMETRES_PER_MAP_UNIT / dx_cm).ceil().max(1.0) as u32 + 3;
        let side = width_cells.div_ceil(8).saturating_mul(8).clamp(8, MAX_FOOTPRINT);
        let uniform_bytes = 4 * (16 + 4 * MAX_BODIES + 12 * MAX_BODIES) as u64;
        let uniforms = ["sparse river body collisions (first frame)", "sparse river body collisions (later frames)"].map(|label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let saved = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river saved footprint colliders"),
            size: u64::from(side * side) * MAX_BODIES as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sparse river body collisions"),
            entries: &[uniform_entry(0), storage_entry(1, true), storage_entry(2, false), storage_entry(3, false), storage_entry(4, true)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sparse river body collisions"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river PreSim body collisions"),
            source: wgpu::ShaderSource::Wgsl(COLLISIONS_SHADER.into()),
        });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            side,
            uniforms,
            saved,
            layout,
            restore: make("restore_bodies"),
            save: make("save_bodies"),
            stamp: make("stamp_bodies"),
            groups: None,
            stamped: Vec::new(),
            current: Vec::new(),
            first_done: false,
        }
    }
}

// Session cvars (r_riverSparse, r_riverTileSize, r_riverTileGuard,
// r_riverWindow). Read when a river spawns; existing rivers keep theirs.
static SPARSE: AtomicU8 = AtomicU8::new(1);
static TILE: AtomicU32 = AtomicU32::new(32);
static GUARD: AtomicU32 = AtomicU32::new(2);
static WINDOW: AtomicU32 = AtomicU32::new(512);

// r_riverAdvectDepth (not Unreal when 0): read when a river spawns.
static ADVECT_DEPTH: AtomicU8 = AtomicU8::new(0);

pub fn advect_depth() -> bool {
    ADVECT_DEPTH.load(Ordering::Relaxed) != 0
}
pub fn set_advect_depth(enabled: bool) {
    ADVECT_DEPTH.store(u8::from(enabled), Ordering::Relaxed);
}
/// The transport a newly placed live river uses: Unreal's defaults, with
/// depth advection per r_riverAdvectDepth.
pub fn spawn_transport() -> Transport {
    Transport { advect_depth: advect_depth(), ..Transport::default() }
}
/// Developer drains per river (spawndrain), stored after the native source
/// boxes in the same box array (kind 2 in shallow.wgsl).
pub const MAX_DRAINS: usize = 32;

/// One drain as a shallow.wgsl `SourceBox` (kind 2).
pub fn drain_box_words(d: &Drain) -> [f32; 8] {
    [d.center[0], d.center[1], d.level, 0.0, d.radius, d.max_removal, 1.0, 2.0]
}

/// Stage-uniform flag word (shallow.wgsl `dimensions.w`).
pub fn transport_flags(t: Transport) -> u32 {
    u32::from(t.extrapolate_into_dry) | (u32::from(!t.advect_depth) << 1)
}

pub fn sparse_enabled() -> bool {
    SPARSE.load(Ordering::Relaxed) != 0
}
pub fn set_sparse_enabled(enabled: bool) {
    SPARSE.store(u8::from(enabled), Ordering::Relaxed);
}
pub fn tile_size() -> u32 {
    TILE.load(Ordering::Relaxed)
}
/// Rounded to a power of two in 16..=128.
pub fn set_tile_size(cells: u32) {
    TILE.store(cells.clamp(16, 128).next_power_of_two().min(128), Ordering::Relaxed);
}
pub fn guard_tiles() -> u32 {
    GUARD.load(Ordering::Relaxed)
}
pub fn set_guard_tiles(tiles: u32) {
    GUARD.store(tiles.clamp(1, 4), Ordering::Relaxed);
}
pub fn window_cells() -> u32 {
    WINDOW.load(Ordering::Relaxed)
}
pub fn set_window_cells(cells: u32) {
    WINDOW.store(cells.clamp(128, 2048), Ordering::Relaxed);
}

/// Most tiles captured and committed in one render frame; the rest wait and
/// the solver keeps to its proven margin meanwhile.
const MAX_ACTIVATIONS_PER_FRAME: usize = 24;
/// Long-dry tile eviction (r_riverStorageMode 1/2; not Unreal): consecutive
/// occupancy samples a resident tile must spend outside every occupied
/// tile's guard ring before it is released, and tiles released per frame.
const EVICT_AFTER_SAMPLES: u32 = 120;
const MAX_EVICTIONS_PER_FRAME: usize = 32;

/// Pool slots for `slots` resident tiles: 1.5x headroom in 16-slot steps
/// (power-of-two doubling left up to half the pool unused).
fn pool_capacity_for(slots: u32) -> u32 {
    (slots + slots / 2).max(16).div_ceil(16) * 16
}
const READBACKS_IN_FLIGHT: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StageUniform {
    dimensions: [u32; 4],
    stepping: [f32; 4],
    limits: [f32; 4],
    fade: [f32; 4],
}

impl StageUniform {
    fn with_parameters(mut self, p: Parameters) -> Self {
        self.stepping = [p.dx, p.dt, p.gravity, p.max_velocity];
        self.limits = [p.friction, p.velocity_dissipation, p.max_depth, p.integration_depth_max];
        self.fade[0] = p.fade_start;
        self.fade[1] = p.fade_speed;
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BoundaryUniform {
    times: [f32; 4],
    origin: [f32; 4],
    flags: [u32; 4],
}

/// Clipped map collision binned per lattice tile, built once per river.
pub struct Bins {
    /// Landscape (mode 0: every clipped bed candidate).
    triangles: Vec<[[f32; 3]; 3]>,
    bins: Vec<Vec<u32>>,
    /// `r_riverCaptureMode 1`: structures above the reachable bed, captured
    /// from above and below. Empty in mode 0.
    actors: Vec<[[f32; 3]; 3]>,
    actor_bins: Vec<Vec<u32>>,
    /// Mode 1 stage 6 WaterInfo height per lattice cell; None in mode 0.
    references: Option<crate::river_reach::References>,
    lattice: Lattice,
    /// Map position of cell (0, 0)'s lower corner.
    min_map: [f64; 2],
    cell_map: f64,
    /// Landscape heights: bed, window and far-mesh rebasing.
    z_range: [f32; 2],
    /// Landscape and actor heights: the capture cameras.
    capture_z_range: [f32; 2],
}

fn validate_layout(cell_map: f64, anchor: [f64; 2]) -> Result<(), String> {
    if !(cell_map.is_finite() && cell_map > 0.0) || anchor.iter().any(|v| !v.is_finite()) {
        return Err("invalid sparse river cell size or anchor".into());
    }
    Ok(())
}

/// The lattice covering an XY box plus a two-cell border, with cell corners
/// aligned on `anchor` (the source centre).
fn layout(lo: [f64; 2], hi: [f64; 2], cell_map: f64, tile: u32, anchor: [f64; 2]) -> Result<(Lattice, [f64; 2]), String> {
    let min_map: [f64; 2] = std::array::from_fn(|a| {
        let below = ((anchor[a] - lo[a]) / cell_map).ceil().max(0.0) + 2.0;
        anchor[a] - below * cell_map
    });
    let cells: [u32; 2] = std::array::from_fn(|a| {
        let n = ((hi[a] - min_map[a]) / cell_map).ceil() + 2.0;
        n.clamp(1.0, f64::from(u32::MAX / 2)) as u32
    });
    let lattice = Lattice::covering(tile, cells).map_err(|e| format!("{e} (map too large for this cell size)"))?;
    Ok((lattice, min_map))
}

fn z_bounds(triangles: &[[[f32; 3]; 3]]) -> [f32; 2] {
    triangles.iter().flatten().fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], p| [lo.min(p[2]), hi.max(p[2])])
}

impl Bins {
    /// Mode 0. The lattice covers every candidate riverbed triangle plus a
    /// two-cell border, with cell corners aligned on `anchor` (the source centre).
    pub fn new(
        collision: &[CollisionSurface],
        water_level: f32,
        cell_map: f64,
        tile: u32,
        anchor: [f64; 2],
    ) -> Result<Self, String> {
        validate_layout(cell_map, anchor)?;
        let mut triangles = Vec::new();
        for surface in collision {
            let Some((polygon, count)) =
                bed_polygon(&surface.triangle, surface.solid_bottom_z, surface.wall, Some(water_level), cell_map as f32)
            else {
                continue;
            };
            for k in 2..count {
                let t = [polygon[0], polygon[k - 1], polygon[k]];
                if t.iter().flatten().all(|v| v.is_finite()) {
                    triangles.push(t);
                }
            }
        }
        if triangles.is_empty() {
            return Err("no collision geometry near or below the river water level".into());
        }
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for p in triangles.iter().flatten() {
            for axis in 0..2 {
                lo[axis] = lo[axis].min(f64::from(p[axis]));
                hi[axis] = hi[axis].max(f64::from(p[axis]));
            }
        }
        let (lattice, min_map) = layout(lo, hi, cell_map, tile, anchor)?;
        Ok(Self::binned(triangles, Vec::new(), None, lattice, min_map, cell_map))
    }

    /// `r_riverCaptureMode 1`: the lattice covers every solid that can hold
    /// water below the source level (plus margin); the bed and its actors come
    /// from `river_reach::classify` over that same lattice, so tile seams and
    /// activation order cannot change a cell's classification.
    pub fn new_classified(
        geometry: &jka_assets::bsp::river::CaptureGeometry,
        seed: [f32; 3],
        water_level: f32,
        cell_map: f64,
        tile: u32,
        anchor: [f64; 2],
    ) -> Result<(Self, crate::river_reach::Stats), String> {
        validate_layout(cell_map, anchor)?;
        let cap = water_level + crate::river_reach::LEVEL_MARGIN;
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        let mut extend = |points: &mut dyn Iterator<Item = [f32; 2]>| {
            for p in points {
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(f64::from(p[axis]));
                    hi[axis] = hi[axis].max(f64::from(p[axis]));
                }
            }
        };
        for brush in geometry.brushes.iter().filter(|b| b.mins[2] <= cap) {
            extend(&mut [[brush.mins[0], brush.mins[1]], [brush.maxs[0], brush.maxs[1]]].into_iter());
        }
        for mesh in geometry.meshes.iter().filter(|m| m.triangle.iter().any(|p| p[2] <= cap)) {
            extend(&mut mesh.triangle.iter().map(|p| [p[0], p[1]]));
        }
        if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
            return Err("no collision geometry near or below the river water level".into());
        }
        let (lattice, min_map) = layout(lo, hi, cell_map, tile, anchor)?;
        let region = crate::river_reach::Region { min: min_map, cell: cell_map, dims: lattice.dimensions() };
        let classified = crate::river_reach::classify(geometry, region, seed, water_level)?;
        if classified.landscape.is_empty() {
            return Err("no reachable river bed (r_riverCaptureMode 1)".into());
        }
        let bins = Self::binned(
            classified.landscape,
            classified.actors,
            Some(classified.references),
            lattice,
            min_map,
            cell_map,
        );
        Ok((bins, classified.stats))
    }

    fn binned(
        triangles: Vec<[[f32; 3]; 3]>,
        actors: Vec<[[f32; 3]; 3]>,
        references: Option<crate::river_reach::References>,
        lattice: Lattice,
        min_map: [f64; 2],
        cell_map: f64,
    ) -> Self {
        let [tw, th] = lattice.tiles();
        let tile_map = cell_map * f64::from(lattice.tile());
        let bin = |triangles: &[[[f32; 3]; 3]]| {
            let mut bins = vec![Vec::new(); tw as usize * th as usize];
            for (index, t) in triangles.iter().enumerate() {
                let range = |axis: usize, limit: u32| {
                    let a = t.iter().map(|p| f64::from(p[axis])).fold(f64::INFINITY, f64::min);
                    let b = t.iter().map(|p| f64::from(p[axis])).fold(f64::NEG_INFINITY, f64::max);
                    let first = ((a - min_map[axis]) / tile_map).floor().clamp(0.0, f64::from(limit - 1)) as u32;
                    let last = ((b - min_map[axis]) / tile_map).floor().clamp(0.0, f64::from(limit - 1)) as u32;
                    first..=last
                };
                for y in range(1, th) {
                    for x in range(0, tw) {
                        bins[y as usize * tw as usize + x as usize].push(index as u32);
                    }
                }
            }
            bins
        };
        let z_range = z_bounds(&triangles);
        let actor_z = z_bounds(&actors);
        Self {
            bins: bin(&triangles),
            actor_bins: if actors.is_empty() { Vec::new() } else { bin(&actors) },
            triangles,
            actors,
            references,
            lattice,
            min_map,
            cell_map,
            z_range,
            capture_z_range: [z_range[0].min(actor_z[0]), z_range[1].max(actor_z[1])],
        }
    }

    pub fn lattice(&self) -> Lattice {
        self.lattice
    }
    pub fn min_map(&self) -> [f64; 2] {
        self.min_map
    }
    pub fn z_range(&self) -> [f32; 2] {
        self.z_range
    }
    pub fn capture_z_range(&self) -> [f32; 2] {
        self.capture_z_range
    }

    /// Mode 1: stage 6's WaterInfo height (local cm) for each cell of one
    /// tile, row-major; `void_cm` where water falls into the void. None in mode 0.
    fn tile_references(&self, tile: [i32; 2], origin_z_cm: f64, void_cm: f32) -> Option<Vec<f32>> {
        let references = self.references.as_ref()?;
        let origin = self.lattice.tile_origin(tile);
        let n = self.lattice.tile() as i64;
        Some(
            (0..n * n)
                .map(|i| {
                    let cell = [i64::from(origin[0]) + i % n, i64::from(origin[1]) + i / n];
                    references.local_cm(cell, origin_z_cm, void_cm)
                })
                .collect(),
        )
    }

    fn bin(&self, tile: [i32; 2]) -> &[u32] {
        if !self.lattice.contains_tile(tile) {
            return &[];
        }
        &self.bins[tile[1] as usize * self.lattice.tiles()[0] as usize + tile[0] as usize]
    }

    /// Lowest captured bed candidate in a range of tiles (map units).
    pub fn min_z(&self, first: [i32; 2], last: [i32; 2]) -> Option<f32> {
        let mut z = f32::INFINITY;
        for y in first[1]..=last[1] {
            for x in first[0]..=last[0] {
                for &i in self.bin([x, y]) {
                    for p in &self.triangles[i as usize] {
                        z = z.min(p[2]);
                    }
                }
            }
        }
        z.is_finite().then_some(z)
    }

    /// One tile's triangles in capture space: cm relative to the tile centre
    /// in XY and to the river origin in Z. Landscape vertices come first;
    /// the second value is where the (mode 1) actor vertices start.
    fn tile_mesh(&self, tile: [i32; 2], origin_z_cm: f64) -> (Vec<[f32; 3]>, usize) {
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let side = self.cell_map * f64::from(self.lattice.tile());
        let centre: [f64; 2] = std::array::from_fn(|a| self.min_map[a] + (f64::from(tile[a]) + 0.5) * side);
        let append = |positions: &mut Vec<[f32; 3]>, triangles: &[[[f32; 3]; 3]], bin: &[u32]| {
            for &i in bin {
                for p in &triangles[i as usize] {
                    positions.push([
                        ((f64::from(p[0]) - centre[0]) * cm) as f32,
                        ((f64::from(p[1]) - centre[1]) * cm) as f32,
                        (f64::from(p[2]) * cm - origin_z_cm) as f32,
                    ]);
                }
            }
        };
        let mut positions = Vec::new();
        append(&mut positions, &self.triangles, self.bin(tile));
        let split = positions.len();
        if !self.actor_bins.is_empty() && self.lattice.contains_tile(tile) {
            let entry = tile[1] as usize * self.lattice.tiles()[0] as usize + tile[0] as usize;
            append(&mut positions, &self.actors, &self.actor_bins[entry]);
        }
        (positions, split)
    }
}

/// Per-cell pool buffers, `capacity` tile slots each.
struct Pool {
    capacity: u32,
    storage: Storage,
    state: [wgpu::Buffer; 2],
    geometry: wgpu::Buffer,
    /// PreSim history and WaterInfo: r_riverStorageMode 0 only.
    history: Option<wgpu::Buffer>,
    water_info: Option<wgpu::Buffer>,
    surface: wgpu::Buffer,
    /// vec4<f32> per cell, or one packed octahedral u32 (Storage::Packed).
    normals: wgpu::Buffer,
    occupancy: wgpu::Buffer,
    slot_tiles: wgpu::Buffer,
}

impl Pool {
    fn new(device: &wgpu::Device, capacity: u32, tile_cells: u64, storage: Storage) -> Result<Self, String> {
        let cells = u64::from(capacity) * tile_cells;
        let limits = device.limits();
        if cells * 16 > u64::from(limits.max_storage_buffer_binding_size) || cells * 16 > limits.max_buffer_size {
            return Err(format!("sparse river pool of {capacity} tiles exceeds the GPU storage limit"));
        }
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(16), usage, mapped_at_creation: false })
        };
        let native = storage.stores_history();
        Ok(Self {
            capacity,
            storage,
            state: [buffer("sparse river state A", cells * 16), buffer("sparse river state B", cells * 16)],
            geometry: buffer("sparse river geometry", cells * 16),
            history: native.then(|| buffer("sparse river previous bottom", cells * 4)),
            // Simulation-first rivers have a dry WaterInfo: zero-initialised.
            water_info: native.then(|| buffer("sparse river WaterInfo", cells * 16)),
            surface: buffer("sparse river rendered surface", cells * 16),
            normals: buffer("sparse river normals", cells * storage.normal_stride()),
            occupancy: buffer("sparse river tile occupancy", u64::from(capacity) * 4),
            slot_tiles: buffer("sparse river slot tiles", u64::from(capacity) * 8),
        })
    }

    fn copy_into(&self, encoder: &mut wgpu::CommandEncoder, to: &Pool, slots: u64, tile_cells: u64) {
        if slots == 0 {
            return;
        }
        let cells = slots * tile_cells;
        debug_assert_eq!(self.storage, to.storage, "pool growth keeps the spawn layout");
        let per_cell = [
            Some((&self.state[0], &to.state[0], 16)),
            Some((&self.state[1], &to.state[1], 16)),
            Some((&self.geometry, &to.geometry, 16)),
            self.history.as_ref().zip(to.history.as_ref()).map(|(a, b)| (a, b, 4)),
            self.water_info.as_ref().zip(to.water_info.as_ref()).map(|(a, b)| (a, b, 16)),
            Some((&self.surface, &to.surface, 16)),
            Some((&self.normals, &to.normals, self.storage.normal_stride())),
        ];
        for (from, into, stride) in per_cell.into_iter().flatten() {
            encoder.copy_buffer_to_buffer(from, 0, into, 0, cells * stride);
        }
        encoder.copy_buffer_to_buffer(&self.slot_tiles, 0, &to.slot_tiles, 0, slots * 8);
    }

    /// Copy slot `from`'s cells over slot `to` in every per-cell buffer,
    /// through `scratch` (one tile of the widest stride).
    fn move_slot(&self, encoder: &mut wgpu::CommandEncoder, scratch: &wgpu::Buffer, from: u32, to: u32, tile_cells: u64) {
        let per_cell = [
            Some((&self.state[0], 16)),
            Some((&self.state[1], 16)),
            Some((&self.geometry, 16)),
            self.history.as_ref().map(|b| (b, 4)),
            self.water_info.as_ref().map(|b| (b, 16)),
            Some((&self.surface, 16)),
            Some((&self.normals, self.storage.normal_stride())),
        ];
        for (buffer, stride) in per_cell.into_iter().flatten() {
            let size = tile_cells * stride;
            encoder.copy_buffer_to_buffer(buffer, u64::from(from) * size, scratch, 0, size);
            encoder.copy_buffer_to_buffer(scratch, 0, buffer, u64::from(to) * size, size);
        }
    }

    fn bytes(&self, tile_cells: u64) -> u64 {
        u64::from(self.capacity) * (tile_cells * self.storage.bytes_per_cell() + 12)
    }
}

enum Stage {
    Copied,
    Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

struct Pending {
    buffer: wgpu::Buffer,
    frame: u64,
    /// Residency epoch at encode: eviction renumbers slots, so a sample
    /// from an older epoch no longer maps flags to tiles.
    epoch: u64,
    slots: u32,
    /// One word after the flags: the tile schedule's scheduled-slot count.
    extra: bool,
    stage: Stage,
}

/// Asynchronous per-slot occupancy readback ring. Never waits on the GPU.
#[derive(Default)]
struct Readback {
    pending: VecDeque<Pending>,
    free: Vec<wgpu::Buffer>,
}

impl Readback {
    /// Oldest completed sample, in submission order: (solver frame, flags,
    /// scheduled-slot count when the tile schedule appended it).
    fn poll(&mut self, device: &wgpu::Device) -> Option<(u64, Vec<u32>, Option<u32>, u64)> {
        for p in &mut self.pending {
            if matches!(p.stage, Stage::Copied) {
                let (tx, rx) = mpsc::channel();
                let words = u64::from(p.slots.max(1)) + u64::from(p.extra);
                p.buffer.slice(..words * 4).map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                p.stage = Stage::Mapping(rx);
            }
        }
        if self.pending.is_empty() {
            return None;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        let front = self.pending.front()?;
        let Stage::Mapping(rx) = &front.stage else { return None };
        let result = rx.try_recv().ok()?;
        let front = self.pending.pop_front()?;
        let mut flags = Vec::new();
        let mut extra = None;
        if result.is_ok() {
            let words = u64::from(front.slots.max(1)) + u64::from(front.extra);
            if let Ok(view) = front.buffer.slice(..words * 4).get_mapped_range() {
                let word = |i: usize| u32::from_le_bytes([view[4 * i], view[4 * i + 1], view[4 * i + 2], view[4 * i + 3]]);
                flags = (0..front.slots as usize).map(word).collect();
                if front.extra {
                    extra = Some(word(front.slots.max(1) as usize));
                }
            }
            front.buffer.unmap();
        }
        self.free.push(front.buffer);
        (result.is_ok() && flags.len() == front.slots as usize).then_some((front.frame, flags, extra, front.epoch))
    }

    fn buffer(&mut self, device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        if let Some(i) = self.free.iter().position(|b| b.size() >= bytes) {
            return self.free.swap_remove(i);
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river occupancy readback"),
            size: bytes.max(4).next_power_of_two(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

/// Host CFS inlet on the pool (river_discharge.rs semantics).
struct Inlet {
    uniform: wgpu::Buffer,
    count: wgpu::Buffer,
    metrics: [f32; 4],
    rect: [i32; 4],
    cfs: f32,
    count_pipeline: wgpu::ComputePipeline,
    inject_pipeline: wgpu::ComputePipeline,
    count_group: Option<wgpu::BindGroup>,
    inject_groups: Option<[wgpu::BindGroup; 2]>,
    counted: bool,
}

struct TileCapture {
    depths: DepthCaptures,
    cameras: [Camera; CHANNELS],
    floor: f32,
    pipeline: wgpu::ComputePipeline,
    view: wgpu::TextureView,
    /// Bound in mode 0, where commit_tile keeps the dry zero WaterInfo.
    no_reference: wgpu::Buffer,
}

/// Tile capture, pool solver and residency of one sparse river.
pub struct SparseSimulation {
    bins: Bins,
    lattice: Lattice,
    residency: Residency,
    tile_cells: u64,
    pool: Pool,
    generation: u64,
    table: wgpu::Buffer,
    tiles_uniform: wgpu::Buffer,
    uniform: wgpu::Buffer,
    boundary_uniform: wgpu::Buffer,
    presentation: wgpu::Buffer,
    boxes: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    groups: [wgpu::BindGroup; 2],
    pipelines: Vec<wgpu::ComputePipeline>,
    /// PreSim history snapshot; None when the layout derives history.
    snapshot: Option<wgpu::ComputePipeline>,
    occupancy: wgpu::ComputePipeline,
    capture: TileCapture,
    inlet: Option<Inlet>,
    inlet_active: bool,
    current: usize,
    history_needs_snapshot: bool,
    started: bool,
    parameters: Parameters,
    /// Substeps per solver frame at spawn (riverdump's CPU replay).
    substeps: u32,
    /// Native source boxes at the front of `boxes`; drains follow them.
    base_boxes: u32,
    drains: Vec<Drain>,
    stage_template: StageUniform,
    boundary_template: BoundaryUniform,
    readback: Readback,
    queue: VecDeque<[i32; 2]>,
    deferred: Option<u64>,
    solver_frames: u64,
    allowed_through: u64,
    budget: u64,
    guard: u32,
    origin_z_cm: f64,
    dx_cm: f32,
    collisions: Option<BodyCollisions>,
    /// Tiles whose latest occupancy sample was non-pristine (the water mesh
    /// quadtree's leaves) and a counter bumped whenever that set changes.
    wet_tiles: BTreeSet<[i32; 2]>,
    wet_generation: u64,
    /// r_riverTileSchedule 1/2 (B); None runs the native dispatch (A).
    schedule: Option<schedule::TileSchedule>,
    schedule_unsupported: bool,
    /// Source-box tile rects (inclusive), forcing for the tile schedule.
    source_tiles: Vec<[i32; 4]>,
    /// PrepareSurface smoothing radius (cells), part of the schedule halo.
    smoothing_radius: u32,
    /// r_riverStorageMode at spawn.
    storage: Storage,
    /// Per-pass GPU timestamps (headless solver benchmark); None normally.
    profile: Option<StageProfile>,
    /// Long-dry tile eviction (derived storage layouts).
    eviction: bool,
    /// Bumped whenever eviction renumbers slots.
    residency_epoch: u64,
    /// Per slot: consecutive samples outside every occupied tile's guard ring.
    dry_samples: Vec<u32>,
    evicted_tiles: u64,
    /// One tile of the widest per-cell buffer, for slot moves (wgpu cannot
    /// copy within one buffer).
    move_scratch: Option<wgpu::Buffer>,
}

/// Per-pass GPU timestamps of the sparse solver, for the headless benchmark
/// (river_sparse_gpu_tests.rs). Needs Features::TIMESTAMP_QUERY. Each timed
/// pass takes two queries; `take_stage_profile` reads and resets them.
pub struct StageProfile {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    labels: Vec<&'static str>,
}

const PROFILE_PASSES: u32 = 2048;

pub struct Spawn<'a> {
    pub bins: Bins,
    pub dx_cm: f32,
    /// River origin Z (cm); heights are relative to it.
    pub origin_z_cm: f64,
    pub parameters: Parameters,
    pub substeps: u32,
    pub transport: Transport,
    pub presentation: Presentation,
    pub boxes: &'a [SourceBox],
    pub controls: Controls,
    /// Lattice cells [x0, y0, x1, y1) that must be resident from the start.
    pub source_cells: [i32; 4],
    pub guard: u32,
    /// Pool layout (r_riverStorageMode); falls back to native when the
    /// spawn needs WaterInfo.
    pub storage: Storage,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl SparseSimulation {
    pub fn new(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, spawn: Spawn<'_>) -> Result<Self, String> {
        let limits = device.limits();
        if limits.max_storage_buffers_per_shader_stage < 11
            || !device.features().contains(wgpu::Features::FLOAT32_FILTERABLE)
        {
            return Err("sparse river needs 11 storage buffers per compute stage and FLOAT32_FILTERABLE".into());
        }
        let p = spawn.parameters;
        let lattice = spawn.bins.lattice();
        let tile = lattice.tile();
        let tile_cells = u64::from(tile) * u64::from(tile);
        let dims = lattice.dimensions();
        if p.dx != spawn.dx_cm || spawn.controls.grid_min != [0.0; 3] || spawn.transport.extrapolation_half_width > 32 {
            return Err("sparse river controls must use the lattice corner as grid minimum".into());
        }
        let stage_template = StageUniform {
            dimensions: [dims[0], dims[1], spawn.transport.extrapolation_half_width, transport_flags(spawn.transport)],
            stepping: [p.dx, p.dt, p.gravity, p.max_velocity],
            limits: [p.friction, p.velocity_dissipation, p.max_depth, p.integration_depth_max],
            fade: [p.fade_start, p.fade_speed, spawn.transport.advection_multiplier, spawn.transport.minimum_advection_depth],
        };
        let c = spawn.controls;
        // Derived WaterInfo is zero: a spawn that reads it (spline matching,
        // native sink boxes) keeps the native buffers.
        let storage = if spawn.storage != Storage::Native && (c.match_spline || spawn.boxes.iter().any(|b| b.sink)) {
            eprintln!("[RIVER] r_riverStorageMode {} needs a dry WaterInfo; this river uses native storage", spawn.storage.name());
            Storage::Native
        } else {
            spawn.storage
        };
        let boundary_template = BoundaryUniform {
            times: [c.frame_dt, c.sink_dissipation, c.height_matching_amount, c.remove_outside_amount],
            origin: [0.0, 0.0, 0.0, c.source_rise_limit_cm],
            flags: [spawn.boxes.len() as u32, u32::from(c.match_spline), 0, 0],
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river stage controls"),
            contents: bytemuck::bytes_of(&stage_template),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let boundary_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river source and sink controls"),
            contents: bytemuck::bytes_of(&boundary_template),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let r = spawn.presentation;
        let presentation = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river presentation controls"),
            contents: bytemuck::cast_slice(&crate::river_gpu::render_control_words(r, false)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let mut volumes: Vec<f32> = spawn.boxes.iter().flat_map(|b| {
            [b.position[0], b.position[1], b.position[2], b.angle, b.size[0], b.size[1], b.size[2], if b.sink { 1.0 } else { 0.0 }]
        }).collect();
        // Room for the developer drains after the native boxes (set_drains).
        volumes.resize((spawn.boxes.len() + MAX_DRAINS) * 8, 0.0);
        let boxes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river source boxes"),
            contents: bytemuck::cast_slice(&volumes),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Initial residency: the source cells' tiles and their guard ring.
        let mut residency = Residency::new(lattice);
        let s = spawn.source_cells;
        let first = lattice.tile_of([s[0].max(0), s[1].max(0)]);
        let last = lattice.tile_of([(s[2] - 1).clamp(0, dims[0] as i32 - 1), (s[3] - 1).clamp(0, dims[1] as i32 - 1)]);
        let touched = (first[1]..=last[1]).flat_map(|y| (first[0]..=last[0]).map(move |x| [x, y]));
        let initial = required_tiles(lattice, touched, spawn.guard);
        for t in &initial {
            residency.insert(*t)?;
        }
        let slots = residency.slot_count() as u32;
        let pool = Pool::new(device, pool_capacity_for(slots), tile_cells, storage)?;
        let table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river tile slot table"),
            contents: bytemuck::cast_slice(residency.table()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let slot_tiles: Vec<i32> = residency.slot_tiles().iter().flatten().copied().collect();
        let staged = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river initial slot tiles"),
            contents: bytemuck::cast_slice(&slot_tiles),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&staged, 0, &pool.slot_tiles, 0, u64::from(slots) * 8);
        let [tw, th] = lattice.tiles();
        let tiles_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river tile layout"),
            // counts.y: the derived history's first-frame reset zero (modes 1/2).
            contents: bytemuck::cast_slice(&[tw, th, tile, lattice.shift(), slots, u32::from(!storage.stores_history()), 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let mut entries = Vec::new();
        for binding in [0u32, 5, 10, 12] {
            entries.push(uniform_entry(binding));
        }
        for (binding, read_only) in [(1, true), (2, true), (3, false), (4, false), (6, false), (7, false), (8, true), (9, true), (13, true), (14, true), (15, false)] {
            // History (4) and WaterInfo (8) exist only in native storage.
            if storage.stores_history() || !matches!(binding, 4 | 8) {
                entries.push(storage_entry(binding, read_only));
            }
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sparse river stage layout"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sparse river pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river stages, sparse addressing"),
            source: wgpu::ShaderSource::Wgsl(storage.stage_shader(false).into()),
        });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipelines = Kernel::ALL.iter().map(|&k| make(storage.stage_entry(k))).collect();
        let snapshot = storage.stores_history().then(|| make("snapshot_bottom"));
        let occupancy = make("tile_occupancy");

        // River-wide capture cameras, so every tile projects the same depths.
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let [zmin, zmax] = spawn.bins.capture_z_range();
        let bottom = (f64::from(zmin) * cm - spawn.origin_z_cm) as f32;
        let top = (f64::from(zmax) * cm - spawn.origin_z_cm) as f32;
        let headroom = CAPTURE_HEADROOM_CM;
        let cameras = std::array::from_fn(|i| Camera {
            height: if Channel::ALL[i].top_down() { top + headroom } else { bottom - headroom },
            near: 0.0,
            far: top - bottom + 2.0 * headroom,
        });
        let depths = DepthCaptures::new(device, tile, tile as f32 * spawn.dx_cm, cameras)?;
        let view = depths.depth_layers().create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let tiles_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sparse river tile pool"),
            source: wgpu::ShaderSource::Wgsl(storage.tiles_shader(TILES_BODY).into()),
        });
        let capture_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("commit_tile"),
            layout: None,
            module: &tiles_shader,
            entry_point: Some("commit_tile"),
            compilation_options: Default::default(),
            cache: None,
        });
        let capture = TileCapture {
            depths,
            cameras,
            floor: (-spawn.dx_cm).min(bottom - headroom),
            pipeline: capture_pipeline,
            view,
            no_reference: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sparse river commit without a bed reference"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
        };
        let groups = Self::bind(device, &layout, &pool, &table, &tiles_uniform, &uniform, &boundary_uniform, &presentation, &boxes);
        let budget = u64::from(solver_frames_within_margin(spawn.guard * tile, spawn.substeps));
        // Bounding square of each (rotated) source box, lattice-local cm.
        let source_tiles = spawn
            .boxes
            .iter()
            .filter(|b| !b.sink)
            .map(|b| {
                let reach = 0.5 * (b.size[0] * b.size[0] + b.size[1] * b.size[1]).sqrt() + spawn.dx_cm;
                let last = dims.map(|v| v as i32 - 1);
                let cell = |v: f32, axis: usize| ((v / spawn.dx_cm).floor().max(0.0) as i32).min(last[axis]);
                let lo = lattice.tile_of([cell(b.position[0] - reach, 0), cell(b.position[1] - reach, 1)]);
                let hi = lattice.tile_of([cell(b.position[0] + reach, 0), cell(b.position[1] + reach, 1)]);
                [lo[0], lo[1], hi[0], hi[1]]
            })
            .collect();
        if budget == 0 {
            return Err("sparse river guard ring is narrower than one solver frame of flow".into());
        }
        let mut simulation = Self {
            bins: spawn.bins,
            lattice,
            residency,
            tile_cells,
            pool,
            generation: 0,
            table,
            tiles_uniform,
            uniform,
            boundary_uniform,
            presentation,
            boxes,
            layout,
            groups,
            pipelines,
            snapshot,
            occupancy,
            capture,
            inlet: None,
            inlet_active: false,
            current: 0,
            history_needs_snapshot: false,
            started: false,
            parameters: p,
            substeps: spawn.substeps,
            base_boxes: spawn.boxes.len() as u32,
            drains: Vec::new(),
            stage_template,
            boundary_template,
            readback: Readback::default(),
            queue: VecDeque::new(),
            deferred: None,
            solver_frames: 0,
            allowed_through: budget,
            budget,
            guard: spawn.guard,
            origin_z_cm: spawn.origin_z_cm,
            dx_cm: spawn.dx_cm,
            collisions: None,
            wet_tiles: BTreeSet::new(),
            wet_generation: 0,
            schedule: None,
            schedule_unsupported: false,
            source_tiles,
            smoothing_radius: spawn.presentation.half_width,
            storage,
            profile: None,
            eviction: !storage.stores_history(),
            residency_epoch: 0,
            dry_samples: Vec::new(),
            evicted_tiles: 0,
            move_scratch: None,
        };
        for (slot, tile) in initial.iter().enumerate() {
            simulation.commit(device, encoder, *tile, slot as u32, true)?;
        }
        Ok(simulation)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        pool: &Pool,
        table: &wgpu::Buffer,
        tiles: &wgpu::Buffer,
        uniform: &wgpu::Buffer,
        boundary: &wgpu::Buffer,
        presentation: &wgpu::Buffer,
        boxes: &wgpu::Buffer,
    ) -> [wgpu::BindGroup; 2] {
        std::array::from_fn(|i| {
            let entries: Vec<_> = [
                (0, uniform),
                (1, &pool.state[i]),
                (2, &pool.geometry),
                (3, &pool.state[1 - i]),
                (5, presentation),
                (6, &pool.normals),
                (7, &pool.surface),
                (9, boxes),
                (10, boundary),
                (12, tiles),
                (13, table),
                (14, &pool.slot_tiles),
                (15, &pool.occupancy),
            ]
            .into_iter()
            .chain(pool.history.as_ref().map(|b| (4, b)))
            .chain(pool.water_info.as_ref().map(|b| (8, b)))
            .map(|(binding, buffer)| wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() })
            .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sparse river ping-pong stage bindings"),
                layout,
                entries: &entries,
            })
        })
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.groups = Self::bind(
            device, &self.layout, &self.pool, &self.table, &self.tiles_uniform,
            &self.uniform, &self.boundary_uniform, &self.presentation, &self.boxes,
        );
        if let Some(inlet) = &mut self.inlet {
            inlet.count_group = None;
            inlet.inject_groups = None;
        }
        self.generation += 1;
    }

    /// Capture one tile's bed and write its pool slot (TileInit or reset).
    fn commit(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, tile: [i32; 2], slot: u32, initial: bool) -> Result<(), String> {
        let (positions, split) = self.bins.tile_mesh(tile, self.origin_z_cm);
        let indices: Vec<u32> = (0..positions.len() as u32).collect();
        // Mode 0 has no actor vertices; the empty group draws nothing.
        let meshes = [
            Mesh { class: MeshClass::Landscape, positions: &positions, indices: &indices[..split], sides: Sides::TwoSided },
            Mesh { class: MeshClass::Actors, positions: &positions, indices: &indices[split..], sides: Sides::TwoSided },
        ];
        self.capture.depths.encode_capture(device, encoder, &meshes)?;
        let reference = self
            .bins
            .tile_references(tile, self.origin_z_cm, self.capture.floor)
            .map(|values| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sparse river tile bed reference"),
                    contents: bytemuck::cast_slice(&values),
                    usage: wgpu::BufferUsages::STORAGE,
                })
            });
        let origin = self.lattice.tile_origin(tile);
        let dims = self.lattice.dimensions();
        let mut words: Vec<u32> = vec![
            origin[0] as u32, origin[1] as u32, dims[0], dims[1],
            slot, self.lattice.tile(), self.capture.depths.resolution(), u32::from(initial),
            self.capture.floor.to_bits(), (self.capture.floor + VOID_MARGIN_CM).to_bits(),
            if reference.is_some() { 1.0f32.to_bits() } else { 0 }, 0,
        ];
        for channel in Channel::ALL {
            let camera = self.capture.cameras[channel as usize];
            words.extend([camera.height.to_bits(), if channel.top_down() { (-1.0f32).to_bits() } else { 1.0f32.to_bits() }, 0, 0]);
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river tile commit"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let pool = &self.pool;
        let resources: [(u32, wgpu::BindingResource<'_>); 8] = [
            (0, uniform.as_entire_binding()),
            (1, wgpu::BindingResource::TextureView(&self.capture.view)),
            (2, pool.geometry.as_entire_binding()),
            (4, pool.state[0].as_entire_binding()),
            (5, pool.state[1].as_entire_binding()),
            (6, pool.surface.as_entire_binding()),
            (7, pool.normals.as_entire_binding()),
            (23, reference.as_ref().unwrap_or(&self.capture.no_reference).as_entire_binding()),
        ];
        let history = pool.history.as_ref().map(|b| (3, b.as_entire_binding()));
        let entries: Vec<_> = resources
            .into_iter()
            .chain(history)
            .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource })
            .collect();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sparse river tile commit"),
            layout: &self.capture.pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river tile commit"), timestamp_writes: None });
        pass.set_pipeline(&self.capture.pipeline);
        pass.set_bind_group(0, &group, &[]);
        let groups = self.lattice.tile().div_ceil(8);
        pass.dispatch_workgroups(groups, groups, 1);
        Ok(())
    }

    /// Make tiles resident: grow the pool if needed, upload table/slot
    /// entries in command order and commit each captured bed.
    fn activate(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, tiles: &[[i32; 2]]) -> Result<(), String> {
        let mut fresh = Vec::new();
        for &tile in tiles {
            let (slot, new) = self.residency.insert(tile)?;
            if new {
                fresh.push((tile, slot));
            }
        }
        if fresh.is_empty() {
            return Ok(());
        }
        let slots = self.residency.slot_count() as u32;
        if slots > self.pool.capacity {
            let used = u64::from(slots) - fresh.len() as u64;
            let capacity = pool_capacity_for(slots).max(self.pool.capacity + self.pool.capacity / 2);
            let pool = Pool::new(device, capacity, self.tile_cells, self.storage)?;
            self.pool.copy_into(encoder, &pool, used, self.tile_cells);
            self.pool = pool;
            self.rebind(device);
        }
        let [tw, _] = self.lattice.tiles();
        let mut words: Vec<i32> = Vec::new();
        let mut copies = Vec::new();
        for &(tile, slot) in &fresh {
            let entry = tile[1] as u64 * u64::from(tw) + tile[0] as u64;
            copies.push((words.len() as u64 * 4, &self.table, entry * 4, 4u64));
            words.push(slot as i32);
            copies.push((words.len() as u64 * 4, &self.pool.slot_tiles, u64::from(slot) * 8, 8u64));
            words.extend(tile);
        }
        copies.push((words.len() as u64 * 4, &self.tiles_uniform, 16, 4));
        words.push(slots as i32);
        let staged = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river residency update"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        for (from, buffer, to, size) in copies {
            encoder.copy_buffer_to_buffer(&staged, from, buffer, to, size);
        }
        let initial = !self.started;
        for (tile, slot) in fresh {
            self.commit(device, encoder, tile, slot, initial)?;
        }
        Ok(())
    }

    /// Read back occupancy, queue guard-ring tiles, activate a bounded batch
    /// and extend the solver allowance once every required tile exists.
    pub fn update_residency(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Result<(), String> {
        while let Some((frame, flags, scheduled, epoch)) = self.readback.poll(device) {
            if epoch != self.residency_epoch {
                // Encoded before an eviction renumbered the slots.
                continue;
            }
            if let (Some(schedule), Some(count)) = (self.schedule.as_mut(), scheduled) {
                schedule.record(count, flags.len() as u32);
            }
            let occupied: Vec<[i32; 2]> = flags
                .iter()
                .enumerate()
                .filter(|(_, f)| **f != 0)
                .map(|(slot, _)| self.residency.slot_tile(slot as u32))
                .collect();
            let wet: BTreeSet<[i32; 2]> = occupied.iter().copied().collect();
            if wet != self.wet_tiles {
                self.wet_tiles = wet;
                self.wet_generation += 1;
            }
            let queued: BTreeSet<[i32; 2]> = self.queue.iter().copied().collect();
            let required: BTreeSet<[i32; 2]> = required_tiles(self.lattice, occupied, self.guard).into_iter().collect();
            for &tile in &required {
                if !self.residency.is_resident(tile) && !queued.contains(&tile) {
                    self.queue.push_back(tile);
                }
            }
            if self.eviction {
                self.dry_samples.resize(self.residency.slot_count(), 0);
                for (count, tile) in self.dry_samples.iter_mut().zip(self.residency.slot_tiles()) {
                    *count = if required.contains(tile) { 0 } else { count.saturating_add(1) };
                }
            }
            self.deferred = Some(self.deferred.map_or(frame, |d| d.max(frame)));
        }
        if self.eviction {
            self.evict(device, encoder)?;
        }
        let batch: Vec<_> = (0..MAX_ACTIVATIONS_PER_FRAME).map_while(|_| self.queue.pop_front()).collect();
        self.activate(device, encoder, &batch)?;
        if self.queue.is_empty() {
            if let Some(frame) = self.deferred.take() {
                self.allowed_through = self.allowed_through.max(frame + self.budget);
            }
        }
        Ok(())
    }

    /// Release tiles that stayed outside every occupied tile's guard ring for
    /// EVICT_AFTER_SAMPLES samples. Not Unreal. Such a tile holds the dry
    /// fixed point, and so does every cell within stencil reach of it, so it
    /// reads exactly like a tile that was never resident: a dry cell beside a
    /// non-resident wall computes what it computes beside a dry bed (the
    /// storage-mode GPU test replays this against a run without eviction).
    /// The residency budget already keeps the wet front off non-resident
    /// cells, and a returning front recommits the tile (TileInit = the same
    /// fixed point). Source, inlet and body tiles stay pinned. The last slot
    /// moves into each freed one; the pool shrinks when mostly empty.
    fn evict(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Result<(), String> {
        self.dry_samples.resize(self.residency.slot_count(), 0);
        let g = self.guard as i32;
        let mut pins: Vec<[i32; 4]> = self.source_tiles.clone();
        let last = self.lattice.dimensions().map(|v| v as i32 - 1);
        let cells_to_tiles = |lo: [i32; 2], hi: [i32; 2]| {
            let a = self.lattice.tile_of([lo[0].clamp(0, last[0]), lo[1].clamp(0, last[1])]);
            let b = self.lattice.tile_of([hi[0].clamp(0, last[0]), hi[1].clamp(0, last[1])]);
            [a[0], a[1], b[0], b[1]]
        };
        if let Some(inlet) = &self.inlet {
            let r = inlet.rect;
            pins.push(cells_to_tiles([r[0], r[1]], [r[2] - 1, r[3] - 1]));
        }
        if let Some(c) = &self.collisions {
            let side = c.side as i32;
            for corner in c.current.iter().chain(&c.stamped) {
                pins.push(cells_to_tiles(*corner, [corner[0] + side - 1, corner[1] + side - 1]));
            }
        }
        let pinned = |t: &[i32; 2]| pins.iter().any(|r| t[0] >= r[0] - g && t[0] <= r[2] + g && t[1] >= r[1] - g && t[1] <= r[3] + g);
        let victims: Vec<[i32; 2]> = self
            .residency
            .slot_tiles()
            .iter()
            .zip(&self.dry_samples)
            .filter(|(t, count)| **count >= EVICT_AFTER_SAMPLES && !pinned(t))
            .map(|(t, _)| *t)
            .take(MAX_EVICTIONS_PER_FRAME)
            .collect();
        if victims.is_empty() {
            return Ok(());
        }
        let scratch = self
            .move_scratch
            .get_or_insert_with(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sparse river tile move"),
                    size: self.tile_cells * 16,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .clone();
        let [tw, _] = self.lattice.tiles();
        let entry = |t: [i32; 2]| (t[1] as u64 * u64::from(tw) + t[0] as u64) * 4;
        let mut words: Vec<i32> = Vec::new();
        let mut copies: Vec<(u64, wgpu::Buffer, u64, u64)> = Vec::new();
        for tile in victims {
            let Some((slot, moved)) = self.residency.swap_remove(tile) else { continue };
            self.dry_samples.swap_remove(slot as usize);
            copies.push((words.len() as u64 * 4, self.table.clone(), entry(tile), 4));
            words.push(-1);
            if let Some((from, moved_tile)) = moved {
                self.pool.move_slot(encoder, &scratch, from, slot, self.tile_cells);
                copies.push((words.len() as u64 * 4, self.table.clone(), entry(moved_tile), 4));
                words.push(slot as i32);
                copies.push((words.len() as u64 * 4, self.pool.slot_tiles.clone(), u64::from(slot) * 8, 8));
                words.extend(moved_tile);
            }
            self.evicted_tiles += 1;
        }
        let slots = self.residency.slot_count() as u32;
        copies.push((words.len() as u64 * 4, self.tiles_uniform.clone(), 16, 4));
        words.push(slots as i32);
        let staged = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river tile eviction"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        for (from, buffer, to, size) in &copies {
            encoder.copy_buffer_to_buffer(&staged, *from, buffer, *to, *size);
        }
        self.residency_epoch += 1;
        // Shrink a mostly empty pool (growth is 1.5x, so no oscillation).
        let capacity = pool_capacity_for(slots);
        if self.pool.capacity >= 64 && capacity * 2 <= self.pool.capacity {
            let pool = Pool::new(device, capacity, self.tile_cells, self.storage)?;
            self.pool.copy_into(encoder, &pool, u64::from(slots), self.tile_cells);
            self.pool = pool;
            self.rebind(device);
        }
        Ok(())
    }

    /// Tiles released by long-dry eviction since spawn.
    pub fn evicted_tiles(&self) -> u64 {
        self.evicted_tiles
    }

    /// Tests compare runs with and without eviction.
    #[cfg(test)]
    pub(crate) fn set_eviction(&mut self, enabled: bool) {
        self.eviction = enabled;
    }

    /// Solver frames that may run now without the wet front outrunning residency.
    pub fn frames_available(&self) -> u64 {
        self.allowed_through.saturating_sub(self.solver_frames)
    }

    fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>) {
        let groups = self.lattice.tile().div_ceil(8);
        pass.dispatch_workgroups(groups, groups, self.residency.slot_count() as u32);
    }

    fn encode_kernel(&mut self, encoder: &mut wgpu::CommandEncoder, kernel: Kernel) {
        if kernel == Kernel::SourceAndSink && self.history_needs_snapshot {
            let slot = self.profile_slot("snapshot_bottom");
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river PreSim snapshot"), timestamp_writes: self.profile_writes(slot) });
            pass.set_bind_group(0, &self.groups[self.current], &[]);
            match &self.schedule {
                Some(schedule) => schedule.dispatch(&mut pass, Kernel::ALL.len()),
                None => {
                    pass.set_pipeline(self.snapshot.as_ref().expect("history snapshot only in native storage"));
                    self.dispatch(&mut pass);
                }
            }
            drop(pass);
            self.history_needs_snapshot = false;
        }
        let slot = self.profile_slot(kernel.entry());
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(kernel.entry()), timestamp_writes: self.profile_writes(slot) });
        self.record_kernel(&mut pass, kernel);
    }

    /// One stage dispatch into an open pass; flips the ping-pong parity.
    fn record_kernel(&mut self, pass: &mut wgpu::ComputePass<'_>, kernel: Kernel) {
        let index = Kernel::ALL.iter().position(|&k| k == kernel).expect("listed kernel");
        pass.set_bind_group(0, &self.groups[self.current], &[]);
        match &self.schedule {
            Some(schedule) => schedule.dispatch(pass, index),
            None => {
                pass.set_pipeline(&self.pipelines[index]);
                self.dispatch(pass);
            }
        }
        self.current = 1 - self.current;
    }

    /// Consecutive stages. Native storage (the reference) keeps one compute
    /// pass per stage. The derived layouts record them as dispatches of one
    /// pass: the same dispatches in the same order, with wgpu's per-dispatch
    /// storage barriers, without a pass begin/end per stage. Profiling keeps
    /// separate passes so each stage gets its own timestamps.
    fn encode_kernels(&mut self, encoder: &mut wgpu::CommandEncoder, kernels: &[Kernel]) {
        if self.storage.stores_history() || self.profile.is_some() {
            for &kernel in kernels {
                self.encode_kernel(encoder, kernel);
            }
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river solver stages"), timestamp_writes: None });
        for &kernel in kernels {
            self.record_kernel(&mut pass, kernel);
        }
    }

    /// Start timing every solver pass and the window gather (benchmark only).
    pub fn enable_stage_profile(&mut self, device: &wgpu::Device) {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("sparse river stage profile"),
            ty: wgpu::QueryType::Timestamp,
            count: PROFILE_PASSES * 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river stage profile resolve"),
            size: u64::from(PROFILE_PASSES) * 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        self.profile = Some(StageProfile { set, resolve, labels: Vec::new() });
    }

    /// Next query pair for a timed pass, if profiling and not full.
    pub fn profile_slot(&mut self, label: &'static str) -> Option<u32> {
        let profile = self.profile.as_mut()?;
        let n = profile.labels.len() as u32;
        if n >= PROFILE_PASSES {
            return None;
        }
        profile.labels.push(label);
        Some(n * 2)
    }

    fn profile_writes(&self, slot: Option<u32>) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let set = &self.profile.as_ref()?.set;
        slot.map(|i| wgpu::ComputePassTimestampWrites {
            query_set: set,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }

    /// Resolve, read (a blocking GPU wait) and reset the pass timings since
    /// the last call: (pass label, GPU ms) in encode order.
    pub fn take_stage_profile(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<(&'static str, f64)> {
        let Some(profile) = self.profile.as_mut() else { return Vec::new() };
        let labels = std::mem::take(&mut profile.labels);
        if labels.is_empty() {
            return Vec::new();
        }
        let bytes = labels.len() as u64 * 16;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river stage profile read"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.resolve_query_set(&profile.set, 0..labels.len() as u32 * 2, &profile.resolve, 0);
        encoder.copy_buffer_to_buffer(&profile.resolve, 0, &read, 0, bytes);
        queue.submit(Some(encoder.finish()));
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        if device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).is_err() {
            return Vec::new();
        }
        let period = f64::from(queue.get_timestamp_period()) * 1e-6;
        let Ok(view) = read.slice(..).get_mapped_range() else { return Vec::new() };
        let ticks: &[u64] = bytemuck::cast_slice(&view);
        labels
            .into_iter()
            .enumerate()
            .map(|(i, label)| (label, ticks[2 * i + 1].wrapping_sub(ticks[2 * i]) as f64 * period))
            .collect()
    }

    /// r_riverTileSchedule for the next solver frames: 0 drops the schedule
    /// (native dispatch, nothing allocated), 1/2 build it on first use. Exact
    /// at any frame boundary: B leaves the pool as A would.
    pub fn set_tile_schedule(&mut self, device: &wgpu::Device, mode: u8) {
        if mode == 0 {
            self.schedule = None;
            return;
        }
        if self.schedule.is_none() {
            if !schedule::TileSchedule::supported(device) {
                if !self.schedule_unsupported {
                    eprintln!("[RIVER] r_riverTileSchedule needs 12 storage buffers per compute stage; using the native dispatch");
                    self.schedule_unsupported = true;
                }
                return;
            }
            self.schedule = Some(schedule::TileSchedule::new(device, &self.layout, self.lattice.tile(), self.storage));
        }
        if let Some(schedule) = self.schedule.as_mut() {
            schedule.set_mode(mode);
        }
    }

    /// Tile rects (inclusive) that force their neighbourhood to run this
    /// solver frame: source boxes, the CFS inlet and every body footprint
    /// stamped or restored. Call before encode_collisions.
    fn forcing_tiles(&self) -> Vec<[i32; 4]> {
        let mut seeds = Vec::new();
        if self.boundary_template.flags[2] & 1 == 0 {
            seeds.extend(self.source_tiles.iter().copied());
        }
        let last = self.lattice.dimensions().map(|v| v as i32 - 1);
        let cells_to_tiles = |lo: [i32; 2], hi: [i32; 2]| {
            let a = self.lattice.tile_of([lo[0].clamp(0, last[0]), lo[1].clamp(0, last[1])]);
            let b = self.lattice.tile_of([hi[0].clamp(0, last[0]), hi[1].clamp(0, last[1])]);
            [a[0], a[1], b[0], b[1]]
        };
        if let (true, Some(inlet)) = (self.inlet_active, &self.inlet) {
            let r = inlet.rect;
            seeds.push(cells_to_tiles([r[0], r[1]], [r[2] - 1, r[3] - 1]));
        }
        if let Some(c) = &self.collisions {
            let side = c.side as i32;
            for corner in c.current.iter().chain(&c.stamped) {
                seeds.push(cells_to_tiles(*corner, [corner[0] + side - 1, corner[1] + side - 1]));
            }
        }
        seeds.truncate(schedule::MAX_SEEDS);
        seeds
    }

    /// encode_reference_frame over the pool: stages 6/7 once, 8..12 per
    /// substep, 13/14 once; the CFS inlet runs in place after stage 6.
    pub fn encode_reference_frame(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, frame: Frame) -> Result<(), &'static str> {
        if frame.parameters().dx != self.parameters.dx {
            return Err("river frame cell size differs from the sparse lattice");
        }
        let stage = self.stage_template.with_parameters(frame.parameters());
        let mut boundary = self.boundary_template;
        boundary.times[0] = frame.engine_delta();
        let mut bytes = [0u8; 112];
        bytes[..64].copy_from_slice(bytemuck::bytes_of(&stage));
        bytes[64..].copy_from_slice(bytemuck::bytes_of(&boundary));
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river queued frame controls"),
            contents: &bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 0, 64);
        encoder.copy_buffer_to_buffer(&upload, 64, &self.boundary_uniform, 0, 48);
        let seeds = if self.schedule.is_some() { self.forcing_tiles() } else { Vec::new() };
        // Stage 5 PreSim: moving-body colliders before the sources (stage 6).
        self.encode_collisions(device, encoder);
        if let Some(schedule) = self.schedule.as_mut() {
            let halo = schedule::halo_tiles(frame.substeps(), self.smoothing_radius, self.stage_template.dimensions[2], self.lattice.tile());
            let slots = self.residency.slot_count() as u32;
            schedule.encode(device, encoder, &self.pool, &self.table, self.generation, self.lattice, slots, halo, &seeds);
        }
        // MatchSpline writes its input unchanged while spline matching is off.
        // The derived layouts leave that copy out (the ping-pong parity just
        // follows the dispatches); native storage keeps the reference pass.
        let identity_match = !self.storage.stores_history() && self.boundary_template.flags[1] == 0;
        let mut kernels = vec![Kernel::SourceAndSink];
        if self.inlet_active {
            // The CFS inlet (copies and its own passes) runs after stage 6.
            self.encode_kernels(encoder, &kernels);
            self.encode_inlet(device, encoder, frame.hydraulic_duration());
            kernels.clear();
        }
        kernels.push(Kernel::DistributeOverlap);
        for _ in 0..frame.substeps() {
            kernels.extend(Kernel::ALL[2..8].iter().filter(|&&k| !(identity_match && k == Kernel::MatchSpline)));
        }
        kernels.extend([Kernel::PrepareSurface, Kernel::ComputeNormalsAndReset]);
        self.encode_kernels(encoder, &kernels);
        if self.storage.stores_history() {
            self.history_needs_snapshot = true;
        } else if !self.started {
            // Derived history: the Niagara reset zero held for this first
            // frame only; afterwards PrevBottomContour is the bed.
            encoder.clear_buffer(&self.tiles_uniform, 20, Some(4));
        }
        self.started = true;
        self.solver_frames += 1;
        Ok(())
    }

    /// Per-slot occupancy of the current state into the async readback ring.
    pub fn encode_occupancy(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        if self.readback.pending.len() >= READBACKS_IN_FLIGHT || !self.started {
            return;
        }
        let slots = self.residency.slot_count() as u32;
        let bytes = u64::from(slots) * 4;
        let extra = self.schedule.is_some();
        encoder.clear_buffer(&self.pool.occupancy, 0, Some(bytes));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river tile occupancy"), timestamp_writes: None });
        pass.set_pipeline(&self.occupancy);
        pass.set_bind_group(0, &self.groups[self.current], &[]);
        self.dispatch(&mut pass);
        drop(pass);
        let buffer = self.readback.buffer(device, (u64::from(slots.max(1)) + u64::from(extra)) * 4);
        encoder.copy_buffer_to_buffer(&self.pool.occupancy, 0, &buffer, 0, bytes);
        if let Some(schedule) = &self.schedule {
            schedule.copy_count(encoder, &buffer, u64::from(slots.max(1)) * 4);
        }
        self.readback.pending.push_back(Pending { buffer, frame: self.solver_frames, epoch: self.residency_epoch, slots, extra, stage: Stage::Copied });
    }

    // ---- CFS inlet (Discharge mode) and height-source controls ----

    /// Lattice cells of a square inlet centred on `centre_cm` (lattice-local).
    pub fn inlet_rect(&self, centre_cm: [f32; 2], width_cm: f32) -> [i32; 4] {
        let dims = self.lattice.dimensions();
        let range = |axis: usize| {
            let lo = ((centre_cm[axis] - 0.5 * width_cm) / self.dx_cm - 0.5).ceil();
            let hi = ((centre_cm[axis] + 0.5 * width_cm) / self.dx_cm - 0.5).floor() + 1.0;
            let n = dims[axis] as f32;
            let lo = lo.clamp(2.0, n - 3.0);
            (lo as i32, hi.clamp(lo + 1.0, n - 2.0) as i32)
        };
        let (x, y) = (range(0), range(1));
        [x.0, y.0, x.1, y.1]
    }

    pub fn enable_inlet(&mut self, device: &wgpu::Device, rect: [i32; 4], level_cm: f32, cfs: f32) {
        let [tw, th] = self.lattice.tiles();
        let metrics = [self.dx_cm * self.dx_cm, level_cm, 0.0, 0.0];
        let mut words: Vec<u32> = rect.iter().map(|v| *v as u32).collect();
        words.extend([tw, th, self.lattice.tile(), self.lattice.shift()]);
        words.extend(metrics.map(f32::to_bits));
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river CFS inlet"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let count = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river open inlet cells"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sparse river CFS inlet"),
            source: wgpu::ShaderSource::Wgsl(self.storage.tiles_shader(TILES_BODY).into()),
        });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        self.inlet = Some(Inlet {
            uniform,
            count,
            metrics,
            rect,
            cfs,
            count_pipeline: make("count_inlet"),
            inject_pipeline: make("inject_inlet"),
            count_group: None,
            inject_groups: None,
            counted: false,
        });
        self.inlet_active = true;
    }

    pub fn set_inlet(&mut self, cfs: Option<f32>) {
        self.inlet_active = cfs.is_some();
        if let (Some(inlet), Some(cfs)) = (&mut self.inlet, cfs) {
            inlet.cfs = cfs;
        }
    }

    pub fn has_inlet(&self) -> bool {
        self.inlet.is_some()
    }

    fn encode_inlet(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, seconds: f64) {
        let Some(inlet) = &mut self.inlet else { return };
        let pool = &self.pool;
        let table = &self.table;
        if inlet.count_group.is_none() {
            let group = |pipeline: &wgpu::ComputePipeline, state: Option<&wgpu::Buffer>| {
                let mut entries = vec![
                    wgpu::BindGroupEntry { binding: 16, resource: inlet.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 17, resource: table.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 18, resource: pool.geometry.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 19, resource: inlet.count.as_entire_binding() },
                ];
                if let Some(state) = state {
                    entries.push(wgpu::BindGroupEntry { binding: 20, resource: state.as_entire_binding() });
                }
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("sparse river CFS inlet"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                })
            };
            let count_group = group(&inlet.count_pipeline, None);
            let inject = [group(&inlet.inject_pipeline, Some(&pool.state[0])), group(&inlet.inject_pipeline, Some(&pool.state[1]))];
            inlet.count_group = Some(count_group);
            inlet.inject_groups = Some(inject);
            inlet.counted = false;
        }
        let size = [(inlet.rect[2] - inlet.rect[0]).max(1) as u32, (inlet.rect[3] - inlet.rect[1]).max(1) as u32];
        if !inlet.counted {
            // Static bed: count the open cells once per binding generation.
            encoder.clear_buffer(&inlet.count, 0, None);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river inlet cells"), timestamp_writes: None });
            pass.set_pipeline(&inlet.count_pipeline);
            pass.set_bind_group(0, inlet.count_group.as_ref(), &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
            drop(pass);
            inlet.counted = true;
        }
        let mut metrics = inlet.metrics;
        metrics[2] = (f64::from(inlet.cfs) * crate::river_discharge::CM3_PER_CUBIC_FOOT * seconds) as f32;
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river queued CFS volume"),
            contents: bytemuck::cast_slice(&metrics),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &inlet.uniform, 32, 16);
        let groups = inlet.inject_groups.as_ref().expect("bound above");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river CFS inject"), timestamp_writes: None });
        pass.set_pipeline(&inlet.inject_pipeline);
        pass.set_bind_group(0, &groups[self.current], &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
    }

    // ---- PreSim moving-body collisions (stage 5) ----

    /// Bodies for the solver frames of this render frame, lattice-local cm.
    /// At most MAX_BODIES are used. An empty list restores the previous
    /// footprints on the next solver frame and then costs nothing.
    /// `velocity_scale` multiplies the element velocity (1 = native).
    pub fn set_bodies(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bodies: &[CollisionBody], velocity_scale: f32) {
        if self.collisions.is_none() {
            if bodies.is_empty() {
                return;
            }
            self.collisions = Some(BodyCollisions::new(device, self.dx_cm));
        }
        let lattice = self.lattice;
        let dx = self.dx_cm;
        let c = self.collisions.as_mut().expect("created above");
        let bodies = &bodies[..bodies.len().min(MAX_BODIES)];
        let half = (c.side / 2) as i32;
        c.current = bodies
            .iter()
            .map(|b| std::array::from_fn(|a| (b.centre[a] / dx).floor() as i32 - half))
            .collect();
        let scale = if velocity_scale.is_finite() { velocity_scale.max(0.0) } else { 1.0 };
        let [tw, th] = lattice.tiles();
        let dims = lattice.dimensions();
        let build = |stamp: &[[i32; 2]], restore: &[[i32; 2]], side: u32| -> Vec<u32> {
            let mut w = Vec::with_capacity(16 + 4 * MAX_BODIES + 12 * MAX_BODIES);
            w.extend([stamp.len() as u32, restore.len() as u32, side, 0]);
            w.extend([tw, th, lattice.tile(), lattice.shift()]);
            w.extend([dims[0], dims[1], 0, 0]);
            w.extend([dx.to_bits(), scale.to_bits(), 0, 0]);
            for i in 0..MAX_BODIES {
                let s = stamp.get(i).copied().unwrap_or([0, 0]);
                let r = restore.get(i).copied().unwrap_or([0, 0]);
                w.extend([s[0] as u32, s[1] as u32, r[0] as u32, r[1] as u32]);
            }
            for i in 0..MAX_BODIES {
                let (centre, extent, velocity) = bodies.get(i).map_or(([0.0; 3], [0.0; 3], [0.0; 3]), |b| (b.centre, b.extent, b.velocity));
                for v in [centre, extent, velocity] {
                    w.extend([v[0].to_bits(), v[1].to_bits(), v[2].to_bits(), 0]);
                }
            }
            w
        };
        // First solver frame: restore what the last encoded stamp left in the
        // bed. Later frames of the same render frame restore their own stamp.
        let first = build(&c.current, &c.stamped, c.side);
        let rest = build(&c.current, &c.current, c.side);
        queue.write_buffer(&c.uniforms[0], 0, bytemuck::cast_slice(&first));
        queue.write_buffer(&c.uniforms[1], 0, bytemuck::cast_slice(&rest));
        c.first_done = false;
    }

    fn encode_collisions(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        let generation = self.generation;
        let parity = self.current;
        let pool = &self.pool;
        let table = &self.table;
        let Some(c) = self.collisions.as_mut() else { return };
        let variant = usize::from(c.first_done);
        let restore = if c.first_done { c.current.len() } else { c.stamped.len() } as u32;
        let stamp = c.current.len() as u32;
        if restore == 0 && stamp == 0 {
            c.first_done = true;
            return;
        }
        if c.groups.as_ref().is_none_or(|(g, _)| *g != generation) {
            let groups = std::array::from_fn(|v| {
                std::array::from_fn(|state| {
                    let entries = [
                        (0, &c.uniforms[v]),
                        (1, table),
                        (2, &pool.geometry),
                        (3, &c.saved),
                        (4, &pool.state[state]),
                    ]
                    .map(|(binding, buffer)| wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() });
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("sparse river body collisions"),
                        layout: &c.layout,
                        entries: &entries,
                    })
                })
            });
            c.groups = Some((generation, groups));
        }
        let group = &c.groups.as_ref().expect("bound above").1[variant][parity];
        let workgroups = c.side.div_ceil(8);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river PreSim body collisions"), timestamp_writes: None });
        pass.set_bind_group(0, group, &[]);
        if restore > 0 {
            pass.set_pipeline(&c.restore);
            pass.dispatch_workgroups(workgroups, workgroups, restore);
        }
        if stamp > 0 {
            pass.set_pipeline(&c.save);
            pass.dispatch_workgroups(workgroups, workgroups, stamp);
            pass.set_pipeline(&c.stamp);
            pass.dispatch_workgroups(workgroups, workgroups, stamp);
        }
        drop(pass);
        c.stamped = c.current.clone();
        c.first_done = true;
    }

    /// Height-mode toggle and Height+CFS rise cap, copied with the next frame.
    pub fn set_height_source(&mut self, enabled: bool, rise_limit_cm: f32) {
        // Flag z bit 0 disables the native source boxes; the box count also
        // covers the drains that follow them.
        self.boundary_template.flags[2] = u32::from(!enabled);
        self.boundary_template.origin[3] = rise_limit_cm;
    }

    /// Replace the developer drains (solver-local cm; at most MAX_DRAINS).
    /// Takes effect with the next solver frame; water and bed are kept.
    pub fn set_drains(&mut self, queue: &wgpu::Queue, drains: &[Drain]) {
        let drains = &drains[..drains.len().min(MAX_DRAINS)];
        if !drains.is_empty() {
            let words: Vec<f32> = drains.iter().flat_map(drain_box_words).collect();
            queue.write_buffer(&self.boxes, u64::from(self.base_boxes) * 32, bytemuck::cast_slice(&words));
        }
        self.boundary_template.flags[0] = self.base_boxes + drains.len() as u32;
        self.drains = drains.to_vec();
    }

    pub fn lattice(&self) -> Lattice {
        self.lattice
    }
    /// Live render-height controls (river_gpu::render_control_words).
    pub fn set_render_controls(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, words: &[f32; 8]) {
        use wgpu::util::DeviceExt;
        if words[0].is_finite() && words[0] >= 0.0 {
            self.smoothing_radius = words[0].ceil() as u32;
        }
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river render controls upload"),
            contents: bytemuck::cast_slice(words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.presentation, 0, 32);
    }

    /// Non-pristine tiles from the latest occupancy sample.
    pub fn wet_tiles(&self) -> &BTreeSet<[i32; 2]> {
        &self.wet_tiles
    }
    pub fn wet_generation(&self) -> u64 {
        self.wet_generation
    }
    /// Bumped when the pool's buffers are replaced (bind groups go stale).
    pub fn pool_generation(&self) -> u64 {
        self.generation
    }
    /// (tile slot table, PrepareSurface output, ComputeNormals output).
    /// Pool layout this river spawned with.
    pub fn storage(&self) -> Storage {
        self.storage
    }
    /// Allocated pool slots (power-of-two growth; >= resident tiles).
    pub fn pool_capacity(&self) -> u32 {
        self.pool.capacity
    }
    /// Tile table, PrepareSurface output and normals (packed per `storage`).
    pub fn presentation_buffers(&self) -> (&wgpu::Buffer, &wgpu::Buffer, &wgpu::Buffer) {
        (&self.table, &self.pool.surface, &self.pool.normals)
    }
    pub fn dx_cm(&self) -> f32 {
        self.dx_cm
    }
    pub fn bins(&self) -> &Bins {
        &self.bins
    }
    pub fn resident_tiles(&self) -> usize {
        self.residency.slot_count()
    }
    pub fn queued_tiles(&self) -> usize {
        self.queue.len()
    }
    pub fn solver_frames(&self) -> u64 {
        self.solver_frames
    }
    pub fn logical_bytes(&self) -> u64 {
        self.pool.bytes(self.tile_cells) + self.table.size()
    }
    pub fn started(&self) -> bool {
        self.started
    }
    /// r_riverDebug line for the tile schedule; None in native mode (A).
    pub fn tile_schedule_report(&self) -> Option<String> {
        self.schedule.as_ref().map(schedule::TileSchedule::report)
    }

    #[cfg(test)]
    pub(crate) fn tile_schedule_skipped(&self) -> Option<f64> {
        self.schedule.as_ref().and_then(schedule::TileSchedule::skipped_fraction)
    }

    /// `riverdump` console diagnostics: one synchronous readback (a deliberate
    /// GPU stall) of every resident tile's bed geometry and latest state.
    pub fn dump_pool(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<PoolDump, String> {
        let slots = self.residency.slot_count() as u64;
        let cells = slots * self.tile_cells;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("riverdump readback") });
        let staging: Vec<wgpu::Buffer> = [&self.pool.geometry, &self.pool.state[self.current]]
            .into_iter()
            .map(|buffer| {
                let out = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("riverdump readback"),
                    size: (cells * 16).max(16),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_buffer_to_buffer(buffer, 0, &out, 0, cells * 16);
                out
            })
            .collect();
        queue.submit(Some(encoder.finish()));
        for buffer in &staging {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .map_err(|e| format!("riverdump readback failed: {e}"))?;
        let mut floats = Vec::with_capacity(2);
        for buffer in &staging {
            let view = buffer.slice(..).get_mapped_range().map_err(|e| format!("riverdump map failed: {e}"))?;
            floats.push(bytemuck::cast_slice::<u8, [f32; 4]>(&view[..(cells * 16) as usize]).to_vec());
        }
        let tile_cells = self.tile_cells as usize;
        let tiles = (0..slots as usize)
            .map(|slot| {
                let range = slot * tile_cells..(slot + 1) * tile_cells;
                let cells = floats[0][range.clone()].iter().copied().zip(floats[1][range].iter().copied()).collect();
                (self.residency.slot_tile(slot as u32), cells)
            })
            .collect();
        Ok(PoolDump {
            lattice_cells: self.lattice.dimensions(),
            tile: self.lattice.tile(),
            dx_cm: self.dx_cm,
            origin_z_cm: self.origin_z_cm,
            floor_cm: self.capture.floor,
            void_threshold_cm: self.capture.floor + VOID_MARGIN_CM,
            wall_cm: self.capture.cameras[0].height,
            parameters: self.parameters,
            substeps: self.substeps,
            transport: {
                let d = self.stage_template.dimensions;
                let f = self.stage_template.fade;
                Transport {
                    extrapolation_half_width: d[2],
                    extrapolate_into_dry: d[3] & 1 != 0,
                    advection_multiplier: f[2],
                    minimum_advection_depth: f[3],
                    advect_depth: d[3] & 2 == 0,
                }
            },
            controls: {
                let b = self.boundary_template;
                Controls {
                    frame_dt: b.times[0],
                    sink_dissipation: b.times[1],
                    match_spline: b.flags[1] != 0,
                    height_matching_amount: b.times[2],
                    remove_outside_amount: b.times[3],
                    grid_min: [0.0; 3],
                    source_rise_limit_cm: b.origin[3],
                }
            },
            height_source: self.boundary_template.flags[2] & 1 == 0,
            drains: self.drains.clone(),
            tiles,
        })
    }

    /// Test readback of every resident tile, per cell: state[current],
    /// PrepareSurface output, normals and [history, 0, 0, 0]; keyed by tile.
    /// Packed normals are decoded; a derived history is rebuilt from the bed
    /// (the reset zero before the first solver frame).
    #[cfg(test)]
    pub(crate) fn read_pool(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> std::collections::BTreeMap<[i32; 2], Vec<[f32; 4]>> {
        let slots = self.residency.slot_count() as u64;
        let cells = slots * self.tile_cells;
        let normal_stride = self.storage.normal_stride();
        let sources: [(&wgpu::Buffer, u64); 4] = [
            (&self.pool.state[self.current], 16),
            (&self.pool.surface, 16),
            (&self.pool.normals, normal_stride),
            match &self.pool.history {
                Some(history) => (history, 4),
                None => (&self.pool.geometry, 16),
            },
        ];
        let mut encoder = device.create_command_encoder(&Default::default());
        let staging: Vec<wgpu::Buffer> = sources
            .iter()
            .map(|(buffer, stride)| {
                let out = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("sparse river test readback"),
                    size: (cells * stride).max(4),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_buffer_to_buffer(buffer, 0, &out, 0, cells * stride);
                out
            })
            .collect();
        queue.submit(Some(encoder.finish()));
        for buffer in &staging {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("pool readback");
        let floats: Vec<Vec<f32>> = staging
            .iter()
            .map(|b| bytemuck::cast_slice::<u8, f32>(&b.slice(..).get_mapped_range().expect("mapped")).to_vec())
            .collect();
        let tile_cells = self.tile_cells as usize;
        let mut out = std::collections::BTreeMap::new();
        for slot in 0..slots as usize {
            let values = (0..tile_cells)
                .flat_map(|lane| {
                    let i = slot * tile_cells + lane;
                    let v4 = |k: usize| [floats[k][i * 4], floats[k][i * 4 + 1], floats[k][i * 4 + 2], floats[k][i * 4 + 3]];
                    let normal = if self.storage.packed_normals() {
                        let [x, y, z] = unpack_normal(floats[2][i].to_bits());
                        [x, y, z, 0.0]
                    } else {
                        v4(2)
                    };
                    let history = match (&self.pool.history, self.started) {
                        (Some(_), _) => floats[3][i],
                        (None, true) => floats[3][i * 4],
                        (None, false) => 0.0,
                    };
                    [v4(0), v4(1), normal, [history, 0.0, 0.0, 0.0]]
                })
                .collect();
            out.insert(self.residency.slot_tile(slot as u32), values);
        }
        out
    }
}

/// `SparseSimulation::dump_pool`: resident tiles only. Heights are solver cm
/// above `origin_z_cm`; cell (0, 0) is the lattice corner.
pub struct PoolDump {
    pub lattice_cells: [u32; 2],
    pub tile: u32,
    pub dx_cm: f32,
    pub origin_z_cm: f64,
    /// Bed value of a column the capture found empty (before the void wall).
    pub floor_cm: f32,
    /// Beds at or below this are void (should all have become walls).
    pub void_threshold_cm: f32,
    /// Bed height given to void walls (the capture top).
    pub wall_cm: f32,
    /// Solver stepping, substeps and boundary controls, for the CPU replay.
    pub parameters: Parameters,
    pub substeps: u32,
    pub transport: Transport,
    pub controls: Controls,
    /// The held-level source box is active (WaterLevel mode).
    pub height_source: bool,
    /// Developer drains (solver-local cm).
    pub drains: Vec<Drain>,
    /// Per resident tile, tile*tile cells row-major: (geometry, state).
    /// Geometry: bottom, collider, collision velocity xy. State: height,
    /// depth, velocity xy.
    pub tiles: BTreeMap<[i32; 2], Vec<([f32; 4], [f32; 4])>>,
}

/// `riverdump`: replay one solver frame of a pool dump on the CPU reference
/// (jka_water::river::frame_reference, the GPU kernels' test oracle) and
/// return each stage's net change of water volume (cm^3, summed over the
/// substeps) in stage order. A conservative stage nets ~0; the host CFS
/// inlet is not replayed.
pub fn replay_budget(dump: &PoolDump, source: Option<SourceBox>, transport: Transport) -> Result<Vec<(&'static str, f64)>, String> {
    use jka_water::river::boundary::WaterInfo;
    use jka_water::river::frame_reference::{frame_observed, Inputs, State};
    use jka_water::river::transport::Storage;
    use jka_water::river::{Geometry, Water};
    let mut residency = Residency::new(Lattice::covering(dump.tile, dump.lattice_cells)?);
    let mut geometry = Vec::new();
    let mut water = Vec::new();
    for (tile, cells) in &dump.tiles {
        residency.insert(*tile)?;
        for &(g, w) in cells {
            geometry.push(Geometry { bottom: g[0], collider: g[1], collision_velocity: [g[2], g[3]] });
            water.push(Water { height: w[0], depth: w[1], velocity: [w[2], w[3]] });
        }
    }
    let mut state = State::reset(geometry.len());
    state.water = water;
    state.previous_bottom = geometry.iter().map(|g| g.bottom).collect();
    let info = vec![WaterInfo::default(); geometry.len()];
    let boxes: Vec<SourceBox> = source.into_iter().filter(|_| dump.height_source).collect();
    let inputs = Inputs {
        storage: Storage::Tiled(&residency),
        geometry: &geometry,
        water_info: &info,
        boxes: &boxes,
        drains: &dump.drains,
        controls: dump.controls,
        transport,
        presentation: Presentation::default(),
        inlet: None,
    };
    let cell_area = f64::from(dump.dx_cm) * f64::from(dump.dx_cm);
    let volume = |w: &[Water]| {
        w.iter().zip(&geometry).filter(|(_, g)| g.collider < 1.0 - 1e-5).map(|(w, _)| f64::from(w.depth.max(0.0))).sum::<f64>() * cell_area
    };
    let mut last = volume(&state.water);
    let mut budget: Vec<(&'static str, f64)> = Vec::new();
    frame_observed(&inputs, &mut state, dump.parameters, dump.substeps, dump.controls.frame_dt, &mut |stage, w| {
        let v = volume(w);
        match budget.iter_mut().find(|(s, _)| *s == stage) {
            Some(entry) => entry.1 += v - last,
            None => budget.push((stage, v - last)),
        }
        last = v;
    })?;
    Ok(budget)
}

/// Live-river capture cameras sit this far (solver cm) beyond the captured
/// geometry, and the contour floor is at least this far below it. Shared by
/// the dense and sparse spawns and the void test below.
pub const CAPTURE_HEADROOM_CM: f32 = 100.0;

/// An empty capture column reconstructs as `(top + headroom) - far`, which in
/// f32 can land just above the separately rounded contour floor, so an exact
/// `<= floor` test misses it (about a third of real map height ranges). The
/// column then becomes a pit at the map's lowest bed and drains the river
/// into the void. Captured geometry is at least the headroom above the floor,
/// so half of it separates the two cases.
const VOID_MARGIN_CM: f32 = 0.5 * CAPTURE_HEADROOM_CM;

/// Dense live rivers: turn columns where the capture found nothing (bed at the
/// contour floor) into walls as tall as the capture, as commit_tile does.
pub fn encode_void_walls(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    geometry: &wgpu::Buffer,
    cells: u32,
    floor: f32,
    wall_height: f32,
) {
    if cells == 0 {
        return;
    }
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("river void walls"),
        source: wgpu::ShaderSource::Wgsl(Storage::Native.tiles_shader(TILES_BODY).into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("wall_void"),
        layout: None,
        module: &shader,
        entry_point: Some("wall_void"),
        compilation_options: Default::default(),
        cache: None,
    });
    let words = [cells, 0, 0, 0, (floor + VOID_MARGIN_CM).to_bits(), wall_height.to_bits(), 0, 0];
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("river void walls"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("river void walls"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 21, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 22, resource: geometry.as_entire_binding() },
        ],
    });
    let groups = cells.div_ceil(64);
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("river void walls"), timestamp_writes: None });
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
}

/// Camera-centred dense copy of the pool for the dense presentation chain.
pub struct Window {
    cells: u32,
    origin_cell: Option<[i32; 2]>,
    height_offset_cm: f32,
    states: [wgpu::Texture; 2],
    surface: wgpu::Buffer,
    normals: wgpu::Buffer,
    uniform: wgpu::Buffer,
    pipeline: wgpu::ComputePipeline,
    groups: Option<(u64, [wgpu::BindGroup; 2])>,
}

impl Window {
    /// `storage`: the simulation's pool layout (the gather decodes it).
    pub fn new(device: &wgpu::Device, cells: u32, storage: Storage) -> Self {
        let state = crate::river_gpu::state_texture(device, [cells, cells], "sparse river window state");
        let count = u64::from(cells) * u64::from(cells);
        let output = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: count * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river window placement"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sparse river window gather"),
            source: wgpu::ShaderSource::Wgsl(storage.tiles_shader(TILES_BODY).into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gather_window"),
            layout: None,
            module: &shader,
            entry_point: Some("gather_window"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            cells,
            origin_cell: None,
            height_offset_cm: 0.0,
            states: [state.clone(), state],
            surface: output("sparse river window surface"),
            normals: output("sparse river window normals"),
            uniform,
            pipeline,
            groups: None,
        }
    }

    pub fn cells(&self) -> u32 {
        self.cells
    }
    pub fn origin_cell(&self) -> Option<[i32; 2]> {
        self.origin_cell
    }
    pub fn height_offset_cm(&self) -> f32 {
        self.height_offset_cm
    }

    /// Recentre on `eye_cell` (lattice cells) when it leaves the central half.
    /// Returns true when the window moved; presentation state tied to the old
    /// placement (foam history, body origin, ambient) must then be rebuilt.
    pub fn follow(&mut self, sim: &SparseSimulation, eye_cell: [i32; 2]) -> bool {
        let tile = sim.lattice().tile() as i32;
        let half = self.cells as i32 / 2;
        let desired = eye_cell.map(|c| (c - half).div_euclid(tile) * tile);
        let moved = match self.origin_cell {
            None => true,
            Some(o) => (0..2).any(|a| (desired[a] - o[a]).abs() >= half / 2),
        };
        if moved {
            self.origin_cell = Some(desired);
            let first = sim.lattice().tile_of(desired.map(|v| v.max(0)));
            let last_cell = [desired[0] + self.cells as i32 - 1, desired[1] + self.cells as i32 - 1];
            let dims = sim.lattice().dimensions();
            let last = sim.lattice().tile_of([last_cell[0].clamp(0, dims[0] as i32 - 1), last_cell[1].clamp(0, dims[1] as i32 - 1)]);
            let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
            self.height_offset_cm = sim
                .bins()
                .min_z(first, last)
                .map_or(0.0, |z| ((f64::from(z) * cm - sim.origin_z_cm).floor().max(0.0)) as f32);
        }
        moved
    }

    /// Gather the pool into the window (call after the frame's solver work).
    /// `profile`: a query pair from `SparseSimulation::profile_slot`.
    pub fn encode_gather(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, sim: &SparseSimulation, profile: Option<u32>) {
        let Some(origin) = self.origin_cell else { return };
        let lattice = sim.lattice();
        let [tw, th] = lattice.tiles();
        let dims = lattice.dimensions();
        let words: [u32; 16] = [
            origin[0] as u32, origin[1] as u32, self.cells, self.cells,
            tw, th, lattice.tile(), lattice.shift(),
            dims[0], dims[1], 0, 0,
            self.height_offset_cm.to_bits(), 0, 0, 0,
        ];
        let staged = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river window placement upload"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&staged, 0, &self.uniform, 0, 64);
        let stale = match &self.groups {
            Some((generation, _)) => *generation != sim.generation,
            None => true,
        };
        if stale {
            let view = self.states[0].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let groups = std::array::from_fn(|i| {
                let resources: [(u32, wgpu::BindingResource<'_>); 8] = [
                    (8, self.uniform.as_entire_binding()),
                    (9, sim.table.as_entire_binding()),
                    (10, sim.pool.state[i].as_entire_binding()),
                    (11, sim.pool.surface.as_entire_binding()),
                    (12, sim.pool.normals.as_entire_binding()),
                    (13, wgpu::BindingResource::TextureView(&view)),
                    (14, self.surface.as_entire_binding()),
                    (15, self.normals.as_entire_binding()),
                ];
                let entries: Vec<_> = resources.into_iter().map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource }).collect();
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("sparse river window gather"),
                    layout: &self.pipeline.get_bind_group_layout(0),
                    entries: &entries,
                })
            });
            self.groups = Some((sim.generation, groups));
        }
        let (_, groups) = self.groups.as_ref().expect("bound above");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river window gather"), timestamp_writes: sim.profile_writes(profile) });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &groups[sim.current], &[]);
        pass.dispatch_workgroups(self.cells.div_ceil(8), self.cells.div_ceil(8), 1);
    }

    /// Dense presentation inputs (material SurfaceFields).
    pub fn fields(&self) -> crate::river_gpu::SurfaceFields {
        crate::river_gpu::SurfaceFields {
            dimensions: [self.cells; 2],
            surface: self.surface.clone(),
            normals: self.normals.clone(),
        }
    }
    /// Foam's hydraulic inputs: the one window state, at either parity.
    pub fn state_textures(&self) -> &[wgpu::Texture; 2] {
        &self.states
    }
    /// Window grid for the presentation chain.
    pub fn grid(&self, dx_cm: f32) -> Grid {
        Grid {
            dimensions: [self.cells; 2],
            world_size: [self.cells as f32 * dx_cm; 2],
            cell_size: [dx_cm; 2],
        }
    }
    pub fn logical_bytes(&self) -> u64 {
        u64::from(self.cells) * u64::from(self.cells) * 48
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(source: &str) {
        let module = wgpu::naga::front::wgsl::parse_str(source).expect("river WGSL parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("river WGSL validates: {e:?}"));
    }

    #[test]
    fn dense_sparse_and_tile_pool_shaders_validate() {
        validate(jka_water::river::SHALLOW_SHADER);
        validate(jka_water::river::SPARSE_SHALLOW_SHADER);
        validate(&Storage::Native.tiles_shader(TILES_BODY));
        validate(COLLISIONS_SHADER);
    }

    #[test]
    fn stage_uniform_matches_dense_layout() {
        assert_eq!(std::mem::size_of::<StageUniform>(), 64);
        assert_eq!(std::mem::size_of::<BoundaryUniform>(), 48);
    }
}
