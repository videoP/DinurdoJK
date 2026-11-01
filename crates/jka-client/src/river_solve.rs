//! River fast-solve convergence: occasional GPU state comparisons and a tiny
//! *asynchronous* readback. No per-step overhead, no blocking GPU waits, and
//! no measurement resources at all once the river has settled to 1x.
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::river_gpu::Simulation;

pub(super) const SHADER: &str = include_str!("river_solve_probe.wgsl");
const SAMPLE_INTERVAL: Duration = Duration::from_millis(850);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const REQUIRED_STEADY_SAMPLES: u32 = 4;

#[derive(Clone, Copy, Debug, Default)]
struct FlowMetrics {
    depth_change_cm: f64,
    velocity_change_cm_s: f64,
    wet_cells: f64,
    water_depth_sum_cm: f64,
}

impl FlowMetrics {
    /// A moving river may be stable: we measure *change* in the depth and
    /// velocity field, never require velocity itself to be zero. Guard against
    /// the completely dry (all-zero) grid falsely qualifying as stable.
    fn qualifies(self) -> bool {
        let wet = self.wet_cells.max(1.0);
        self.wet_cells >= 2.0
            && self.depth_change_cm.is_finite()
            && self.velocity_change_cm_s.is_finite()
            && self.water_depth_sum_cm.is_finite()
            && self.water_depth_sum_cm > 1.0
            && self.depth_change_cm / wet < 0.5
            && self.depth_change_cm / self.water_depth_sum_cm < 0.005
            && self.velocity_change_cm_s / wet < 10.0
    }
}

