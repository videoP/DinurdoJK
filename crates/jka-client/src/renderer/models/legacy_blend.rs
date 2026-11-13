//! Display-space compositing for legacy additive and destination-colour shells.
//! Encode the destination once, use the authored fixed-function equations for
//! every layer, then decode once. Sampling an old destination per fragment would
//! be wrong when shells overlap or have more than one shader stage.
use crate::renderer::{
    pipeline_hash, sampler_entry, texture_entry, DynamicModelAlphaMode as Mode,
    DynamicModelRenderer, DynamicModelSurface, Mat4, PipelineJobKey, PipelineJobManager, depth_format,
};

pub(in crate::renderer) fn eligible(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::AdditiveOne | Mode::DstColorAdd | Mode::SightShellNoDepth
    )
}

/// Rectangle shared by the encoded-scene draw and the return-to-linear draw.
/// Both operate in absolute framebuffer coordinates (`textureLoad`), so an
/// ordinary scissor preserves their original samples and blend equations.
pub(in crate::renderer) type LegacyTransferRect = (u32, u32, u32, u32);

/// Conservative CPU-world-geometry bounds for legacy HDR/offscreen blending.
///
/// Only the fallback from requested mode 4 uses this. Explicit mode 0 stays
/// completely full-frame as an image-correctness reference. A failed proof
/// deliberately returns None (full-frame), never a guessed rectangle.
/// This never changes the format or dimensions of the FP16 scene/encoded target.
pub(in crate::renderer) fn bounded_legacy_transfer_rect(
    surfaces: &[DynamicModelSurface], view_proj: Mat4, width: u32, height: u32,
) -> Option<LegacyTransferRect> {
    if width == 0 || height == 0 { return None; }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut seen = false;
    let mut total_vertices = 0usize;
    for surface in surfaces.iter().filter(|s| eligible(s.alpha_mode)) {
        // Eligible GPU-skinned shells can move anywhere within their pose;
        // source vertices/bind-pose bounds are NOT world-space bounds. FX
        // instance/partial blob buffers likewise differ from source vertices.
        if surface.ghoul2_gpu.is_some() || surface.fx_gpu_sprites.is_some() {
            return None;
        }
        if surface.indices.is_empty() { continue; }
        if surface.vertices.is_empty() { return None; }
        total_vertices = total_vertices.saturating_add(surface.vertices.len());
        // Avoid spending a large CPU budget scanning a crowded character scene
        // just to save a pair of GPU fullscreen transfer draws.
        if total_vertices > 24_000 { return None; }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for vertex in surface.vertices.iter() {
            // WGSL foliage_model_position changes vertex positions in the VS.
            // We cannot bound it from the undeformed source without its wind.
            if vertex.foliage_wind != 0.0 { return None; }
            for axis in 0..3 {
                let v = vertex.position[axis];
                if !v.is_finite() { return None; }
                lo[axis] = lo[axis].min(v);
                hi[axis] = hi[axis].max(v);
            }
        }
        let (x0, y0, x1, y1) =
            crate::renderer::aabb_screen_rect(lo, hi, view_proj, width, height)?;
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) { return None; }
        min_x = min_x.min(x0);
        min_y = min_y.min(y0);
        max_x = max_x.max(x1);
        max_y = max_y.max(y1);
        seen = true;
    }
    if !seen { return None; }
    padded_transfer_rect((min_x, min_y, max_x, max_y), width, height)
}

/// Extra pixels cover raster precision and the small projection/vertex math
/// differences between the CPU and WGSL. If the union is nearly fullscreen,
/// avoid the extra CPU/encoder state: use the original unscissored path.
fn padded_transfer_rect(
    (x0, y0, x1, y1): (f32, f32, f32, f32), width: u32, height: u32,
) -> Option<LegacyTransferRect> {
    if width == 0 || height == 0 || ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let x0 = (x0.floor() - 4.0).clamp(0.0, width as f32) as u32;
    let y0 = (y0.floor() - 4.0).clamp(0.0, height as f32) as u32;
    let x1 = (x1.ceil() + 4.0).clamp(0.0, width as f32) as u32;
    let y1 = (y1.ceil() + 4.0).clamp(0.0, height as f32) as u32;
    if x1 <= x0 || y1 <= y0 { return None; }
    let area = u64::from(x1 - x0) * u64::from(y1 - y0);
    let full = u64::from(width) * u64::from(height);
    if area.saturating_mul(10) >= full.saturating_mul(9) { return None; }
    Some((x0, y0, x1 - x0, y1 - y0))
}

