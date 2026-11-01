//! Explicit CFS volume forcing at the host boundary of the native solver.
use jka_water::river::control::Grid;
use wgpu::util::DeviceExt;

pub const SHADER: &str = include_str!("river_discharge.wgsl");
/// International foot = 30.48 cm, exactly. CFS is cm3 per simulated second.
pub const CM3_PER_CUBIC_FOOT: f64 = 28_316.846_592;

pub struct Discharge {
    pub count: wgpu::Buffer,
    uniform: wgpu::Buffer,
    metrics: [f32; 4],
    cfs: f32,
    count_pipeline: wgpu::ComputePipeline,
    count_group: wgpu::BindGroup,
    count_size: [u32; 2],
    pipeline: wgpu::ComputePipeline,
    groups: [wgpu::BindGroup; 2],
    dimensions: [u32; 2],
}
impl Discharge {
    pub fn new(
        device: &wgpu::Device,
        grid: Grid,
        states: &[wgpu::Texture; 2],
        geometry: &wgpu::Buffer,
        width_cm: f32,
        level_cm: f32,
        cfs: f32,
    ) -> Self {
        let ranges: [(u32, u32); 2] = std::array::from_fn(|axis| {
            let n = grid.dimensions[axis];
            let lo = ((grid.world_size[axis] - width_cm) * 0.5 / grid.cell_size[axis] - 0.5).ceil();
            let hi = ((grid.world_size[axis] + width_cm) * 0.5 / grid.cell_size[axis] - 0.5)
                .floor()
                + 1.0;
            let lo = (lo.max(2.0) as u32).min(n - 3);
            (lo, (hi.max((lo + 1) as f32) as u32).min(n - 2))
        });
        let rect = [ranges[0].0, ranges[1].0, ranges[0].1, ranges[1].1];
        let metrics = [
            grid.cell_size[0] * grid.cell_size[1],
            level_cm,
            0.0,
            grid.dimensions[0] as f32,
        ];
        let mut bytes = bytemuck::cast_slice(&rect).to_vec();
        bytes.extend_from_slice(bytemuck::cast_slice(&metrics));
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river CFS source"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let count = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river open inlet cell count"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river discharge adapter"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let make_pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let count_pipeline = make_pipeline("count_cells");
        let count_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &count_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: geometry.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: count.as_entire_binding(),
                },
            ],
        });
        let pipeline = make_pipeline("inject");
        let views = states
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: geometry.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: count.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&views[1 - i]),
                    },
                ],
            })
        });
        Self {
            count,
            uniform,
            metrics,
            cfs,
            count_pipeline,
            count_group,
            count_size: [rect[2] - rect[0], rect[3] - rect[1]],
            pipeline,
            groups,
            dimensions: grid.dimensions,
        }
    }
    pub fn initialize(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.count, 0, None);
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&self.count_pipeline);
        pass.set_bind_group(0, &self.count_group, &[]);
        pass.dispatch_workgroups(
            self.count_size[0].div_ceil(8),
            self.count_size[1].div_ceil(8),
            1,
        );
    }
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        state: usize,
        seconds: f64,
    ) {
        let mut metrics = self.metrics;
        metrics[2] = (f64::from(self.cfs) * CM3_PER_CUBIC_FOOT * seconds) as f32;
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river queued CFS volume"),
            contents: bytemuck::cast_slice(&metrics),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 16, 16);
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.groups[state], &[]);
        pass.dispatch_workgroups(
            self.dimensions[0].div_ceil(8),
            self.dimensions[1].div_ceil(8),
            1,
        );
    }
}
