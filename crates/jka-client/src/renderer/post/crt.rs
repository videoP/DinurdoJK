//! Native crt-lottes-fast presentation pass.
use crate::renderer::{ui, Renderer};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CrtUniform {
    output_input_size: [f32; 4],
    filter_params: [f32; 4],
    mask_corner: [f32; 4],
}

pub(in crate::renderer) struct CrtResources {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
    pub(in crate::renderer) encoded_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    fast_pipeline: wgpu::RenderPipeline,
    quality_pipeline: wgpu::RenderPipeline,
    lottes_pipeline: wgpu::RenderPipeline,
}

impl CrtResources {
    fn texture(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
        let encoded_view_formats = [format.remove_srgb_suffix()];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA CRT presentation intermediate"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: if format.is_srgb() { &encoded_view_formats } else { &[] },
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let encoded_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(format.remove_srgb_suffix()),
            ..Default::default()
        });
        (texture, view, encoded_view)
    }

    fn uniform_values(renderer: &Renderer) -> CrtUniform {
        let width = renderer.config.width.max(1);
        let height = renderer.config.height.max(1);
        let reference_height = renderer.crt_reference.height(height);
        let reference_width = reference_height * width as f32 / height as f32;
        CrtUniform {
            output_input_size: [width as f32, height as f32, reference_width, reference_height],
            filter_params: [
                renderer.crt_scanline.clamp(0.0, 1.0),
                renderer.crt_sharpness.clamp(1.0, 6.0),
                renderer.crt_mask_intensity.clamp(0.0, 1.0),
                renderer.crt_curvature.clamp(0.0, 0.25),
            ],
            mask_corner: [
                renderer.crt_corner.clamp(0.0, 11.0),
                renderer.crt_mask.shader_value() as f32,
                0.0,
                0.0,
            ],
        }
    }

    fn new(renderer: &Renderer) -> Self {
        let device = &renderer.device;
        let width = renderer.config.width.max(1);
        let height = renderer.config.height.max(1);
        let format = renderer.config.format;
        let (texture, view, encoded_view) = Self::texture(device, width, height, format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA CRT source sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA CRT layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA CRT uniform"),
            contents: bytemuck::bytes_of(&Self::uniform_values(renderer)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA CRT bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA crt-lottes-fast WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../crt_lottes_fast.wgsl").into()),
        });
        let lottes_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA crt-lottes WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../crt_lottes.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA CRT pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make_pipeline = |entry: &'static str, label: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: crate::pipeline_cache::get().as_ref(),
            })
        };
        let fast_pipeline = make_pipeline("fs_fast", "JKA CRT Lottes fast 2-tap pipeline");
        let quality_pipeline = make_pipeline("fs_quality", "JKA CRT Lottes quality 8-tap pipeline");
        let lottes_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("JKA CRT Lottes original pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &lottes_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &lottes_shader,
                entry_point: Some("fs_lottes"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        });
        Self {
            width,
            height,
            format,
            _texture: texture,
            view,
            encoded_view,
            sampler,
            layout,
            uniform,
            bind_group,
            fast_pipeline,
            quality_pipeline,
            lottes_pipeline,
        }
    }

    fn rebuild_target(&mut self, renderer: &Renderer) {
        self.width = renderer.config.width.max(1);
        self.height = renderer.config.height.max(1);
        self.format = renderer.config.format;
        let (texture, view, encoded_view) = Self::texture(&renderer.device, self.width, self.height, self.format);
        let bind_group = renderer.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA CRT bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            ],
        });
        self._texture = texture;
        self.view = view;
        self.encoded_view = encoded_view;
        self.bind_group = bind_group;
    }
}

impl Renderer {
    pub(in crate::renderer) fn ensure_crt_resources(&mut self) {
        if self.crt_scope == ui::CrtScope::Off {
            self.crt_resources = None;
            return;
        }
        if self.crt_resources.is_none() {
            let resources = CrtResources::new(self);
            self.crt_resources = Some(resources);
        }
        let format_changed = self
            .crt_resources
            .as_ref()
            .is_some_and(|resources| resources.format != self.config.format);
        if format_changed {
            // Render-pipeline color target formats are fixed, so a surface
            // format change requires rebuilding the pipelines as well.
            let resources = CrtResources::new(self);
            self.crt_resources = Some(resources);
        } else {
            let needs_resize = self.crt_resources.as_ref().is_some_and(|resources| {
                resources.width != self.config.width.max(1)
                    || resources.height != self.config.height.max(1)
            });
            if needs_resize {
                // Size-only changes keep the already-compiled pipelines and
                // rebuild just the source texture/bind group.
                let mut resources = self.crt_resources.take().expect("checked above");
                resources.rebuild_target(self);
                self.crt_resources = Some(resources);
            }
        }
        self.update_crt_uniform();
    }

    pub(in crate::renderer) fn update_crt_uniform(&self) {
        let Some(resources) = self.crt_resources.as_ref() else { return; };
        self.queue.write_buffer(
            &resources.uniform,
            0,
            bytemuck::bytes_of(&CrtResources::uniform_values(self)),
        );
    }

    pub(in crate::renderer) fn encode_crt_resources(
        resources: &CrtResources,
        style: ui::CrtStyle,
        quality: ui::CrtQuality,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA CRT Lottes presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(match style {
            ui::CrtStyle::Lottes => &resources.lottes_pipeline,
            ui::CrtStyle::LottesFast => match quality {
                ui::CrtQuality::Fast => &resources.fast_pipeline,
                ui::CrtQuality::Quality => &resources.quality_pipeline,
            },
        });
        pass.set_bind_group(0, &resources.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(in crate::renderer) fn encode_crt(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) {
        let Some(resources) = self.crt_resources.as_ref() else { return; };
        Self::encode_crt_resources(resources, self.crt_style, self.crt_quality, encoder, output);
    }

    pub(in crate::renderer) fn submit_crt(&self, output: &wgpu::TextureView) {
        if self.crt_resources.is_none() {
            return;
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("JKA CRT presentation encoder"),
        });
        self.encode_crt(&mut encoder, output);
        self.queue.submit([encoder.finish()]);
    }
}
