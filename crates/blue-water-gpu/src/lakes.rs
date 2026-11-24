//! Upstream lake material with bounded, low density clipmaps, independent of the ocean FFT.
//! Copyright (c) 2026 Bill Yang. MIT; see vendor/blue-water-sea-and-sky/LICENSE.
use crate::original_surface::{
    buffer, entry_buffer, entry_sampler, entry_texture, placeholder, sampled, sampler,
    storage, try_size_of_binding, uniform, SurfaceBoundary, SurfaceCamera,
};
use bytemuck::{Pod, Zeroable};
use naga_oil::compose::{NagaModuleDescriptor, ShaderDefValue, ShaderType};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LakeVertex {
    pub position: [f32; 3],
    pub lake: [f32; 4],
}
const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LakeUniforms {
    pub offset: [f32; 4],
    pub wind: [f32; 4],
    pub mask: [f32; 4],
    pub land: [f32; 4],
    pub sea: [f32; 4],
    pub fresh_absorption: [f32; 4],
    pub fresh_backscatter: [f32; 4],
    pub salt_absorption: [f32; 4],
    pub salt_backscatter: [f32; 4],
    pub photo: [f32; 4],
    pub photo_haze: [f32; 4],
    pub frames: [[f32; 4]; crate::geometry::MAX_CLIPMAP_LEVELS],
    pub clipmap: [f32; 4],
    pub deformation: [f32; 4],
    pub tint: [f32; 4],
    pub reflection: [f32; 4],
    pub base_color: [f32; 4],
    pub appearance: [f32; 4],
    pub transmission: [f32; 4],
    pub shallow_color: [f32; 4],
    pub deep_color: [f32; 4],
    pub ripples: [f32; 4],
    /// Hybrid underwater fog RGB and absorption distance, in metres.
    pub underwater_fog: [f32; 4],
    /// Fog capture valid, depth darkening distance (m), host units per metre, spare.
    pub underwater: [f32; 4],
}
impl LakeUniforms {
    /// The upstream struct layout is kept for the shared vertex stage; fields
    /// only the removed original/Cheapwater materials read stay zero.
    pub fn new(direction: f32, aerial: bool) -> Self {
        let mut u = Self::zeroed();
        u.offset[2] = f32::from(aerial);
        u.tint = [1.0, 1.0, 1.0, 1.0];
        u.reflection = [-1.0, 0.0, 24.0, 64.0];
        u.appearance = [0.27, 0.0, 0.0, 0.0];
        u.transmission = [0.0, 0.026, 0.0, 2.4384];
        u.deep_color = [0.03, 0.12, 0.14, 4.0];
        u.ripples = [0.0, 0.0, 0.0, -1.0];
        u.underwater_fog = [0.0, 0.0, 0.0, 1.0];
        u.underwater = [0.0, 1.0, 40.0, 0.0];
        u.wind = [direction.cos(), direction.sin(), 0.0, 0.0];
        u
    }
}

/// Public builds have no Fluid Ninja lake wake: the contract function returns nothing.
pub const NINJA_LAKE_WAKE_OFF: &str = "fn ninja_lake_wake(vd: texture_2d<f32>, pd: texture_2d<f32>, flow_map: texture_2d<f32>,
    clamp_sampler: sampler, wrap_sampler: sampler, uv: vec2<f32>, world_cm: vec2<f32>, time: f32) -> vec4<f32> {
    return vec4<f32>(0.0);
}
fn ninja_lake_caustics(caustics: texture_3d<f32>, wrap_sampler: sampler, vd: texture_2d<f32>,
    pd: texture_2d<f32>, clamp_sampler: sampler, sim_uv: vec2<f32>, sim_on: bool, bed_cm: vec2<f32>,
    behind_cm: f32, eye_distance_cm: f32, time: f32, gx: vec2<f32>, gy: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(1.0);
}
";

