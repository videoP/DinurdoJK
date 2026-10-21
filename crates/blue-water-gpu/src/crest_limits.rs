//! Small GPU reduction used only by the opt-in Hybrid crest experiment.
use crate::{pipeline, sampled, storage, uniform, FieldSet};

pub(crate) struct CrestLimits {
    pub buffer: wgpu::Buffer,
    scan: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    groups: [wgpu::BindGroup; 2],
    pub enabled: bool,
    composite: Option<Composite>,
}

struct Composite {
    scan: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    empty: wgpu::BindGroup,
    waves: [wgpu::BindGroup; 2],
    limits: wgpu::Buffer,
}

impl CrestLimits {
    pub fn new(
        device: &wgpu::Device,
        sets: &[FieldSet; 2],
        clock: &wgpu::Buffer,
        n: u32,
        count: u32,
    ) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Hybrid crest compression bounds"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let partial = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Hybrid crest compression partials"),
            size: u64::from(n.div_ceil(8).pow(2) * count) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Hybrid crest reduction"),
            entries: &[
                sampled(0),
                sampled(1),
                storage(2, false),
                storage(3, false),
                uniform(4),
            ],
        });
        let shader = include_str!("crest_limits.wgsl");
        let scan = pipeline(device, "Hybrid crest scan", shader, &layout, "scan");
        let finish = pipeline(device, "Hybrid crest reduction", shader, &layout, "finish");
        let groups = std::array::from_fn(|set| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Hybrid crest reduction inputs"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&sets[set].views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&sets[set].views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: partial.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: clock.as_entire_binding(),
                    },
                ],
            })
        });
        Self {
            buffer,
            scan,
            finish,
            groups,
            enabled: false,
            composite: None,
        }
    }

    pub fn prepare_composite(
        &mut self,
        device: &wgpu::Device,
        waves: &crate::surface_waves::SurfaceWaves,
        clock: &wgpu::Buffer,
    ) {
        if self.composite.is_some() {
            return;
        }
        let partial = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Hybrid composite compression partials"),
            size: 512 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        // Wave bind groups also contain the read-only final bounds. Keep the
        // writable reduction output separate, then copy its fourth component.
        let limits = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Hybrid composite compression bound"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Hybrid composite crest reduction"),
            entries: &[storage(0, false), storage(1, false), uniform(2)],
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let mut composer = crate::shader_composer::composer().expect("native wave modules");
        let module = composer
            .make_naga_module(naga_oil::compose::NagaModuleDescriptor {
                source: include_str!("crest_composite.wgsl"),
                file_path: "Hybrid composite crest scan",
                shader_type: naga_oil::compose::ShaderType::Wgsl,
                shader_defs: [(
                    "CASCADES".into(),
                    naga_oil::compose::ShaderDefValue::UInt(3),
                )]
                .into(),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&composer)));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Hybrid composite crest scan"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(&empty_layout), Some(waves.layout())],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        self.composite = Some(Composite {
            scan: pipeline("scan"),
            finish: pipeline("finish"),
            group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: partial.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: limits.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: clock.as_entire_binding(),
                    },
                ],
            }),
            empty: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &empty_layout,
                entries: &[],
            }),
            waves: waves.groups.clone(),
            limits,
        });
    }

    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, set: usize, n: u32, count: u32) {
        if !self.enabled {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Hybrid crest compression limit"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.groups[set], &[]);
        pass.set_pipeline(&self.scan);
        pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), count);
        pass.set_pipeline(&self.finish);
        pass.dispatch_workgroups(1, 1, 1);
        if let Some(composite) = &self.composite {
            pass.set_pipeline(&composite.scan);
            pass.set_bind_group(0, &composite.group, &[]);
            pass.set_bind_group(1, &composite.empty, &[]);
            pass.set_bind_group(2, &composite.waves[set], &[]);
            pass.dispatch_workgroups(16, 16, 2);
            pass.set_pipeline(&composite.finish);
            pass.dispatch_workgroups(1, 1, 1);
        }
        drop(pass);
        if let Some(composite) = &self.composite {
            encoder.copy_buffer_to_buffer(&composite.limits, 12, &self.buffer, 12, 4);
        }
    }

    pub fn reset(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.buffer, 0, None);
        if let Some(composite) = &self.composite {
            encoder.clear_buffer(&composite.limits, 0, None);
        }
    }
}
