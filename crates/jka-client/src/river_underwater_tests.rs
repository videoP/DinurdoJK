//! Exercise the production submerged passes on the native solved live fixture.
use wgpu::util::DeviceExt;

pub(super) fn check(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    water: &mut crate::river_surface::RiverSurface,
    river: &crate::river_runtime::LiveRiver,
    camera_layout: &wgpu::BindGroupLayout,
    shadow_layout: &wgpu::BindGroupLayout,
) {
    const SIZE: u32 = 32;
    let texture = |format, samples, layers| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("river underwater fixture"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if samples == 1 {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        })
    };
    let noise = texture(wgpu::TextureFormat::Rgba8Unorm, 1, 1);
    let noise_view = noise.create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    let shadow = texture(wgpu::TextureFormat::Depth32Float, 1, 2);
    let shadow_view = shadow.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let shadow_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &[0u8; 352],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: shadow_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: shadow_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&shadow_view),
            },
        ],
    });
    // Flat wave height makes the waterline deterministic; native solved heights
    // and the displaced-grid triangle interpolation remain in the production path.
    let mut settings = crate::river_settings::Settings::default();
    settings.wave_height = 0.0;
    settings.underwater_visibility = 180.0;
    river.set_settings(queue, settings);
    for samples in [1, 4] {
        water.ensure_pipeline(
            device,
            camera_layout,
            shadow_layout,
            wgpu::TextureFormat::Rgba16Float,
            samples,
            wgpu::TextureFormat::Depth32Float,
            wgpu::CompareFunction::GreaterEqual,
        );
        for (position, pitch, wet) in [
            ([0.0, 0.0, 160.0], 0.0, false),  // above the surface
            ([0.0, 0.0, -20.0], 0.0, false),  // below the bed
            ([700.0, 0.0, 10.0], 0.0, false), // outside the patch
            ([0.0, 0.0, 10.0], 0.0, true),
            ([0.0, 0.0, 10.0], 1.2, true), // upward through the underside
        ] {
            let color = texture(wgpu::TextureFormat::Rgba16Float, samples, 1);
            let depth = texture(wgpu::TextureFormat::Depth32Float, samples, 1);
            let resolved = texture(wgpu::TextureFormat::Rgba16Float, 1, 1);
            let color_view = color.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            let resolved_view = resolved.create_view(&Default::default());
            let resolve_target = (samples > 1).then_some(&resolved_view);
            let mut camera =
                crate::camera::Camera::new(crate::scene::render_position(position), 0.0);
            camera.pitch = pitch;
            let view = crate::river_surface::ViewUniform::new(
                &camera,
                [SIZE, SIZE],
                None,
                false,
                4.0,
                1.0 / 60.0,
                false,
            );
            water.prepare(device, queue, [SIZE, SIZE], samples, &view);
            let mut bytes = [0u8; 512];
            bytes[..64].copy_from_slice(bytemuck::cast_slice(
                &camera.view_projection(SIZE, SIZE).to_cols_array(),
            ));
            bytes[96..100].copy_from_slice(&1u32.to_le_bytes());
            let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: camera_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: camera_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&noise_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("underwater initial scene"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.5,
                                g: 0.5,
                                b: 0.5,
                                a: 1.0,
                            }),
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
                    ..Default::default()
                });
            }
            water.capture(device, &mut encoder, &color_view, &depth_view);
            // Dry cases isolate the medium: it must leave every pixel intact.
            if wet {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Hybrid underside"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                water.draw(&mut pass, &camera_group, &shadow_group, None, river);
                water.draw_composite(&mut pass, &camera_group, &shadow_group, None, river);
            }
            water.capture_submerged(device, &mut encoder, &color_view, &depth_view);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Hybrid submerged medium"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                water.draw_submerged(&mut pass, &camera_group, &shadow_group, None, river);
            }
            queue.submit(Some(encoder.finish()));
            let pixels = super::material_tests::read_texture(
                device,
                queue,
                if samples > 1 { &resolved } else { &color },
                0,
            );
            assert!(
                pixels.iter().flatten().all(|v| v.is_finite()),
                "underwater must remain finite, {samples}x, {position:?}"
            );
            assert!(
                pixels.iter().all(|p| p[3] == 1.0),
                "underwater preserves scene alpha"
            );
            if wet {
                assert!(pixels.iter().any(|p|p[0]<0.45 && p[2]>p[0]),"submerged rays must receive wavelength-dependent fog: {samples}x, {position:?}");
            } else {
                assert!(
                    pixels.iter().all(|p| *p == [0.5, 0.5, 0.5, 1.0]),
                    "dry/outside/below-bed eyes must not fog: {samples}x, {position:?}"
                );
            }
            if device
                .features()
                .contains(wgpu::Features::POLYGON_MODE_LINE)
                && !wet
                && position[2] > 100.0
            {
                camera.pitch = -1.4;
                let view = crate::river_surface::ViewUniform::new(
                    &camera,
                    [SIZE, SIZE],
                    None,
                    false,
                    4.0,
                    0.0,
                    false,
                );
                water.prepare(device, queue, [SIZE, SIZE], samples, &view);
                bytes[..64].copy_from_slice(bytemuck::cast_slice(
                    &camera.view_projection(SIZE, SIZE).to_cols_array(),
                ));
                let wire_camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &bytes,
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let wire_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: camera_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wire_camera.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&noise_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("real river mesh overlay"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color_view,
                            depth_slice: None,
                            resolve_target,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    water.draw_wire(&mut pass, &wire_group, &shadow_group, None, river);
                }
                queue.submit(Some(encoder.finish()));
                let pixels = super::material_tests::read_texture(
                    device,
                    queue,
                    if samples > 1 { &resolved } else { &color },
                    0,
                );
                assert!(
                    pixels.iter().any(|p| p[1] > 0.8 && p[0] < 0.1),
                    "actual displaced grid must draw wireframe at {samples}x"
                );
            }
        }
    }
    river.set_settings(queue, crate::river_settings::Settings::default());
}
