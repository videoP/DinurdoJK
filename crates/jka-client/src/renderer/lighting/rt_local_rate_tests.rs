use super::*;

// Execute the production cache shader with a counted visibility oracle instead
// of hardware traversal. This measures skipped work, not just output similarity.
fn counted_source() -> String {
    let mut source = shader_source()
        .unwrap()
        .replace("enable wgpu_ray_query;", "");
    let start = source.find("@group(2) @binding(8)").unwrap();
    let last = source.find("@group(2) @binding(14)").unwrap();
    let end = last + source[last..].find(';').unwrap() + 1;
    source.replace_range(start..end, "");
    for name in [
        "rt_alpha_repeat_coord",
        "rt_rgba_texel",
        "rt_sample_rgba",
        "rt_sample_alpha",
        "rt_tc_stretch_wave",
        "rt_alpha_generated_uv",
        "rt_alpha_candidate_blocks",
        "rt_candidate_glass_throughput",
        "rt_trace_shadow_query",
        "rt_trace_shadow_visibility",
    ] {
        let anchor = format!("fn {name}(");
        if let Some(start) = source.find(&anchor) {
            let len = rt_models::declaration(&source, &anchor).unwrap().len();
            source.replace_range(start..start + len, "");
        }
    }
    source.push_str(
        r#"
struct TraceStats { rays: atomic<u32>, visibility: f32 };
@group(1) @binding(9) var<storage, read_write> trace_stats: TraceStats;
fn rt_trace_shadow_visibility(origin: vec3<f32>, direction: vec3<f32>, distance: f32) -> f32 {
    atomicAdd(&trace_stats.rays, 1u);
    return trace_stats.visibility;
}
"#,
    );
    source
}