enum Readback {
    Idle,
    Copied,
    Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

pub(super) struct SolveConvergence {
    pipeline: wgpu::ComputePipeline,
    groups: [wgpu::BindGroup; 2],
    snapshot: wgpu::Texture,
    summaries: wgpu::Buffer,
    readback: wgpu::Buffer,
    groups_xy: [u32; 2],
    bytes: u64,
    readback_state: Readback,
    has_snapshot: bool,
    last_sample: Instant,
    last_poll: Instant,
    last_sim_seconds: f64,
    consecutive: u32,
    samples: u32,
}

impl SolveConvergence {
    pub(super) fn new(device: &wgpu::Device, simulation: &Simulation) -> Self {
        let state = simulation.state();
        let size = state.size();
        let snapshot = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("River solve previous hydraulic state"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::river_gpu::WATER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let groups_xy = [size.width.div_ceil(8), size.height.div_ceil(8)];
        let bytes = u64::from(groups_xy[0]) * u64::from(groups_xy[1]) * 16;
        let summaries = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("River solve per-tile change summaries"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("River solve async summary readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("River solve probe layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // Borrow the ping-pong textures; the Simulation retains ownership.
        let views: [wgpu::TextureView; 2] = std::array::from_fn(|i| {
            simulation.state_textures()[i].create_view(&wgpu::TextureViewDescriptor::default())
        });
        let prior_view = snapshot.create_view(&wgpu::TextureViewDescriptor::default());
        let groups = std::array::from_fn(|i| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("River solve hydraulic state + previous snapshot"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[i]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&prior_view) },
                wgpu::BindGroupEntry { binding: 2, resource: summaries.as_entire_binding() },
            ],
        }));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("River solve convergence probe"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("River solve probe pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("River solve convergence probe"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let now = Instant::now();
        Self {
            pipeline, groups, snapshot, summaries, readback, groups_xy, bytes,
            readback_state: Readback::Idle, has_snapshot: false,
            last_sample: now, last_poll: now, last_sim_seconds: 0.0,
            consecutive: 0, samples: 0,
        }
    }

    /// Poll only occasionally and never Wait. Called each render frame while
    /// fast-solving; async callbacks complete as WGPU is polled normally.
    /// Returns true exactly when the sample streak establishes steady flow.
    pub(super) fn poll(&mut self, device: &wgpu::Device, solve_speed: f32, debug: u8) -> bool {
        if matches!(&self.readback_state, Readback::Copied) {
            let (tx, rx) = mpsc::channel();
            self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            self.readback_state = Readback::Mapping(rx);
        }
        if matches!(&self.readback_state, Readback::Mapping(_)) && self.last_poll.elapsed() >= POLL_INTERVAL {
            let _ = device.poll(wgpu::PollType::Poll);
            self.last_poll = Instant::now();
        }
        let result = match &self.readback_state {
            Readback::Mapping(rx) => rx.try_recv().ok(),
            _ => None,
        };
        let Some(result) = result else { return false; };
        self.readback_state = Readback::Idle;
        if let Err(error) = result {
            self.consecutive = 0;
            if debug > 0 { eprintln!("[RIVER SOLVE] async readback failed: {error}"); }
            return false;
        }
        let mut metrics = FlowMetrics::default();
        // Even after a successful map_async, wgpu 30 returns a Result here.
        // Handle a failed view acquisition without attempting to read it.
        let view = match self.readback.slice(..).get_mapped_range() {
            Ok(view) => view,
            Err(error) => {
                self.readback.unmap();
                self.consecutive = 0;
                if debug > 0 {
                    eprintln!("[RIVER SOLVE] mapped readback access failed: {error:?}");
                }
                return false;
            }
        };
        for chunk in view.chunks_exact(16) {
            let read = |offset: usize| -> f64 {
                f64::from(f32::from_le_bytes(chunk[offset..offset + 4].try_into().unwrap()))
            };
            metrics.depth_change_cm += read(0);
            metrics.velocity_change_cm_s += read(4);
            metrics.wet_cells += read(8);
            metrics.water_depth_sum_cm += read(12);
        }
        drop(view); // Release BufferView before unmapping its buffer.
        self.readback.unmap();
        self.samples += 1;
        let qualifies = metrics.qualifies();
        self.consecutive = if qualifies { self.consecutive + 1 } else { 0 };
        let mean_depth = metrics.depth_change_cm / metrics.wet_cells.max(1.0);
        let mean_velocity = metrics.velocity_change_cm_s / metrics.wet_cells.max(1.0);
        if debug > 0 {
            eprintln!("[RIVER SOLVE] state={} speed={solve_speed}x wet_cells={:.0} depth_change={mean_depth:.3}cm/cell ({:.3}% volume) velocity_change={mean_velocity:.2}cm/s steady={}/{} samples={}{}",
                if qualifies { "SETTLING" } else if metrics.wet_cells < 2.0 { "DRY" } else { "CHANGING" },
                metrics.wet_cells,
                100.0 * metrics.depth_change_cm / metrics.water_depth_sum_cm.max(1.0),
                self.consecutive, REQUIRED_STEADY_SAMPLES, self.samples,
                if self.consecutive == REQUIRED_STEADY_SAMPLES { " -> NORMAL" } else { "" });
        }
        self.consecutive >= REQUIRED_STEADY_SAMPLES
    }

    /// Queue one GPU comparison per ~850 ms real time. One group per 8x8
    /// hydraulic cells; a copy of the full state is kept GPU-only. The CPU
    /// receives only (grid_width/8 * grid_height/8) x 16 bytes. The first
    /// sample establishes the baseline and never counts as stable.
    pub(super) fn encode_sample(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        simulation: &Simulation,
        simulated_seconds: f64,
    ) {
        if !matches!(&self.readback_state, Readback::Idle)
            || self.last_sample.elapsed() < SAMPLE_INTERVAL
            || simulated_seconds - self.last_sim_seconds < 0.5
        {
            return;
        }
        self.last_sim_seconds = simulated_seconds;
        self.last_sample = Instant::now();
        if self.has_snapshot {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("River solve steady flow comparison"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.groups[simulation.state_index()], &[]);
            pass.dispatch_workgroups(self.groups_xy[0], self.groups_xy[1], 1);
            drop(pass);
            encoder.copy_buffer_to_buffer(&self.summaries, 0, &self.readback, 0, self.bytes);
            self.readback_state = Readback::Copied;
        }
        encoder.copy_texture_to_texture(
            simulation.state().as_image_copy(),
            self.snapshot.as_image_copy(),
            simulation.state().size(),
        );
        self.has_snapshot = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_or_still_filling_never_counts_as_stable() {
        assert!(!FlowMetrics::default().qualifies());
        assert!(!FlowMetrics { depth_change_cm: 20.0, velocity_change_cm_s: 0.0, wet_cells: 10.0, water_depth_sum_cm: 100.0 }.qualifies());
        assert!(FlowMetrics { depth_change_cm: 0.01, velocity_change_cm_s: 10.0, wet_cells: 30.0, water_depth_sum_cm: 1200.0 }.qualifies());
        assert!(!FlowMetrics { depth_change_cm: f64::NAN, velocity_change_cm_s: 0.0, wet_cells: 30.0, water_depth_sum_cm: 1200.0 }.qualifies());
    }

    #[test]
    fn probe_shader_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).expect("river solve probe parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        ).validate(&module).expect("river solve probe validates");
    }
}
