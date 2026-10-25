//! Upstream lake material with bounded, low density clipmaps, independent of the ocean FFT.
//! Copyright (c) 2026 Bill Yang. MIT; see vendor/blue-water-sea-and-sky/LICENSE.
use crate::original_surface::{
    buffer, entry_buffer, entry_sampler, entry_texture, placeholder, sampled, sampler,
    size_of_binding, storage, uniform, SurfaceBoundary, SurfaceCamera,
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
    pub fn new(wind_speed: f32, direction: f32, transparency: f32, aerial: bool) -> Self {
        let mut u = Self::zeroed();
        u.offset[2] = f32::from(aerial);
        u.tint = [1.0, 1.0, 1.0, 1.0];
        u.reflection = [-1.0, 0.0, 24.0, 64.0];
        u.appearance = [0.27, 0.0, 1.0, 0.1];
        u.transmission = [0.0, 0.01, transparency, 2.4384];
        u.shallow_color = [0.18, 0.40, 0.32, 0.0];
        u.deep_color = [0.03, 0.12, 0.14, 4.0];
        u.ripples = [0.0, 0.0, 0.0, -1.0];
        u.underwater_fog = [0.0, 0.0, 0.0, 1.0];
        u.underwater = [0.0, 1.0, 40.0, 0.0];
        u.wind = [direction.cos(), direction.sin(), wind_speed.max(0.0), 0.0];
        u.set_body_color([0.16, 0.25, 0.12], transparency);
        u
    }
    /// Retain coloured absorption and backscattering (the original lake material),
    /// while exposing the optical colour independently from the surface tint.
    pub fn set_body_color(&mut self, color: [f32; 3], transparency: f32) {
        let clarity = transparency.clamp(0.1, 10.0);
        for (i, c) in color.into_iter().enumerate() {
            let c = c.clamp(0.0, 1.0);
            self.base_color[i] = c;
            self.fresh_absorption[i] = (0.52 - 0.43 * c) / clarity;
            self.fresh_backscatter[i] = (0.003 + 0.030 * c) / clarity;
        }
        // Upstream layout retained; no salt/fresh preset switch.
        self.salt_absorption = self.fresh_absorption;
        self.salt_backscatter = self.fresh_backscatter;
    }
}

pub fn module() -> wgpu::naga::Module {
    let mut composer = crate::shader_composer::composer().expect("lake shader modules");
    let mut source = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/lake.wgsl")
        .replace("\r\n", "\n");
    source = source.replace("    photo_haze: vec4<f32>,", "    photo_haze: vec4<f32>,\n    frames: array<vec4<f32>, 10>,\n    clipmap: vec4<f32>,\n    deformation: vec4<f32>,\n    tint: vec4<f32>,\n    reflection: vec4<f32>,\n    base_color: vec4<f32>,\n    appearance: vec4<f32>,\n    transmission: vec4<f32>,\n    shallow_color: vec4<f32>,\n    deep_color: vec4<f32>,\n    ripples: vec4<f32>,");
    source = source.replace("let body = water_body(bed, deep, column_transmittance(absorption, backscatter, column, view));",
        "let body = water_body(bed, deep, column_transmittance(absorption, backscatter, column, view)) * lake.tint.rgb;");
    source = source.replace(
        "mix(body, sky, reflectance)",
        "mix(body, jka_lake_reflection(world, up, sky), reflectance)",
    );
    source = source.replace(
        "mix(body, jka_lake_reflection(world, up, sky), reflectance) + glint * shadow",
        "mix(body, jka_lake_reflection(world, up, sky), reflectance * lake.tint.w) + glint * shadow * lake.tint.w",
    );
    source = source.replace("let world = vec3<f32>(input.position.x + lake.offset.x, input.position.y, input.position.z + lake.offset.y);",
        "let world = jka_lake_world(input);");
    // Like Gerstner, shade the continuous field per pixel, never the coarse triangle faces.
    source = source.replace("let up = normalize(vec3<f32>(-rippled.slope.x, 1.0, -rippled.slope.y));",
        "let slope = jka_lake_surface(world.xz).yz;\n    let up = normalize(vec3<f32>(-rippled.slope.x - slope.x, 1.0, -rippled.slope.y - slope.y));");
    source.push_str(include_str!("lake_deformation.wgsl"));
    source.push_str(include_str!("cheapwater.wgsl"));
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
    let start = source.find("fn mask_at(").unwrap();
    let end = source[start..].find("\n}").unwrap() + start + 2;
    source.replace_range(start..end, "fn mask_at(world: vec3<f32>) -> f32 { return select(0.0, 1.0, jka_water_contains(world.xz)); }");
    source = source
        .replace(
            "return mask <= contour(share) || sea || hidden;",
            "return mask <= contour(share) || hidden;",
        )
        .replace(
            "camera.view_projection * vec4<f32>(pulled, 1.0)",
            "jka_lake_project(drawn, input.lake.w)",
        );
    source.push_str(crate::shader_composer::SURFACE_BOUNDARY_WGSL);
    composer
        .make_naga_module(NagaModuleDescriptor {
            source: &source,
            file_path: "JKA original lake material",
            shader_type: ShaderType::Wgsl,
            shader_defs: [("LAKE_HIGH".into(), ShaderDefValue::Bool(true))].into(),
            ..Default::default()
        })
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&composer)))
}