/// Lake wake window uniform (binding 26): window centre (cm, track frame x/y),
/// 1 / extent (cm), enabled; spare.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct NinjaLakeWakeUniform {
    pub window: [f32; 4],
    pub params: [f32; 4],
}

/// `ninja_wgsl`: the private `jka_ninja::lake_wake_wgsl()` (contract in
/// jka-ninja lake_api.rs), or None for the inert public stand-in.
pub fn module(ninja_wgsl: Option<&str>) -> wgpu::naga::Module {
    if let Some(ninja) = ninja_wgsl {
        // The private material is validated only here: on failure keep the lake
        // without the Fluid Ninja wake rather than losing the lake material.
        match compose_module(Some(ninja)) {
            Ok(module) => return module,
            Err(e) => eprintln!("[LAKE NINJA] wake material failed, lakes draw without it: {e}"),
        }
    }
    compose_module(None).unwrap_or_else(|e| panic!("{e}"))
}

fn compose_module(ninja_wgsl: Option<&str>) -> Result<wgpu::naga::Module, String> {
    let mut composer = crate::shader_composer::composer().expect("lake shader modules");
    let mut source = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/lake.wgsl")
        .replace("\r\n", "\n");
    // Only the upstream vertex stage and helpers are used. Its own shading
    // (`fragment`, `motion`) ends the file and is dropped; the lake material
    // is the Source water fragment appended below.
    let shading = source.find("@fragment\nfn fragment(").expect("upstream lake fragment");
    source.truncate(shading);
    source = source.replace("    photo_haze: vec4<f32>,", "    photo_haze: vec4<f32>,\n    frames: array<vec4<f32>, 10>,\n    clipmap: vec4<f32>,\n    deformation: vec4<f32>,\n    tint: vec4<f32>,\n    reflection: vec4<f32>,\n    base_color: vec4<f32>,\n    appearance: vec4<f32>,\n    transmission: vec4<f32>,\n    shallow_color: vec4<f32>,\n    deep_color: vec4<f32>,\n    ripples: vec4<f32>,");
    source = source.replace("let world = vec3<f32>(input.position.x + lake.offset.x, input.position.y, input.position.z + lake.offset.y);",
        "let world = jka_lake_world(input);");
    source.push_str(include_str!("lake_deformation.wgsl"));
    source.push_str(crate::underwater_optics::SHADER);
    source.push_str(ninja_wgsl.unwrap_or(NINJA_LAKE_WAKE_OFF));
    source.push('\n');
    source.push_str(include_str!("lake_shared.wgsl"));
    source.push_str(include_str!("source_water.wgsl"));
    source = source.replace(
        "    ripples: vec4<f32>,",
        "    ripples: vec4<f32>,\n    underwater_fog: vec4<f32>,\n    underwater: vec4<f32>,",
    );
    // JKA supplies exact footprint masks and PVS, not upstream's terrain/sea-level bake.
    let start = source.find("fn mask_led(").unwrap();
    let end = source[start..].find("\n}").unwrap() + start + 2;
    source.replace_range(
        start..end,
        "fn mask_led(distance: f32) -> f32 { return 0.0; }",
    );
    source = source.replace(
        "camera.view_projection * vec4<f32>(pulled, 1.0)",
        "jka_lake_project(drawn)",
    );
    source.push_str(crate::shader_composer::SURFACE_BOUNDARY_WGSL);
    composer
        .make_naga_module(NagaModuleDescriptor {
            source: &source,
            file_path: "JKA lake material",
            shader_type: ShaderType::Wgsl,
            shader_defs: [("LAKE_HIGH".into(), ShaderDefValue::Bool(true))].into(),
            ..Default::default()
        })
        .map_err(|e| e.emit_to_string(&composer))
}

fn lake_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    fragment: &str,
    wireframe: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blue Water lakes and ponds"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LakeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRS,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            polygon_mode: if wireframe {
                wgpu::PolygonMode::Line
            } else {
                wgpu::PolygonMode::Fill
            },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(!wireframe),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

