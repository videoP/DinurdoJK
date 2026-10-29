use super::*;
use wgpu::util::DeviceExt;

#[test]
#[ignore = "requires a Vulkan GPU"]
fn legacy_model_blends_match_display_equations_for_overlapping_layers() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let md3 = include_str!("../../md3.wgsl");
        let declaration = |name| crate::renderer::rt_models::declaration(md3, name).unwrap();
        // The actual unlit fragment, colour transfers and blend pipeline are
        // exercised. Only the camera/pose/fog are replaced by fixture geometry.
        let source = format!(
            r#"
{}
{}
override ENABLE_LEGACY_FOG:bool = false;
override FORCE_FOG_BLACK:bool = false;
override ENABLE_MODEL_WET:bool = false;
@group(1) @binding(0) var base_texture:texture_2d<f32>;
@group(1) @binding(1) var base_sampler:sampler;
fn fogged(c:vec4<f32>,p:vec3<f32>,b:bool)->vec4<f32>{{return c;}}
{}
@vertex fn vs_main(@builtin(vertex_index) i:u32)->VertexOut {{
    let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var v:VertexOut;v.clip_position=vec4(p[i],0.5,1.0);v.uv=vec2(0.5);
    v.color=vec4(0.8,0.5,0.2,1.0);v.normal=vec3(0.0,1.0,0.0);
    v.world_position=vec3(0.0);v.water_wetness=0.0;v.legacy_blob=0.0;return v;
}}
@fragment {}
@fragment fn fs_unlit_fog_black(input:VertexOut)->@location(0) vec4<f32>{{return fs_unlit(input);}}
"#,
            include_str!("../../legacy_model_blend.wgsl").replace(
                "const LEGACY_ENCODED_BLEND: bool = false;",
                "const LEGACY_ENCODED_BLEND: bool = true;"
            ),
            declaration("struct VertexOut"),
            declaration("fn legacy_blob_alpha("),
            declaration("fn fs_unlit(")
        );
        // WGSL entry points cannot call another entry point. The black-fog twin
        // uses the same production fragment body with a different name.
        let source = source.replace("@fragment fn fs_unlit_fog_black(input:VertexOut)->@location(0) vec4<f32>{return fs_unlit(input);}",
            &format!("@fragment {}",declaration("fn fs_unlit(").replace("fn fs_unlit(","fn fs_unlit_fog_black(")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production legacy model fragment fixture"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &empty_layout,
            entries: &[],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&empty_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            source_texture.as_image_copy(),
            &[38, 64, 110, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            source_texture.size(),
        );
        let view = source_texture.create_view(&Default::default());
        let sampler = device.create_sampler(&Default::default());
        let texture_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &vec![0; 3 * std::mem::size_of::<crate::renderer::DynamicModelVertex>()],
            usage: wgpu::BufferUsages::VERTEX,
        });
        let decode = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let encode = |v: f64| {
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float,
        ] {
            for samples in [1, 4] {
                let blend = LegacyModelBlend::new(&device, (1, 1, samples, format));
                let make = |format, samples| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: None,
                        size: source_texture.size(),
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | if samples == 1 { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
                        view_formats: &[],
                    })
                };
                let scene = make(format, 1);
                let scene_view = scene.create_view(&Default::default());
                let msaa = (samples > 1).then(|| make(format, samples));
                let msaa_view = msaa.as_ref().map(|t| t.create_view(&Default::default()));
                let depth = make(DEPTH_FORMAT, samples);
                let depth_view = depth.create_view(&Default::default());
                let encode_source = blend.bind(&device, &scene_view);
                let decode_source = blend.bind(&device, &blend.view);
                for mode in [
                    Mode::AdditiveOne,
                    Mode::DstColorAdd,
                    Mode::SightShellNoDepth,
                ] {
                    let pipeline = super::super::pipelines::create_dynamic_model_pipeline(
                        &device,
                        &layout,
                        &shader,
                        LegacyModelBlend::FORMAT,
                        samples,
                        mode,
                        false,
                        false,
                    );
                    for background in [0.02, 0.2, 0.6] {
                        for layers in [1, 2] {
                            let mut encoder = device.create_command_encoder(&Default::default());
                            {
                                let _pass =
                                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: None,
                                        color_attachments: &[Some(
                                            wgpu::RenderPassColorAttachment {
                                                view: &scene_view,
                                                depth_slice: None,
                                                resolve_target: None,
                                                ops: wgpu::Operations {
                                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                                        r: decode(background),
                                                        g: decode(background),
                                                        b: decode(background),
                                                        a: 1.0,
                                                    }),
                                                    store: wgpu::StoreOp::Store,
                                                },
                                            },
                                        )],
                                        ..Default::default()
                                    });
                            }
                            {
                                let mut pass =
                                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: None,
                                        color_attachments: &[Some(
                                            wgpu::RenderPassColorAttachment {
                                                view: blend
                                                    .msaa_view
                                                    .as_ref()
                                                    .unwrap_or(&blend.view),
                                                depth_slice: None,
                                                resolve_target: blend
                                                    .msaa_view
                                                    .as_ref()
                                                    .map(|_| &blend.view),
                                                ops: wgpu::Operations {
                                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                                    store: wgpu::StoreOp::Store,
                                                },
                                            },
                                        )],
                                        depth_stencil_attachment: Some(
                                            wgpu::RenderPassDepthStencilAttachment {
                                                view: &depth_view,
                                                depth_ops: Some(wgpu::Operations {
                                                    load: wgpu::LoadOp::Clear(
                                                        crate::renderer::camera_depth_clear(),
                                                    ),
                                                    store: wgpu::StoreOp::Store,
                                                }),
                                                stencil_ops: None,
                                            },
                                        ),
                                        ..Default::default()
                                    });
                                pass.set_pipeline(&blend.encode);
                                pass.set_bind_group(0, &encode_source, &[]);
                                pass.draw(0..3, 0..1);
                                pass.set_pipeline(&pipeline);
                                pass.set_bind_group(0, &camera, &[]);
                                pass.set_bind_group(1, &texture_group, &[]);
                                pass.set_vertex_buffer(0, vertex.slice(..));
                                for _ in 0..layers {
                                    pass.draw(0..3, 0..1);
                                }
                            }
                            {
                                let mut pass =
                                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                        label: None,
                                        color_attachments: &[Some(
                                            wgpu::RenderPassColorAttachment {
                                                view: msaa_view.as_ref().unwrap_or(&scene_view),
                                                depth_slice: None,
                                                resolve_target: msaa_view
                                                    .as_ref()
                                                    .map(|_| &scene_view),
                                                ops: wgpu::Operations {
                                                    load: wgpu::LoadOp::Load,
                                                    store: wgpu::StoreOp::Store,
                                                },
                                            },
                                        )],
                                        ..Default::default()
                                    });
                                pass.set_pipeline(&blend.decode);
                                pass.set_bind_group(0, &decode_source, &[]);
                                pass.draw(0..3, 0..1);
                            }
                            let read = device.create_buffer(&wgpu::BufferDescriptor {
                                label: None,
                                size: 256,
                                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            });
                            encoder.copy_texture_to_buffer(
                                scene.as_image_copy(),
                                wgpu::TexelCopyBufferInfo {
                                    buffer: &read,
                                    layout: wgpu::TexelCopyBufferLayout {
                                        offset: 0,
                                        bytes_per_row: Some(256),
                                        rows_per_image: Some(1),
                                    },
                                },
                                scene.size(),
                            );
                            queue.submit([encoder.finish()]);
                            read.slice(..)
                                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                            device
                                .poll(wgpu::PollType::Wait {
                                    submission_index: None,
                                    timeout: None,
                                })
                                .unwrap();
                            let data = read.slice(..).get_mapped_range().unwrap();
                            for channel in 0..3 {
                                let source: f64 =
                                    [38.0 / 255.0 * 0.8, 64.0 / 255.0 * 0.5, 110.0 / 255.0 * 0.2]
                                        [channel];
                                let expected = if mode == Mode::DstColorAdd {
                                    background * (1.0 + source).powi(layers)
                                } else {
                                    background + source * f64::from(layers)
                                };
                                let shown = if format.is_srgb() {
                                    f64::from(data[channel]) / 255.0
                                } else {
                                    let bits = u16::from_le_bytes([
                                        data[2 * channel],
                                        data[2 * channel + 1],
                                    ]);
                                    let e = ((bits >> 10) & 31) as i32;
                                    encode(if e == 0 {
                                        f64::from(bits & 1023) * 2.0f64.powi(-24)
                                    } else {
                                        (1.0 + f64::from(bits & 1023) / 1024.0)
                                            * 2.0f64.powi(e - 15)
                                    })
                                };
                                assert!((shown-expected).abs()<0.007,"{format:?} samples={samples} {mode:?} background={background} layers={layers} channel={channel}: {shown} != {expected}");
                            }
                        }
                    }
                }
            }
        }
        assert!(scope.pop().await.is_none());
    });
}
