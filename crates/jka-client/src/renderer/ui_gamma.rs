//! Optional, UI-local inverse of the Windows SDR display gamma ramp.
//! No extra scene work: only menu frames receive the transparent egui pass.
//! The native HUD uses its own fragment variant over existing geometry.
use super::{create_ui_pipeline, Renderer};

pub(super) struct NativeUiGamma {
    pub pipeline: wgpu::RenderPipeline,
    pub group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
}

pub(super) struct EguiUiGamma {
    pub view: wgpu::TextureView,
    _texture: wgpu::Texture,
    pub pipeline: wgpu::RenderPipeline,
    pub group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    size: (u32, u32),
}

fn uniform_values(gamma: f32, srgb: bool) -> [f32; 4] {
    [gamma.clamp(0.5, 3.0), if srgb { 1.0 } else { 0.0 }, 0.0, 0.0]
}

impl Renderer {
    pub(super) fn ui_gamma_active(&self) -> bool {
        self.gamma_method == crate::gamma::GammaMethod::HardwareSceneOnly
            && self.hardware_gamma_active
            && (self.gamma - 1.0).abs() > 0.0001
    }

    pub(super) fn ensure_native_ui_gamma(&mut self) {
        if self.ui_gamma_native.is_some() { return; }
        let device = &self.device;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HW scene-only native UI gamma"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let gamma_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HW scene-only native UI gamma layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HW scene-only native UI gamma group"), layout: &gamma_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HW scene-only native UI pipeline layout"),
            bind_group_layouts: &[Some(&self.ui_texture_layout), Some(&gamma_layout)], immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HW scene-only native UI shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../ui_scene_gamma.wgsl").into()),
        });
        let pipeline = create_ui_pipeline(device, &layout, &shader, self.config.format);
        self.queue.write_buffer(&uniform, 0, bytemuck::cast_slice(&uniform_values(self.gamma, self.config.format.is_srgb())));
        self.ui_gamma_native = Some(NativeUiGamma { pipeline, group, uniform });
    }

    pub(super) fn sync_ui_gamma_uniforms(&mut self) {
        let values = uniform_values(self.gamma, self.config.format.is_srgb());
        let bytes = bytemuck::cast_slice(&values);
        if let Some(resources) = &self.ui_gamma_native { self.queue.write_buffer(&resources.uniform, 0, bytes); }
        if let Some(resources) = &self.ui_gamma_egui { self.queue.write_buffer(&resources.uniform, 0, bytes); }
    }

    pub(super) fn ensure_egui_ui_gamma(&mut self) {
        let size = (self.config.width.max(1), self.config.height.max(1));
        if self.ui_gamma_egui.as_ref().is_some_and(|resource| resource.size == size) { return; }
        let device = &self.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HW scene-only egui intermediate"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("HW scene-only egui sampler"),
            mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HW scene-only egui gamma"), size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HW scene-only egui source layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HW scene-only egui source"), layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HW scene-only egui composite layout"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HW scene-only egui composite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui_gamma_composite.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HW scene-only egui composite"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs_main"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.config.format,
                    // egui intermediate RGB is premultiplied by coverage.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
                        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None, cache: None,
        });
        self.queue.write_buffer(&uniform, 0, bytemuck::cast_slice(&uniform_values(self.gamma, self.config.format.is_srgb())));
        self.ui_gamma_egui = Some(EguiUiGamma { view, _texture: texture, pipeline, group, uniform, size });
    }
}
