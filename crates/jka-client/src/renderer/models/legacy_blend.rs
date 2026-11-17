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
/// Used by the mode-4 fallback transfer and the scene alias's MSAA resolve.
/// A failed proof deliberately returns an error (full-frame), never a guessed
/// rectangle.
/// This never changes the format or dimensions of the FP16 scene/encoded target.
pub(in crate::renderer) fn bounded_legacy_transfer_rect(
    surfaces: &[DynamicModelSurface], view_proj: Mat4, width: u32, height: u32,
) -> Result<LegacyTransferRect, &'static str> {
    if width == 0 || height == 0 { return Err("zero_extent"); }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut seen = false;
    let mut total_vertices = 0usize;
    for surface in surfaces.iter().filter(|s| eligible(s.alpha_mode)) {
        // RT-only casters never reach colour, so they cannot need a transfer.
        if !surface.raster_visible { continue; }
        // GPU FX sprites (saber blade glow/core, flares) never draw in the
        // encoded pass (draw_phase_inner skips them for legacy_effects), so
        // they need no transfer region.
        if surface.fx_gpu_sprites.is_some() { continue; }
        if let Some(skin) = surface.ghoul2_gpu.as_ref() {
            // Source vertices are bind-pose, NOT world positions: skin them.
            total_vertices = total_vertices.saturating_add(skin.vertices.len());
            if total_vertices > 24_000 { return Err("vertex_budget"); }
            let (lo, hi) = ghoul2_render_bounds(skin)?;
            // Entirely behind the camera: nothing of it can be rasterized.
            let Some((x0, y0, x1, y1)) = surface_screen_rect(lo, hi, view_proj, width, height)?
            else { continue; };
            min_x = min_x.min(x0);
            min_y = min_y.min(y0);
            max_x = max_x.max(x1);
            max_y = max_y.max(y1);
            seen = true;
            continue;
        }
        if surface.indices.is_empty() { continue; }
        if surface.vertices.is_empty() { return Err("no_cpu_vertices"); }
        total_vertices = total_vertices.saturating_add(surface.vertices.len());
        // Avoid spending a large CPU budget scanning a crowded character scene
        // just to save a pair of GPU fullscreen transfer draws.
        if total_vertices > 24_000 { return Err("vertex_budget"); }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for vertex in surface.vertices.iter() {
            // WGSL foliage_model_position changes vertex positions in the VS.
            // We cannot bound it from the undeformed source without its wind.
            if vertex.foliage_wind != 0.0 { return Err("foliage_wind"); }
            for axis in 0..3 {
                let v = vertex.position[axis];
                if !v.is_finite() { return Err("non_finite"); }
                lo[axis] = lo[axis].min(v);
                hi[axis] = hi[axis].max(v);
            }
        }
        let Some((x0, y0, x1, y1)) = surface_screen_rect(lo, hi, view_proj, width, height)?
        else { continue; };
        min_x = min_x.min(x0);
        min_y = min_y.min(y0);
        max_x = max_x.max(x1);
        max_y = max_y.max(y1);
        seen = true;
    }
    if !seen {
        // Every eligible surface was skipped (GPU sprites, RT-only, or
        // entirely behind the camera), so the encoded pass rasterizes nothing.
        return Ok((0, 0, 1, 1));
    }
    padded_transfer_rect((min_x, min_y, max_x, max_y), width, height).ok_or("near_fullscreen")
}

