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
use crate::river_capture::{clip_collision, DepthCaptures, Mesh, MeshClass, Sides};
use crate::river_gpu::Kernel;
use bytemuck::{Pod, Zeroable};
use jka_assets::bsp::river::CollisionSurface;
use jka_water::river::boundary::{Controls, SourceBox};
use jka_water::river::capture::{Camera, Channel, CHANNELS};
use jka_water::river::control::{Frame, Grid};
use jka_water::river::sparse::{required_tiles, solver_frames_within_margin, Lattice, Residency};
use jka_water::river::{Parameters, Presentation, Transport, CENTIMETRES_PER_MAP_UNIT, SPARSE_SHALLOW_SHADER};
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver};
use wgpu::util::DeviceExt;

pub const SHADER: &str = include_str!("river_tiles.wgsl");

// Session cvars (r_riverSparse, r_riverTileSize, r_riverTileGuard,
// r_riverWindow). Read when a river spawns; existing rivers keep theirs.
static SPARSE: AtomicU8 = AtomicU8::new(1);
static TILE: AtomicU32 = AtomicU32::new(32);
static GUARD: AtomicU32 = AtomicU32::new(2);
static WINDOW: AtomicU32 = AtomicU32::new(512);

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
    triangles: Vec<[[f32; 3]; 3]>,
    bins: Vec<Vec<u32>>,
    lattice: Lattice,
    /// Map position of cell (0, 0)'s lower corner.
    min_map: [f64; 2],
    cell_map: f64,
    z_range: [f32; 2],
}