pub struct LakePipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub cheap: wgpu::RenderPipeline,
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
}
pub struct LakeBindings {
    pub camera: wgpu::Buffer,
    pub uniforms: wgpu::Buffer,
    pub boundary: wgpu::Buffer,
    pub camera_group: wgpu::BindGroup,
    pub material: wgpu::BindGroup,
}
impl LakePipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let m = module();
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
                sampled(17, frag, t2, false),
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
            label: Some("original Blue Water lake"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(m.clone())),
        });
        let create_pipeline = |wireframe, cheap| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Blue Water lakes and ponds"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LakeVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRS,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if wireframe {
                        "jka_lake_wireframe"
                    } else if cheap {
                        "jka_cheapwater_fragment"
                    } else {
                        "fragment"
                    }),
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
        };
        let pipeline = create_pipeline(false, false);
        let cheap = create_pipeline(false, true);
        let wireframe = device
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE)
            .then(|| create_pipeline(true, false));
        Self {
            pipeline,
            cheap,
            wireframe,
            camera_layout,
            material_layout,
            environment_layout,
            shadows: buffer(
                device,
                "lake disabled object shadows",
                size_of_binding(&m, 0, 1),
                false,
            ),
            motion: buffer(device, "lake motion slot", 144, false),
            occluders: buffer(device, "lake disabled occluders", 16, true),
            occluder_params: buffer(
                device,
                "lake occluder params",
                size_of_binding(&m, 0, 9),
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
        ripples: Option<&wgpu::TextureView>,
        submerged: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
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
                entry_texture(15, ripples.unwrap_or(&self.photo)),
                entry_buffer(16, &boundary),
                entry_texture(17, submerged.map_or(&self.cloud, |p| p.0)),
                entry_texture(18, submerged.map_or(&self.cloud, |p| p.1)),
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
            label: Some("Cheapwater repeating normal map"),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lakes_render_below_sea_level_respect_pvs_and_opaque_depth() {
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
            let pipeline = LakePipeline::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1);
            let _hdr_msaa = LakePipeline::new(&device, wgpu::TextureFormat::Rgba16Float, 4);
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
            let noise = LakePipeline::noise(&device, &queue, &[[0.4, 0.02, -0.03, 0.0]; 4], 2);
            let normal_pixels: Vec<u8> = (0..64)
                .flat_map(|i| {
                    [
                        if i % 8 < 4 { 220 } else { 36 },
                        if i / 8 < 4 { 220 } else { 36 },
                        220,
                        255,
                    ]
                })
                .collect();
            let normal = LakePipeline::normal_map(&device, &queue, &normal_pixels, 8, 8);
            let planar_texture = texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            queue.write_texture(
                planar_texture.as_image_copy(),
                &vec![[255u8, 0, 0, 255]; 64 * 64].concat(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
                planar_texture.size(),
            );
            let planar_view = planar_texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let submerged_color = texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let submerged_pixels: Vec<u8> = (0..64 * 64)
                .flat_map(|i| {
                    if (i % 64 / 8 + i / 64 / 8) % 2 == 0 {
                        [160, 200, 255, 255]
                    } else {
                        [40, 80, 160, 255]
                    }
                })
                .collect();
            queue.write_texture(
                submerged_color.as_image_copy(),
                &submerged_pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
                submerged_color.size(),
            );
            let submerged_color = submerged_color.create_view(&Default::default());
            let submerged_depth = texture(
                wgpu::TextureFormat::R32Float,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            queue.write_texture(
                submerged_depth.as_image_copy(),
                bytemuck::cast_slice(&[1e6f32; 64 * 64]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
                submerged_depth.size(),
            );
            let submerged_depth = submerged_depth.create_view(&Default::default());
            let ripples = crate::lake_ripples::RippleAtlas::new(&device);
            let mut ripple_encoder = device.create_command_encoder(&Default::default());
            ripples.encode(
                &queue,
                &mut ripple_encoder,
                0,
                &[crate::lake_ripples::Stamp {
                    position: [0.5, 0.5, 1.25 / 8.0, 1.0],
                    shape: [1.25, 0.25, 0.1, 0.0],
                }],
            );
            queue.submit(Some(ripple_encoder.finish()));
            let bindings = pipeline.bindings(
                &device,
                &scene_color,
                &scene_depth,
                None,
                None,
                Some(&shore.create_view(&Default::default())),
                Some(&noise),
                Some((&planar_view, &pipeline.linear)),
                Some(&normal),
                Some(&ripples.view),
                Some((&submerged_color, &submerged_depth)),
            );
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
            let moving_bindings = pipeline.bindings(
                &device,
                &scene_color,
                &scene_depth,
                None,
                None,
                Some(&shore.create_view(&Default::default())),
                Some(&moving_noise),
                None,
                Some(&normal),
                Some(&ripples.view),
                Some((&submerged_color, &submerged_depth)),
            );
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
            // A red receiver above the water, limited to a strip at screen right.
            // Water elsewhere can see it only through reflection.
            let make_receiver = |receiver_depth: f32| {
                let receiver_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("SSR red receiver"),
                    source: wgpu::ShaderSource::Wgsl(
                        format!(
                            r#"
                    @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
                        let p = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
                        return vec4(p[i], {receiver_depth}, 1.);
                    }}
                    @fragment fn fs() -> @location(0) vec4<f32> {{ return vec4(1.,0.,0.,1.); }}
                "#
                        )
                        .into(),
                    ),
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("SSR receiver fixture"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &receiver_shader,
                        entry_point: Some("vs"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &receiver_shader,
                        entry_point: Some("fs"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba16Float,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: Default::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
            let receiver_pipeline = make_receiver(vp.project_point3(glam::Vec3::new(0., 2., 0.)).z);
            let bed_pipeline = make_receiver(vp.project_point3(glam::Vec3::new(0., -2., 0.)).z);
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
            queue.write_buffer(&bindings.camera, 0, bytemuck::bytes_of(&camera));
            queue.write_buffer(
                &bindings.uniforms,
                0,
                bytemuck::bytes_of(&LakeUniforms::new(4., 0., 1., false)),
            );
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
            let mut images = Vec::new();
            let mut pixels = Vec::new();
            let mut heights = Vec::new();
            let mut colors = Vec::new();
            let mut channels: Vec<[u64; 3]> = Vec::new();
            let mut depth_ranges = Vec::new();
            // Bed at -2m must remain visible through a lake at 0m. The original terrain-bake
            // sea-floor rule would remove it. A receiver above the plane must still hide the lake.
            for (case, (visible, receiver_y, relief, masked)) in [
                (true, -2.0, false, false),
                (false, -2.0, true, false),
                (true, 2.0, true, false),
                (true, -2.0, true, false),
                (true, -2.0, true, true),
                (true, -2.0, true, false),
                (true, -2.0, false, false), // White body, reflections off.
                (true, -2.0, false, false), // Red body, reflections off.
                (true, -2.0, true, false),  // Multi-ring seam coverage.
                (true, -2.0, false, false), // Planar world reflection.
                (true, -2.0, false, false), // Sky-only control.
                (true, -2.0, true, false),  // Animated at t=0.
                (true, -2.0, true, false),  // Animated at t=1.5.
                (true, -2.0, true, false),  // Frozen at t=0.
                (true, -2.0, true, false),  // Frozen at t=1.5.
                (true, -2.0, false, false), // Original SSR with red receiver.
                (true, -2.0, false, false), // Original Sky with same receiver.
                (true, -2.0, false, false), // Cheapwater SSR with red receiver.
                (true, -2.0, false, false), // Cheapwater Sky with same receiver.
                (true, -2.0, true, false),  // Cheapwater mesh at t=0.
                (true, -2.0, true, false),  // Cheapwater mesh at t=1.5.
                (true, -2.0, true, false),  // Cheapwater frozen at t=0.
                (true, -2.0, true, false),  // Cheapwater frozen at t=1.5.
                (true, -2.0, false, false), // Opaque control over red bed.
                (true, -2.0, false, false), // Transmission over red bed.
                (true, -2.0, false, false), // Transparent with no distortion.
                (true, -2.0, false, false), // Normal-driven distortion.
                (true, -0.1, false, false), // Shallow color.
                (true, -8.0, false, false), // Deep color.
                (true, -2.0, false, false), // Finite water beyond a short far plane.
                (true, -2.0, false, false), // Infinite water beyond the same far plane.
                (true, -2.0, false, false), // Green body with white specular.
                (true, -2.0, false, false), // Green body, reflections off.
                (true, -2.0, false, false), // Red body with white specular.
                (true, -2.0, false, false), // Red body, reflections off.
                (true, -2.0, false, false), // No bed: transparency enabled.
                (true, -2.0, false, false), // No bed: opaque control.
                (true, -0.0762, false, false), // Three inches: clear red bed.
                (true, -1.2192, false, false), // Four feet: partial fade.
                (true, -2.4384, false, false), // Eight feet: fully opaque.
                (true, -12.192, false, false), // Forty feet: fully opaque, high clarity.
                (true, -2.4384, false, false), // Eight feet: opaque control.
                (true, -12.192, false, false), // Forty feet: opaque control.
                (true, -2.0, false, false), // Local wake normal.
                (true, -2.0, false, false), // Identical geometry without a wake.
                (true, -2.0, false, false), // Foreground silhouette, overhead refraction.
                (true, -2.0, false, false), // Same view without foreground.
                (true, -2.0, false, false), // Foreground silhouette, oblique refraction.
                (true, -2.0, false, false), // Same oblique view without foreground.
                (true, -2.0, false, false), // Underside scrolling normals at t=0.
                (true, -2.0, false, false), // Underside scrolling normals at t=1.5.
                (true, -2.0, true, false),  // Underside mesh relief at t=0.
                (true, -2.0, true, false),  // Underside mesh relief at t=1.5.
                (true, -2.0, false, false), // Underside player ripple.
                (true, -2.0, false, false), // Underside without player ripple.
                (true, -2.0, true, false),  // Underside frozen at t=0.
                (true, -2.0, true, false),  // Underside frozen at t=1.5.
            ]
            .into_iter()
            .enumerate()
            {
                let bindings = if case >= 11 {
                    &moving_bindings
                } else {
                    &bindings
                };
                camera.position_time[3] = if matches!(case, 11 | 13 | 19 | 21 | 49 | 51 | 55) {
                    0.0
                } else if case >= 12 {
                    1.5
                } else {
                    1.0
                };
                let eye = if case >= 49 {
                    glam::Vec3::new(0., -2., 0.)
                } else if case >= 47 {
                    glam::Vec3::new(0., 5., 5.)
                } else {
                    glam::Vec3::new(0., 5., 0.)
                };
                let fixture_view = if case >= 49 {
                    glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Z)
                } else if case >= 47 {
                    glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Y)
                } else {
                    view
                };
                camera.view = fixture_view.to_cols_array_2d();
                camera.inverse_view = fixture_view.inverse().to_cols_array_2d();
                camera.position_time[..3].copy_from_slice(&eye.to_array());
                let fixture_projection = if matches!(case, 29 | 30) {
                    glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1., 1., 0.1)
                } else {
                    projection
                };
                camera.projection = fixture_projection.to_cols_array_2d();
                camera.inverse_projection = fixture_projection.inverse().to_cols_array_2d();
                let fixture_vp = fixture_projection * fixture_view;
                camera.view_projection = fixture_vp.to_cols_array_2d();
                camera.inverse_view_projection = fixture_vp.inverse().to_cols_array_2d();
                camera.far = if matches!(case, 29 | 30) { 1.0 } else { 100.0 };
                queue.write_buffer(&bindings.camera, 0, bytemuck::bytes_of(&camera));
                let mut uniforms = LakeUniforms::new(4., 0., 1., false);
                uniforms.frames = [[0., 0., 0.5, 2.]; 10];
                uniforms.clipmap = [32., 0., 0., 0.];
                uniforms.deformation = [
                    0.0508,
                    f32::from(relief),
                    0.,
                    if matches!(case, 13 | 14 | 21 | 22 | 55 | 56) {
                        0.0
                    } else {
                        1.0
                    },
                ];
                uniforms.mask = [-4., -4., 0.125, 0.125];
                if case == 5 {
                    uniforms.tint = [0.; 4];
                }
                if case == 6 {
                    uniforms.tint = [1., 1., 1., 0.];
                    uniforms.set_body_color([1.; 3], 1.0);
                }
                if case == 7 {
                    uniforms.tint = [1., 1., 1., 0.];
                    uniforms.set_body_color([1., 0., 0.], 1.0);
                }
                if case == 9 || case == 10 {
                    uniforms.tint = [0., 0., 0., 1.];
                    if case == 9 {
                        uniforms.reflection[0] = 0.0;
                        uniforms.reflection[1] = 2.0;
                    }
                }
                if case >= 15 {
                    uniforms.tint = [1.; 4];
                    uniforms.set_body_color([1.; 3], 1.0);
                    uniforms.reflection[1] = if case == 15 || case == 17 { 1.0 } else { 0.0 };
                }
                if case >= 17 && case <= 18 {
                    uniforms.appearance[2] = 0.0;
                }
                if case >= 23 {
                    uniforms.tint[3] = 0.0;
                    uniforms.transmission[0] = if case == 23 { 0.0 } else { 0.8 };
                    uniforms.transmission[1] = if case == 26 { 0.05 } else { 0.0 };
                }
                if case >= 27 {
                    uniforms.transmission[0] = 0.0;
                    uniforms.appearance[2] = 0.0;
                    uniforms.shallow_color = [0.7, 0.0, 0.0, 1.0];
                    uniforms.deep_color = [0.0, 0.0, 0.7, 2.0];
                }
                if case >= 29 {
                    uniforms.shallow_color[3] = 0.0;
                    uniforms.tint[3] = 1.0;
                    uniforms.offset[3] = f32::from(case == 30);
                }
                if case >= 31 {
                    let body = if case < 33 {
                        [0.0, 0.7, 0.0]
                    } else {
                        [0.7, 0.0, 0.0]
                    };
                    uniforms.shallow_color = [body[0], body[1], body[2], 1.0];
                    uniforms.deep_color = [body[0], body[1], body[2], 2.0];
                    uniforms.tint[3] = f32::from(case == 31 || case == 33);
                }
                if case >= 35 {
                    uniforms.transmission[0] = if case == 35 { 0.8 } else { 0.0 };
                }
                if case >= 37 {
                    uniforms.tint = [1.0, 1.0, 1.0, 0.0];
                    uniforms.shallow_color[3] = 0.0;
                    uniforms.transmission[0] = if case <= 40 { 1.0 } else { 0.0 };
                    uniforms.transmission[1] = 0.0;
                    uniforms
                        .set_body_color([1.; 3], if case == 40 || case == 42 { 10.0 } else { 1.0 });
                }
                if case >= 43 {
                    uniforms.tint = [1.; 4];
                    if case == 43 {
                        uniforms.ripples = [-4., -4., 0.125, 0.];
                    }
                }
                if case >= 45 {
                    uniforms.tint[3] = 0.0;
                    uniforms.appearance[2] = 1.0;
                    uniforms.transmission = [1.0, 0.2, 10.0, 100.0];
                    uniforms.set_body_color([1.; 3], 10.0);
                }
                if case >= 49 {
                    uniforms.underwater_fog = [0.1, 0.3, 0.2, 20.0];
                    uniforms.underwater = [1.0, 20.0, 40.0, 0.0];
                    uniforms.transmission[1] = 0.08;
                    if matches!(case, 53 | 54) {
                        uniforms.appearance[2] = 0.0;
                    }
                    if case == 53 {
                        uniforms.ripples = [-4., -4., 0.125, 0.];
                    }
                }
                if case == 8 {
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
                if case == 8 {
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
                let d = if matches!(case, 29 | 30 | 35 | 36) || case >= 49 {
                    0.0
                } else {
                    fixture_vp
                        .project_point3(glam::Vec3::new(0., receiver_y, 0.))
                        .z
                };
                let foreground_pipeline = matches!(case, 45 | 47).then(|| {
                    make_receiver(
                        fixture_vp
                            .project_point3(glam::Vec3::new(
                                0.,
                                if case == 45 { 2.0 } else { 0.25 },
                                0.,
                            ))
                            .z,
                    )
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("lake opaque fixture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &scene_color,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(if (37..45).contains(&case) {
                                    wgpu::Color {
                                        r: 1.,
                                        g: 0.,
                                        b: 0.,
                                        a: 1.,
                                    }
                                } else {
                                    wgpu::Color::BLACK
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &scene_depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(d),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    if (15..=18).contains(&case) || (23..=26).contains(&case) {
                        pass.set_pipeline(if case >= 23 {
                            &bed_pipeline
                        } else {
                            &receiver_pipeline
                        });
                        pass.set_scissor_rect(48, 0, 16, 64);
                        pass.draw(0..3, 0..1);
                    }
                    if let Some(pipeline) = &foreground_pipeline {
                        pass.set_pipeline(pipeline);
                        if case == 45 {
                            pass.set_scissor_rect(0, 0, 16, 64);
                        } else {
                            pass.set_scissor_rect(0, 16, 64, 16);
                        }
                        pass.draw(0..3, 0..1);
                    }
                }
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
                    pass.set_pipeline(if case >= 17 {
                        &pipeline.cheap
                    } else {
                        &pipeline.pipeline
                    });
                    pass.set_bind_group(0, &bindings.camera_group, &[]);
                    pass.set_bind_group(1, &bindings.material, &[]);
                    pass.set_bind_group(2, &env, &[]);
                    if case == 8 {
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
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &color,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(64),
                        },
                    },
                    wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit(Some(encoder.finish()));
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                let data = read.slice(..).get_mapped_range().unwrap();
                images.push(data.to_vec());
                channels.push(std::array::from_fn::<_, 3, _>(|c| {
                    data.chunks_exact(4).map(|p| p[c] as u64).sum()
                }));
                colors.push(
                    data.chunks_exact(4)
                        .map(|p| p[..3].iter().map(|&v| v as u64).sum::<u64>())
                        .sum::<u64>(),
                );
                pixels.push(
                    data.chunks_exact(4)
                        .filter(|p| p[..3].iter().any(|v| *v > 0))
                        .count(),
                );
                drop(data);
                read.unmap();
                // Read the actual surface depth too: shading alone cannot pass this check.
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &depth,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::DepthOnly,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(64),
                        },
                    },
                    depth.size(),
                );
                queue.submit(Some(encoder.finish()));
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                let data = read.slice(..).get_mapped_range().unwrap();
                // This overhead camera's right vector is -X; admitted negative-X
                // water lies in the right half of the image.
                // Looking up reverses screen X: the admitted half is now left.
                let offset = (32 * 64 + if case >= 49 { 16 } else { 48 }) * 4;
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
                drop(data);
                read.unmap();
            }
            let error = scope.pop().await;
            assert!(error.is_none(), "lake GPU validation error: {error:?}");
            assert!(
                pixels[0] > 200 && pixels[0] < 1800,
                "partial PVS should show half the lake: {pixels:?}"
            );
            assert_eq!(pixels[1], 0, "hidden PVS must discard water");
            assert_eq!(pixels[2], 0, "opaque geometry in front must occlude water");
            assert!(
                (heights[0] - heights[3]).abs() > 1e-7,
                "relief must move geometry: {heights:?}"
            );
            assert!(
                pixels[3].abs_diff(pixels[0]) < 100,
                "subtle relief should preserve coverage: {pixels:?}"
            );
            assert!(
                pixels[4] > 100 && pixels[4] < pixels[3] * 3 / 4,
                "clipmap must respect the exact triangle outline as well as PVS: {pixels:?}"
            );
            assert!(
                colors[3] > colors[5],
                "water tint must affect the rendered body: {colors:?}"
            );
            assert!(
                channels[6][0].abs_diff(channels[6][1]) < 100
                    && channels[6][1].abs_diff(channels[6][2]) < 100,
                "white must stay neutral without reflections: {:?}",
                channels[6]
            );
            assert!(
                channels[7][0] > 1000
                    && channels[7][0] > channels[7][1] * 10
                    && channels[7][0] > channels[7][2] * 10,
                "chosen body color must not inherit an aqua preset: {:?}",
                channels[7]
            );
            assert_eq!(
                pixels[8], 4096,
                "clipmap seams must leave no uncovered pixels"
            );
            assert!(
                depth_ranges[8].1 - depth_ranges[8].0 < 1e-6,
                "identical heights must survive ring transitions: {:?}",
                depth_ranges[8]
            );
            assert!(
                channels[9][0] > channels[10][0] + 1000,
                "lake must sample the reflected world image: {:?} vs {:?}",
                channels[9],
                channels[10]
            );
            assert!(
                (heights[11] - heights[12]).abs() > 1e-6,
                "nonzero speed must visibly animate the actual mesh over time: {heights:?}"
            );
            assert_eq!(heights[13], heights[14], "zero speed must freeze geometry");
            assert!(
                (heights[19] - heights[20]).abs() > 1e-6,
                "Cheapwater must animate the actual mesh"
            );
            assert_eq!(
                heights[21], heights[22],
                "Cheapwater zero speed must freeze the mesh"
            );
            assert!(
                channels[24][0] > channels[23][0] + 1000,
                "transmission must reveal the red bed: {:?}",
                &channels[23..25]
            );
            assert_ne!(
                images[25], images[26],
                "normal-map refraction must distort the visible bed"
            );
            assert!(
                channels[27][0] > channels[28][0] + 500 && channels[28][2] > channels[27][2] + 500,
                "bed depth must select shallow/deep colors: {:?}",
                &channels[27..29]
            );
            assert_eq!(pixels[29], 0, "finite water must retain the far clip");
            assert!(pixels[30] > 200, "infinite water must survive distanceCull");
            assert_eq!(
                images[35], images[36],
                "no-bed transmission fast path must preserve the opaque horizon"
            );
            assert!(
                channels[37][0] > pixels[37] as u64 * 230,
                "three-inch water should remain very clear: {:?}, {} pixels",
                channels[37],
                pixels[37]
            );
            let shallow_signal = channels[37][0] - channels[37][1];
            let middle_signal = channels[38][0] - channels[38][1];
            assert!(
                middle_signal > 1000 && middle_signal < shallow_signal * 3 / 4,
                "four feet should fade smoothly between clear and opaque"
            );
            assert_eq!(
                images[39], images[41],
                "bed visibility must reach exactly zero by eight feet"
            );
            assert_eq!(
                images[40], images[42],
                "forty-foot bed must stay hidden even at maximum clarity"
            );
            assert_ne!(
                images[43], images[44],
                "local wakes must change surface lighting"
            );
            assert_eq!(
                heights[43], heights[44],
                "normal wakes must preserve mesh height"
            );
            for case in [45, 47] {
                let leaked = images[case]
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(i, p)| {
                        let outside = if case == 45 {
                            i % 64 >= 16
                        } else {
                            !(16..32).contains(&(i / 64))
                        };
                        outside && p[0] > p[1].saturating_add(2)
                    })
                    .count();
                assert_eq!(
                    leaked, 0,
                    "foreground color must not leak into refracted water, case {case}"
                );
            }
            assert_ne!(
                images[49], images[50],
                "underside ripple normals must keep scrolling"
            );
            assert_eq!(
                heights[49], heights[50],
                "zero amplitude stays flat while normals animate"
            );
            assert!(
                (heights[51] - heights[52]).abs() > 1e-6,
                "lake amplitude must deform the mesh from below too: {} vs {}",
                heights[51],
                heights[52]
            );
            assert_ne!(
                images[53], images[54],
                "player wakes must also bend the underside"
            );
            assert_eq!(
                heights[53], heights[54],
                "normal wakes do not add mesh displacement"
            );
            assert_eq!(
                images[55], images[56],
                "zero movement speed freezes the underside too"
            );
            for (on, off) in [(31, 32), (33, 34)] {
                let added: [i64; 3] =
                    std::array::from_fn(|c| channels[on][c] as i64 - channels[off][c] as i64);
                assert!(
                    added[2] > 1000
                        && added[0].abs_diff(added[2]) < 2000
                        && added[1].abs_diff(added[2]) < 2000,
                    "white-light specular must stay neutral on colored shallow water: {added:?}"
                );
            }
            for (ssr, sky) in [(15, 16), (17, 18)] {
                assert!(
                    channels[ssr][0] > channels[sky][0] + 50,
                    "SSR must reflect the red scene receiver in each material: {:?} vs {:?}",
                    channels[ssr],
                    channels[sky]
                );
                assert!(
                    channels[ssr][1].abs_diff(channels[sky][1]) < 200,
                    "SSR receiver should contribute red, not a white highlight"
                );
            }
        });
    }
}