pub(in crate::renderer) struct LegacyModelBlend {
    size: (u32, u32, u32, wgpu::TextureFormat, wgpu::TextureFormat),
    /// Single-sample, except per-sample mode where it is the MSAA target.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    msaa_texture: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    encode: wgpu::RenderPipeline,
    decode: wgpu::RenderPipeline,
    per_sample: bool,
    /// Cached world transfer sources: encode keyed by its scene view, decode
    /// always this object's own target. Rebuilt when the scene view changes.
    world_encode_source: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    world_decode_source: Option<wgpu::BindGroup>,
}

impl LegacyModelBlend {
    pub(in crate::renderer) fn resize_world(
        slot: &mut Option<Self>, device: &wgpu::Device, width: u32, height: u32,
        samples: u32, format: wgpu::TextureFormat, needed: bool,
    ) {
        if !needed { *slot = None; return; }
        let size = (width, height, samples, format, wgpu::TextureFormat::Rgba16Float);
        if slot.as_ref().is_none_or(|blend| blend.size != size) {
            *slot = Some(Self::new_with_samples(device, size, samples > 1));
        }
    }

    pub(in crate::renderer) fn color_view(&self) -> &wgpu::TextureView {
        self.msaa_view.as_ref().unwrap_or(&self.view)
    }

