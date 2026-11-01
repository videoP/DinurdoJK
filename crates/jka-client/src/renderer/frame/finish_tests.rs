//! Actual Vulkan color/depth parity, ordered submission and timestamp checks.
use super::*;
use wgpu::util::DeviceExt;

fn record(
    encoder: &mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    clear: bool,
    range: std::ops::Range<u32>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("finish parity pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(0.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(range, 0..1);
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn parallel_finish_preserves_loaded_color_depth_submission_and_queries() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let timing =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let features = if adapter.features().contains(timing) {
            timing
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
            .await
            .unwrap();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform = |values: [[f32; 4]; 2]| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&values),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, group)
        };
        let (_, world) = uniform([[0.0, 0.0, 1.0, 1.0], [0.7, 0.0, 0.0, 0.0]]);
        let (model_buffer, model) = uniform([[1.0, 0.0, 0.0, 0.5], [0.9, 0.0, 0.0, 0.0]]);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct Data { color:vec4<f32>, params:vec4<f32> }
                @group(0) @binding(0) var<uniform> data:Data;
                @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
                    let p=array<vec2<f32>,6>(vec2(-0.9,-0.9),vec2(0.9,-0.9),vec2(0.0,0.9),
                        vec2(-0.5,-0.5),vec2(0.5,-0.5),vec2(0.0,0.5));
                    return vec4(p[i],data.params.x,1.0);
                }
                @fragment fn fs()->@location(0)vec4<f32> { return data.color; }
            "#
                .into(),
            ),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |blend: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::renderer::depth_format(),
                    depth_write_enabled: Some(!blend),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = pipeline(false);
        let blended = pipeline(true);
        let mut worker = EncoderFinisher::new().unwrap();
        for depth_value in [0.9f32, 0.3] {
            queue.write_buffer(
                &model_buffer,
                0,
                bytemuck::cast_slice(&[[1.0f32, 0.0, 0.0, 0.5], [depth_value, 0.0, 0.0, 0.0]]),
            );
            let mut images = Vec::new();
            for mode in 0..=2 {
                let color = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 32,
                        height: 32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let depth = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: color.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: crate::renderer::depth_format(),
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let color_view = color.create_view(&Default::default());
                let depth_view = depth.create_view(&Default::default());
                let query = features.contains(timing).then(|| {
                    device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: None,
                        ty: wgpu::QueryType::Timestamp,
                        count: 4,
                    })
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                if let Some(q) = query.as_ref() {
                    encoder.write_timestamp(q, 0);
                }
                if mode == 0 {
                    // Original single pass, including depth and alpha ordering.
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color_view,
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
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&opaque);
                    pass.set_bind_group(0, &world, &[]);
                    pass.draw(0..3, 0..1);
                    pass.set_pipeline(&blended);
                    pass.set_bind_group(0, &model, &[]);
                    pass.draw(3..6, 0..1);
                } else {
                    record(
                        &mut encoder,
                        &color_view,
                        &depth_view,
                        &opaque,
                        &world,
                        true,
                        0..3,
                    );
                }
                if let Some(q) = query.as_ref() {
                    encoder.write_timestamp(q, 1);
                }
                let prefix = if mode == 0 {
                    None
                } else {
                    let world_encoder = std::mem::replace(
                        &mut encoder,
                        device.create_command_encoder(&Default::default()),
                    );
                    if mode == 2 {
                        worker.dispatch(world_encoder, true).unwrap();
                        None
                    } else {
                        Some(world_encoder.finish())
                    }
                };
                if let Some(q) = query.as_ref() {
                    encoder.write_timestamp(q, 2);
                }
                if mode != 0 {
                    record(
                        &mut encoder,
                        &color_view,
                        &depth_view,
                        &blended,
                        &model,
                        false,
                        3..6,
                    );
                }
                if let Some(q) = query.as_ref() {
                    encoder.write_timestamp(q, 3);
                }
                let tail = encoder.finish();
                let prefix = if mode == 2 {
                    let result = worker.wait().unwrap();
                    assert!(result.milliseconds.is_finite());
                    Some(result.buffer)
                } else {
                    prefix
                };
                let pixels = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 32,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut readback = device.create_command_encoder(&Default::default());
                readback.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &color,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &pixels,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(32),
                        },
                    },
                    color.size(),
                );
                let times = query.as_ref().map(|q| {
                    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 32,
                        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    });
                    let mapped = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 32,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    readback.resolve_query_set(q, 0..4, &resolve, 0);
                    readback.copy_buffer_to_buffer(&resolve, 0, &mapped, 0, 32);
                    mapped
                });
                let submission =
                    queue.submit(ordered_buffers(prefix, tail, Some(readback.finish())));
                let (tx, rx) = mpsc::channel();
                pixels
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                let (tx_time, rx_time) = mpsc::channel();
                if let Some(times) = times.as_ref() {
                    times
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |r| tx_time.send(r).unwrap());
                }
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(submission),
                        timeout: Some(std::time::Duration::from_secs(30)),
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let data = pixels.slice(..).get_mapped_range().expect("GPU readback range");
                images.push(
                    data.chunks_exact(256)
                        .flat_map(|r| r[..128].iter().copied())
                        .collect::<Vec<_>>(),
                );
                if let Some(times) = times.as_ref() {
                    rx_time.recv().unwrap().unwrap();
                    let data = times.slice(..).get_mapped_range().expect("GPU readback range");
                    let values = bytemuck::cast_slice::<u8, u64>(&data);
                    assert!(values.windows(2).all(|v| v[0] <= v[1]));
                    assert!(values[3] > values[0]);
                }
            }
            assert_eq!(images[0], images[1]);
            assert_eq!(images[0], images[2]);
            assert!(images[0].chunks_exact(4).any(|p| p[..3] == [0, 0, 255]));
            if depth_value > 0.7 {
                assert!(images[0].chunks_exact(4).any(|p| p[0] > 100 && p[2] > 100));
            } else {
                assert!(!images[0].chunks_exact(4).any(|p| p[0] > 0));
            }
        }
        // Queued job cancellation/shutdown must not deadlock when unconsumed.
        worker
            .dispatch(device.create_command_encoder(&Default::default()), false)
            .unwrap();
        assert!(worker
            .dispatch(device.create_command_encoder(&Default::default()), false)
            .is_err());
        drop(worker);
    });
}
