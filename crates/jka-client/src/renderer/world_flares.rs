//! RBSP MST_FLARE authored-candidate pipeline; completely separate from saber
//! overlays. A single compute dispatch tests each candidate's CENTER against
//! this frame's reversed-Z scene depth, then one instanced quad pass draws the
//! visible candidates. No CPU/GPU visibility readback or per-flare draw calls.
//!
//! Material stages reuse the map material interpreter, with GPU textures and
//! per-shader instanced groups. Visibility remains independent of the shader.
use super::{camera_depth_compare, gpu_blend_factor, scene, Camera, GpuImage, PlanarReflectionView, Pod, Zeroable};
use crate::materials::{self, BlendFactor, BlendFunc, StageTexture};
use jka_assets::shader::{AlphaGen, RgbGen, TcMod};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;
use std::time::Instant;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlareCamera {
    view_proj: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
    eye: [f32; 4],
    viewport_time: [f32; 4],
    frame: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlareInstance {
    origin_radius: [f32; 4],
    color: [f32; 4],
    normal: [f32; 4], // .w is stable world-flare candidate index
    fog: [f32; 4], // local authored fog RGB + depth
}

// One tiny immutable uniform per unique authored flare stage (not per flare).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlareStageUniform {
    rgb: [f32; 4], // rgbGen values and selector
    alpha: [f32; 4], // opacity, cutoff, alphaGen selector, procedural fallback
    // Up to 4 authored tcMods (type, x, y, z + one extra value).
    tc: [[f32; 4]; 4],
    tc_extra: [[f32; 4]; 4],
    fade_control: [f32; 4], // rgb multiplier, alpha multiplier, portal range, unused
    rgb_wave: [f32; 4], // base, amplitude, phase, frequency
    alpha_wave: [f32; 4],
    wave_funcs: [f32; 4], // rgb func, alpha func, unused, unused
}

struct FlareStageGpu {
    blend: Option<BlendFunc>,
    bind_group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
}

fn stage_color_mode(gen: RgbGen) -> f32 {
    match gen {
        RgbGen::Identity | RgbGen::IdentityLighting => 0.0,
        // Flare primitives are emitted as world geometry. TaystJK's world
        // lightingDiffuse is black; alpha is still independently generated.
        RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity => 5.0,
        RgbGen::Vertex | RgbGen::ExactVertex => 1.0,
        RgbGen::OneMinusVertex => 2.0,
        RgbGen::Const => 3.0,
        RgbGen::Wave(_) => 4.0,
    }
}

fn stage_uniform(stage: &materials::MaterialStage, fallback: bool, portal_alpha: Option<f32>) -> FlareStageUniform {
    let blend = stage.blend.unwrap_or(FLARE_OPAQUE_BLEND);
    // Source-alpha stage: its color already fades when opacity falls. A second
    // RGB multiplier would square the fade. Other blends need RGB fading.
    let src_uses_alpha = blend.src == BlendFactor::SrcAlpha;
    let dst_uses_alpha = matches!(blend.dst, BlendFactor::SrcAlpha | BlendFactor::OneMinusSrcAlpha);
    let mut result = FlareStageUniform {
        rgb: [stage.color[0], stage.color[1], stage.color[2], stage_color_mode(stage.rgb_gen)],
        alpha: [stage.opacity, stage.alpha_cutoff,
            if portal_alpha.is_some() { 3.0 } else { match stage.alpha_gen {
                AlphaGen::OneMinusVertex => 2.0,
                AlphaGen::Vertex => 1.0,
                AlphaGen::Wave(_) => 4.0,
                _ => 0.0,
            } }, if fallback { 1.0 } else { 0.0 }],
        tc: [[0.0; 4]; 4], tc_extra: [[0.0; 4]; 4],
        fade_control: [if src_uses_alpha { 0.0 } else { 1.0 },
                       if src_uses_alpha || dst_uses_alpha { 1.0 } else { 0.0 },
                       portal_alpha.unwrap_or(1.0), 0.0],
        rgb_wave: [0.0; 4], alpha_wave: [0.0; 4], wave_funcs: [0.0; 4],
    };
    if let RgbGen::Wave(wave) = stage.rgb_gen {
        result.rgb_wave = [wave.base, wave.amplitude, wave.phase, wave.frequency];
        result.wave_funcs[0] = wave.func.id() as f32;
    }
    if let AlphaGen::Wave(wave) = stage.alpha_gen {
        result.alpha_wave = [wave.base, wave.amplitude, wave.phase, wave.frequency];
        result.wave_funcs[1] = wave.func.id() as f32;
    }
    for (slot, modification) in stage.tc_mods.iter().take(4).enumerate() {
        result.tc[slot] = match *modification {
            TcMod::Scroll(x, y) => [1.0, x, y, 0.0],
            TcMod::Scale(x, y) => [2.0, x, y, 0.0],
            TcMod::Rotate(degrees_per_second) => [3.0, degrees_per_second, 0.0, 0.0],
            TcMod::Transform(m) => {
                // Preserve all six transform components for every authored mod.
                result.tc_extra[slot] = [m[2], m[3], m[4], m[5]];
                [4.0, m[0], m[1], 0.0]
            }
            TcMod::Turb { base, amplitude, phase, frequency } => {
                result.tc_extra[slot] = [base, amplitude, phase, frequency];
                [5.0, 0.0, 0.0, 0.0]
            }
            TcMod::Stretch(wave) => {
                result.tc_extra[slot] = [wave.phase, wave.frequency, 0.0, 0.0];
                [6.0, wave.base, wave.amplitude, wave.func.id() as f32]
            }
        };
    }
    result
}

