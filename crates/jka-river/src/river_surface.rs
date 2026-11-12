//! The live river's water body: the ported Single Layer Water base pass
//! (river_surface.wgsl) drawn over a river-owned copy of the scene without the
//! river. It replaces the old grid debug tint.
//!
//! Surface detail (stage 2): the shared native FFT detail texture drives the
//! live-grid WPO and the detail-normal graph; the native foam field (per river)
//! and the T_ShallowWater_Foam detail drive primary foam, its normal, its
//! attribute lerps and scattering. The foam detail (`FOAM_DETAIL_TEXTURE`, an
//! 8-bit linear PNG) loads like any JKA texture, mips and r_picmip included;
//! without it foam is absent, never substituted.
//!
//! Reflections: native SLW composite, active sky and quality-2 SSR (same-frame
//! color; no temporal history or local reflection captures).
//! Not ported yet, each a documented
//! difference from Unreal rather than a substitute: the refraction-mask pass
//! (every copy texel reads as water),
//! previous-frame WPO/motion vectors, local lights, air fog,
//! cloud shadow, thin-glass sun transmission and the GGX energy LUTs
//! (r.Shading.EnergyConservation 0).
use crate::river_slw::{ClassicAmbient, PreintegratedGf, SceneSun};
use blue_water_gpu::scene_copy::SceneCopy;
use jka_water::river::{foam, material, surface, wave_advection, CENTIMETRES_PER_MAP_UNIT};

pub const SHADER: &str = include_str!("river_surface.wgsl");

/// T_ShallowWater_Foam (linear: RGB foam normal, A mask) in an asset pk3.
pub const FOAM_DETAIL_TEXTURE: &str = "textures/japro/river_foam_detail";

/// The live-instance material settings as WGSL constructors.
pub fn settings_shader() -> String {
    let f = |v: f32| format!("{v:?}");
    let v4 = |a: [f32; 4]| format!("vec4<f32>({},{},{},{})", f(a[0]), f(a[1]), f(a[2]), f(a[3]));
    let advection = |name: &str, s: wave_advection::Settings| {
        format!(
            "fn {name}()->RiverAdvection {{return RiverAdvection({},{});}}",
            v4([
                s.angle_degrees,
                s.magnitude_delta,
                s.pulse_length,
                s.texture_scale
            ]),
            v4([s.speed, s.vertex_mip, 0.0, 0.0])
        )
    };
    let m = material::Motion::default();
    let h = wave_advection::Height::default();
    [
        format!(
            "fn river_motion(delta:f32)->RiverMotion {{return RiverMotion({},vec4<f32>({},{},delta,0.0));}}",
            format!("vec4<f32>({},{},{},authored.detail.x)", f(m.low_velocity_scale), f(m.attenuation_min), f(m.attenuation_max)),
            f(m.previous_velocity_multiplier),
            f(m.wave_speed)
        ),
        advection("river_wpo_advection", wave_advection::Settings::default()),
        advection("river_detail_advection", surface::detail_advection()),
        advection("river_foam_advection", foam::advection()),
        format!(
            "fn river_wave_height()->RiverWaveHeight {{return RiverWaveHeight({},{});}}",
            v4([h.low_velocity_scale, h.velocity_min, h.velocity_max, h.power]),
            format!("vec4<f32>(authored.detail.y,{},0.0,0.0)", f(h.camera_falloff))
        ),
    ]
    .join("\n")
    + "\nfn river_foam_settings()->RiverFoamSettings {return authored.foam;}\nfn river_slw_parameters()->RiverSlwParameters {return authored.water;}"
}

/// The full WGSL before naga_oil resolves the Blue Water environment import.
pub fn shader_source() -> String {
    [
        "#import naval_sky::environment::{SkyEnvironment, env_irradiance, env_radiance}",
        material::SHADER,
        wave_advection::SHADER,
        wave_advection::VERTEX_SHADER,
        surface::SHADER,
        foam::SHADER,
        jka_water::river::slw::SHADER,
        crate::river_slw::ADAPTER_SHADER,
        &settings_shader(),
        SHADER,
        crate::river_composite::TRACE_SHADER,
        crate::river_composite::SHADER,
        blue_water_gpu::underwater_optics::SHADER,
        include_str!("river_underwater.wgsl"),
        jka_water::river::NORMAL_CODEC,
        include_str!("river_far.wgsl"),
        &jka_water::river::secondary::library(),
        include_str!("river_spray.wgsl"),
    ]
    .join("\n")
}