/// Screen bounds of a renderer-space AABB. Box edges are clipped at a tiny
/// positive w instead of rejecting a box that crosses the camera plane, so a large effect mesh around the viewer
/// (e.g. map light cones) still yields a conservative rectangle; points at that
/// plane project far off-screen and are clamped by `padded_transfer_rect`.
/// Ok(None): the box is entirely behind the camera and cannot draw.
fn surface_screen_rect(
    lo: [f32; 3], hi: [f32; 3], view_proj: Mat4, width: u32, height: u32,
) -> Result<Option<(f32, f32, f32, f32)>, &'static str> {
    const MIN_W: f32 = 1.0e-4;
    let corner = |i: usize| view_proj * glam::Vec4::new(
        if i & 1 == 0 { lo[0] } else { hi[0] },
        if i & 2 == 0 { lo[1] } else { hi[1] },
        if i & 4 == 0 { lo[2] } else { hi[2] },
        1.0,
    );
    let clip: [glam::Vec4; 8] = std::array::from_fn(corner);
    let (w, h) = (width as f32, height as f32);
    let mut rect = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut add = |c: glam::Vec4| {
        let x = (c.x / c.w * 0.5 + 0.5) * w;
        let y = (0.5 - c.y / c.w * 0.5) * h;
        rect = (rect.0.min(x), rect.1.min(y), rect.2.max(x), rect.3.max(y));
    };
    for (i, c) in clip.iter().enumerate() {
        if c.w > MIN_W { add(*c); }
        // Each box edge once: to the corner that differs in one higher bit.
        for bit in [1usize, 2, 4] {
            if i & bit != 0 { continue; }
            let d = clip[i | bit];
            if (c.w > MIN_W) != (d.w > MIN_W) {
                let t = (MIN_W - c.w) / (d.w - c.w);
                add(*c + (d - *c) * t);
            }
        }
    }
    if !rect.0.is_finite() { return Ok(None); }
    if ![rect.0, rect.1, rect.2, rect.3].iter().all(|v| v.is_finite()) { return Err("non_finite"); }
    Ok(Some(rect))
}

