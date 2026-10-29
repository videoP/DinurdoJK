//! GPU-resident two-source demo compositing for the embedded Studio preview.
//! Neither source leaves VRAM. Only the two world scene passes do real 3D work;
//! the final transition is a single fullscreen triangle within the preview rect.

pub(in crate::renderer) struct StudioTransitionGpu {
    pub size: wgpu::Extent3d,
    pub format: wgpu::TextureFormat,
    pub pipeline: wgpu::RenderPipeline,
    pub bindings: wgpu::BindGroupLayout,
    pub params: wgpu::Buffer,
    _empty_entity_mask: wgpu::Texture,
    pub empty_entity_mask_view: wgpu::TextureView,
}

impl StudioTransitionGpu {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Studio live demo GPU transition"),
            source: wgpu::ShaderSource::Wgsl(include_str!("studio_transition.wgsl").into()),
        });
        let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Studio transition source textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Each independent scene already owns an R32Float radial-depth
                // target. Never read it for ordinary color-only transitions.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Visible per-pixel identity masks: player (R), dynamic model (G).
                // Non-identity transitions use the cheap 1x1 transparent fallback.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Studio transition pipeline"),
            bind_group_layouts: &[Some(&bindings)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Studio transition compositor"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Studio transition shader parameters"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let empty_entity_mask=device.create_texture(&wgpu::TextureDescriptor {
            label:Some("Studio transparent identity mask fallback"),
            size:wgpu::Extent3d {width:1,height:1,depth_or_array_layers:1},
            mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,
            format:wgpu::TextureFormat::Rgba8Unorm,
            usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,
            view_formats:&[],
        });
        let empty_entity_mask_view=empty_entity_mask.create_view(&Default::default());
        Self {
            size,
            format,
            pipeline,
            bindings,
            params,
            _empty_entity_mask:empty_entity_mask,
            empty_entity_mask_view,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_compositor_wgsl_is_valid() {
        let shader = naga::front::wgsl::parse_str(include_str!("studio_transition.wgsl"))
            .expect("Studio GPU transition WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&shader)
        .expect("Studio GPU transition WGSL validates");
    }
}