const DEFAULT_FLARE_BLEND: BlendFunc = BlendFunc {
    src: BlendFactor::SrcAlpha, dst: BlendFactor::One,
};
const FLARE_OPAQUE_BLEND: BlendFunc = BlendFunc {
    src: BlendFactor::SrcAlpha, dst: BlendFactor::OneMinusSrcAlpha,
};

// Exactly matches WGSL FlareState: 16 bytes.
const FLARE_STATE_BYTES: u64 = 16;

#[derive(Clone, Copy)]
struct FlareView {
    view_proj: glam::Mat4,
    eye: glam::Vec3,
    forward: glam::Vec3,
    right: glam::Vec3,
    up: glam::Vec3,
    clip_plane: Option<[f32; 4]>,
    cluster: Option<usize>,
}

pub(super) struct WorldFlareRenderer {
    candidates: Vec<scene::WorldFlare>,
    scratch: Vec<FlareInstance>,
    // Indexed by BSP shader, then authored stage order.
    stages: BTreeMap<usize, Vec<FlareStageGpu>>,
    stage_layout: Option<wgpu::BindGroupLayout>,
    stage_pipeline: BTreeMap<Option<BlendFunc>, wgpu::RenderPipeline>,
    stage_pipeline_format: Option<(wgpu::TextureFormat, u32)>,
    last_camera: Option<(glam::Vec3, glam::Vec3)>,
    uniform: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    states: Option<wgpu::Buffer>,
    draw_bind_group: Option<wgpu::BindGroup>,
    compute_bind_group: Option<wgpu::BindGroup>,
    draw_layout: Option<wgpu::BindGroupLayout>,
    compute_layout: Option<(u32, wgpu::BindGroupLayout)>,
    compute_pipeline: Option<(u32, wgpu::ComputePipeline)>,
    clock: Instant,
    frame_id: u32,
    pub(super) last_candidates: usize,
    pub(super) last_pvs_rejected: usize,
    pub(super) last_frustum_rejected: usize,
    /// Number of GPU center-occlusion candidates (NOT CPU-known visible flares).
    pub(super) last_drawn: usize,
    pub(super) last_dispatches: u32,
    pub(super) last_cpu_ms: f64,
    pub(super) last_stage_draws: usize,
    pub(super) last_filter_ms: f64,
    pub(super) last_upload_ms: f64,
    pub(super) last_probe_encode_ms: f64,
    pub(super) last_draw_encode_ms: f64,
}

impl Default for WorldFlareRenderer {
    fn default() -> Self {
        Self {
            candidates: Vec::new(), scratch: Vec::new(), stages: BTreeMap::new(),
            stage_layout: None, stage_pipeline: BTreeMap::new(),
            stage_pipeline_format: None, last_camera: None, uniform: None,
            instances: None, states: None, draw_bind_group: None,
            compute_bind_group: None, draw_layout: None,
            compute_layout: None, compute_pipeline: None,
            clock: Instant::now(), frame_id: 0,
            last_candidates: 0, last_pvs_rejected: 0,
            last_frustum_rejected: 0, last_drawn: 0,
            last_dispatches: 0, last_cpu_ms: 0.0, last_stage_draws: 0,
            last_filter_ms: 0.0, last_upload_ms: 0.0, last_probe_encode_ms: 0.0,
            last_draw_encode_ms: 0.0,
        }
    }
}

