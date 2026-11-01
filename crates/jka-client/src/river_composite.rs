//! Resources for the native SingleLayerWaterComposite reflection adapter.
//! HZB inputs are the same-frame scene without water, avoiding water self hits.
pub const HZB_SHADER: &str = include_str!("river_hzb.wgsl");
pub const TRACE_SHADER: &str = include_str!("river_ssr.wgsl");
pub const SHADER: &str = include_str!("river_composite.wgsl");

pub struct DepthPyramid {
    pub view: wgpu::TextureView,
    passes: Vec<(wgpu::ComputePipeline, wgpu::BindGroup, [u32; 2])>,
}
impl DepthPyramid {
    pub fn new(device: &wgpu::Device, size: [u32; 2], depth: &wgpu::TextureView) -> Self {
        // Power-of-two padding matches UE's HZB UV factors. Padding is far
        // depth, so coarse cells crossing the viewport cannot become occluders.
        let size = size.map(|n| n.next_power_of_two().max(2) / 2);
        let levels = size[0].max(size[1]).ilog2() + 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("river reflection HZB"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river HZB reduction"),
            source: wgpu::ShaderSource::Wgsl(HZB_SHADER.into()),
        });
        let views: Vec<_> = (0..levels)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: i,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let mut passes = Vec::new();
        for i in 0..levels {
            let first = i == 0;
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("river HZB"),
                layout: None,
                module: &shader,
                entry_point: Some(if first { "first" } else { "reduce" }),
                compilation_options: Default::default(),
                cache: None,
            });
            let input = if first { depth } else { &views[i as usize - 1] };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("river HZB mip"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: if first { 0 } else { 1 },
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[i as usize]),
                    },
                ],
            });
            passes.push((pipeline, group, size.map(|n| (n >> i).max(1))));
        }
        Self {
            view: texture.create_view(&Default::default()),
            passes,
        }
    }
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        for (pipeline, group, size) in &self.passes {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("river reflection HZB"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
    }
}

pub fn sky_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries: Vec<_> = (0..6)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        })
        .collect();
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 6,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    });
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("river map sky"),
        entries: &entries,
    })
}
pub fn sky_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    faces: Option<[&wgpu::TextureView; 6]>,
) -> wgpu::BindGroup {
    let black = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("river absent map sky"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let faces = faces.unwrap_or([&black; 6]);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("river map sky trilinear clamp"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let mut entries: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(i, view)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(view),
        })
        .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: 6,
        resource: wgpu::BindingResource::Sampler(&sampler),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("river map sky"),
        layout,
        entries: &entries,
    })
}
