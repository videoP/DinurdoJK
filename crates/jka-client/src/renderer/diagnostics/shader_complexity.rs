//! Opt-in GPU accumulation target for BSP material-complexity diagnostics.
//! No resources are created until the heatmap is enabled.

pub(in crate::renderer) struct ShaderComplexityTarget {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub scene_format: wgpu::TextureFormat,
    _resolved: wgpu::Texture,
    _multisampled: Option<wgpu::Texture>,
    pub resolved_view: wgpu::TextureView,
    pub multisampled_view: Option<wgpu::TextureView>,
    pub overlay_group: wgpu::BindGroup,
    pub overlay_pipeline: wgpu::RenderPipeline,
}

impl ShaderComplexityTarget {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        samples: u32,
        scene_format: wgpu::TextureFormat,
    ) -> Self {
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
        let make_texture = |name, sample_count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let resolved = make_texture(
            "BSP per-pixel accumulated cost",
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let resolved_view = resolved.create_view(&Default::default());
        let multisampled = (samples > 1).then(|| make_texture(
            "BSP per-pixel cost MSAA",
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ));
        let multisampled_view = multisampled.as_ref().map(|t| t.create_view(&Default::default()));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BSP heatmap diagnostic compositor"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", crate::material_flags::shader_constants(), include_str!("../../bsp_complexity.wgsl")).into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BSP heatmap cost texture layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            }],
        });
        let overlay_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BSP heatmap cost texture"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&resolved_view) }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("BSP heatmap compositor layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("BSP accumulated complexity compositor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_composite"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        });
        Self {
            width, height, samples, scene_format,
            _resolved: resolved, _multisampled: multisampled,
            resolved_view, multisampled_view,
            overlay_group, overlay_pipeline,
        }
    }

    /// Each pass is a distinct GPU operation. The accumulation buffer is
    /// cleared every frame, then one contribution per visible BSP fragment
    /// is added using ONE+ONE blending and depth testing (no depth writes).
    pub fn composite(
        &self, encoder: &mut wgpu::CommandEncoder,
        scene_msaa: Option<&wgpu::TextureView>,
        scene_output: &wgpu::TextureView,
    ) {
        let color_view = scene_msaa.unwrap_or(scene_output);
        let resolve_target = scene_msaa.map(|_| scene_output);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("BSP accumulated shader complexity colorize"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.overlay_pipeline);
        pass.set_bind_group(0, &self.overlay_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
