//! Display-space compositing for legacy additive and destination-colour shells.
//! Encode the destination once, use the authored fixed-function equations for
//! every layer, then decode once. Sampling an old destination per fragment would
//! be wrong when shells overlap or have more than one shader stage.
use crate::renderer::{
    pipeline_hash, sampler_entry, texture_entry, DynamicModelAlphaMode as Mode,
    DynamicModelRenderer, PipelineJobKey, PipelineJobManager, DEPTH_FORMAT,
};

pub(in crate::renderer) fn eligible(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::AdditiveOne | Mode::DstColorAdd | Mode::SightShellNoDepth
    )
}

pub(in crate::renderer) struct LegacyModelBlend {
    size: (u32, u32, u32, wgpu::TextureFormat),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    msaa_texture: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    encode: wgpu::RenderPipeline,
    decode: wgpu::RenderPipeline,
}

impl LegacyModelBlend {
    pub(in crate::renderer) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

    fn new(device: &wgpu::Device, size: (u32, u32, u32, wgpu::TextureFormat)) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("legacy model colour transfer"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("legacy model colour transfer"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../../legacy_model_blend.wgsl"),
                    TRANSFER
                )
                .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &'static str, format: wgpu::TextureFormat, depth: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: Default::default(),
                depth_stencil: depth.then_some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: size.2,
                    ..Default::default()
                },
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
        let encode = pipeline("encode", Self::FORMAT, true);
        let decode = pipeline("decode", size.3, false);
        let make_texture = |samples| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("legacy model encoded scene"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: Self::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let texture = make_texture(1);
        let view = texture.create_view(&Default::default());
        let msaa_texture = (size.2 > 1).then(|| make_texture(size.2));
        let msaa_view = msaa_texture
            .as_ref()
            .map(|t| t.create_view(&Default::default()));
        Self {
            size,
            texture,
            view,
            msaa_texture,
            msaa_view,
            layout,
            sampler,
            encode,
            decode,
        }
    }

    fn bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("legacy model transfer source"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    pub(in crate::renderer) fn bytes(&self) -> u64 {
        crate::renderer::diagnostics::memory::texture_bytes(&self.texture)
            + self
                .msaa_texture
                .as_ref()
                .map_or(0, crate::renderer::diagnostics::memory::texture_bytes)
    }
}

impl DynamicModelRenderer {
    pub(in crate::renderer) fn legacy_blend_active(&self) -> bool {
        self.legacy_blend.is_some()
            && self
                .draws
                .iter()
                .filter(|d| eligible(d.alpha_mode))
                .all(|d| self.legacy_model_pipelines[d.alpha_mode.index()].is_some())
            && self
                .skin_draws
                .iter()
                .filter(|d| eligible(d.alpha_mode))
                .all(|d| self.legacy_skin_pipelines[d.alpha_mode.index()].is_some())
    }

    pub(in crate::renderer) fn prepare_legacy_blend(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        model_mask: u16,
        skin_mask: u16,
    ) {
        let samples = self.fx_sprite_samples;
        let config = pipeline_hash(&(samples, self.legacy_fog_compiled, self.model_wet_compiled));
        for mode in crate::renderer::DYNAMIC_MODEL_ALPHA_ORDER
            .into_iter()
            .filter(|m| eligible(*m))
        {
            for skin in [false, true] {
                let mask = if skin { skin_mask } else { model_mask };
                if mask & (1 << mode.index()) == 0 {
                    continue;
                }
                let slots = if skin {
                    &mut self.legacy_skin_pipelines
                } else {
                    &mut self.legacy_model_pipelines
                };
                if slots[mode.index()].is_some() {
                    continue;
                }
                let key = PipelineJobKey::new(
                    if skin { "legacy-skin" } else { "legacy-model" },
                    mode.index() as u32,
                    config,
                );
                if let Some(pipeline) = jobs.take_ready(key) {
                    slots[mode.index()] = Some(pipeline);
                    continue;
                }
                let device = device.clone();
                let layout = if skin {
                    self.skin_pipeline_layout.clone()
                } else {
                    self.pipeline_layout.clone()
                };
                let fog = self.legacy_fog_compiled;
                let wet = self.model_wet_compiled;
                jobs.request(key, "legacy model display-space blend", move || {
                    let source = crate::glass::model_shader(
                        if skin {
                            include_str!("../../ghoul2_skin.wgsl")
                        } else {
                            include_str!("../../md3.wgsl")
                        },
                        if skin { 3 } else { 2 },
                    )
                    .replace(
                        "const LEGACY_ENCODED_BLEND: bool = false;",
                        "const LEGACY_ENCODED_BLEND: bool = true;",
                    );
                    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("legacy model encoded source"),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    });
                    if skin {
                        super::pipelines::create_ghoul2_skin_pipeline(
                            &device,
                            &layout,
                            &shader,
                            LegacyModelBlend::FORMAT,
                            samples,
                            mode,
                            fog,
                            wet,
                            false,
                            false,
                        )
                    } else {
                        super::pipelines::create_dynamic_model_pipeline(
                            &device,
                            &layout,
                            &shader,
                            LegacyModelBlend::FORMAT,
                            samples,
                            mode,
                            fog,
                            wet,
                        )
                    }
                });
            }
        }
    }

    pub(in crate::renderer) fn resize_legacy_blend(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        samples: u32,
        format: wgpu::TextureFormat,
    ) {
        let needed = self.draws.iter().any(|d| eligible(d.alpha_mode))
            || self.skin_draws.iter().any(|d| eligible(d.alpha_mode));
        if !needed {
            self.legacy_blend = None;
            return;
        }
        let size = (width, height, samples, format);
        if self.legacy_blend.as_ref().is_none_or(|b| b.size != size) {
            self.legacy_blend = Some(LegacyModelBlend::new(device, size));
        }
    }

    pub(in crate::renderer) fn composite_legacy_blend(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        msaa: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        camera: &wgpu::BindGroup,
    ) {
        if !self.legacy_blend_active() {
            return;
        }
        let blend = self.legacy_blend.as_ref().unwrap();
        let encode_source = blend.bind(device, scene);
        let decode_source = blend.bind(device, &blend.view);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("legacy model encoded blending"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: blend.msaa_view.as_ref().unwrap_or(&blend.view),
                    depth_slice: None,
                    resolve_target: blend.msaa_view.as_ref().map(|_| &blend.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&blend.encode);
            pass.set_bind_group(0, &encode_source, &[]);
            pass.draw(0..3, 0..1);
            self.draw_encoded_legacy_models(&mut pass, camera);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("legacy model return to linear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: msaa.unwrap_or(scene),
                depth_slice: None,
                resolve_target: msaa.map(|_| scene),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&blend.decode);
        pass.set_bind_group(0, &decode_source, &[]);
        pass.draw(0..3, 0..1);
    }
}

const TRANSFER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    return vec4(p[i],0.0,1.0);
}
@fragment fn encode(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(image,vec2<i32>(p.xy),0);
    return vec4(legacy_effect_encode(c.rgb),c.a);
}
@fragment fn decode(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(image,vec2<i32>(p.xy),0);
    return vec4(legacy_effect_decode(c.rgb),c.a);
}
"#;

#[cfg(test)]
#[path = "legacy_blend_tests.rs"]
mod tests;
