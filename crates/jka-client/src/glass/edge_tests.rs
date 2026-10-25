use super::*;

fn half_value(bytes: &[u8]) -> f32 {
    let bits = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
    if bits & 0x7fff == 0 {
        return 0.0;
    }
    f32::from_bits(
        ((bits & 0x8000) << 16) | (((bits >> 10 & 31) + 112) << 23) | ((bits & 1023) << 13),
    )
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly"]
fn glass_controls_gpu_preserve_pane_edges_and_exclude_foreground_halos() {
    futures_lite::future::block_on(async {
        let adapter = wgpu::Instance::default()
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let extent = wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        };
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glass test opaque depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let resources = Resources::new(&device, 64, 64, 1, &depth_view);
        let mut coverage = Vec::<u16>::new();
        let mut opaque_depth = Vec::<f32>::new();
        let mut colors = Vec::<u16>::new();
        for y in 0..64 {
            for x in 0..64 {
                let pane = (8..56).contains(&x) && (8..56).contains(&y);
                let actor = (24..40).contains(&x) && (24..40).contains(&y);
                coverage.extend([
                    0,
                    0,
                    if pane { 0x4900 } else { 0 },
                    if pane { 0x3c00 } else { 0 },
                ]);
                opaque_depth.push(if actor { 5.0 } else { 20.0 });
                // A red foreground actor over fine green/blue background detail.
                colors.extend([
                    if actor { 0x3c00 } else { 0 },
                    if !actor && x % 2 == 0 { 0x3c00 } else { 0 },
                    if !actor && x % 2 != 0 { 0x3c00 } else { 0 },
                    0x3c00,
                ]);
            }
        }
        let write = |texture: &wgpu::Texture, bytes: &[u8], stride| {
            queue.write_texture(
                texture.as_image_copy(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(64),
                },
                extent,
            )
        };
        write(
            &resources._mask_texture,
            bytemuck::cast_slice(&coverage),
            512,
        );
        write(&depth, bytemuck::cast_slice(&opaque_depth), 256);
        let scene = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glass test color pyramid"),
            size: extent,
            mip_level_count: 2,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write(&scene, bytemuck::cast_slice(&colors), 512);
        let world_background = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glass test world without foreground actor"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut background_colors = colors.clone();
        for color in background_colors.chunks_exact_mut(4) {
            if color[0] != 0 {
                color[..3].copy_from_slice(&[0, 0x3a00, 0x3400]);
            }
            color[3] = 0x4d00; // linear world depth 20
        }
        write(
            &world_background,
            bytemuck::cast_slice(&background_colors),
            512,
        );
        let background_view = world_background.create_view(&Default::default());
        let mut recovery_settings = crate::glass::Settings::preset(crate::glass::Style::Bevy);
        recovery_settings.background_recovery = 1.0;
        let blur_settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&recovery_settings.uniform([0.0; 3])),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(include_str!("../glass_blur.wgsl").into()),
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &blur_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blur_shader,
                entry_point: Some("fs_first"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mip = |level| {
            scene.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let input = mip(0);
        let output = mip(1);
        let blur_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &blur_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&resources.mask),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&background_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: blur_settings.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: resources.bounds().as_entire_binding(),
                },
            ],
        });
        let shader_source = format!(
            "{}\n{}",
            crate::glass::shader_bindings(0),
            r#"
@vertex fn vs_test(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(p[i],0.0,1.0);
}
@fragment fn fs_test(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
    if (glass_settings.environment.x>2.5) {
        return vec4<f32>(glass_background(vec2<f32>(32.5/64.0),vec2<f32>(32.5/64.0),2.0),1.0);
    }
    if (glass_settings.environment.x>1.5) {
        return vec4<f32>(glass_background(vec2<f32>(32.5/64.0),vec2<f32>(32.5/64.0),10.0),1.0);
    }
    if (glass_settings.environment.x>0.5) {
        return vec4<f32>(glass_background(vec2<f32>(40.1/64.0,32.5/64.0),vec2<f32>(41.5/64.0,32.5/64.0),10.0),1.0);
    }
    let identity=mat4x4<f32>(vec4<f32>(1.0,0.0,0.0,0.0),vec4<f32>(0.0,1.0,0.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,0.0,1.0));
    return glass_shade(vec3<f32>(0.0),vec3<f32>(0.0,0.0,1.0),p,identity,
        vec3<f32>(0.0,0.0,10.0),vec3<f32>(0.0,0.0,-1.0),0.0,vec3<f32>(1.0),0.04);
}
"#
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let optics_layout = crate::ocean::optics::layout(&device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&optics_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_test"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_test"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let inert = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 384],
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let all_mips = scene.create_view(&Default::default());
        let field_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 8192,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let result_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 7 * 32768,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let reach = Reach { edge: edge_pad(64), blur: recovery_settings.blur_reach(1) };
        resources.locate(&queue, &mut encoder, reach, true);
        resources.resolve(&mut encoder, 64);
        encoder.copy_texture_to_buffer(
            resources.textures[0].as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &field_read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(32),
                },
            },
            wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 1,
            },
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&blur_pipeline);
            pass.set_bind_group(0, &blur_group, &[]);
            pass.draw(0..6, 1..2);
        }
        for case in 0..7 {
            let mut settings = crate::glass::Settings::preset(crate::glass::Style::Bevy);
            settings.distortion = 0.0;
            settings.reflection = 0.0;
            settings.roughness = 0.0;
            settings.foreground_guard = 0.0;
            settings.tint_strength = 0.0;
            settings.blur = 8.0;
            settings.frost = if case == 1 { 1.0 } else { 0.0 };
            if case >= 2 {
                settings.opacity = 1.0;
                settings.tint = [0.2, 0.4, 0.6];
            }
            if case == 3 {
                settings.effect_strength = 0.0;
            }
            let mut data = settings.uniform([0.0; 3]);
            if case == 4 {
                data.environment[0] = 1.0;
                data.highlights[3] = 0.0;
            }
            if case >= 5 {
                data.environment[0] = (case - 3) as f32;
                data.highlights[3] = 1.0;
            }
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&data),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &optics_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&all_mips),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&depth_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: inert.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&resources.field),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&resources.mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&background_view),
                    },
                ],
            });
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &result_read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: case * 32768,
                        bytes_per_row: Some(512),
                        rows_per_image: Some(64),
                    },
                },
                extent,
            );
        }
        let submission = queue.submit([encoder.finish()]);
        for buffer in [&field_read, &result_read] {
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission.clone()),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
        }
        assert!(
            scope.pop().await.is_none(),
            "glass GPU resources must validate"
        );
        let field = field_read.slice(..).get_mapped_range().unwrap();
        let start = (16 * 32 + 20) * 8;
        let edge_x = half_value(&field[start..]);
        let edge_y = half_value(&field[start + 2..]);
        let fg_x = half_value(&field[start + 4..]);
        let fg_y = half_value(&field[start + 6..]);
        let point = [41.0 / 64.0, 33.0 / 64.0];
        let distance =
            |x: f32, y: f32| ((x - point[0]).powi(2) + (y - point[1]).powi(2)).sqrt() * 64.0;
        assert!(
            distance(edge_x, edge_y) > 10.0,
            "an actor silhouette must not become a pane boundary"
        );
        assert!(
            distance(fg_x, fg_y) < 3.0,
            "foreground guard must still locate the actor silhouette"
        );
        let pixels = result_read.slice(..).get_mapped_range().unwrap();
        let color = |case: usize| {
            let start = case * 32768 + (32 * 64 + 41) * 8;
            std::array::from_fn::<_, 3, _>(|channel| half_value(&pixels[start + channel * 2..]))
        };
        let sharp = color(0);
        let frost = color(1);
        let opaque = color(2);
        let disabled = color(3);
        assert!(
            sharp[2] > 0.99 && sharp[1] < 0.001,
            "frost 0 must retain sharp authored scene detail"
        );
        assert!(
            frost[0] < 0.001 && frost[1] > 0.1 && frost[2] > 0.1,
            "blur must soften background without leaking the red actor"
        );
        assert!(
            frost[1] + frost[2] > 0.99,
            "excluded foreground pixels must not create a dark halo"
        );
        for (value, expected) in opaque.iter().zip([0.2, 0.4, 0.6]) {
            assert!(
                (value - expected).abs() < 0.002,
                "haze opacity must fill independently of frost"
            );
        }
        assert!(
            disabled[2] > 0.99 && disabled[1] < 0.001,
            "effect strength 0 must restore the original scene sample"
        );
        let refracted = color(4);
        assert!(
            refracted[0] < 0.001 && refracted[1] > 0.99,
            "subpixel refraction beside an actor must reject its bilinear color contribution"
        );
        let recovered = color(5);
        assert!(
            recovered[0] < 0.001
                && (recovered[1] - 0.75).abs() < 0.002
                && (recovered[2] - 0.25).abs() < 0.002,
            "an entirely foreground-occluded sample must recover the hidden world background"
        );
        let behind = color(6);
        assert!(
            behind[0] > 0.99 && behind[1] < 0.001,
            "a model behind the pane must remain in the refracted scene"
        );
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly"]
fn glass_bounds_gpu_pad_visible_panes_and_skip_hidden_ones() {
    futures_lite::future::block_on(async {
        let adapter = wgpu::Instance::default()
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let extent = wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 };
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let resources = Resources::new(&device, 64, 64, 1, &depth_view);
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 3 * 64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Case 0: a 4x2 pane at depth 10 in front of the world (20).
        // Case 1: the same pane behind an opaque wall (5): nothing to run.
        // Case 2: the hidden pane without the depth test: still padded.
        for (case, (wall, use_depth)) in [(20.0f32, true), (5.0, true), (5.0, false)].into_iter().enumerate() {
            let mut coverage = vec![0u16; 64 * 64 * 4];
            for y in 30..32 {
                for x in 20..24 {
                    coverage[(y * 64 + x) * 4 + 2] = 0x4900; // z = 10
                    coverage[(y * 64 + x) * 4 + 3] = 0x3c00; // w = 1
                }
            }
            let write = |texture: &wgpu::Texture, bytes: &[u8], stride| {
                queue.write_texture(
                    texture.as_image_copy(),
                    bytes,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(64) },
                    extent,
                )
            };
            write(&resources._mask_texture, bytemuck::cast_slice(&coverage), 512);
            write(&depth, bytemuck::cast_slice(&vec![wall; 64 * 64]), 256);
            let mut encoder = device.create_command_encoder(&Default::default());
            resources.locate(&queue, &mut encoder, Reach { edge: 6, blur: 3 }, use_depth);
            encoder.copy_buffer_to_buffer(resources.bounds(), 0, &read, case as u64 * 64, 64);
            queue.submit([encoder.finish()]);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) })
            .unwrap();
        rx.recv().unwrap().unwrap();
        assert!(scope.pop().await.is_none(), "glass bounds must validate");
        let data = read.slice(..).get_mapped_range().unwrap();
        let words: &[u32] = bytemuck::cast_slice(&data);
        let case = |index: usize| &words[index * 16..index * 16 + 16];
        // Full-res [14, 30) x [24, 38) halves to [7, 15) x [12, 19): 1 x 1 groups.
        assert_eq!(case(0), &[1, 1, 1, 0, 7, 12, 15, 19, 17, 27, 27, 35, 64, 64, 0, 0]);
        assert_eq!(case(1), &[0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 64, 64, 0, 0]);
        assert_eq!(case(2), case(0), "without the depth test hidden glass still counts");
    });
}