// Keep the bind-group stage masks together with the matching WGSL declarations.
// `camera` is used by both vs() and fs() (tcMods, rgb/alpha waves and fog).
// A VERTEX-only binding compiles the Rust code but wgpu rejects the pipeline
// as soon as r_worldFlares is activated.
fn flare_draw_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        buffer_layout_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT, true, true),
        buffer_layout_entry(1, wgpu::ShaderStages::VERTEX, true, false),
        buffer_layout_entry(2, wgpu::ShaderStages::VERTEX, true, false),
    ]
}

fn buffer_layout_entry(binding: u32, stages: wgpu::ShaderStages, read_only: bool, uniform: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility: stages,
        ty: wgpu::BindingType::Buffer {
            ty: if uniform { wgpu::BufferBindingType::Uniform }
                else { wgpu::BufferBindingType::Storage { read_only } },
            has_dynamic_offset: false, min_binding_size: None,
        }, count: None,
    }
}

impl WorldFlareRenderer {
    pub(super) fn invalidate_history(&mut self) {
        self.states = None;
        self.draw_bind_group = None;
        self.compute_bind_group = None;
        self.frame_id = 0;
        self.last_camera = None;
    }

    pub(super) fn invalidate_depth_binding(&mut self) {
        self.compute_bind_group = None;
    }

    pub(super) fn load(&mut self, candidates: &[scene::WorldFlare]) {
        self.candidates = candidates.to_vec();
        self.stages.clear();
        self.stage_pipeline.clear();
        self.last_camera = None;
        self.scratch = Vec::with_capacity(candidates.len());
        // GPU states are indexed by stable world-flare ID and map-specific.
        self.instances = None;
        self.states = None;
        self.draw_bind_group = None;
        self.compute_bind_group = None;
        self.last_candidates = candidates.len();
        self.last_drawn = 0;
        self.last_dispatches = 0;
        self.last_stage_draws = 0;
        // Discard visibility history when changing maps/cameras.
        self.frame_id = 0;
    }

