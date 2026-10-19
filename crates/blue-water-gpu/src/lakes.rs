//! Upstream lake material with exact JKA footprints, independent of the ocean FFT/clipmap.
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
}
impl LakeUniforms {
    pub fn new(wind_speed: f32, direction: f32, transparency: f32, aerial: bool) -> Self {
        let mut u = Self::zeroed();
        u.offset[2] = f32::from(aerial);
        u.wind = [direction.cos(), direction.sin(), wind_speed.max(0.0), 0.0];
        let clarity = transparency.clamp(0.1, 10.0);
        // The original lake's freshwater and lagoon coefficients (lakes.rs).
        u.fresh_absorption = [0.31 / clarity, 0.17 / clarity, 0.52 / clarity, 0.0];
        u.fresh_backscatter = [0.010 / clarity, 0.012 / clarity, 0.013 / clarity, 0.0];
        u.salt_absorption = [0.30 / clarity, 0.09 / clarity, 0.12 / clarity, 0.0];
        u.salt_backscatter = [0.032 / clarity, 0.040 / clarity, 0.042 / clarity, 0.0];
        u
    }
}

pub fn module() -> wgpu::naga::Module {
    let mut composer = crate::shader_composer::composer().expect("lake shader modules");
    let mut source = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/lake.wgsl")
        .replace("\r\n", "\n");
    // JKA supplies exact geometry and PVS, not upstream's coarse terrain grid or sea-level bake.
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
        .replace("vec4<f32>(pulled, 1.0)", "vec4<f32>(drawn, 1.0)");
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
                sampled(1, frag, t2, false),
                sampler(2, frag, false),
                sampled(3, frag, t2, false),
                sampled(4, frag, t2, true),
                sampled(5, frag, t2, false),
                sampler(6, frag, false),
                uniform(7, frag),
                sampled(8, frag, wgpu::TextureViewDimension::D2Array, false),
                sampler(9, frag, false),
                uniform(16, frag),
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
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
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
        });
        Self {
            pipeline,
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
                entry_texture(1, &self.cloud),
                entry_sampler(2, &self.linear),
                entry_texture(3, color),
                entry_texture(4, depth),
                entry_texture(5, cloud_view),
                entry_sampler(6, cloud_sampler),
                entry_buffer(7, cloud_params),
                entry_texture(8, &self.photo),
                entry_sampler(9, &self.linear),
                entry_buffer(16, &boundary),
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
            label: Some("flat lake footprint"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        })
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
                wgpu::TextureUsages::RENDER_ATTACHMENT,
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
            let bindings = pipeline.bindings(&device, &scene_color, &scene_depth, None, None);
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
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lake readback"),
                size: 64 * 256,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut pixels = Vec::new();
            // Bed at -2m must remain visible through a lake at 0m. The original terrain-bake
            // sea-floor rule would remove it. A receiver above the plane must still hide the lake.
            for (visible, receiver_y) in [(true, -2.0), (false, -2.0), (true, 2.0)] {
                let mut boundary = SurfaceBoundary::zeroed();
                boundary.bounds = [-160., -160., 160., 160.];
                boundary.scale = [40., 0., 0., 0.];
                boundary.counts[2] = 1;
                boundary.pvs = [[if visible { 0xffff } else { 0 }; 4]; 8];
                queue.write_buffer(&bindings.boundary, 0, bytemuck::bytes_of(&boundary));
                let d = vp.project_point3(glam::Vec3::new(0., receiver_y, 0.)).z;
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                                load: wgpu::LoadOp::Clear(d),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
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
                    pass.set_pipeline(&pipeline.pipeline);
                    pass.set_bind_group(0, &bindings.camera_group, &[]);
                    pass.set_bind_group(1, &bindings.material, &[]);
                    pass.set_bind_group(2, &env, &[]);
                    pass.set_vertex_buffer(0, mesh.slice(..));
                    pass.draw(0..6, 0..1);
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
                pixels.push(
                    data.chunks_exact(4)
                        .filter(|p| p[..3].iter().any(|v| *v > 0))
                        .count(),
                );
                drop(data);
                read.unmap();
            }
            assert!(scope.pop().await.is_none(), "lake GPU validation error");
            assert!(
                pixels[0] > 200 && pixels[0] < 1800,
                "partial PVS should show half the lake: {pixels:?}"
            );
            assert_eq!(pixels[1], 0, "hidden PVS must discard water");
            assert_eq!(pixels[2], 0, "opaque geometry in front must occlude water");
        });
    }
}
