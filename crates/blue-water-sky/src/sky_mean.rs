//! Host adapter for the original GPU sky-reference sampler.
use crate::{EnvironmentMap, environment_bind_group_layout, shaders};

pub struct SkyMeanSampler {
    pub buffer: wgpu::Buffer,
    output: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
}

impl SkyMeanSampler {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water sky mean and clear-sky luminance"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water sky mean output"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let output = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water sky mean output"),
            layout: &output_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let shader = shaders::shader_module(
            device,
            include_str!("../../../vendor/blue-water-sea-and-sky/sky/shaders/sky_mean.wgsl"),
            "naval-sky/shaders/sky_mean.wgsl",
        );
        let environment_layout = environment_bind_group_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water sky mean"),
            bind_group_layouts: &[Some(&environment_layout), Some(&output_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Blue Water original sky mean"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            buffer,
            output,
            pipeline,
        }
    }

    /// Run after the environment bake, using its live group after quality changes.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, environment: &EnvironmentMap) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blue Water sky reference"),
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &environment.bind_group, &[]);
        pass.set_bind_group(1, &self.output, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