impl Bins {
    /// The lattice covers every candidate riverbed triangle plus a two-cell
    /// border, with cell corners aligned on `anchor` (the source centre).
    pub fn new(
        collision: &[CollisionSurface],
        water_level: f32,
        cell_map: f64,
        tile: u32,
        anchor: [f64; 2],
    ) -> Result<Self, String> {
        if !(cell_map.is_finite() && cell_map > 0.0) || anchor.iter().any(|v| !v.is_finite()) {
            return Err("invalid sparse river cell size or anchor".into());
        }
        let mut triangles = Vec::new();
        for surface in collision {
            let Some((polygon, count)) = clip_collision(&surface.triangle, surface.solid_bottom_z, Some(water_level)) else {
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
        let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
        for t in &triangles {
            for p in t {
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(f64::from(p[axis]));
                    hi[axis] = hi[axis].max(f64::from(p[axis]));
                }
                z_range = [z_range[0].min(p[2]), z_range[1].max(p[2])];
            }
        }
        let min_map: [f64; 2] = std::array::from_fn(|a| {
            let below = ((anchor[a] - lo[a]) / cell_map).ceil().max(0.0) + 2.0;
            anchor[a] - below * cell_map
        });
        let cells: [u32; 2] = std::array::from_fn(|a| {
            let n = ((hi[a] - min_map[a]) / cell_map).ceil() + 2.0;
            n.clamp(1.0, f64::from(u32::MAX / 2)) as u32
        });
        let lattice = Lattice::covering(tile, cells).map_err(|e| format!("{e} (map too large for this cell size)"))?;
        let [tw, th] = lattice.tiles();
        let mut bins = vec![Vec::new(); tw as usize * th as usize];
        let tile_map = cell_map * f64::from(tile);
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
        Ok(Self { triangles, bins, lattice, min_map, cell_map, z_range })
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
    /// in XY and to the river origin in Z.
    fn tile_mesh(&self, tile: [i32; 2], origin_z_cm: f64) -> Vec<[f32; 3]> {
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let side = self.cell_map * f64::from(self.lattice.tile());
        let centre: [f64; 2] = std::array::from_fn(|a| self.min_map[a] + (f64::from(tile[a]) + 0.5) * side);
        let mut positions = Vec::new();
        for &i in self.bin(tile) {
            for p in &self.triangles[i as usize] {
                positions.push([
                    ((f64::from(p[0]) - centre[0]) * cm) as f32,
                    ((f64::from(p[1]) - centre[1]) * cm) as f32,
                    (f64::from(p[2]) * cm - origin_z_cm) as f32,
                ]);
            }
        }
        positions
    }
}

/// Per-cell pool buffers, `capacity` tile slots each.
struct Pool {
    capacity: u32,
    state: [wgpu::Buffer; 2],
    geometry: wgpu::Buffer,
    history: wgpu::Buffer,
    water_info: wgpu::Buffer,
    surface: wgpu::Buffer,
    normals: wgpu::Buffer,
    occupancy: wgpu::Buffer,
    slot_tiles: wgpu::Buffer,
}

impl Pool {
    fn new(device: &wgpu::Device, capacity: u32, tile_cells: u64) -> Result<Self, String> {
        let cells = u64::from(capacity) * tile_cells;
        let limits = device.limits();
        if cells * 16 > u64::from(limits.max_storage_buffer_binding_size) || cells * 16 > limits.max_buffer_size {
            return Err(format!("sparse river pool of {capacity} tiles exceeds the GPU storage limit"));
        }
        let usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let buffer = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(16), usage, mapped_at_creation: false })
        };
        Ok(Self {
            capacity,
            state: [buffer("sparse river state A", cells * 16), buffer("sparse river state B", cells * 16)],
            geometry: buffer("sparse river geometry", cells * 16),
            history: buffer("sparse river previous bottom", cells * 4),
            // Simulation-first rivers have a dry WaterInfo: zero-initialised.
            water_info: buffer("sparse river WaterInfo", cells * 16),
            surface: buffer("sparse river rendered surface", cells * 16),
            normals: buffer("sparse river normals", cells * 16),
            occupancy: buffer("sparse river tile occupancy", u64::from(capacity) * 4),
            slot_tiles: buffer("sparse river slot tiles", u64::from(capacity) * 8),
        })
    }

    fn copy_into(&self, encoder: &mut wgpu::CommandEncoder, to: &Pool, slots: u64, tile_cells: u64) {
        if slots == 0 {
            return;
        }
        let cells = slots * tile_cells;
        let per_cell: [(&wgpu::Buffer, &wgpu::Buffer, u64); 7] = [
            (&self.state[0], &to.state[0], 16),
            (&self.state[1], &to.state[1], 16),
            (&self.geometry, &to.geometry, 16),
            (&self.history, &to.history, 4),
            (&self.water_info, &to.water_info, 16),
            (&self.surface, &to.surface, 16),
            (&self.normals, &to.normals, 16),
        ];
        for (from, into, stride) in per_cell {
            encoder.copy_buffer_to_buffer(from, 0, into, 0, cells * stride);
        }
        encoder.copy_buffer_to_buffer(&self.slot_tiles, 0, &to.slot_tiles, 0, slots * 8);
    }

    fn bytes(&self, tile_cells: u64) -> u64 {
        u64::from(self.capacity) * (tile_cells * 100 + 12)
    }
}