/// M_SimpleSpray's noise (linear RGBA) in an asset pack; see
/// scripts/ue-river-spray-textures.py.
pub const SPRAY_NOISE_TEXTURE: &str = "textures/japro/river_spray_noise";
/// M_SimpleSpray's normal map (linear RGB, Z rebuilt from XY).
pub const SPRAY_NORMALS_TEXTURE: &str = "textures/japro/river_spray_normals";

pub fn compose() -> Result<wgpu::naga::Module, String> {
    blue_water_clouds::shaders::compose(&shader_source(), "jka-client/river_surface.wgsl", &[])
}

/// `RiverView` in river_surface.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewUniform {
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub distortion: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_illuminance: [f32; 4],
    /// Material Time, foam detail present (1/0), Engine.DeltaTime, bed clamp.
    pub time: [f32; 4],
    /// Runtime fragment material LOD (x = 0 original, 1 wave, 2 wave + foam; y = mip cutoff).
    pub material_lod: [f32; 4],
}

impl ViewUniform {
    /// `size` is the scene target (UnscaledViewRect). `blue_water_ambient`
    /// must match the environment group the draw binds; `foam` whether the
    /// foam detail resource is bound.
    pub fn new(
        camera: &jka_render_core::camera::Camera,
        size: [u32; 2],
        sun: Option<SceneSun>,
        blue_water_ambient: bool,
        time: f32,
        delta: f32,
        foam: bool,
    ) -> Self {
        let forward = camera.forward();
        let roll = glam::Quat::from_axis_angle(forward, camera.roll);
        let right = forward.cross(glam::Vec3::Y).normalize_or_zero();
        let up = roll * right.cross(forward).normalize_or_zero();
        let right = roll * right;
        let river = |v: glam::Vec3, w: f32| {
            let [x, y, z] = jka_render_core::coordinates::jka_position(v.to_array());
            [x, y, z, w]
        };
        let [w, h] = size.map(|n| n.max(1));
        let aspect = w as f32 / h as f32;
        // ViewToClip M[0][0] = 1 / tan(horizontal half FOV).
        let m00 = 1.0 / (aspect * (camera.fov_y_for_viewport(w, h) * 0.5).tan());
        let (direction, illuminance) = sun.map_or(([0.0, 0.0, 1.0, 0.0], [0.0; 3]), |sun| {
            let [x, y, z] = sun.toward_light;
            ([x, y, z, 1.0], sun.illuminance())
        });
        Self {
            eye: river(
                camera.position * CENTIMETRES_PER_MAP_UNIT,
                camera.far_distance(),
            ),
            forward: river(forward, camera.near_distance()),
            right: river(right, 0.0),
            up: river(up, 0.0),
            distortion: [m00, aspect, w as f32, h as f32],
            sun_direction: direction,
            sun_illuminance: [
                illuminance[0],
                illuminance[1],
                illuminance[2],
                if blue_water_ambient { 1.0 } else { 0.0 },
            ],
            time: [time, if foam { 1.0 } else { 0.0 }, delta, 0.0],
            material_lod: [0.0; 4],
        }
    }
}

