use super::*;

#[test]
fn regional_shader_validates_without_ray_queries() {
    let source = source().unwrap();
    assert!(!source.contains("ray_query"));
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn regional_taa_filters_only_light_regions_and_rejects_invalid_history() {
    use crate::renderer::{block_on, GpuPointLight, LightingSettings, PostUniform};
    use bytemuck::Zeroable;
    use wgpu::util::DeviceExt;
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut shader_source = source().unwrap();
        shader_source.push_str(
            r#"
@group(2) @binding(1) var<storage, read_write> probe_result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(8.0, 4.0);
    let value = rt_taa_resolve(uv);
    probe_result[id.y * 8u + id.x] = vec4<f32>(value.color, value.history_confidence);
}
"#,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("regional RT TAA probe"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        // Compile the actual two-output post entry, in both existing history
        // formats. The camera/world MSAA resolve precedes this single-sample pass.
        for format in [
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main_taa"),
                    compilation_options: Default::default(),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                }),
                multiview_mask: None,
                cache: None,
            });
        }
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("probe"),
            compilation_options: Default::default(),
            cache: None,
        });
        let tex = |format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 8,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let scene = tex(wgpu::TextureFormat::Rgba16Float);
        let history = tex(wgpu::TextureFormat::Rgba16Float);
        let depth = tex(wgpu::TextureFormat::R32Float);
        let motion = tex(wgpu::TextureFormat::Rg16Float);
        let write = |texture: &wgpu::Texture, data: &[u8], bytes_per_pixel: u32| {
            queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8 * bytes_per_pixel),
                    rows_per_image: Some(4),
                },
                texture.size(),
            )
        };
        let raw: Vec<[f32; 4]> = (0..32)
            .map(|i| {
                let v = if (i % 8 + i / 8) % 2 == 0 { 0.25 } else { 0.75 };
                [v, v * 0.5, v * 0.25, 1.0]
            })
            .collect();
        // Test inputs are positive, normalized and exactly representable except
        // packed depth. Round its mantissa to the nearest half-float value.
        let half_bytes = |pixels: &[[f32; 4]]| {
            pixels
                .iter()
                .flat_map(|p| {
                    p.iter().map(|v| {
                        if *v == 0.0 {
                            0
                        } else {
                            let bits = v.to_bits();
                            let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
                            ((exponent as u32 * 1024) + (((bits & 0x7fffff) + 0x1000) >> 13)) as u16
                        }
                    })
                })
                .collect::<Vec<u16>>()
        };
        write(&scene, bytemuck::cast_slice(&half_bytes(&raw)), 8);
        let packed_depth = 10.0f32.log2() / 20.0 + 0.02;
        write(
            &history,
            bytemuck::cast_slice(&half_bytes(&[[0.5, 0.25, 0.125, packed_depth]; 32])),
            8,
        );
        write(&depth, bytemuck::cast_slice(&[10.0f32; 32]), 4);
        write(&motion, bytemuck::cast_slice(&[0u16; 64]), 4);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut post = PostUniform::zeroed();
        post.inv_view_proj = glam::Mat4::perspective_rh(1.0, 2.0, 1.0, 100.0)
            .inverse()
            .to_cols_array_2d();
        post.scene.history_valid = 1.0;
        let post_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&post),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let mut light = GpuPointLight::zeroed();
        light.position_radius = [-4.0, 0.0, -9.0, 4.0];
        light.color_intensity = [1.0; 4];
        light.emitter[3] = -1.0;
        light.shadow[3] = 1.0;
        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&light),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let mut clusters = vec![0u32; 16 * 9 * 24 * 33];
        for cluster in clusters.chunks_exact_mut(33) {
            cluster[0] = 1;
        }
        let cluster_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&clusters),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut lighting = LightingSettings::zeroed();
        lighting.values = [1, 1, 8, 4];
        let lighting_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&lighting),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 512,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 512,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views = [&scene, &history, &depth, &motion].map(|t| t.create_view(&Default::default()));
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: post_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 21,
                    resource: wgpu::BindingResource::TextureView(&views[3]),
                },
            ],
        });
        let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cluster_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: lighting_buffer.as_entire_binding(),
                },
            ],
        });
        let group2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            }],
        });
        let run = || {
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group0, &[]);
                pass.set_bind_group(1, &group1, &[]);
                pass.set_bind_group(2, &group2, &[]);
                pass.dispatch_workgroups(8, 4, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 512);
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let result =
                bytemuck::cast_slice::<u8, [f32; 4]>(&readback.slice(..).get_mapped_range().expect("GPU readback range"))
                    .to_vec();
            readback.unmap();
            result
        };
        let filtered = run();
        let mut affected = 0;
        let mut untouched = 0;
        for (i, p) in filtered.iter().enumerate() {
            if p[3] > 0.0 {
                affected += 1;
                assert!((p[0] - 0.5).abs() < (raw[i][0] - 0.5).abs(), "{i}: {p:?}");
            } else {
                untouched += 1;
                assert_eq!(&p[..3], &raw[i][..3]);
            }
        }
        assert!(affected > 0 && untouched > 0);
        // No lights, removed light, revealed surface, invalid history and camera
        // reset and native-weather motion markers must all return the current
        // MSAA color, without temporal trails.
        for case in 0..6 {
            write(&motion, bytemuck::cast_slice(&[if case == 5 { 0x4000u16 } else { 0 }; 64]), 4);
            lighting.values[1] = if case == 0 { 0 } else { 1 };
            light.shadow[3] = if case == 1 { 0.0 } else { 1.0 };
            post.scene.history_valid = if case == 4 { 0.0 } else { 1.0 };
            let alpha = if case == 2 {
                packed_depth + 0.05
            } else if case == 3 {
                0.0
            } else {
                packed_depth
            };
            write(
                &history,
                bytemuck::cast_slice(&half_bytes(&[[0.5, 0.25, 0.125, alpha]; 32])),
                8,
            );
            queue.write_buffer(&lighting_buffer, 0, bytemuck::bytes_of(&lighting));
            queue.write_buffer(&light_buffer, 0, bytemuck::bytes_of(&light));
            queue.write_buffer(&post_buffer, 0, bytemuck::bytes_of(&post));
            for (i, p) in run().iter().enumerate() {
                assert_eq!(&p[..3], &raw[i][..3], "case {case} pixel {i}");
            }
        }
        println!("regional TAA: {affected} filtered pixels, {untouched} unchanged; 6 history rejection cases passed");
    });
}