pub struct LakePipeline {
    /// Source water (Water_DX90 port), the only lake material.
    pub pipeline: wgpu::RenderPipeline,
    pub wireframe: Option<wgpu::RenderPipeline>,
    camera_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    environment_layout: wgpu::BindGroupLayout,
    shadows: wgpu::Buffer,
    motion: wgpu::Buffer,
    occluders: wgpu::Buffer,
    occluder_params: wgpu::Buffer,
    shadow_array: wgpu::TextureView,
    shadow_wide: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    air: wgpu::TextureView,
    linear: wgpu::Sampler,
    repeat: wgpu::Sampler,
    cloud: wgpu::TextureView,
    cloud_params: wgpu::Buffer,
    photo: wgpu::TextureView,
    ninja_off: wgpu::Buffer,
}
pub struct LakeBindings {
    pub camera: wgpu::Buffer,
    pub uniforms: wgpu::Buffer,
    pub boundary: wgpu::Buffer,
    pub camera_group: wgpu::BindGroup,
    pub material: wgpu::BindGroup,
}
impl LakePipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32, ninja_wgsl: Option<&str>) -> Self {
        let m = module(ninja_wgsl);
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let frag = wgpu::ShaderStages::FRAGMENT;
        let t2 = wgpu::TextureViewDimension::D2;
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lake camera"),
            entries: &[
                uniform(0, both),
                uniform(1, both),
                sampled(2, both, wgpu::TextureViewDimension::D2Array, true),
                sampled(3, both, t2, true),
                sampler(4, both, true),
                uniform(5, both),
                sampled(6, both, wgpu::TextureViewDimension::D3, false),
                sampler(7, both, false),
                storage(8, both),
                uniform(9, both),
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lake material"),
            entries: &[
                uniform(0, both),
                sampled(1, both, t2, false),
                sampler(2, both, false),
                sampled(3, frag, t2, false),
                sampled(4, frag, t2, true),
                sampled(5, frag, t2, false),
                sampler(6, frag, false),
                uniform(7, frag),
                sampled(8, frag, wgpu::TextureViewDimension::D2Array, false),
                sampler(9, frag, false),
                sampled(10, both, t2, false),
                sampler(11, both, false),
                sampled(12, frag, wgpu::TextureViewDimension::D2Array, false),
                sampler(13, frag, false),
                sampled(14, frag, t2, false),
                sampled(15, frag, wgpu::TextureViewDimension::D2Array, false),
                uniform(16, both),
                crate::original_surface::roof_sampled(frag),
                sampled(17, frag, t2, false),
                // Source water's animated $normalmap frames (placeholder: 1x1 photo).
                sampled(19, frag, wgpu::TextureViewDimension::D2Array, false),
                // Fluid Ninja lake wake: velocity/density, pressure/divergence, flow map, window.
                sampled(23, frag, t2, false),
                sampled(24, frag, t2, false),
                sampled(25, frag, t2, false),
                uniform(26, frag),
                // Fluid Ninja caustics volume (T_Caustics_Animated_02).
                sampled(27, frag, wgpu::TextureViewDimension::D3, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 18,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: t2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let environment_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("lake sky"),
                entries: &[
                    sampled(0, both, t2, false),
                    sampler(1, both, false),
                    uniform(2, both),
                ],
            });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lake pipeline"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&material_layout),
                Some(&environment_layout),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water lake"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(m.clone())),
        });
        let create_pipeline = |wireframe| {
            lake_render_pipeline(
                device,
                &layout,
                &shader,
                format,
                samples,
                if wireframe {
                    "jka_lake_wireframe"
                } else {
                    "jka_source_water_fragment"
                },
                wireframe,
            )
        };
        let pipeline = create_pipeline(false);
        let wireframe = device
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE)
            .then(|| create_pipeline(true));
        Self {
            pipeline,
            wireframe,
            camera_layout,
            material_layout,
            environment_layout,
            shadows: buffer(
                device,
                "lake disabled object shadows",
                // The upstream shading that read these is dropped; the layout keeps
                // the slots (min size None), so an unused one gets a placeholder.
                try_size_of_binding(&m, 0, 1).unwrap_or(16),
                false,
            ),
            motion: buffer(device, "lake motion slot", 144, false),
            occluders: buffer(device, "lake disabled occluders", 16, true),
            occluder_params: buffer(
                device,
                "lake occluder params",
                try_size_of_binding(&m, 0, 9).unwrap_or(16),
                false,
            ),
            shadow_array: placeholder(
                device,
                "lake shadow array",
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureViewDimension::D2Array,
                7,
            ),
            shadow_wide: placeholder(
                device,
                "lake wide shadow",
                wgpu::TextureFormat::Depth32Float,
                t2,
                1,
            ),
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                compare: Some(wgpu::CompareFunction::GreaterEqual),
                ..Default::default()
            }),
            air: placeholder(
                device,
                "lake clear air",
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureViewDimension::D3,
                1,
            ),
            repeat: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("lake seamless noise"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                // The Fluid Ninja caustics volume scrolls along W with time.
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            linear: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            cloud: placeholder(
                device,
                "lake neutral cloud",
                wgpu::TextureFormat::Rgba8Unorm,
                t2,
                1,
            ),
            cloud_params: buffer(device, "lake disabled cloud", 64, false),
            ninja_off: buffer(device, "lake Fluid Ninja wake off", std::mem::size_of::<NinjaLakeWakeUniform>() as u64, false),
            photo: placeholder(
                device,
                "lake no satellite photo",
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureViewDimension::D2Array,
                1,
            ),
        }
    }
    pub fn bindings(
        &self,
        device: &wgpu::Device,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        air: Option<(&wgpu::TextureView, &wgpu::Sampler)>,
        cloud: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
        shore: Option<&wgpu::TextureView>,
        noise: Option<&wgpu::TextureView>,
        planar: Option<(&wgpu::TextureView, &wgpu::Sampler)>,
        normal: Option<&wgpu::TextureView>,
        source_normal: Option<&wgpu::TextureView>,
        ripples: Option<&wgpu::TextureView>,
        submerged: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
        sky_roof: Option<&wgpu::TextureView>,
        ninja: Option<(&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView, &wgpu::Buffer)>,
        caustics: Option<&wgpu::TextureView>,
    ) -> LakeBindings {
        let camera = buffer(
            device,
            "lake camera",
            std::mem::size_of::<SurfaceCamera>() as u64,
            false,
        );
        let uniforms = buffer(
            device,
            "lake uniforms",
            std::mem::size_of::<LakeUniforms>() as u64,
            false,
        );
        let boundary = buffer(
            device,
            "lake footprint and PVS",
            std::mem::size_of::<SurfaceBoundary>() as u64,
            false,
        );
        let (air_view, air_sampler) = air.unwrap_or((&self.air, &self.linear));
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lake camera"),
            layout: &self.camera_layout,
            entries: &[
                entry_buffer(0, &camera),
                entry_buffer(1, &self.shadows),
                entry_texture(2, &self.shadow_array),
                entry_texture(3, &self.shadow_wide),
                entry_sampler(4, &self.shadow_sampler),
                entry_buffer(5, &self.motion),
                entry_texture(6, air_view),
                entry_sampler(7, air_sampler),
                entry_buffer(8, &self.occluders),
                entry_buffer(9, &self.occluder_params),
            ],
        });
        let (cloud_view, cloud_sampler, cloud_params) =
            cloud.unwrap_or((&self.cloud, &self.linear, &self.cloud_params));
        let material = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lake material"),
            layout: &self.material_layout,
            entries: &[
                entry_buffer(0, &uniforms),
                entry_texture(1, shore.unwrap_or(&self.cloud)),
                entry_sampler(2, &self.linear),
                entry_texture(3, color),
                entry_texture(4, depth),
                entry_texture(5, cloud_view),
                entry_sampler(6, cloud_sampler),
                entry_buffer(7, cloud_params),
                entry_texture(8, &self.photo),
                entry_sampler(9, &self.linear),
                entry_texture(10, noise.unwrap_or(&self.cloud)),
                entry_sampler(11, &self.repeat),
                entry_texture(12, planar.map_or(&self.photo, |p| p.0)),
                entry_sampler(13, planar.map_or(&self.linear, |p| p.1)),
                entry_texture(14, normal.unwrap_or(&self.cloud)),
                entry_texture(19, source_normal.unwrap_or(&self.photo)),
                entry_texture(15, ripples.unwrap_or(&self.photo)),
                entry_buffer(16, &boundary),
                entry_texture(22, sky_roof.unwrap_or(&self.cloud)),
                entry_texture(17, submerged.map_or(&self.cloud, |p| p.0)),
                entry_texture(18, submerged.map_or(&self.cloud, |p| p.1)),
                entry_texture(23, ninja.map_or(&self.cloud, |n| n.0)),
                entry_texture(24, ninja.map_or(&self.cloud, |n| n.1)),
                entry_texture(25, ninja.map_or(&self.cloud, |n| n.2)),
                entry_buffer(26, ninja.map_or(&self.ninja_off, |n| n.3)),
                entry_texture(27, caustics.unwrap_or(&self.air)),
            ],
        });
        LakeBindings {
            camera,
            uniforms,
            boundary,
            camera_group,
            material,
        }
    }
    pub fn environment(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniforms: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lake environment"),
            layout: &self.environment_layout,
            entries: &[
                entry_texture(0, view),
                entry_sampler(1, sampler),
                entry_buffer(2, uniforms),
            ],
        })
    }
    pub fn vertices(device: &wgpu::Device, vertices: &[LakeVertex]) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("low density lake clipmap"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        })
    }
    pub fn noise(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texels: &[[f32; 4]],
        size: u32,
    ) -> wgpu::TextureView {
        assert_eq!(texels.len(), (size * size) as usize);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lake smooth noise height and gradient"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let packed: Vec<[u16; 4]> = texels
            .iter()
            .map(|p| p.map(|v| half::f16::from_f32(v).to_bits()))
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&packed),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
            texture.size(),
        );
        texture.create_view(&Default::default())
    }

    pub fn normal_map(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        let levels = width.max(height).ilog2() + 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lake fallback normal map"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut pixels = rgba.to_vec();
        let (mut w, mut h) = (width, height);
        for mip in 0..levels {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            if w == 1 && h == 1 {
                break;
            }
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            pixels = (0..nw * nh * 4)
                .map(|i| {
                    let (x, y, c) = ((i / 4) % nw * 2, (i / 4) / nw * 2, i % 4);
                    let at = |dx: u32, dy: u32| {
                        pixels[((y + dy).min(h - 1) * w * 4 + (x + dx).min(w - 1) * 4 + c) as usize]
                            as u32
                    };
                    ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8
                })
                .collect();
            (w, h) = (nw, nh);
        }
        texture.create_view(&Default::default())
    }
    /// Animated normal map frames as one array layer each, with the authored
    /// mip chains (each `layers` entry: every level, largest first, tightly packed).
    pub fn normal_array(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: &[&[u8]],
        width: u32,
        height: u32,
        mips: u32,
    ) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Source water animated normal map"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers.len() as u32,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, rgba) in layers.iter().enumerate() {
            let mut offset = 0usize;
            for mip in 0..mips {
                let (w, h) = ((width >> mip).max(1), (height >> mip).max(1));
                let size = (w * h * 4) as usize;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: mip,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &rgba[offset..offset + size],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                offset += size;
            }
        }
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Geometry, footprint/PVS clipping, clipmap seams and animation of the
    /// Source lake pipeline. Material appearance is not asserted here.
    #[test]
    fn lake_geometry_respects_pvs_footprint_seams_and_speed() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("lake GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    required_features: adapter.features() & wgpu::Features::POLYGON_MODE_LINE,
                    ..Default::default()
                })
                .await
                .unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let pipeline = LakePipeline::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1, None);
            let _hdr_msaa = LakePipeline::new(&device, wgpu::TextureFormat::Rgba16Float, 4, None);
            let texture = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("lake fixture"),
                    size: wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let output = color.create_view(&Default::default());
            let depth = texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth_view = depth.create_view(&Default::default());
            let scene_color = texture(
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            )
            .create_view(&Default::default());
            let scene_depth = texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            )
            .create_view(&Default::default());
            let shore = texture(
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            queue.write_texture(
                shore.as_image_copy(),
                &[255; 64 * 64],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(64),
                    rows_per_image: Some(64),
                },
                shore.size(),
            );
            let shore = shore.create_view(&Default::default());
            let normal = LakePipeline::normal_map(&device, &queue, &[128, 128, 255, 255], 1, 1);
            let noise = LakePipeline::noise(&device, &queue, &[[0.4, 0.02, -0.03, 0.0]; 4], 2);
            let changing: Vec<[f32; 4]> = (0..64)
                .map(|i| {
                    let x = (i % 8) as f32 * std::f32::consts::TAU / 8.0;
                    let z = (i / 8) as f32 * std::f32::consts::TAU / 8.0;
                    let k = std::f32::consts::TAU / 32.0;
                    [
                        x.sin() * z.cos(),
                        x.cos() * z.cos() * k,
                        -x.sin() * z.sin() * k,
                        0.0,
                    ]
                })
                .collect();
            let moving_noise = LakePipeline::noise(&device, &queue, &changing, 8);
            let bind = |noise: &wgpu::TextureView| {
                pipeline.bindings(
                    &device,
                    &scene_color,
                    &scene_depth,
                    None,
                    None,
                    Some(&shore),
                    Some(noise),
                    None,
                    Some(&normal),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            };
            let bindings = bind(&noise);
            let moving_bindings = bind(&moving_noise);
            let environment = crate::sky_environment::EnvironmentMap::new(
                &device,
                crate::sky_environment::CubeUvLayout::from_max_mip(7),
            );
            let light = crate::sky_environment::CelestialLight {
                direction: glam::Vec3::Y,
                color: glam::Vec3::ONE,
                intensity: 6.6,
                night: false,
            };
            let env = crate::sky_environment::EnvironmentUniforms::new(
                &environment.layout,
                0,
                &light,
                glam::Vec3::Y,
                -glam::Vec3::Y,
                0.5,
            );
            queue.write_buffer(&environment.uniforms, 0, bytemuck::bytes_of(&env));
            let env = pipeline.environment(
                &device,
                &environment.view,
                &environment.sampler,
                &environment.uniforms,
            );
            let view = glam::Mat4::look_at_rh(
                glam::Vec3::new(0., 5., 0.),
                glam::Vec3::ZERO,
                glam::Vec3::Z,
            );
            let projection = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1., 100., 0.1);
            let vp = projection * view;
            let mut camera = SurfaceCamera::zeroed();
            camera.view = view.to_cols_array_2d();
            camera.projection = projection.to_cols_array_2d();
            camera.view_projection = vp.to_cols_array_2d();
            camera.inverse_view = view.inverse().to_cols_array_2d();
            camera.inverse_projection = projection.inverse().to_cols_array_2d();
            camera.inverse_view_projection = vp.inverse().to_cols_array_2d();
            camera.position_time = [0., 5., 0., 1.];
            camera.viewport = [64., 64., 1. / 64., 1. / 64.];
            camera.near = 0.1;
            camera.far = 100.;
            camera.tan_half_fov_y = 1.;
            camera.aspect = 1.;
            let vertices = [
                [-4., 0., -4.],
                [-4., 0., 4.],
                [4., 0., -4.],
                [4., 0., -4.],
                [-4., 0., 4.],
                [4., 0., 4.],
            ]
            .map(|position| LakeVertex {
                position,
                lake: [8., 0., 0.5, 0.],
            });
            let mesh = LakePipeline::vertices(&device, &vertices);
            // Actual moving clipmap, with several seams inside the fixture view.
            // A constant height field must stay constant through every LOD boundary.
            let clipmap = crate::geometry::Clipmap::new(32, 4);
            let clip_scale = 0.125 / clipmap.cell(0) as f32;
            let clip = crate::geometry::build(clipmap, 512.0);
            let clip_end = clip.rings[3] + 32 * 32 * 3 / 4 * 6;
            let clip_vertices: Vec<LakeVertex> = clip
                .vertices
                .iter()
                .take(33 * 33 * 4)
                .map(|v| LakeVertex {
                    position: [v.position[0] * clip_scale, 0.0, v.position[1] * clip_scale],
                    lake: [8.0, 0.0, v.level[0], 0.0],
                })
                .collect();
            let clip_buffer = LakePipeline::vertices(&device, &clip_vertices);
            let mut clip_indices =
                clip.indices[clip.fills[0].start as usize..clip.fills[0].end as usize].to_vec();
            clip_indices
                .extend_from_slice(&clip.indices[clip.rings[0] as usize..clip_end as usize]);
            let clip_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lake seam regression indices"),
                contents: bytemuck::cast_slice(&clip_indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lake readback"),
                size: 64 * 256,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bed = vp.project_point3(glam::Vec3::new(0., -2., 0.)).z;
            let mut pixels = Vec::new();
            let mut heights = Vec::new();
            let mut depth_ranges = Vec::new();
            // (visible PVS, relief, footprint masked, moving noise, time, speed)
            for (case, (visible, relief, masked, moving, time, speed)) in [
                (true, false, false, false, 1.0, 1.0),  // 0 flat, half the lake in PVS.
                (false, false, false, false, 1.0, 1.0), // 1 hidden PVS.
                (true, true, false, false, 1.0, 1.0),   // 2 relief.
                (true, true, true, false, 1.0, 1.0),    // 3 relief, footprint outline.
                (true, true, false, false, 1.0, 1.0),   // 4 multi-ring seam coverage.
                (true, true, false, true, 0.0, 1.0),    // 5 animated at t=0.
                (true, true, false, true, 1.5, 1.0),    // 6 animated at t=1.5.
                (true, true, false, true, 0.0, 0.0),    // 7 frozen at t=0.
                (true, true, false, true, 1.5, 0.0),    // 8 frozen at t=1.5.
            ]
            .into_iter()
            .enumerate()
            {
                let bindings = if moving { &moving_bindings } else { &bindings };
                camera.position_time[3] = time;
                queue.write_buffer(&bindings.camera, 0, bytemuck::bytes_of(&camera));
                let mut uniforms = LakeUniforms::new(0., false);
                uniforms.frames = [[0., 0., 0.5, 2.]; 10];
                uniforms.clipmap = [32., 0., 0., 0.];
                uniforms.deformation = [0.0508, f32::from(relief), 0., speed];
                uniforms.mask = [-4., -4., 0.125, 0.125];
                if case == 4 {
                    uniforms.frames = clipmap
                        .frames((0.23 / clip_scale) as f64, (-0.17 / clip_scale) as f64)
                        .map(|f| {
                            [
                                f[0] * clip_scale,
                                f[1] * clip_scale,
                                f[2] * clip_scale,
                                f[3] / clip_scale,
                            ]
                        });
                    uniforms.clipmap = [32., 0.23, -0.17, 0.];
                }
                queue.write_buffer(&bindings.uniforms, 0, bytemuck::bytes_of(&uniforms));
                let mut boundary = SurfaceBoundary::zeroed();
                boundary.bounds = [-160., -160., 160., 160.];
                boundary.scale = [40., 0., 0., 0.];
                boundary.counts[2] = 1;
                boundary.pvs = [[if visible { 0xffff } else { 0 }; 4]; 8];
                if case == 4 {
                    boundary.bounds = [-400., -400., 400., 400.];
                    boundary.counts[2] = 0;
                }
                if masked {
                    boundary.counts[0] = 1;
                    boundary.triangles[0] = [-160., -160., 0., 160.];
                    boundary.triangles[1] = [-160., 160., 0., 0.];
                    boundary.prepare_candidates();
                    boundary.counts[3] = 3;
                }
                queue.write_buffer(&bindings.boundary, 0, bytemuck::bytes_of(&boundary));
                let mut encoder = device.create_command_encoder(&Default::default());
                // The opaque scene: a black bed 2 m below the water.
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("lake opaque fixture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &scene_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &scene_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(bed),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("lake GPU fixture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &output,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    pass.set_pipeline(&pipeline.pipeline);
                    pass.set_bind_group(0, &bindings.camera_group, &[]);
                    pass.set_bind_group(1, &bindings.material, &[]);
                    pass.set_bind_group(2, &env, &[]);
                    if case == 4 {
                        pass.set_vertex_buffer(0, clip_buffer.slice(..));
                        pass.set_index_buffer(
                            clip_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..clip_indices.len() as u32, 0, 0..1);
                    } else {
                        pass.set_vertex_buffer(0, mesh.slice(..));
                        pass.draw(0..6, 0..1);
                    }
                }
                for (source, aspect) in [
                    (&color, wgpu::TextureAspect::All),
                    (&depth, wgpu::TextureAspect::DepthOnly),
                ] {
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: source,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &read,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(64),
                            },
                        },
                        source.size(),
                    );
                    queue.submit(Some(std::mem::replace(
                        &mut encoder,
                        device.create_command_encoder(&Default::default()),
                    ).finish()));
                    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    let data = read.slice(..).get_mapped_range().unwrap();
                    if aspect == wgpu::TextureAspect::All {
                        pixels.push(
                            data.chunks_exact(4)
                                .filter(|p| p[..3].iter().any(|v| *v > 0))
                                .count(),
                        );
                    } else {
                        // This overhead camera's right vector is -X; admitted
                        // negative-X water lies in the right half of the image.
                        let offset = (32 * 64 + 48) * 4;
                        heights.push(f32::from_le_bytes(
                            data[offset..offset + 4].try_into().unwrap(),
                        ));
                        depth_ranges.push(
                            data.chunks_exact(4)
                                .map(|p| f32::from_le_bytes(p.try_into().unwrap()))
                                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                                    (lo.min(v), hi.max(v))
                                }),
                        );
                    }
                    drop(data);
                    read.unmap();
                }
            }
            let error = scope.pop().await;
            assert!(error.is_none(), "lake GPU validation error: {error:?}");
            assert!(
                pixels[0] > 200 && pixels[0] < 1800,
                "partial PVS should show half the lake: {pixels:?}"
            );
            assert_eq!(pixels[1], 0, "hidden PVS must discard water");
            assert!(
                (heights[0] - heights[2]).abs() > 1e-7,
                "relief must move geometry: {heights:?}"
            );
            assert!(
                pixels[2].abs_diff(pixels[0]) < 100,
                "subtle relief should preserve coverage: {pixels:?}"
            );
            assert!(
                pixels[3] > 100 && pixels[3] < pixels[2] * 3 / 4,
                "the lake must respect the exact triangle outline as well as PVS: {pixels:?}"
            );
            assert_eq!(pixels[4], 4096, "clipmap seams must leave no uncovered pixels");
            assert!(
                depth_ranges[4].1 - depth_ranges[4].0 < 1e-6,
                "identical heights must survive ring transitions: {:?}",
                depth_ranges[4]
            );
            assert!(
                (heights[5] - heights[6]).abs() > 1e-6,
                "nonzero speed must animate the actual mesh over time: {heights:?}"
            );
            assert_eq!(heights[7], heights[8], "zero speed must freeze geometry");
        });
    }
}