/// Renderer-wide river water resources. Layouts, samplers, the shared FFT
/// detail and the shader are built once; the pipeline per scene (format,
/// samples); the scene copy per target size. Per-frame work is the FFT, one
/// uniform write and the copy pass.
pub struct RiverSurface {
    pub(super) module: wgpu::ShaderModule,
    pub(super) body_layout: wgpu::BindGroupLayout,
    /// Group 1 of the water-mesh quadtree tiles (river_far.wgsl).
    far_layout: wgpu::BindGroupLayout,
    far_pipeline: Option<wgpu::RenderPipeline>,
    far_composite: Option<wgpu::RenderPipeline>,
    far_wire: Option<wgpu::RenderPipeline>,
    /// r_riverWire 1: every water-mesh tile, dry parts included.
    far_wire_full: Option<wgpu::RenderPipeline>,
    pub(super) frame_layout: wgpu::BindGroupLayout,
    pub(super) env_layout: wgpu::BindGroupLayout,
    pipeline: Option<((wgpu::TextureFormat, u32), wgpu::RenderPipeline)>,
    composite: Option<wgpu::RenderPipeline>,
    submerged: Option<wgpu::RenderPipeline>,
    audio_probe: Option<crate::river_audio_probe::AudioProbe>,
    /// r_riverWire 0: the live window blocks, masked as the water draws.
    wire: Option<wgpu::RenderPipeline>,
    /// r_riverWire 1: the whole window grid mesh, unmasked.
    wire_full: Option<wgpu::RenderPipeline>,
    /// Window block culling (river_blocks): compute pipelines, block indices.
    blocks: crate::river_blocks::Pipelines,
    pub(super) sky_layout: wgpu::BindGroupLayout,
    pub(super) sky_group: wgpu::BindGroup,
    hzb: Option<crate::river_composite::DepthPyramid>,
    /// BasePassRendering.cpp: bilinear clamp, as the classic field is sampled.
    ambient_sampler: wgpu::Sampler,
    /// Gathers only (filtering does not apply); point clamp.
    scene_sampler: wgpu::Sampler,
    gf: PreintegratedGf,
    /// Shared native Grid2D_OceanPatch FFT detail (one per renderer).
    ocean: crate::river_ocean::Ocean,
    /// T_ShallowWater_Foam from the JKA texture loader; None if missing.
    foam_detail: Option<wgpu::TextureView>,
    /// Repeating trilinear, as world textures sample.
    foam_detail_sampler: wgpu::Sampler,
    /// Bound in its place while absent (never read: foam is off then).
    inert_detail: (wgpu::TextureView, wgpu::Sampler),
    /// Bilinear clamp for FoamRT's cubic reconstruction edge fallback.
    foam_rt_sampler: wgpu::Sampler,
    view: wgpu::Buffer,
    copy: Option<SceneCopy>,
    pub(super) frame: Option<wgpu::BindGroup>,
    /// Bound when Blue Water is off; the shader reads the classic field then.
    pub(super) inert_env: wgpu::BindGroup,
    /// Without post or MSAA the scene is the swapchain, which cannot be copied.
    pub warned_unsampleable: bool,
    /// Material Time / FFT system age / foam world time, seconds since creation.
    pub clock: f64,
    /// Group 6 of the whitewater sprites: particles, noise, normals, sampler.
    spray_layout: wgpu::BindGroupLayout,
    spray_pipeline: Option<wgpu::RenderPipeline>,
    /// M_SimpleSpray textures; neutral 1x1 stand-ins until loaded (noise 0.7
    /// = no distortion, flat normal).
    spray_textures: (wgpu::TextureView, wgpu::TextureView),
    spray_sampler: wgpu::Sampler,
    spray_generation: u32,
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty,
        count: None,
    }
}
fn uniform() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}
fn texture(filterable: bool) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}
fn sampler(kind: wgpu::SamplerBindingType) -> wgpu::BindingType {
    wgpu::BindingType::Sampler(kind)
}