    pub(super) fn install_textures(
        &mut self, device: &wgpu::Device, textures: &[GpuImage],
        white: &GpuImage, repeat: &wgpu::Sampler, clamp: &wgpu::Sampler, texture_clamp: &[bool],
    ) {
        self.stages.clear();
        self.stage_pipeline.clear();
        if self.candidates.is_empty() { return; }
        if self.stage_layout.is_none() {
            self.stage_layout = Some(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("RBSP flare authored material stage layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None,
                    },
                    buffer_layout_entry(2, wgpu::ShaderStages::FRAGMENT, true, true),
                ],
            }));
        }
        for flare in &self.candidates {
            if self.stages.contains_key(&flare.shader_index) { continue; }
            let fallback = flare.stages.is_empty();
            let default_stage = materials::MaterialStage {
                texture: StageTexture::White, enhancements: Default::default(),
                animation: None,
                blend: Some(DEFAULT_FLARE_BLEND), alpha_cutoff: 0.0, opacity: 1.0,
                color: [1.0; 3], rgb_gen: RgbGen::Vertex, alpha_gen: AlphaGen::Identity,
                tc_gen: Default::default(), tc_mods: Vec::new(),
                depth_write: false, depth_equal: false, glow: false,
            };
            let stages: &[materials::MaterialStage] = if fallback { std::slice::from_ref(&default_stage) } else { &flare.stages };
            let mut gpu_stages = Vec::new();
            for (stage_index, stage) in stages.iter().enumerate() {
                let portal_alpha = flare.alpha_portal_stages.get(stage_index)
                    .copied().unwrap_or(false).then_some(flare.radius);
                let gpu = match stage.texture {
                    StageTexture::Image(index) => textures.get(index).unwrap_or(white),
                    StageTexture::Lightmap | StageTexture::White => white,
                };
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("RBSP flare stage uniform"),
                    contents: bytemuck::bytes_of(&{
                        let mut uniform = stage_uniform(stage, fallback, portal_alpha);
                        uniform.wave_funcs[2] = flare.shader_clamp_time.unwrap_or(0.0);
                        uniform
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("RBSP flare authored stage"),
                    layout: self.stage_layout.as_ref().unwrap(),
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&gpu.view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(if matches!(stage.texture, StageTexture::Image(index) if texture_clamp.get(index) == Some(&true)) { clamp } else { repeat }) },
                        wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                    ],
                });
                gpu_stages.push(FlareStageGpu { blend: Some(stage.blend.unwrap_or(FLARE_OPAQUE_BLEND)), bind_group: group, _uniform: uniform });
            }
            self.stages.insert(flare.shader_index, gpu_stages);
        }
    }

    fn resources(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat,
                 samples: u32, depth: &wgpu::TextureView) {
        if self.uniform.is_none() {
            self.uniform = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RBSP world flare camera"),
                size: std::mem::size_of::<FlareCamera>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if self.instances.is_none() {
            self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RBSP visible world flare candidates"),
                size: (self.candidates.len().max(1) * std::mem::size_of::<FlareInstance>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if self.states.is_none() {
            self.states = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("RBSP world flare GPU occlusion and fade states"),
                size: self.candidates.len().max(1) as u64 * FLARE_STATE_BYTES,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
        }
        if self.draw_layout.is_none() {
            self.draw_layout = Some(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("RBSP world flare draw layout"),
                entries: &flare_draw_layout_entries(),
            }));
        }
        if self.draw_bind_group.is_none() {
            self.draw_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RBSP world flare draw data"), layout: self.draw_layout.as_ref().unwrap(),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_ref().unwrap().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.instances.as_ref().unwrap().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.states.as_ref().unwrap().as_entire_binding() },
                ],
            }));
        }
        // A different sample count requires a different depth-texture layout
        // and occlusion compute pipeline. The view changes on resize even when
        // the sample count does not: always rebuild the compute bind group.
        if !self.compute_layout.as_ref().is_some_and(|(s, _)| *s == samples) {
            self.compute_layout = Some((samples, device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("RBSP world flare depth probe layout"),
                entries: &[
                    buffer_layout_entry(0, wgpu::ShaderStages::COMPUTE, true, true),
                    buffer_layout_entry(1, wgpu::ShaderStages::COMPUTE, true, false),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: samples > 1,
                        }, count: None,
                    },
                    buffer_layout_entry(3, wgpu::ShaderStages::COMPUTE, false, false),
                ],
            })));
            self.compute_pipeline = None;
        }
        // Bind the depth view only when the map or render targets change;
        // per-frame bind-group creation would be expensive at JKA frame rates.
        if self.compute_bind_group.is_none() {
        self.compute_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RBSP world flare GPU center occlusion"),
            layout: &self.compute_layout.as_ref().unwrap().1,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_ref().unwrap().as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.instances.as_ref().unwrap().as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
                wgpu::BindGroupEntry { binding: 3, resource: self.states.as_ref().unwrap().as_entire_binding() },
            ],
        }));
        }
        if !self.compute_pipeline.as_ref().is_some_and(|(s, _)| *s == samples) {
            let source = include_str!("../world_flare_occlusion.wgsl");
            let load = if samples > 1 {
                format!("var d = 0.0; for (var i = 0; i < {samples}; i = i + 1) {{ d = max(d, textureLoad(scene_depth, pixel, i)); }} return d;")
            } else {
                "return textureLoad(scene_depth, pixel, 0);".to_owned()
            };
            let wgsl = source.replace("DEPTH_DECL", if samples > 1 {
                "var scene_depth: texture_depth_multisampled_2d"
            } else {
                "var scene_depth: texture_depth_2d"
            }).replace("DEPTH_LOAD", &load);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("RBSP world flare per-center depth compute"),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("RBSP world flare compute pipeline layout"),
                bind_group_layouts: &[Some(&self.compute_layout.as_ref().unwrap().1)],
                immediate_size: 0,
            });
            self.compute_pipeline = Some((samples, device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("RBSP world flare GPU center depth test"),
                layout: Some(&pipeline_layout), module: &shader,
                entry_point: Some("cs_main"), compilation_options: Default::default(),
                cache: crate::pipeline_cache::get().as_ref(),
            })));
        }
        if self.stage_pipeline_format == Some((format, samples))
            && !self.stage_pipeline.is_empty() { return; }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RBSP world flares"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../world_flares.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RBSP world flare draw pipeline layout"),
            bind_group_layouts: &[Some(self.draw_layout.as_ref().unwrap()), Some(self.stage_layout.as_ref().unwrap())],
            immediate_size: 0,
        });
        if self.stage_pipeline_format != Some((format, samples)) {
            self.stage_pipeline.clear();
            self.stage_pipeline_format = Some((format, samples));
        }
        let mut blends = vec![Some(DEFAULT_FLARE_BLEND)];
        for stages in self.stages.values() {
            for stage in stages { if !blends.contains(&stage.blend) { blends.push(stage.blend); } }
        }
        for blend in blends {
        if self.stage_pipeline.contains_key(&blend) { continue; }
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RBSP world flare batched additive draw"),
            layout: Some(&pipeline_layout), vertex: wgpu::VertexState {
                module: &shader, entry_point: Some("vs"), buffers: &[],
                compilation_options: Default::default(),
            }, fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: blend.map(|func| wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: gpu_blend_factor(func.src),
                            dst_factor: gpu_blend_factor(func.dst),
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })], compilation_options: Default::default(),
            }), primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, ..Default::default()
            }, depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(camera_depth_compare()),
                stencil: Default::default(), bias: Default::default(),
            }), multisample: wgpu::MultisampleState {
                count: samples, ..Default::default()
            }, multiview_mask: None, cache: crate::pipeline_cache::get().as_ref(),
        });
        self.stage_pipeline.insert(blend, pipeline);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>, depth: &wgpu::TextureView,
        format: wgpu::TextureFormat, samples: u32, camera: &Camera,
        width: u32, height: u32,
        visibility: Option<&jka_assets::bsp::Visibility>,
        pvs_enabled: bool, fog_mode: u8,
        // Only active thermal captures re-use the already-computed flare
        // occlusion results in the authored-glow texture.
        thermal_glow: Option<(&wgpu::TextureView, Option<&wgpu::TextureView>)>,
    ) {
        let forward = camera.forward();
        let right = forward.cross(glam::Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        self.draw_view(device, queue, encoder, color, resolve, depth, format, samples,
            FlareView {
                view_proj: camera.view_projection(width, height), eye: camera.position,
                forward, right, up, clip_plane: None, cluster: None,
            }, width, height, visibility, pvs_enabled, fog_mode, thermal_glow);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_sky(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView,
        depth: &wgpu::TextureView, format: wgpu::TextureFormat,
        samples: u32, position: glam::Vec3, view_proj: glam::Mat4,
        camera: &Camera, width: u32, height: u32,
        visibility: Option<&jka_assets::bsp::Visibility>,
        pvs_enabled: bool, fog_mode: u8,
    ) {
        let forward = camera.forward();
        let right = forward.cross(glam::Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        self.draw_view(device, queue, encoder, color, None, depth, format, samples,
            FlareView { view_proj, eye: position, forward, right, up,
                clip_plane: None, cluster: None },
            width, height, visibility, pvs_enabled, fog_mode, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_planar(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView,
        depth: &wgpu::TextureView, format: wgpu::TextureFormat,
        reflection: &PlanarReflectionView, width: u32, height: u32,
        visibility: Option<&jka_assets::bsp::Visibility>,
        pvs_enabled: bool, fog_mode: u8,
    ) {
        let forward = reflection.camera_forward;
        // The reflected view matrix already encodes handedness; mirror the
        // sprite's horizontal axis to agree with JKSunny's PV_MIRROR branch.
        let right = -forward.cross(glam::Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        self.draw_view(device, queue, encoder, color, None, depth, format, 1,
            FlareView {
                view_proj: reflection.view_proj, eye: reflection.camera_position,
                forward, right, up, clip_plane: Some(reflection.plane),
                cluster: reflection.cluster,
            }, width, height, visibility, pvs_enabled, fog_mode, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_view(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>, depth: &wgpu::TextureView,
        format: wgpu::TextureFormat, samples: u32, view: FlareView,
        width: u32, height: u32,
        visibility: Option<&jka_assets::bsp::Visibility>,
        pvs_enabled: bool, fog_mode: u8,
        thermal_glow: Option<(&wgpu::TextureView, Option<&wgpu::TextureView>)>,
    ) {
        let begin = Instant::now();
        self.last_candidates = self.candidates.len();
        self.last_pvs_rejected = 0;
        self.last_frustum_rejected = 0;
        self.last_drawn = 0;
        self.last_dispatches = 0;
        self.last_stage_draws = 0;
        self.last_filter_ms = 0.0;
        self.last_upload_ms = 0.0;
        self.last_probe_encode_ms = 0.0;
        self.last_draw_encode_ms = 0.0;
        if self.candidates.is_empty() || width == 0 || height == 0 { return; }
        let direction = view.forward;
        let cut = self.last_camera.is_some_and(|(origin, forward)| {
            origin.distance_squared(view.eye) > 512.0 * 512.0
                || forward.dot(direction) < 0.35
        });
        if cut { self.invalidate_history(); }
        self.last_camera = Some((view.eye, direction));
        let vp = view.view_proj;
        let view_cluster = view.cluster.or_else(|| visibility.and_then(|v| v.cluster_at(scene::jka_position(view.eye.to_array()))));
        self.scratch.clear();
        for (id, flare) in self.candidates.iter().enumerate() {
            if pvs_enabled && visibility.is_some_and(|v| !v.visible(view_cluster, &flare.clusters)) {
                self.last_pvs_rejected += 1;
                continue;
            }
            let position = glam::Vec3::from_array(scene::render_position(flare.origin));
            let normal = scene::render_position(flare.normal);
            let center = position + 3.0 * glam::Vec3::from_array(normal);
            if view.clip_plane.is_some_and(|plane| {
                glam::Vec4::from_array(plane).dot(center.extend(1.0)) < 0.0
            }) {
                self.last_frustum_rejected += 1;
                continue;
            }
            let clip = vp * center.extend(1.0);
            if !clip.is_finite() || clip.w <= 0.0 ||
                clip.x.abs() > clip.w || clip.y.abs() > clip.w ||
                clip.z < 0.0 || clip.z > clip.w {
                self.last_frustum_rejected += 1;
                continue;
            }
            self.scratch.push(FlareInstance {
                origin_radius: [position.x, position.y, position.z, flare.radius],
                color: [flare.color[0], flare.color[1], flare.color[2], if flare.fog_is_global { 1.0 } else { 0.0 }],
                normal: [normal[0], normal[1], normal[2], id as f32],
                fog: flare.fog,
            });
        }
        // Group by authored shader, not flare. All stages in the shader draw
        // instanced ranges; there is never a draw call per individual flare.
        self.scratch.sort_unstable_by_key(|instance| {
            self.candidates[instance.normal[3] as usize].shader_index
        });
        self.last_drawn = self.scratch.len();
        self.last_filter_ms = begin.elapsed().as_secs_f64() * 1000.0;
        // Advance even if every candidate is culled this frame: a later
        // re-entry must reset the GPU history and fade in again.
        self.frame_id = self.frame_id.wrapping_add(1).max(1);
        if self.scratch.is_empty() {
            self.last_cpu_ms = begin.elapsed().as_secs_f64() * 1000.0;
            return;
        }
        self.resources(device, format, samples, depth);
        let cam = FlareCamera {
            view_proj: vp.to_cols_array_2d(),
            right: view.right.extend(0.0).to_array(),
            up: view.up.extend(0.0).to_array(),
            eye: view.eye.extend(0.0).to_array(),
            viewport_time: [width as f32, height as f32, self.clock.elapsed().as_secs_f32(), fog_mode as f32],
            frame: [self.frame_id, self.scratch.len() as u32, 0, 0],
        };
        queue.write_buffer(self.uniform.as_ref().unwrap(), 0, bytemuck::bytes_of(&cam));
        queue.write_buffer(self.instances.as_ref().unwrap(), 0, bytemuck::cast_slice(&self.scratch));
        self.last_upload_ms = (begin.elapsed().as_secs_f64() * 1000.0 - self.last_filter_ms).max(0.0);
        let probe_begin = Instant::now();
        // Same-frame compute probe; the fragment shader and depth attachment
        // additionally clip peripheral pixels that cross behind solid walls.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RBSP world flare GPU center-depth probes"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_pipeline.as_ref().unwrap().1);
            pass.set_bind_group(0, self.compute_bind_group.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups((self.scratch.len() as u32).div_ceil(64), 1, 1);
        }
        self.last_dispatches = 1;
        self.last_probe_encode_ms = probe_begin.elapsed().as_secs_f64() * 1000.0;
        let draw_begin = Instant::now();
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("RBSP world flares: one batched draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color, depth_slice: None, resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth, depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store,
                    }), stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_bind_group(0, self.draw_bind_group.as_ref().unwrap(), &[]);
            let mut first = 0usize;
            while first < self.scratch.len() {
                let shader = self.candidates[self.scratch[first].normal[3] as usize].shader_index;
                let mut end = first + 1;
                while end < self.scratch.len()
                    && self.candidates[self.scratch[end].normal[3] as usize].shader_index == shader {
                    end += 1;
                }
                if let Some(stages) = self.stages.get(&shader) {
                    for stage in stages {
                        if let Some(pipeline) = self.stage_pipeline.get(&stage.blend) {
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(1, &stage.bind_group, &[]);
                            pass.draw(0..6, first as u32..end as u32);
                            self.last_stage_draws += 1;
                        }
                    }
                }
                first = end;
            }
        }
        if let Some((glow_view, glow_resolve)) = thermal_glow {
            // Replay only visible flare sprites, not their GPU occlusion probe.
            // The color attachment is the same format/MSAA as the scene; its
            // source was cleared and populated by authored JKA glow earlier.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Thermal world flare heat sources"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: glow_view, depth_slice: None, resolve_target: glow_resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth, depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store,
                    }), stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_bind_group(0, self.draw_bind_group.as_ref().unwrap(), &[]);
            let mut first = 0usize;
            while first < self.scratch.len() {
                let shader = self.candidates[self.scratch[first].normal[3] as usize].shader_index;
                let mut end = first + 1;
                while end < self.scratch.len()
                    && self.candidates[self.scratch[end].normal[3] as usize].shader_index == shader {
                    end += 1;
                }
                if let Some(stages) = self.stages.get(&shader) {
                    for stage in stages {
                        if let Some(pipeline) = self.stage_pipeline.get(&stage.blend) {
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(1, &stage.bind_group, &[]);
                            pass.draw(0..6, first as u32..end as u32);
                        }
                    }
                }
                first = end;
            }
        }
        self.last_draw_encode_ms = draw_begin.elapsed().as_secs_f64() * 1000.0;
        self.last_cpu_ms = begin.elapsed().as_secs_f64() * 1000.0;
        // Verbosity-gated, amortized counters: no GPU readback. `last_drawn`
        // means center probes; the number passing depth is intentionally GPU-only.
        if self.frame_id % 4096 == 0 {
            rverbose!(2,
                "World flares: authored={} PVS_rejected={} frustum_rejected={} center_probes={} GPU_dispatches={} grouped_stage_draws={} CPU_ms={:.3} filter_ms={:.3} uploads_and_pipeline_ms={:.3} probe_encode_ms={:.3} draw_encode_ms={:.3}",
                self.last_candidates, self.last_pvs_rejected,
                self.last_frustum_rejected, self.last_drawn,
                self.last_dispatches, self.last_stage_draws, self.last_cpu_ms,
                self.last_filter_ms, self.last_upload_ms, self.last_probe_encode_ms, self.last_draw_encode_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the crash when enabling r_worldFlares: fragment shader
    /// accesses camera.viewport_time for stage animation and fog.
    #[test]
    fn world_flare_draw_binding_visibility_matches_both_shader_stages() {
        let entries = flare_draw_layout_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].binding, 0);
        assert!(entries[0].visibility.contains(wgpu::ShaderStages::VERTEX));
        assert!(entries[0].visibility.contains(wgpu::ShaderStages::FRAGMENT));
        for entry in &entries[1..] {
            assert!(entry.visibility.contains(wgpu::ShaderStages::VERTEX));
        }
        let source = include_str!("../world_flares.wgsl");
        assert!(source.contains("@group(0) @binding(0) var<uniform> camera: FlareCamera;"));
        let fragment = source.split_once("@fragment fn fs").unwrap().1;
        assert!(fragment.contains("camera.viewport_time"));
    }

    #[test]
    fn world_flare_depth_probe_has_matching_compute_bindings() {
        let source = include_str!("../world_flare_occlusion.wgsl");
        for binding in 0..=3 {
            assert!(source.contains(&format!("@group(0) @binding({binding})")));
        }
        assert!(source.contains("@compute @workgroup_size(64)"));
    }
}