    pub(in crate::renderer) fn encode_world(
        &self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView,
    ) {
        let fresh;
        let source = match &self.world_encode_source {
            Some((view, group)) if view == scene => group,
            _ => { fresh = self.bind(device, scene); &fresh }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA world encode destination"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.color_view(), depth_slice: None,
                // Never resolved: with MSAA, decode reads the multisampled
                // target and the following normal world pass resolves.
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.encode);
        pass.set_bind_group(0, source, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(in crate::renderer) fn decode_world(
        &self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
    ) {
        let fresh;
        let source = match &self.world_decode_source {
            Some(group) => group,
            None => {
                fresh = self.bind(device, if self.per_sample { self.color_view() } else { &self.view });
                &fresh
            }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA world return to linear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color, depth_slice: None,
                // The normal world pass resumed right after decode resolves.
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.decode);
        pass.set_bind_group(0, source, &[]);
        pass.draw(0..3, 0..1);
    }

    // SDR output can still have a 16-bit FLOAT encoded intermediate. Mode 1
    // intentionally tests quantization + bandwidth without dropping transfers.
    pub(in crate::renderer) fn intermediate_format(mode: u8) -> wgpu::TextureFormat {
        if mode == 1 { wgpu::TextureFormat::Rgba8Unorm }
        else { wgpu::TextureFormat::Rgba16Float }
    }

    fn new(device: &wgpu::Device, size: (u32, u32, u32, wgpu::TextureFormat, wgpu::TextureFormat)) -> Self {
        Self::new_with_samples(device, size, false)
    }

    fn new_with_samples(device: &wgpu::Device, size: (u32, u32, u32, wgpu::TextureFormat, wgpu::TextureFormat), per_sample: bool) -> Self {
        let mut image_entry = texture_entry(0);
        if per_sample {
            image_entry.ty = wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2, multisampled: true,
            };
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("legacy model colour transfer"),
            entries: &[image_entry, sampler_entry(1)],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("legacy model colour transfer"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../../legacy_model_blend.wgsl"),
                    if per_sample { TRANSFER_MSAA } else { TRANSFER }
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
                    format: depth_format(),
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
        let encode = pipeline("encode", size.4, true);
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
                format: size.4,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        // Per-sample (MSAA world blending) reads the multisampled target
        // directly and never resolves, so it is the only texture. Otherwise an
        // MSAA attachment resolves into the single-sample `texture`.
        let texture = make_texture(if per_sample { size.2 } else { 1 });
        let view = texture.create_view(&Default::default());
        let msaa_texture = (!per_sample && size.2 > 1).then(|| make_texture(size.2));
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
            per_sample,
            world_encode_source: None,
            world_decode_source: None,
        }
    }

    /// Outside any render pass: (re)build the cached world transfer bind
    /// groups. `scene` is the view `encode_world` will receive.
    pub(in crate::renderer) fn prepare_world_sources(
        &mut self, device: &wgpu::Device, scene: Option<&wgpu::TextureView>,
    ) {
        match scene {
            Some(scene) if self.world_encode_source.as_ref().is_none_or(|(view, _)| view != scene) => {
                self.world_encode_source = Some((scene.clone(), self.bind(device, scene)));
            }
            None => self.world_encode_source = None,
            _ => {}
        }
        if self.world_decode_source.is_none() {
            let source = if self.per_sample { self.color_view() } else { &self.view };
            self.world_decode_source = Some(self.bind(device, source));
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
        (self.legacy_blend_mode == 4 ||
            (self.legacy_blend_mode <= 1 && self.legacy_blend.is_some()))
            && (self.draws.iter().any(|d| eligible(d.alpha_mode))
                || self.skin_draws.iter().any(|d| eligible(d.alpha_mode)))
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
        if self.legacy_blend_mode > 1 && self.legacy_blend_mode != 4 { return; }
        // Mode 4 draws encoded model colors into a UNORM alias of the SDR
        // surface, whose ordinary scene view is sRGB.
        let format = if self.legacy_blend_mode == 4 {
            self.fx_sprite_surface_format.remove_srgb_suffix()
        } else {
            LegacyModelBlend::intermediate_format(self.legacy_blend_mode)
        };
        if format != self.legacy_blend_pipeline_format {
            self.legacy_model_pipelines.fill(None);
            self.legacy_skin_pipelines.fill(None);
            self.legacy_blend_pipeline_format = format;
        }
        let samples = self.fx_sprite_samples;
        let config = pipeline_hash(&(format, samples, self.legacy_fog_compiled, self.model_wet_compiled));
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
                let format = format;
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
                            format,
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
                            format,
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
        let needed = self.legacy_blend_mode <= 1
            && (self.draws.iter().any(|d| eligible(d.alpha_mode))
                || self.skin_draws.iter().any(|d| eligible(d.alpha_mode)));
        if !needed {
            self.legacy_blend = None;
            return;
        }
        let size = (width, height, samples, format,
            LegacyModelBlend::intermediate_format(self.legacy_blend_mode));
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
        native_unorm_view: Option<&wgpu::TextureView>,
        transfer_rect: Option<LegacyTransferRect>,
    ) {
        if !self.legacy_blend_active() {
            return;
        }
        if self.legacy_blend_mode == 4 {
            // Direct SDR route: no offscreen HDR texture, no scene sampling,
            // no fullscreen encode/decode and no forced post resolve. Only the
            // eligible geometry is redrawn into the encoded UNORM attachment.
            let Some(view) = native_unorm_view else { return; };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("legacy model direct display-space blend (UNORM alias)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
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
            self.draw_encoded_legacy_models(&mut pass, camera);
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
                        // In the bounded case the encode triangle overwrites
                        // every pixel subsequently decoded; a full-image
                        // clear would negate much of the bandwidth saving.
                        load: if transfer_rect.is_some() {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        },
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
            if let Some((x, y, w, h)) = transfer_rect {
                pass.set_scissor_rect(x, y, w, h);
            }
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
        if let Some((x, y, w, h)) = transfer_rect {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.set_pipeline(&blend.decode);
        pass.set_bind_group(0, &decode_source, &[]);
        pass.draw(0..3, 0..1);
    }
}

// Preserve per-sample color/depth correspondence across world stage chains.
// Resolving before a transfer would mix uncovered samples into subsequent
// depth-equal/alpha-tested draws, leaving seams along ordinary BSP triangles.
const TRANSFER_MSAA: &str = r#"
@group(0) @binding(0) var image: texture_multisampled_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    return vec4(p[i],0.0,1.0);
}
@fragment fn encode(@builtin(position) p:vec4<f32>, @builtin(sample_index) sample:u32) -> @location(0) vec4<f32> {
    let c = textureLoad(image,vec2<i32>(p.xy),i32(sample));
    return vec4(legacy_effect_encode(c.rgb),c.a);
}
@fragment fn decode(@builtin(position) p:vec4<f32>, @builtin(sample_index) sample:u32) -> @location(0) vec4<f32> {
    let c = textureLoad(image,vec2<i32>(p.xy),i32(sample));
    return vec4(legacy_effect_decode(c.rgb),c.a);
}
"#;

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

#[cfg(test)]
mod transfer_rect_tests {
    use super::padded_transfer_rect;

    #[test]
    fn rect_is_padded_and_clamped_to_attachment() {
        assert_eq!(padded_transfer_rect((10.4, 20.1, 80.2, 45.8), 200, 100),
            Some((6, 16, 79, 34)));
        assert_eq!(padded_transfer_rect((-10.0, 90.0, 40.0, 150.0), 200, 100),
            Some((0, 86, 44, 14)));
    }

    #[test]
    fn unbounded_and_fullscreen_rects_use_reference_path() {
        assert_eq!(padded_transfer_rect((0.0, 0.0, 1920.0, 1080.0), 1920, 1080), None);
        assert_eq!(padded_transfer_rect((10.0, 10.0, 10.0, 10.0), 0, 100), None);
        assert_eq!(padded_transfer_rect((f32::NAN, 0.0, 20.0, 20.0), 200, 100), None);
        assert_eq!(padded_transfer_rect((-100.0, -100.0, -10.0, -10.0), 200, 100), None);
    }
}
