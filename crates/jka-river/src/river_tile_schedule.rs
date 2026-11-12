//! Sparse river tile schedule A/B (r_riverTileSchedule). Not an Unreal system
//! and NOT local time stepping; see `Development Docs/Patch20/LTS_DESIGN.md`.
//!
//! 0 (A, default): the native sparse path, every stage dispatched over every
//!   resident slot. Nothing here is allocated.
//! 1 (B, parity): every resident slot through a GPU worklist and indirect
//!   dispatch. Same stage bodies; checks the worklist plumbing against A.
//! 2 (B, dormant-dry): the worklist leaves out resident slots on which a
//!   whole solver frame is the identity: the slot and every resident tile
//!   within `halo` tiles hold the exact dry fixed point and no forcing
//!   (source box, CFS inlet, moving body) reaches them. Every slot that runs
//!   still runs every native stage at the native dt, so the pool after each
//!   frame is the one A writes, and A <-> B may switch at any frame.
//!
//! Wet tiles are never slowed down: the cached river variant clamps velocity
//! to dx/dt and depth to (dx/dt)^2/g (control.rs), i.e. it runs at Courant 1
//! by construction, and its depth update has no shared face flux to integrate
//! across a rate interface. Per the handoff's stop rule that part is not built.
use super::{storage_entry, Kernel, Pool, Storage};
use jka_water::river::sparse::{WET_FRONT_CELLS_PER_SUBSTEP, WET_STENCIL_MARGIN};
use std::sync::atomic::{AtomicU8, Ordering};
use wgpu::util::DeviceExt;

/// river_tile_schedule.wgsl without its storage prelude (`Storage::schedule_shader`).
pub(super) const BODY: &str = include_str!("river_tile_schedule.wgsl");
/// Forcing rectangles per solver frame: source box, inlet, 2 x 16 bodies.
pub(super) const MAX_SEEDS: usize = 40;
const UNIFORM_BYTES: usize = 32 + 16 * MAX_SEEDS;

static MODE: AtomicU8 = AtomicU8::new(0);

pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}
pub fn set_mode(mode: u8) {
    MODE.store(mode.min(2), Ordering::Relaxed);
}
pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "native",
        1 => "worklist",
        _ => "dormant-dry",
    }
}

/// Whole tiles of clearance a dormant slot needs from any dirty tile for one
/// solver frame. Its nearest dirty cell is then at least `halo * tile + 1`
/// cells away, which must exceed what one frame can reach: the wet front
/// (`WET_FRONT_CELLS_PER_SUBSTEP` per substep), the residency proof's stencil
/// margin, the collider extrapolation radius (extrapolate_velocity/height)
/// and the PrepareSurface smoothing radius (its output for dry cells near
/// water). The terms are summed although they do not stack.
pub fn halo_tiles(substeps: u32, smoothing_radius: u32, extrapolation_radius: u32, tile: u32) -> u32 {
    let cells = substeps.max(1) * WET_FRONT_CELLS_PER_SUBSTEP + WET_STENCIL_MARGIN + smoothing_radius + extrapolation_radius;
    cells.div_ceil(tile.max(1)).max(1)
}

/// Buffers sized to one pool generation.
struct Bound {
    generation: u64,
    capacity: u32,
    dirty: wgpu::Buffer,
    stage_group: wgpu::BindGroup,
    classify_group: wgpu::BindGroup,
    build_group: wgpu::BindGroup,
}

#[derive(Default)]
struct Stats {
    /// (scheduled, resident) of the latest sampled solver frame.
    last: Option<(u32, u32)>,
    scheduled: u64,
    resident: u64,
}

pub(super) struct TileSchedule {
    mode: u8,
    /// Kernel::ALL order, then snapshot_bottom. Group 0 = the native layout.
    stages: Vec<wgpu::ComputePipeline>,
    stage_layout: wgpu::BindGroupLayout,
    classify: wgpu::ComputePipeline,
    build: wgpu::ComputePipeline,
    uniform: wgpu::Buffer,
    args: wgpu::Buffer,
    bound: Option<Bound>,
    halo: u32,
    stats: Stats,
}