/// Conservative renderer-space AABB of a GPU-skinned Ghoul2 surface, from the
/// same skinning, jiggle, bulge and entity transform as `ghoul2_skin.wgsl`
/// vs_main. Only the few eligible additive shells (e.g. a saber hilt glow) are
/// evaluated here, under the caller's vertex budget.
fn ghoul2_render_bounds(
    skin: &crate::renderer::Ghoul2GpuSkinning,
) -> Result<([f32; 3], [f32; 3]), &'static str> {
    // Foliage wind moves vertices in the VS from a time-varying field.
    if skin.foliage_wind { return Err("foliage_wind"); }
    let transform = |bone: &crate::renderer::Ghoul2GpuBone, p: [f32; 3]| -> [f32; 3] {
        let row = |r: &[f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        [row(&bone.row0), row(&bone.row1), row(&bone.row2)]
    };
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for vertex in skin.vertices.iter() {
        let count = vertex.weight_count.clamp(1, 4) as usize;
        let mut points = [[0.0f32; 3]; 4];
        for (slot, point) in points.iter_mut().enumerate().take(count) {
            let bone = skin.bones.get(vertex.bone_indices[slot] as usize).ok_or("bone_index")?;
            *point = transform(bone, vertex.position);
        }
        let w = vertex.weights;
        let p: [f32; 3] = std::array::from_fn(|axis| match count {
            1 => points[0][axis],
            2 => w[0] * (points[0][axis] - points[1][axis]) + points[1][axis],
            3 => w[0] * points[0][axis] + w[1] * points[1][axis]
                + (1.0 - w[0] - w[1]) * points[2][axis],
            _ => w[0] * points[0][axis] + w[1] * points[1][axis] + w[2] * points[2][axis]
                + (1.0 - w[0] - w[1] - w[2]) * points[3][axis],
        });
        // Jiggle adds offset * effective weight, and the effective weight is
        // never above the stored weight.
        let pad = if vertex.jiggle_weight > 0.0 {
            skin.jiggle_offsets.get(vertex.jiggle_region as usize).map_or(0.0, |offset| {
                (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt()
                    * vertex.jiggle_weight.abs()
            })
        } else {
            0.0
        };
        for axis in 0..3 {
            if !p[axis].is_finite() || !pad.is_finite() { return Err("non_finite"); }
            lo[axis] = lo[axis].min(p[axis] - pad);
            hi[axis] = hi[axis].max(p[axis] + pad);
        }
    }
    if !lo[0].is_finite() { return Err("no_cpu_vertices"); }
    // Bulge pushes along the normalized model normal, before the entity axis.
    let bulge = skin.bulge_height.abs();
    let mut render_lo = [f32::INFINITY; 3];
    let mut render_hi = [f32::NEG_INFINITY; 3];
    for corner in 0..8 {
        let m = [
            if corner & 1 == 0 { lo[0] - bulge } else { hi[0] + bulge },
            if corner & 2 == 0 { lo[1] - bulge } else { hi[1] + bulge },
            if corner & 4 == 0 { lo[2] - bulge } else { hi[2] + bulge },
        ];
        let a = &skin.axis;
        let world: [f32; 3] = std::array::from_fn(|i|
            skin.origin[i] + a[0][i] * m[0] + a[1][i] * m[1] + a[2][i] * m[2]);
        // JKA -> renderer: [x, z, -y] (ghoul2_skin.wgsl jka_to_render).
        let render = [world[0], world[2], -world[1]];
        for axis in 0..3 {
            render_lo[axis] = render_lo[axis].min(render[axis]);
            render_hi[axis] = render_hi[axis].max(render[axis]);
        }
    }
    Ok((render_lo, render_hi))
}

/// Perf-trace diagnostic: one entry per legacy-blend-eligible surface.
pub(in crate::renderer) fn describe_eligible_surfaces(surfaces: &[DynamicModelSurface]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for surface in surfaces.iter().filter(|s| eligible(s.alpha_mode)) {
        let kind = if surface.ghoul2_gpu.is_some() {
            "g2gpu"
        } else if surface.fx_gpu_sprites.is_some() {
            "fxgpu"
        } else {
            "cpu"
        };
        let name = surface.ghoul2_gpu.as_ref().map(|skin| skin.mesh_key.to_string())
            .or_else(|| surface.material_trace.as_deref().map(str::to_owned))
            .unwrap_or_default();
        let texture = surface.texture.as_ref().map_or("-", |texture| texture.label.as_str());
        let _ = write!(out, " [ent={} {kind} {:?} visible={} verts={} mesh={} tex={}]",
            surface.entity_num, surface.alpha_mode, u8::from(surface.raster_visible),
            surface.vertex_count(), name.chars().take(96).collect::<String>(), texture);
    }
    out
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
    /// Model-path MSAA only: decode that averages the
    /// encoded samples itself, so a bounded transfer needs no full-frame
    /// hardware resolves. Pipelines: [0] MSAA scene target, [1] resolved scene.
    msaa_resolve_decode: Option<(wgpu::BindGroupLayout, [wgpu::RenderPipeline; 2])>,
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
            msaa_resolve_decode: (!per_sample && size.2 > 1)
                .then(|| Self::msaa_resolve_decode(device, size.2, size.3)),
        }
    }

    fn msaa_resolve_decode(
        device: &wgpu::Device, samples: u32, format: wgpu::TextureFormat,
    ) -> (wgpu::BindGroupLayout, [wgpu::RenderPipeline; 2]) {
        let mut image_entry = texture_entry(0);
        image_entry.ty = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: true,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("legacy model bounded resolve-decode"),
            entries: &[image_entry],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("legacy model bounded resolve-decode"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{}", include_str!("../../legacy_model_blend.wgsl"), TRANSFER_RESOLVE_DECODE).into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |count: u32| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("legacy model bounded resolve-decode"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("decode_resolve"),
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
        let pipelines = [pipeline(samples), pipeline(1)];
        (layout, pipelines)
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

    /// Scene-alias MSAA only: the scissored linear resolve pipeline, built on
    /// first use and rebuilt only when the scene format or sample count changes.
    pub(in crate::renderer) fn ensure_legacy_scene_resolve(
        &mut self, device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32,
    ) {
        if self.legacy_scene_resolve.as_ref().is_some_and(|(f, s, _, _)| *f == format && *s == samples) {
            return;
        }
        let mut image_entry = texture_entry(0);
        image_entry.ty = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: true,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("legacy model scene alias resolve"),
            entries: &[image_entry],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("legacy model scene alias resolve"),
            source: wgpu::ShaderSource::Wgsl(SCENE_RESOLVE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("legacy model scene alias resolve"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("resolve"),
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
        self.legacy_scene_resolve = Some((format, samples, layout, pipeline));
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
            // Scene alias: the same, into a UNORM view of the offscreen sRGB
            // scene (MSAA) target rather than the swapchain image.
            let scene_alias;
            let view = if self.legacy_scene_alias {
                let target = msaa.unwrap_or(scene);
                scene_alias = target.texture().create_view(&wgpu::TextureViewDescriptor {
                    label: Some("legacy model scene display-space alias"),
                    format: Some(target.texture().format().remove_srgb_suffix()),
                    ..Default::default()
                });
                &scene_alias
            } else {
                let Some(view) = native_unorm_view else { return; };
                view
            };
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
            drop(pass);
            // The alias pass never resolves (its attachment is the UNORM view).
            // Keep the resolved scene equal to resolve(MSAA): only pixels the
            // effects can touch changed, so a bounded rect resolves just those
            // with the same linear box filter; otherwise one full resolve.
            if let (true, Some(msaa_scene)) = (self.legacy_scene_alias, msaa) {
                match (transfer_rect, self.legacy_scene_resolve.as_ref()) {
                    (Some((x, y, w, h)), Some((_, _, layout, pipeline))) => {
                        let source = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("legacy model scene alias resolve source"),
                            layout,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(msaa_scene),
                            }],
                        });
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("legacy model scene alias bounded resolve"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: scene,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                            })],
                            ..Default::default()
                        });
                        pass.set_scissor_rect(x, y, w, h);
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &source, &[]);
                        pass.draw(0..3, 0..1);
                    }
                    _ => {
                        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("legacy model scene alias resolve"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: msaa_scene,
                                depth_slice: None,
                                resolve_target: Some(scene),
                                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                            })],
                            ..Default::default()
                        });
                    }
                }
            }
            return;
        }
        let blend = self.legacy_blend.as_ref().unwrap();
        // With MSAA and a bounded rect, skip both
        // full-frame hardware resolves. Inside the rect the decode averages the
        // encoded samples itself (the resolve's box filter) and writes the same
        // value to the MSAA scene and the resolved scene; outside it neither
        // target changes, and the scene already equals resolve(MSAA scene).
        let manual_resolve = match (transfer_rect, msaa,
            blend.msaa_view.as_ref(), blend.msaa_resolve_decode.as_ref())
        {
            (Some(rect), Some(msaa_scene), Some(encoded), Some((layout, pipelines))) =>
                Some((rect, msaa_scene, encoded, layout, pipelines)),
            _ => None,
        };
        let encode_source = blend.bind(device, scene);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("legacy model encoded blending"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: blend.msaa_view.as_ref().unwrap_or(&blend.view),
                    depth_slice: None,
                    resolve_target: if manual_resolve.is_some() {
                        None
                    } else {
                        blend.msaa_view.as_ref().map(|_| &blend.view)
                    },
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
        if let Some(((x, y, w, h), msaa_scene, encoded, layout, pipelines)) = manual_resolve {
            let source = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("legacy model bounded resolve-decode source"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(encoded),
                }],
            });
            for (target, pipeline) in [(msaa_scene, &pipelines[0]), (scene, &pipelines[1])] {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("legacy model bounded return to linear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_scissor_rect(x, y, w, h);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &source, &[]);
                pass.draw(0..3, 0..1);
            }
            return;
        }
        let decode_source = blend.bind(device, &blend.view);
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

// Scene-alias MSAA resolve: reading the sRGB MSAA view yields linear values,
// so averaging and writing through the sRGB scene view matches the hardware
// resolve of every other scene pass.
const SCENE_RESOLVE: &str = r#"
@group(0) @binding(0) var image: texture_multisampled_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    return vec4(p[i],0.0,1.0);
}
@fragment fn resolve(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(p.xy);
    let count = textureNumSamples(image);
    var sum = vec4<f32>(0.0);
    for (var s = 0u; s < count; s = s + 1u) {
        sum = sum + textureLoad(image, pixel, i32(s));
    }
    return sum / f32(count);
}
"#;

// Bounded MSAA model transfer: box-resolve the encoded samples (what the
// hardware resolve would produce), then decode. Written at pixel rate.
const TRANSFER_RESOLVE_DECODE: &str = r#"
@group(0) @binding(0) var image: texture_multisampled_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    return vec4(p[i],0.0,1.0);
}
@fragment fn decode_resolve(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(p.xy);
    let count = textureNumSamples(image);
    var sum = vec4<f32>(0.0);
    for (var s = 0u; s < count; s = s + 1u) {
        sum = sum + textureLoad(image, pixel, i32(s));
    }
    let c = sum / f32(count);
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