impl RiverSurface {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river Single Layer Water"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(compose()?)),
        });
        use wgpu::SamplerBindingType::{Filtering, NonFiltering};
        let body_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river water body"),
            entries: &[
                entry(0, uniform()),
                entry(1, texture(true)),
                entry(2, texture(true)),
                entry(3, sampler(Filtering)),
                entry(4, texture(true)),
                entry(5, texture(true)),
                entry(6, sampler(Filtering)),
                entry(7, texture(true)),
                entry(8, sampler(Filtering)),
                entry(9, uniform()),
                // vs_block's live block list (river_blocks).
                entry(15, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }),
            ],
        });
        let storage = || wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let far_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river water mesh"),
            entries: &[
                entry(9, uniform()),
                entry(10, uniform()),
                entry(11, storage()),
                entry(12, storage()),
                entry(13, storage()),
                entry(14, storage()),
            ],
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river water frame"),
            entries: &[
                entry(0, uniform()),
                entry(1, texture(true)),
                entry(
                    2,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(3, sampler(NonFiltering)),
                entry(4, texture(true)),
                entry(5, sampler(Filtering)),
                entry(6, texture(true)),
                entry(7, sampler(Filtering)),
                entry(8, texture(true)),
                entry(9, sampler(Filtering)),
                entry(11, texture(false)),
            ],
        });
        let sky_layout = crate::river_composite::sky_layout(device);
        let sky_group = crate::river_composite::sky_group(device, &sky_layout, None);
        let env_layout = blue_water_sky::environment_bind_group_layout(device);
        let ambient_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river classic indirect field"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let scene_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river scene without water gathers"),
            ..Default::default()
        });
        let view = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river water view"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let inert_env = inert_environment(device, &env_layout);
        let ocean = crate::river_ocean::Ocean::new(device, queue)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("river ocean reset"),
        });
        ocean.encode_reset(&mut encoder);
        queue.submit(Some(encoder.finish()));
        let inert = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("river absent foam detail"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let inert_detail = (
            inert.create_view(&Default::default()),
            device.create_sampler(&Default::default()),
        );
        let foam_detail_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river foam detail"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let foam_rt_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river FoamRT bilinear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let spray_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river whitewater sprites"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: storage(),
                    count: None,
                },
                entry(1, texture(true)),
                entry(2, texture(true)),
                entry(3, sampler(Filtering)),
            ],
        });
        let neutral = |label: &'static str, texel: [f32; 4]| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&texel),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(1) },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            texture.create_view(&Default::default())
        };
        let material = jka_water::river::secondary::material::NOISE_CENTRE;
        let spray_textures = (
            neutral("river spray noise stand-in", [material, material, material, 1.0]),
            neutral("river spray normals stand-in", [0.5, 0.5, 1.0, 1.0]),
        );
        let spray_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river spray material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            module,
            body_layout,
            far_layout,
            far_pipeline: None,
            far_composite: None,
            far_wire: None,
            far_wire_full: None,
            frame_layout,
            env_layout,
            pipeline: None,
            composite: None,
            submerged: None,
            audio_probe: None,
            wire: None,
            wire_full: None,
            blocks: crate::river_blocks::Pipelines::new(device),
            sky_layout,
            sky_group,
            hzb: None,
            ambient_sampler,
            scene_sampler,
            gf: PreintegratedGf::new(device, queue),
            ocean,
            foam_detail: None,
            foam_detail_sampler,
            inert_detail,
            foam_rt_sampler,
            view,
            copy: None,
            frame: None,
            inert_env,
            warned_unsampleable: false,
            clock: 0.0,
            spray_layout,
            spray_pipeline: None,
            spray_textures,
            spray_sampler,
            spray_generation: 0,
        })
    }

    /// Build the pipeline for the scene pass's colour format and samples.
    pub fn ensure_pipeline(
        &mut self,
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        shadow_receiver_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
        depth_compare: wgpu::CompareFunction,
    ) {
        if self
            .pipeline
            .as_ref()
            .is_some_and(|(key, _)| *key == (format, samples))
        {
            return;
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river Single Layer Water"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&self.body_layout),
                Some(shadow_receiver_layout),
                Some(&self.frame_layout),
                Some(&self.env_layout),
                Some(&self.sky_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("river Single Layer Water"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_block"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            // Two-sided material. The grid winds clockwise seen from above
            // (+Z), so the top is the front face.
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Cw,
                cull_mode: None,
                ..Default::default()
            },
            // SLW output is opaque and writes depth.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("river SingleLayerWaterComposite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_block"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::SrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Cw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        self.composite = Some(composite);
        self.submerged = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("river Hybrid underwater optics"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_submerged"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &self.module, entry_point: Some("fs_submerged"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: depth_format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
        }));
        // A line polygon pipeline is created only on devices that enable the
        // same wireframe feature used by the rest of r_showtris.
        let lines = device.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        let wire = |label: &str, vertex: &str, buffers: &[Option<wgpu::VertexBufferLayout<'_>>], fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label), layout: Some(&layout),
                vertex: wgpu::VertexState { module: &self.module, entry_point: Some(vertex), compilation_options: Default::default(), buffers },
                fragment: Some(wgpu::FragmentState { module: &self.module, entry_point: Some(fragment), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::COLOR })] }),
                primitive: wgpu::PrimitiveState { polygon_mode: wgpu::PolygonMode::Line, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: depth_format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
            })
        };
        self.wire = lines.then(|| wire("river displaced mesh wireframe", "vs_block", &[], "fs_river_wire_masked"));
        self.wire_full = lines.then(|| wire("river full grid wireframe", "vs", &[Some(crate::river_grid_mesh::vertex_layout())], "fs_river_wire"));
        // Water-mesh quadtree tiles: the same pass states, group 1 = pool.
        let far_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river water mesh"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&self.far_layout),
                Some(shadow_receiver_layout),
                Some(&self.frame_layout),
                Some(&self.env_layout),
                Some(&self.sky_layout),
            ],
            immediate_size: 0,
        });
        let far = |entry: &str, blend: Option<wgpu::BlendState>, write: wgpu::ColorWrites, depth_write: bool, compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&far_layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vs_far"),
                    compilation_options: Default::default(),
                    buffers: &[Some(crate::river_far::vertex_layout())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: write })],
                }),
                // FWaterMeshIndexBuffer winds clockwise seen from above, as the grid.
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Cw, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        self.far_pipeline = Some(far("fs_far", None, wgpu::ColorWrites::ALL, true, depth_compare));
        self.far_composite = Some(far(
            "fs_composite_far",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::SrcAlpha, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            }),
            wgpu::ColorWrites::COLOR,
            false,
            wgpu::CompareFunction::Equal,
        ));
        let far_wire = |label: &str, fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&far_layout),
                vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_far"), compilation_options: Default::default(), buffers: &[Some(crate::river_far::vertex_layout())] },
                fragment: Some(wgpu::FragmentState { module: &self.module, entry_point: Some(fragment), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::COLOR })] }),
                primitive: wgpu::PrimitiveState { polygon_mode: wgpu::PolygonMode::Line, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: depth_format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        self.far_wire = lines.then(|| far_wire("river water mesh wireframe", "fs_far_wire"));
        self.far_wire_full = lines.then(|| far_wire("river water mesh full wireframe", "fs_far_wire_full"));
        // Whitewater sprites: BLEND_Translucent, two-sided, depth-tested
        // against the scene and the river, no depth write.
        let spray_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river whitewater sprites"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&self.body_layout),
                Some(shadow_receiver_layout),
                Some(&self.frame_layout),
                Some(&self.env_layout),
                Some(&self.sky_layout),
                Some(&self.spray_layout),
            ],
            immediate_size: 0,
        });
        self.spray_pipeline = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("river whitewater sprites"),
            layout: Some(&spray_layout),
            vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_spray"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_spray"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
                        alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None,
            cache: None,
        }));
        self.pipeline = Some(((format, samples), pipeline));
    }

    /// M_SimpleSpray's noise and normal textures (loaded once per renderer).
    /// Rivers rebuild their sprite groups on their next frame.
    pub fn set_spray_textures(&mut self, noise: wgpu::TextureView, normals: wgpu::TextureView) {
        self.spray_textures = (noise, normals);
        self.spray_generation += 1;
    }

    /// Changes whenever the sprite textures do; see `set_spray_textures`.
    pub fn spray_generation(&self) -> u32 {
        self.spray_generation
    }

    /// A river's whitewater sprite group over its particle buffer.
    pub fn spray_group(&self, device: &wgpu::Device, particles: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river whitewater sprites"),
            layout: &self.spray_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: particles.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.spray_textures.0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.spray_textures.1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.spray_sampler) },
            ],
        })
    }

    /// The river's whitewater sprites: one instanced draw of every particle;
    /// hidden ones are clipped in the vertex shader.
    pub fn draw_spray<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        let (Some(pipeline), Some(frame), Some(body), Some(spray)) = (&self.spray_pipeline, &self.frame, river.body_group(), river.spray_group()) else { return; };
        pass.set_pipeline(pipeline);
        self.bind_patch(pass, camera, shadow_receiver, environment, frame, body);
        pass.set_bind_group(6, spray, &[]);
        pass.draw(0..6, 0..jka_water::river::secondary::PARTICLE_COUNT);
    }

    /// r_showtris river bit: the water mesh tiles, coloured by LOD level.
    pub fn draw_wire_far<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        let pipeline = if crate::river_blocks::wire_mode() == 1 { &self.far_wire_full } else { &self.far_wire };
        if let Some(pipeline) = pipeline {
            self.draw_far_with(pass, pipeline, camera, shadow_receiver, environment, river);
        }
    }

    pub fn far_layout(&self) -> &wgpu::BindGroupLayout {
        &self.far_layout
    }

    /// The water-mesh quadtree tiles beyond the camera window (base pass).
    pub fn draw_far<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        if let Some(pipeline) = &self.far_pipeline {
            self.draw_far_with(pass, pipeline, camera, shadow_receiver, environment, river);
        }
    }

    /// SingleLayerWaterComposite over the water-mesh quadtree tiles.
    pub fn draw_composite_far<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        if let Some(pipeline) = &self.far_composite {
            self.draw_far_with(pass, pipeline, camera, shadow_receiver, environment, river);
        }
    }

    fn draw_far_with<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, pipeline: &'a wgpu::RenderPipeline, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        let (Some(frame), Some(far)) = (&self.frame, river.far_mesh()) else { return };
        let Some(group) = far.group() else { return };
        if far.draws.iter().all(|(_, count)| *count == 0) {
            return;
        }
        pass.set_pipeline(pipeline);
        self.bind_patch(pass, camera, shadow_receiver, environment, frame, group);
        // One instanced draw per density bucket (one material).
        for (density, &(first, count)) in far.draws.iter().enumerate() {
            if count == 0 {
                continue;
            }
            let Some(mesh) = far.densities().get(density) else { continue };
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, first..first + count);
        }
    }

    pub fn ready_for(&self, format: wgpu::TextureFormat, samples: u32) -> bool {
        self.frame.is_some()
            && self
                .pipeline
                .as_ref()
                .is_some_and(|(key, _)| *key == (format, samples))
    }

    pub fn has_foam_detail(&self) -> bool {
        self.foam_detail.is_some()
    }

    /// Bind the foam detail texture (loaded once per renderer).
    pub fn set_foam_detail(&mut self, detail: wgpu::TextureView) {
        self.foam_detail = Some(detail);
        self.frame = None;
    }

    pub fn set_map_sky(&mut self, device: &wgpu::Device, faces: Option<[&wgpu::TextureView; 6]>) {
        self.sky_group = crate::river_composite::sky_group(device, &self.sky_layout, faces);
    }

    /// The shared FFT detail for this frame. `age` is the system age in seconds.
    pub fn encode_ocean(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        age: f32,
    ) -> Result<(), String> {
        let frame = self.ocean.prepare_frame(
            device,
            age,
            jka_water::river::ocean::Parameters::default(),
        )?;
        frame.encode(encoder);
        Ok(())
    }

    /// Per frame, before the scene pass: the copy for this target and the view.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
        samples: u32,
        view: &ViewUniform,
    ) {
        let key = (size[0], size[1], samples);
        if !self.copy.as_ref().is_some_and(|copy| copy.key == key) {
            let copy = SceneCopy::new(device, size[0], size[1], samples);
            self.hzb = Some(crate::river_composite::DepthPyramid::new(
                device,
                size,
                &copy.depth,
            ));
            self.copy = Some(copy);
            self.frame = None;
        }
        if self.frame.is_none() {
            let copy = self.copy.as_ref().expect("created above");
            let gf = self.gf.texture.create_view(&Default::default());
            let ocean = self.ocean.texture.create_view(&Default::default());
            let (detail, detail_sampler) = match &self.foam_detail {
                Some(view) => (view, &self.foam_detail_sampler),
                None => (&self.inert_detail.0, &self.inert_detail.1),
            };
            self.frame = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("river water frame"),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.view.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&copy.color),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&copy.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.scene_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&gf),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.gf.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&ocean),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&self.ocean.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(detail),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::Sampler(detail_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::TextureView(
                            &self.hzb.as_ref().expect("copy HZB").view,
                        ),
                    },
                ],
            }));
        }
        queue.write_buffer(&self.view, 0, bytemuck::bytes_of(view));
    }

    /// Copy the scene without the river (colour + reversed depth, MSAA
    /// resolved to the nearest sample). The scene pass must be ended.
    pub fn capture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        if let Some(copy) = &self.copy {
            copy.capture(device, encoder, color, depth);
            self.hzb.as_ref().expect("copy HZB").encode(encoder);
        }
    }

    /// Capture completed river surfaces for the submerged medium. No second HZB.
    pub fn capture_submerged(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, depth: &wgpu::TextureView) {
        if let Some(copy) = &self.copy { copy.capture(device, encoder, color, depth); }
    }

    /// The per-river group: SimGridRT, NormalsRT, the classic indirect field
    /// and one FoamRT parity.
    pub fn body_group(
        &self,
        device: &wgpu::Device,
        origin_cm: [f64; 3],
        mesh: jka_water::river::grid_mesh::GridMesh,
        textures: &crate::river_material::Textures,
        ambient: &ClassicAmbient,
        foam_rt: &wgpu::Texture,
        authored: &wgpu::Buffer,
        blocks: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        use wgpu::util::DeviceExt;
        let origin = origin_cm.map(|v| v as f32);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river water body origin"),
            contents: bytemuck::cast_slice(&[origin[0], origin[1], origin[2], 0.0, mesh.size()[0], mesh.size()[1], mesh.cells()[0] as f32, mesh.cells()[1] as f32]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let surface = textures.surface.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let normals = textures.normals.create_view(&Default::default());
        let (a, d) = (
            ambient.ambient.create_view(&Default::default()),
            ambient.directed.create_view(&Default::default()),
        );
        let foam = foam_rt.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river water body"),
            layout: &self.body_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&surface),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&normals),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&textures.normal_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&a),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&d),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.ambient_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&foam),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.foam_rt_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: authored.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: blocks.as_entire_binding(),
                },
            ],
        })
    }

    /// `environment` is Blue Water's sky environment group while that sky is
    /// on (the same condition the view uniform's ambient flag was built with).
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        shadow_receiver: &'a wgpu::BindGroup,
        environment: Option<&'a wgpu::BindGroup>,
        river: &'a crate::river_runtime::LiveRiver,
    ) {
        let (Some((_, pipeline)), Some(frame), Some(body)) =
            (&self.pipeline, &self.frame, river.body_group())
        else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, body, &[]);
        pass.set_bind_group(2, shadow_receiver, &[]);
        pass.set_bind_group(3, frame, &[]);
        pass.set_bind_group(4, environment.unwrap_or(&self.inert_env), &[]);
        pass.set_bind_group(5, &self.sky_group, &[]);
        self.draw_blocks(pass, river);
    }
    pub fn draw_composite<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        shadow_receiver: &'a wgpu::BindGroup,
        environment: Option<&'a wgpu::BindGroup>,
        river: &'a crate::river_runtime::LiveRiver,
    ) {
        let (Some(pipeline), Some(frame), Some(body)) =
            (&self.composite, &self.frame, river.body_group())
        else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, body, &[]);
        pass.set_bind_group(2, shadow_receiver, &[]);
        pass.set_bind_group(3, frame, &[]);
        pass.set_bind_group(4, environment.unwrap_or(&self.inert_env), &[]);
        pass.set_bind_group(5, &self.sky_group, &[]);
        self.draw_blocks(pass, river);
    }

    pub fn draw_submerged<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        let (Some(pipeline), Some(frame), Some(body)) = (&self.submerged, &self.frame, river.body_group()) else { return; };
        pass.set_pipeline(pipeline);
        self.bind_patch(pass, camera, shadow_receiver, environment, frame, body);
        pass.draw(0..3, 0..1);
    }

    /// Invalidate a pending listener probe on river/map replacement.
    pub fn reset_audio_probe(&mut self) { self.audio_probe = None; }

    /// Audio samples share the exact visible water-height classifier, with a
    /// separate listener location and no synchronous GPU readback.
    pub fn encode_audio_probe(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, camera_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout, camera: &wgpu::BindGroup,
        shadow: &wgpu::BindGroup, environment: Option<&wgpu::BindGroup>,
        rivers: &[crate::river_runtime::LiveRiver],
    ) {
        if let Some(probe) = &mut self.audio_probe { probe.poll(device); }
        let Some(sample) = crate::river_runtime::audio_listener_probe() else { return; };
        if self.audio_probe.is_none() && self.frame.is_some() {
            self.audio_probe = Some(crate::river_audio_probe::AudioProbe::new(device, self, camera_layout, shadow_layout));
        }
        if let Some(mut probe) = self.audio_probe.take() {
            probe.encode(queue, encoder, self, camera, shadow, environment, rivers, sample);
            self.audio_probe = Some(probe);
        }
    }

    /// r_showtris river bit. r_riverWire 0: the live blocks, masked as the
    /// water draws; 1: the whole grid mesh, unmasked.
    pub fn draw_wire<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow_receiver: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, river: &'a crate::river_runtime::LiveRiver) {
        let full = crate::river_blocks::wire_mode() == 1;
        let (Some(pipeline), Some(frame), Some(body)) = (if full { &self.wire_full } else { &self.wire }, &self.frame, river.body_group()) else { return; };
        pass.set_pipeline(pipeline);
        self.bind_patch(pass, camera, shadow_receiver, environment, frame, body);
        if full {
            pass.set_vertex_buffer(0, river.mesh.vertices.slice(..));
            pass.set_index_buffer(river.mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..river.mesh.geometry.index_count(), 0, 0..1);
        } else {
            self.draw_blocks(pass, river);
        }
    }

    /// The window grid's live blocks (river_blocks), instanced through vs_block.
    fn draw_blocks<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, river: &'a crate::river_runtime::LiveRiver) {
        let Some(cull) = river.block_cull() else { return };
        pass.set_index_buffer(self.blocks.indices.slice(..), wgpu::IndexFormat::Uint32);
        // B: one record per template (empty buckets draw nothing).
        for record in 0..cull.records() {
            pass.draw_indexed_indirect(&cull.args, record * 5 * std::mem::size_of::<u32>() as u64);
        }
    }

    pub fn block_pipelines(&self) -> &crate::river_blocks::Pipelines {
        &self.blocks
    }

    fn bind_patch<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup, shadow: &'a wgpu::BindGroup, environment: Option<&'a wgpu::BindGroup>, frame: &'a wgpu::BindGroup, body: &'a wgpu::BindGroup) {
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, body, &[]);
        pass.set_bind_group(2, shadow, &[]);
        pass.set_bind_group(3, frame, &[]);
        pass.set_bind_group(4, environment.unwrap_or(&self.inert_env), &[]);
        pass.set_bind_group(5, &self.sky_group, &[]);
    }
}

/// A valid, never-read environment group (1x1 atlas, zero uniforms).
fn inert_environment(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
    let atlas = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("river inert sky environment"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = atlas.create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river inert sky environment"),
        size: std::mem::size_of::<blue_water_sky::EnvironmentUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("river inert sky environment"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}