impl TileSchedule {
    /// Worklist stages need one storage binding more than the native ones.
    pub(super) fn supported(device: &wgpu::Device) -> bool {
        device.limits().max_storage_buffers_per_shader_stage >= 12
    }

    pub(super) fn new(device: &wgpu::Device, native_layout: &wgpu::BindGroupLayout, tile: u32, storage: Storage) -> Self {
        let stage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sparse river tile schedule worklist"),
            entries: &[storage_entry(0, true)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sparse river scheduled stages"),
            bind_group_layouts: &[Some(native_layout), Some(&stage_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river stages, sparse worklist addressing"),
            source: wgpu::ShaderSource::Wgsl(storage.stage_shader(true).into()),
        });
        let stages = Kernel::ALL
            .iter()
            .map(|&k| storage.stage_entry(k))
            // Index Kernel::ALL.len(): the PreSim snapshot, native storage only.
            .chain(storage.stores_history().then_some("snapshot_bottom"))
            .map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        let schedule_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sparse river tile schedule"),
            source: wgpu::ShaderSource::Wgsl(storage.schedule_shader(BODY).into()),
        });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &schedule_module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sparse river tile schedule controls"),
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let groups = tile.div_ceil(8);
        let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river scheduled dispatch"),
            contents: bytemuck::cast_slice(&[groups, groups, 0u32, 0u32]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        });
        Self {
            mode: 1,
            stages,
            stage_layout,
            classify: make("classify_tiles"),
            build: make("build_worklist"),
            uniform,
            args,
            bound: None,
            halo: 0,
            stats: Stats::default(),
        }
    }

    pub(super) fn set_mode(&mut self, mode: u8) {
        if mode != self.mode {
            self.stats = Stats::default();
        }
        self.mode = mode;
    }

    fn bind(&mut self, device: &wgpu::Device, pool: &Pool, table: &wgpu::Buffer, generation: u64) {
        if self.bound.as_ref().is_some_and(|b| b.generation == generation && b.capacity == pool.capacity) {
            return;
        }
        let bytes = u64::from(pool.capacity.max(1)) * 4;
        let buffer = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let dirty = buffer("sparse river dirty slots");
        let worklist = buffer("sparse river slot worklist");
        let group = |layout: &wgpu::BindGroupLayout, entries: &[(u32, &wgpu::Buffer)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|(binding, buffer)| wgpu::BindGroupEntry { binding: *binding, resource: buffer.as_entire_binding() })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("sparse river tile schedule"), layout, entries: &entries })
        };
        let stage_group = group(&self.stage_layout, &[(0, &worklist)]);
        let mut classify_entries = vec![
            (0, &self.uniform),
            (1, &pool.state[0]),
            (2, &pool.state[1]),
            (3, &pool.geometry),
            (5, &pool.surface),
            (6, &pool.normals),
            (7, &dirty),
        ];
        if let Some(history) = &pool.history {
            classify_entries.push((4, history));
        }
        let classify_group = group(&self.classify.get_bind_group_layout(0), &classify_entries);
        let build_group = group(
            &self.build.get_bind_group_layout(0),
            &[(0, &self.uniform), (7, &dirty), (8, table), (9, &pool.slot_tiles), (10, &worklist), (11, &self.args)],
        );
        self.bound = Some(Bound { generation, capacity: pool.capacity, dirty, stage_group, classify_group, build_group });
    }

    /// Build this solver frame's worklist from the current pool. Call after
    /// the PreSim body collisions and before the first stage.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pool: &Pool,
        table: &wgpu::Buffer,
        generation: u64,
        lattice: jka_water::river::sparse::Lattice,
        slots: u32,
        halo: u32,
        seeds: &[[i32; 4]],
    ) {
        self.bind(device, pool, table, generation);
        self.halo = halo;
        let skip = self.mode >= 2;
        let [tw, th] = lattice.tiles();
        let mut words: Vec<u32> = vec![slots, lattice.tile(), halo, seeds.len().min(MAX_SEEDS) as u32, tw, th, u32::from(skip), 0];
        for seed in seeds.iter().take(MAX_SEEDS) {
            words.extend(seed.map(|v| v as u32));
        }
        words.resize(UNIFORM_BYTES / 4, 0);
        let staged = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sparse river tile schedule upload"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&staged, 0, &self.uniform, 0, UNIFORM_BYTES as u64);
        encoder.clear_buffer(&self.args, 8, Some(4));
        let bound = self.bound.as_ref().expect("bound above");
        if slots == 0 {
            return;
        }
        if skip {
            encoder.clear_buffer(&bound.dirty, 0, Some(u64::from(slots) * 4));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river classify tiles"), timestamp_writes: None });
            pass.set_pipeline(&self.classify);
            pass.set_bind_group(0, &bound.classify_group, &[]);
            let groups = lattice.tile().div_ceil(8);
            pass.dispatch_workgroups(groups, groups, slots);
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("sparse river slot worklist"), timestamp_writes: None });
        pass.set_pipeline(&self.build);
        pass.set_bind_group(0, &bound.build_group, &[]);
        pass.dispatch_workgroups(slots.div_ceil(64), 1, 1);
    }

    /// Bind and dispatch one scheduled stage; `stage` indexes Kernel::ALL,
    /// `Kernel::ALL.len()` is snapshot_bottom. Group 0 is already set.
    pub(super) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, stage: usize) {
        let bound = self.bound.as_ref().expect("worklist encoded before stages");
        pass.set_pipeline(&self.stages[stage]);
        pass.set_bind_group(1, &bound.stage_group, &[]);
        pass.dispatch_workgroups_indirect(&self.args, 0);
    }

    /// Scheduled-slot count of the last encoded frame, for the occupancy
    /// readback (one extra word after the per-slot flags).
    pub(super) fn copy_count(&self, encoder: &mut wgpu::CommandEncoder, into: &wgpu::Buffer, offset: u64) {
        encoder.copy_buffer_to_buffer(&self.args, 8, into, offset, 4);
    }

    pub(super) fn record(&mut self, scheduled: u32, resident: u32) {
        self.stats.last = Some((scheduled, resident));
        self.stats.scheduled += u64::from(scheduled);
        self.stats.resident += u64::from(resident);
    }

    /// Fraction of resident slot-frames left out, over the sampled frames.
    pub(super) fn skipped_fraction(&self) -> Option<f64> {
        (self.stats.resident > 0).then(|| 1.0 - self.stats.scheduled as f64 / self.stats.resident as f64)
    }

    pub(super) fn report(&self) -> String {
        let skipped = 100.0 * self.skipped_fraction().unwrap_or(0.0);
        match self.stats.last {
            Some((scheduled, resident)) => format!(
                "schedule={} scheduled={}/{} halo={}t skipped_avg={:.1}%",
                mode_name(self.mode), scheduled, resident, self.halo, skipped
            ),
            None => format!("schedule={} (no sample yet) halo={}t", mode_name(self.mode), self.halo),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo_covers_one_solver_frame_of_reach() {
        // Defaults: 10 substeps, smoothing 5, extrapolation 2 -> 20 cells.
        assert_eq!(halo_tiles(10, 5, 2, 32), 1);
        assert_eq!(halo_tiles(10, 5, 2, 16), 2);
        assert_eq!(halo_tiles(10, 5, 2, 128), 1);
        assert_eq!(halo_tiles(40, 5, 2, 32), 2);
        for (substeps, tile) in [(1, 16), (10, 16), (10, 32), (30, 32), (60, 64)] {
            let reach = substeps * WET_FRONT_CELLS_PER_SUBSTEP + WET_STENCIL_MARGIN + 7;
            assert!(halo_tiles(substeps, 5, 2, tile) * tile + 1 > reach);
        }
    }

    #[test]
    fn schedule_shaders_validate() {
        for source in [Storage::Native.schedule_shader(BODY), jka_water::river::SPARSE_WORKLIST_SHALLOW_SHADER.to_string()] {
            let module = wgpu::naga::front::wgsl::parse_str(&source).expect("schedule WGSL parses");
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::default())
                .validate(&module)
                .unwrap_or_else(|e| panic!("schedule WGSL validates: {e:?}"));
        }
    }
}