fn read_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn rt_local_rate_gpu_counts_rays_and_rejects_invalid_history() {
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
        println!("RT rate GPU: {:?}", adapter.get_info());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("counted production RT local cache"),
            source: wgpu::ShaderSource::Wgsl(counted_source().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = |bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytes,
                usage,
            })
        };
        let size = [8, 8];
        let eye = Vec3::new(0.0, 0.0, 4.0);
        let vp = Mat4::perspective_rh(60_f32.to_radians(), 1.0, 0.1, 100.0)
            * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let mut camera = CameraUniform::zeroed();
        camera.camera_pos_time = eye.extend(0.0).to_array();
        camera.render_flags[3] = 7;
        let camera_buffer = buffer(
            bytemuck::bytes_of(&camera),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let texture = |format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 8,
                    height: 8,
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
        let depth = texture(wgpu::TextureFormat::R32Float);
        let motion = texture(wgpu::TextureFormat::Rg32Float);
        let depth_view = depth.create_view(&Default::default());
        let motion_view = motion.create_view(&Default::default());
        let mut depths = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let h = vp.inverse()
                    * Vec4::new(
                        (x as f32 + 0.5) / 4.0 - 1.0,
                        1.0 - (y as f32 + 0.5) / 4.0,
                        0.5,
                        1.0,
                    );
                let direction = (h.truncate() / h.w - eye).normalize();
                depths.push(-eye.z / direction.z);
            }
        }
        queue.write_texture(
            depth.as_image_copy(),
            bytemuck::cast_slice(&depths),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(32),
                rows_per_image: Some(8),
            },
            depth.size(),
        );
        let mut light = GpuPointLight::zeroed();
        light.position_radius = [0.0, 0.0, 20.0, 100.0];
        light.color_intensity = [1.0; 4];
        light.shadow = [0.0, 0.0, 0.1, 1.0];
        let lights = buffer(
            bytemuck::bytes_of(&light),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let mut clusters = vec![0u32; CLUSTER_COUNT as usize * 33];
        for cluster in clusters.chunks_exact_mut(33) {
            cluster[0] = 1;
        }
        let clusters = buffer(bytemuck::cast_slice(&clusters), wgpu::BufferUsages::STORAGE);
        let stats = buffer(
            &[0; 16],
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16 + 8 * 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        for samples in [1u32, 2, 4] {
            for rate in [2u32, 4] {
                let mut history = RtLocalRateHistory::new(&device);
                history.resize(&device, size, samples);
                let mut lighting = LightingSettings::zeroed();
                lighting.values = [1, 1, 8, 8];
                lighting.map_ambient[3] = samples as f32;
                let lighting = buffer(bytemuck::bytes_of(&lighting), wgpu::BufferUsages::UNIFORM);
                let mut settings = Settings {
                    inverse_view_proj: vp.inverse().to_cols_array_2d(),
                    dimensions: [8, 8, 0, rate],
                    flags: [1, 0, 0, 0],
                    prev_camera_pos: eye.extend(0.0).to_array(),
                };
                let run = |settings: &Settings, visibility: f32| {
                    queue.write_buffer(&history.uniform, 0, bytemuck::bytes_of(settings));
                    queue.write_buffer(
                        &stats,
                        0,
                        bytemuck::cast_slice(&[0u32, visibility.to_bits(), 0, 0]),
                    );
                    let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(1),
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: history.uniform.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&depth_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&motion_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(&history.views[0]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(&history.views[1]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: lighting.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 6,
                                resource: lights.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 7,
                                resource: clusters.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 8,
                                resource: wgpu::BindingResource::TextureView(&history.published),
                            },
                            wgpu::BindGroupEntry {
                                binding: 9,
                                resource: stats.as_entire_binding(),
                            },
                        ],
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &camera_group, &[]);
                        pass.set_bind_group(1, &inputs, &[]);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                    encoder.copy_buffer_to_buffer(&stats, 0, &readback, 0, 16);
                    encoder.copy_texture_to_buffer(
                        history.textures[2].as_image_copy(),
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 16,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(8),
                            },
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                    // Keep a known, uniform previous frame for the next independent case.
                    encoder.copy_texture_to_texture(
                        history.textures[2].as_image_copy(),
                        history.textures[0].as_image_copy(),
                        history.textures[2].size(),
                    );
                    queue.submit([encoder.finish()]);
                    let bytes = read_buffer(&device, &readback);
                    let rays = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
                    let mut records = Vec::new();
                    for y in 0..8 {
                        for x in 0..8 {
                            let offset = 16 + y * 256 + x * 16;
                            records.push(bytemuck::pod_read_unaligned::<[u32; 4]>(
                                &bytes[offset..offset + 16],
                            ));
                        }
                    }
                    (rays, records)
                };
                // Initial invalid history traces everything; all samples are shadowed.
                let (rays, _) = run(&settings, 0.0);
                assert_eq!(rays, 64 * samples, "cold samples={samples} rate={rate}");
                settings.flags[1] = 1;
                let (rays, records) = run(&settings, 1.0);
                assert_eq!(
                    rays,
                    64 * samples / rate,
                    "stable samples={samples} rate={rate}"
                );
                for y in 0..8 {
                    for x in 0..8 {
                        let scheduled = if rate == 2 {
                            (x + y) % 2 == 0
                        } else {
                            x % 2 == 0 && y % 2 == 0
                        };
                        assert_eq!(records[y * 8 + x][3] & 1, u32::from(scheduled));
                    }
                }
                settings.flags[1] = 0;
                run(&settings, 0.0);
                settings.flags[1] = 1;
                settings.prev_camera_pos[2] += 10.0;
                assert_eq!(
                    run(&settings, 1.0).0,
                    64 * samples,
                    "depth mismatch rejects history"
                );
                settings.prev_camera_pos = eye.extend(0.0).to_array();
                // Reset to a uniform shadowed frame, then move a light: every pixel
                // must refresh even when it is outside the normal scheduled subset.
                settings.flags[1] = 0;
                run(&settings, 0.0);
                settings.flags[1] = 1;
                light.position_radius[0] += 0.1;
                queue.write_buffer(&lights, 0, bytemuck::bytes_of(&light));
                let (rays, records) = run(&settings, 1.0);
                assert_eq!(rays, 64 * samples, "moving light invalidation");
                assert!(records.iter().all(|r| r[3] & 1 == 1));
                // The previous checker pattern is a shadow edge everywhere.
                settings.flags[1] = 0;
                run(&settings, 0.0);
                settings.flags[1] = 1;
                run(&settings, 1.0);
                let (rays, _) = run(&settings, 1.0);
                assert_eq!(
                    rays,
                    (if rate == 2 { 64 } else { 48 }) * samples,
                    "shadow boundaries refresh immediately"
                );
            }
        }
    });
}
