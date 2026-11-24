use super::{FlameTarget, NinjaFireState, demo_bloom, resolve_scopes};

pub(super) struct LocalPost {
    key: (FlameTarget, (u32, u32)),
    layer: wgpu::TextureView,
    bloom: demo_bloom::DemoBloom,
    composite: wgpu::RenderPipeline,
    inputs: wgpu::BindGroup,
}

impl LocalPost {
    pub(super) fn matches(&self, target: FlameTarget, size: (u32, u32)) -> bool {
        self.key == (target, size)
    }

    pub(super) fn new(device: &wgpu::Device, target: FlameTarget, size: (u32, u32)) -> Result<Self, String> {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let layer = demo_bloom::texture(device, [size.0, size.1]);
        let glow = demo_bloom::texture(device, [size.0.div_ceil(2), size.1.div_ceil(2)]);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fire Smoke local composite"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}\n{}",
                include_str!("demo_bloom.wgsl"), jka_ninja::flame_demo_film_wgsl(),
                "@fragment fn composite(input: Vertex) -> @location(0) vec4<f32> {
                    let fire = textureLoad(source_image, vec2<i32>(input.position.xy), 0);
                    let bloom = textureSampleLevel(coarser_bloom, bilinear, input.uv, 0.0).rgb;
                    let color = fire.rgb + bloom;
                    if (fire.a == 0.0 && all(color == vec3<f32>(0.0))) { discard; }
                    return vec4<f32>(ninja_demo_film(color), fire.a);
                }",
            ).into()),
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fire Smoke local composite"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some("fullscreen"), buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("composite"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.0, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: target.2.map(|format| wgpu::DepthStencilState {
                format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(), bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: target.1, ..Default::default() },
            multiview_mask: None, cache: None,
        });
        resolve_scopes(memory, validation)?;

        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default()
        });
        let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fire Smoke local composite"),
            layout: &composite.get_bind_group_layout(0),
            entries: &[
                demo_bloom::texture_entry(0, &layer), demo_bloom::texture_entry(1, &glow),
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        resolve_scopes(memory, validation)?;
        let bloom = demo_bloom::DemoBloom::new(device, &layer, &glow, size)?;
        Ok(Self { key: (target, size), layer, bloom, composite, inputs })
    }

    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder, fire: &NinjaFireState) -> bool {
        let drawn = {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fire Smoke local HDR layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.layer, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            fire.draw_fields(&mut pass)
        };
        if drawn {
            self.bloom.encode(encoder);
        }
        drawn
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &self.inputs, &[]);
        pass.draw(0..3, 0..1);
    }
}
