//! Heat haze behind Ninja surface flames (r_ninjaFireHeatHaze).
//!
//! Runs on the HDR scene before post, over the flame quads (extended above
//! the flames) only: a gather pass writes the displaced scene and its mask
//! into a render-size scratch, then an apply pass blends it back. The flame
//! composite is deferred until after these passes, so only the background is
//! refracted, including behind the translucent flame. The scratch exists
//! only while the haze is enabled; nothing runs while no surface flame is
//! drawn.
use bytemuck::{Pod, Zeroable};

use super::{FlameGpu, resolve_scopes};

/// Curl-noise tiles per second the shimmer rises.
const RISE_TILES_PER_SECOND: f32 = 1.0;
const STRENGTH: f32 = 1.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HazeUniform {
    params: [f32; 4],
    inv_size: [f32; 4],
}

pub(super) struct HazePipelines {
    scene_format: wgpu::TextureFormat,
    gather: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
}

struct HazeGroups {
    scene: wgpu::TextureView,
    layer: wgpu::TextureView,
    scratch: wgpu::TextureView,
    gather: wgpu::BindGroup,
    apply: wgpu::BindGroup,
}

pub(super) struct HeatHaze {
    pub(super) layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    repeat: wgpu::Sampler,
    clamp: wgpu::Sampler,
    /// Gather target at the flame layer's (render) size.
    scratch: Option<(wgpu::TextureView, (u32, u32))>,
    groups: Option<HazeGroups>,
}

impl HeatHaze {
    pub(super) fn new(device: &wgpu::Device) -> Result<Self, String> {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let texture = |view_dimension| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension,
            multisampled: false,
        };
        let filtering = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja heat haze"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<HazeUniform>() as u64),
                }),
                entry(1, texture(wgpu::TextureViewDimension::D2)),
                entry(2, texture(wgpu::TextureViewDimension::D3)),
                entry(3, filtering),
                entry(4, filtering),
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ninja heat haze"),
            size: std::mem::size_of::<HazeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let linear = |address_mode| wgpu::SamplerDescriptor {
            label: Some("Ninja heat haze"),
            address_mode_u: address_mode,
            address_mode_v: address_mode,
            address_mode_w: address_mode,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        };
        let repeat = device.create_sampler(&linear(wgpu::AddressMode::Repeat));
        let clamp = device.create_sampler(&linear(wgpu::AddressMode::ClampToEdge));
        resolve_scopes(memory, validation)?;
        Ok(Self { layout, uniform, repeat, clamp, scratch: None, groups: None })
    }

    /// Writes this frame's uniform and (re)binds the scene and layer views.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &wgpu::TextureView,
        scene_size: [u32; 2],
        layer: &wgpu::TextureView,
        layer_size: (u32, u32),
        curl: &wgpu::TextureView,
        curl_inv_rms: f32,
        time: f32,
    ) {
        let uniform = HazeUniform {
            params: [time, STRENGTH, curl_inv_rms, RISE_TILES_PER_SECOND],
            inv_size: [
                1.0 / layer_size.0.max(1) as f32,
                1.0 / layer_size.1.max(1) as f32,
                1.0 / scene_size[0].max(1) as f32,
                1.0 / scene_size[1].max(1) as f32,
            ],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        if self.scratch.as_ref().is_none_or(|(_, size)| *size != layer_size) {
            let scratch = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Ninja heat haze scratch"),
                size: wgpu::Extent3d { width: layer_size.0.max(1), height: layer_size.1.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            }).create_view(&Default::default());
            self.scratch = Some((scratch, layer_size));
        }
        let scratch = &self.scratch.as_ref().expect("heat haze scratch").0;
        if self.groups.as_ref().is_some_and(|groups| {
            &groups.scene == scene && &groups.layer == layer && &groups.scratch == scratch
        }) {
            return;
        }
        let group = |label, source: &wgpu::TextureView| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                super::texture(1, source),
                super::texture(2, curl),
                super::sampler(3, &self.repeat),
                super::sampler(4, &self.clamp),
            ],
        });
        let groups = HazeGroups {
            scene: scene.clone(),
            layer: layer.clone(),
            scratch: scratch.clone(),
            gather: group("Ninja heat haze gather", scene),
            apply: group("Ninja heat haze apply", scratch),
        };
        self.groups = Some(groups);
    }

    /// Gather into the scratch, then blend onto `scene`. `fields` yields each
    /// surface preset's pipelines and the flame bind groups of its fields.
    pub(super) fn encode<'a>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        fields: impl Iterator<Item = (&'a HazePipelines, &'a wgpu::BindGroup)> + Clone,
    ) {
        let Some(groups) = self.groups.as_ref() else { return };
        for (target, apply) in [(&groups.scratch, false), (scene, true)] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if apply { "Ninja heat haze apply" } else { "Ninja heat haze gather" }),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if apply { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_bind_group(2, if apply { &groups.apply } else { &groups.gather }, &[]);
            let mut bound: Option<&HazePipelines> = None;
            for (pipelines, flame) in fields.clone() {
                if !bound.is_some_and(|bound| std::ptr::eq(bound, pipelines)) {
                    pass.set_pipeline(if apply { &pipelines.apply } else { &pipelines.gather });
                    bound = Some(pipelines);
                }
                pass.set_bind_group(0, flame, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }
}

impl FlameGpu {
    pub(super) fn ensure_haze_pipelines(
        &mut self,
        device: &wgpu::Device,
        haze_layout: &wgpu::BindGroupLayout,
        scene_format: wgpu::TextureFormat,
    ) -> Result<(), String> {
        if self.haze.as_ref().is_some_and(|haze| haze.scene_format == scene_format) {
            return Ok(());
        }
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja heat haze"),
            bind_group_layouts: &[Some(&self.layout), None, Some(haze_layout)],
            immediate_size: 0,
        });
        let pipeline = |label, fragment, format, blend| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("haze_vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let gather = pipeline("Ninja heat haze gather", "haze_gather", wgpu::TextureFormat::Rgba16Float, None);
        let apply = pipeline("Ninja heat haze apply", "haze_apply", scene_format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            // Leave the scene's alpha as it was.
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }));
        resolve_scopes(memory, validation)?;
        self.haze = Some(HazePipelines { scene_format, gather, apply });
        Ok(())
    }
}

/// 1 / RMS of the curl field's xy lanes, so the shader sees unit-RMS offsets.
pub(super) fn curl_inv_rms(f32x3: &[u8]) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for xyz in f32x3.chunks_exact(12) {
        let x = f32::from_le_bytes(xyz[0..4].try_into().expect("x lane"));
        let y = f32::from_le_bytes(xyz[4..8].try_into().expect("y lane"));
        if x.is_finite() && y.is_finite() {
            sum += f64::from(x * x + y * y) * 0.5;
            count += 1;
        }
    }
    let rms = (sum / count.max(1) as f64).sqrt() as f32;
    if rms > 1e-6 { rms.recip() } else { 0.0 }
}