enum Stage {
    Copied,
    Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

struct Pending {
    buffer: wgpu::Buffer,
    frame: u64,
    slots: u32,
    stage: Stage,
}

/// Asynchronous per-slot occupancy readback ring. Never waits on the GPU.
#[derive(Default)]
struct Readback {
    pending: VecDeque<Pending>,
    free: Vec<wgpu::Buffer>,
}

impl Readback {
    /// Oldest completed sample, in submission order: (solver frame, flags).
    fn poll(&mut self, device: &wgpu::Device) -> Option<(u64, Vec<u32>)> {
        for p in &mut self.pending {
            if matches!(p.stage, Stage::Copied) {
                let (tx, rx) = mpsc::channel();
                p.buffer.slice(..u64::from(p.slots.max(1)) * 4).map_async(wgpu::MapMode::Read, move |r| {
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
        if result.is_ok() {
            if let Ok(view) = front.buffer.slice(..u64::from(front.slots.max(1)) * 4).get_mapped_range() {
                flags = view.chunks_exact(4).take(front.slots as usize).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            }
            front.buffer.unmap();
        }
        self.free.push(front.buffer);
        (result.is_ok() && flags.len() == front.slots as usize).then_some((front.frame, flags))
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
    snapshot: wgpu::ComputePipeline,
    occupancy: wgpu::ComputePipeline,
    capture: TileCapture,
    inlet: Option<Inlet>,
    inlet_active: bool,
    current: usize,
    history_needs_snapshot: bool,
    started: bool,
    parameters: Parameters,
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
}

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
            dimensions: [dims[0], dims[1], spawn.transport.extrapolation_half_width, u32::from(spawn.transport.extrapolate_into_dry)],
            stepping: [p.dx, p.dt, p.gravity, p.max_velocity],
            limits: [p.friction, p.velocity_dissipation, p.max_depth, p.integration_depth_max],
            fade: [p.fade_start, p.fade_speed, spawn.transport.advection_multiplier, spawn.transport.minimum_advection_depth],
        };
        let c = spawn.controls;
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
            contents: bytemuck::cast_slice(&[r.half_width as f32, r.height_cutoff, r.smoothing_method as f32, r.border_width as f32]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut volumes: Vec<f32> = spawn.boxes.iter().flat_map(|b| {
            [b.position[0], b.position[1], b.position[2], b.angle, b.size[0], b.size[1], b.size[2], if b.sink { 1.0 } else { 0.0 }]
        }).collect();
        if volumes.is_empty() {
            volumes.resize(8, 0.0);
        }
        let boxes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river source boxes"),
            contents: bytemuck::cast_slice(&volumes),
            usage: wgpu::BufferUsages::STORAGE,
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
        let pool = Pool::new(device, slots.max(16).next_power_of_two(), tile_cells)?;
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
            contents: bytemuck::cast_slice(&[tw, th, tile, lattice.shift(), slots, 0, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let mut entries = Vec::new();
        for binding in [0u32, 5, 10, 12] {
            entries.push(uniform_entry(binding));
        }
        for (binding, read_only) in [(1, true), (2, true), (3, false), (4, false), (6, false), (7, false), (8, true), (9, true), (13, true), (14, true), (15, false)] {
            entries.push(storage_entry(binding, read_only));
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
            source: wgpu::ShaderSource::Wgsl(SPARSE_SHALLOW_SHADER.into()),
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
        let pipelines = Kernel::ALL.iter().map(|k| make(k.entry())).collect();
        let snapshot = make("snapshot_bottom");
        let occupancy = make("tile_occupancy");

        // River-wide capture cameras, so every tile projects the same depths.
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let [zmin, zmax] = spawn.bins.z_range();
        let bottom = (f64::from(zmin) * cm - spawn.origin_z_cm) as f32;
        let top = (f64::from(zmax) * cm - spawn.origin_z_cm) as f32;
        let headroom = 100.0;
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
        };
        let groups = Self::bind(device, &layout, &pool, &table, &tiles_uniform, &uniform, &boundary_uniform, &presentation, &boxes);
        let budget = u64::from(solver_frames_within_margin(spawn.guard * tile, spawn.substeps));
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
                (4, &pool.history),
                (5, presentation),
                (6, &pool.normals),
                (7, &pool.surface),
                (8, &pool.water_info),
                (9, boxes),
                (10, boundary),
                (12, tiles),
                (13, table),
                (14, &pool.slot_tiles),
                (15, &pool.occupancy),
            ]
            .into_iter()
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
        let positions = self.bins.tile_mesh(tile, self.origin_z_cm);
        let indices: Vec<u32> = (0..positions.len() as u32).collect();
        let meshes = [Mesh { class: MeshClass::Landscape, positions: &positions, indices: &indices, sides: Sides::TwoSided }];
        self.capture.depths.encode_capture(device, encoder, &meshes)?;
        let origin = self.lattice.tile_origin(tile);
        let dims = self.lattice.dimensions();
        let mut words: Vec<u32> = vec![
            origin[0] as u32, origin[1] as u32, dims[0], dims[1],
            slot, self.lattice.tile(), self.capture.depths.resolution(), u32::from(initial),
            self.capture.floor.to_bits(), 0, 0, 0,
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
            (3, pool.history.as_entire_binding()),
            (4, pool.state[0].as_entire_binding()),
            (5, pool.state[1].as_entire_binding()),
            (6, pool.surface.as_entire_binding()),
            (7, pool.normals.as_entire_binding()),
        ];
        let entries: Vec<_> = resources.into_iter().map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource }).collect();
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
            let capacity = slots.next_power_of_two().max(self.pool.capacity * 2);
            let pool = Pool::new(device, capacity, self.tile_cells)?;
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
        while let Some((frame, flags)) = self.readback.poll(device) {
            let occupied: Vec<[i32; 2]> = flags
                .iter()
                .enumerate()
                .filter(|(_, f)| **f != 0)
                .map(|(slot, _)| self.residency.slot_tile(slot as u32))
                .collect();
            let queued: BTreeSet<[i32; 2]> = self.queue.iter().copied().collect();
            for tile in required_tiles(self.lattice, occupied, self.guard) {
                if !self.residency.is_resident(tile) && !queued.contains(&tile) {
                    self.queue.push_back(tile);
                }
            }
            self.deferred = Some(self.deferred.map_or(frame, |d| d.max(frame)));
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
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river PreSim snapshot"), timestamp_writes: None });
            pass.set_pipeline(&self.snapshot);
            pass.set_bind_group(0, &self.groups[self.current], &[]);
            self.dispatch(&mut pass);
            drop(pass);
            self.history_needs_snapshot = false;
        }
        let index = Kernel::ALL.iter().position(|&k| k == kernel).expect("listed kernel");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(kernel.entry()), timestamp_writes: None });
        pass.set_pipeline(&self.pipelines[index]);
        pass.set_bind_group(0, &self.groups[self.current], &[]);
        self.dispatch(&mut pass);
        drop(pass);
        self.current = 1 - self.current;
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
        self.encode_kernel(encoder, Kernel::SourceAndSink);
        if self.inlet_active {
            self.encode_inlet(device, encoder, frame.hydraulic_duration());
        }
        self.encode_kernel(encoder, Kernel::DistributeOverlap);
        for _ in 0..frame.substeps() {
            for kernel in &Kernel::ALL[2..8] {
                self.encode_kernel(encoder, *kernel);
            }
        }
        self.encode_kernel(encoder, Kernel::PrepareSurface);
        self.encode_kernel(encoder, Kernel::ComputeNormalsAndReset);
        self.history_needs_snapshot = true;
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
        encoder.clear_buffer(&self.pool.occupancy, 0, Some(bytes));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river tile occupancy"), timestamp_writes: None });
        pass.set_pipeline(&self.occupancy);
        pass.set_bind_group(0, &self.groups[self.current], &[]);
        self.dispatch(&mut pass);
        drop(pass);
        let buffer = self.readback.buffer(device, bytes);
        encoder.copy_buffer_to_buffer(&self.pool.occupancy, 0, &buffer, 0, bytes);
        self.readback.pending.push_back(Pending { buffer, frame: self.solver_frames, slots, stage: Stage::Copied });
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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

    /// Height-mode toggle and Height+CFS rise cap, copied with the next frame.
    pub fn set_height_source(&mut self, enabled: bool, rise_limit_cm: f32) {
        self.boundary_template.flags[0] = u32::from(enabled);
        self.boundary_template.origin[3] = rise_limit_cm;
    }

    pub fn lattice(&self) -> Lattice {
        self.lattice
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
}

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
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("wall_void"),
        layout: None,
        module: &shader,
        entry_point: Some("wall_void"),
        compilation_options: Default::default(),
        cache: None,
    });
    let words = [cells, 0, 0, 0, floor.to_bits(), wall_height.to_bits(), 0, 0];
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
    pub fn new(device: &wgpu::Device, cells: u32) -> Self {
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
    pub fn encode_gather(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, sim: &SparseSimulation) {
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
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river window gather"), timestamp_writes: None });
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
        validate(SPARSE_SHALLOW_SHADER);
        validate(SHADER);
    }

    #[test]
    fn stage_uniform_matches_dense_layout() {
        assert_eq!(std::mem::size_of::<StageUniform>(), 64);
        assert_eq!(std::mem::size_of::<BoundaryUniform>(), 48);
    }
}
