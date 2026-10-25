//! Real BLAS/TLAS and GPU readback checks for the tinted-glass contract.
use super::*;

fn pane(
    x: f32,
    z: f32,
    tint: [f32; 3],
    glass: bool,
    was_opaque: bool,
    alpha: u8,
) -> RtAlphaMaskSource {
    let mut uniform = MaterialUniform::zeroed();
    uniform.color = [tint[0], tint[1], tint[2], 1.0];
    uniform.params[0] = 0.5;
    RtAlphaMaskSource {
        vertices: [[x - 2.0, -2.0, z], [x + 2.0, -2.0, z], [x, 2.0, z]]
            .map(|p| RtAlphaVertexGpu {
                position_u: [p[0], p[1], p[2], 0.5],
                normal_v: [0.0, 0.0, 1.0, 0.5],
                lightmap_alpha: [0.0, 0.0, 1.0, 0.0],
            })
            .to_vec(),
        indices: vec![0, 1, 2],
        material: RtAlphaMaterialSource {
            uniform,
            glass,
            was_opaque,
            texture: RtAlphaTextureSource {
                key: format!("glass-test-white-alpha-{alpha}"),
                width: 1,
                height: 1,
                clamp: true,
                color: true,
                pixels: vec![0x00ffffff | (u32::from(alpha) << 24)],
            },
        },
    }
}

fn acceptance_shader() -> String {
    let donor = ray_traced_world_shader_source_with_features(
        include_str!("../../bsp.wgsl"),
        ReceiverFeatures {
            glass: true,
        },
    )
    .unwrap();
    let start = donor.find("@group(3) @binding(8)").unwrap();
    let last = donor.find("@group(3) @binding(14)").unwrap();
    let end = last + donor[last..].find(';').unwrap() + 1;
    let mut shader = String::from("enable wgpu_ray_query;\n");
    shader.push_str(&donor[start..end].replace("@group(3)", "@group(0)"));
    shader.push_str(
        r#"
struct TestCamera { camera_pos_time: vec4<f32> };
const camera = TestCamera(vec4<f32>(0.0));
struct TestFlags { enabled: vec4<u32> };
struct TestRay { origin: vec4<f32>, direction: vec4<f32> };
@group(0) @binding(0) var<storage, read_write> results: array<vec4<f32>>;
@group(0) @binding(1) var<uniform> test_flags: TestFlags;
@group(0) @binding(2) var<storage, read> rays: array<TestRay>;
"#,
    );
    for name in [
        "rt_alpha_repeat_coord",
        "rt_rgba_texel",
        "rt_sample_rgba",
        "rt_sample_alpha",
        "rt_tc_stretch_wave",
        "rt_alpha_generated_uv",
        "rt_alpha_candidate_blocks",
        "rt_candidate_glass_throughput",
        "rt_trace_shadow_query_mask",
        "rt_trace_shadow_query",
        "rt_trace_shadow_visibility",
        "rt_trace_shadow_throughput",
    ] {
        shader.push_str(rt_models::declaration(&donor, &format!("fn {name}(")).unwrap());
        shader.push('\n');
    }
    shader.push_str(r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let ray = rays[id.x];
    var color = vec3<f32>(1.0);
    if (test_flags.enabled.x != 0u) {
        color = rt_trace_shadow_throughput(ray.origin.xyz, ray.direction.xyz, ray.direction.w);
    } else {
        color = vec3<f32>(rt_trace_shadow_visibility(ray.origin.xyz, ray.direction.xyz, ray.direction.w));
    }
    results[id.x] = vec4<f32>(color, 1.0);
}
"#);
    shader
}

#[test]
fn rt_glass_acceptance_shader_validates() {
    let source = acceptance_shader();
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
}

#[test]
#[ignore = "requires a Vulkan GPU with hardware ray queries"]
fn rt_glass_gpu_transmission_acceptance() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        println!("Glass acceptance GPU: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
                required_limits: adapter.limits(),
                experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                ..Default::default()
            })
            .await
            .unwrap();
        let red = [1.0, 0.2, 0.1];
        let blue = [0.1, 0.2, 1.0];
        // Every ray has its own separated lane. Nothing is faked at the output:
        // opaque geometry, alpha masks, and glass all use real acceleration structures.
        let mut sources = vec![
            pane(0.0, 2.0, red, true, false, 128),
            pane(8.0, 2.0, red, true, false, 128),
            pane(16.0, 2.0, red, true, false, 128),
            pane(16.0, 4.0, blue, true, false, 128),
            pane(24.0, 2.0, red, true, false, 128),
            pane(32.0, 4.0, red, true, false, 128),
            pane(40.0, 8.0, red, true, false, 128),
            pane(48.0, 2.0, [1.0; 3], false, false, 255),
            pane(56.0, 2.0, [1.0; 3], false, false, 0),
            pane(56.0, 4.0, red, true, false, 128),
            pane(72.0, 2.0, red, true, true, 128),
        ];
        let mut textured = pane(88.0, 2.0, [1.0; 3], true, false, 128);
        textured.material.texture.key = "glass-test-rgb-texture".into();
        textured.material.texture.pixels[0] = 0x804080ff;
        sources.push(textured);
        let (mut vertices, mut indices, mut geometries, mut materials, mut textures, mut texels) = (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let mut texture_cache = HashMap::new();
        let masked = create_rt_mask_blas(
            &device,
            "glass acceptance static",
            &sources,
            &mut vertices,
            &mut indices,
            &mut geometries,
            &mut materials,
            &mut textures,
            &mut texels,
            &mut texture_cache,
        )
        .unwrap();
        let inline = create_rt_mask_blas(
            &device,
            "glass acceptance inline",
            &[pane(0.0, 2.0, blue, true, false, 128)],
            &mut vertices,
            &mut indices,
            &mut geometries,
            &mut materials,
            &mut textures,
            &mut texels,
            &mut texture_cache,
        )
        .unwrap();
        let mut wall_positions = Vec::new();
        let mut wall_indices = Vec::new();
        for (x, z) in [(24.0, 4.0), (32.0, 2.0), (64.0, 2.0)] {
            let first = wall_positions.len() as u32;
            wall_positions.extend([[x - 2.0, -2.0, z], [x + 2.0, -2.0, z], [x, 2.0, z]]);
            wall_indices.extend([first, first + 1, first + 2]);
        }
        let walls = create_rt_triangle_blas(
            &device,
            "glass acceptance walls",
            &wall_positions,
            &wall_indices,
            false,
        )
        .unwrap();
        let alpha = [
            create_rt_alpha_buffer(
                &device,
                "vertices",
                &vertices,
                wgpu::BufferUsages::BLAS_INPUT,
            ),
            create_rt_alpha_buffer(&device, "indices", &indices, wgpu::BufferUsages::BLAS_INPUT),
            create_rt_alpha_buffer(
                &device,
                "geometries",
                &geometries,
                wgpu::BufferUsages::empty(),
            ),
            create_rt_alpha_buffer(
                &device,
                "materials",
                &materials,
                wgpu::BufferUsages::empty(),
            ),
            create_rt_alpha_buffer(&device, "textures", &textures, wgpu::BufferUsages::empty()),
            create_rt_alpha_buffer(&device, "texels", &texels, wgpu::BufferUsages::empty()),
        ];
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("glass acceptance scene"),
            max_instances: 3,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        tlas[0] = Some(wgpu::TlasInstance::new(
            &masked.blas,
            identity,
            masked.geometry_base,
            255,
        ));
        tlas[1] = Some(wgpu::TlasInstance::new(&walls.blas, identity, 0, 255));
        let mut pose = identity;
        pose[3] = 80.0;
        tlas[2] = Some(wgpu::TlasInstance::new(
            &inline.blas,
            pose,
            inline.geometry_base,
            255,
        ));
        let source = acceptance_shader();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass acceptance"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("glass acceptance"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let rays: Vec<[[f32; 4]; 2]> = (0..12)
            .map(|i| {
                let origin_z = if i == 1 { 3.0 } else { 0.0 };
                [[i as f32 * 8.0, 0.0, origin_z, 0.0], [0.0, 0.0, 1.0, 6.0]]
            })
            .collect();
        let ray_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&rays),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let flags = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0u32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bytes = (rays.len() * 16) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: flags.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: ray_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: tlas.as_binding(),
            },
        ];
        entries.extend(
            alpha
                .iter()
                .enumerate()
                .map(|(i, buffer)| wgpu::BindGroupEntry {
                    binding: 9 + i as u32,
                    resource: buffer.as_entire_binding(),
                }),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut build = device.create_command_encoder(&Default::default());
        build.build_acceleration_structures(
            [
                rt_mask_blas_build_entry(&masked, &alpha[0], &alpha[1]),
                rt_mask_blas_build_entry(&inline, &alpha[0], &alpha[1]),
                rt_triangle_blas_build_entry(&walls),
            ]
            .iter(),
            [&tlas],
        );
        queue.submit([build.finish()]);
        let transparency = 1.0 - 128.0 / 255.0;
        let red_t = red.map(|c| c * transparency);
        let blue_t = blue.map(|c| c * transparency);
        let stacked = std::array::from_fn(|i| red_t[i] * blue_t[i]);
        // Authored lightfilter on an opaque stage uses the established nominal
        // thin-sheet opacity, even when its source texture carries alpha.
        let opaque_authored = red.map(|c| c * 0.75);
        let texture_t = [255.0_f32, 128.0, 64.0].map(|c| {
            let srgb = c / 255.0;
            let linear = if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            };
            linear * transparency
        });
        let expected_on = [
            red_t,
            [1.0; 3],
            stacked,
            [0.0; 3],
            [0.0; 3],
            [1.0; 3],
            [0.0; 3],
            red_t,
            [0.0; 3],
            opaque_authored,
            blue_t,
            texture_t,
        ];
        let expected_off = [
            [1.0; 3], [1.0; 3], [1.0; 3], [0.0; 3], [0.0; 3], [1.0; 3], [0.0; 3], [1.0; 3],
            [0.0; 3], [0.0; 3], [1.0; 3], [1.0; 3],
        ];
        let names = [
            "one pane",
            "pane behind receiver",
            "stacked panes",
            "wall after glass",
            "wall before glass",
            "pane beyond light",
            "solid alpha mask",
            "alpha hole then glass",
            "ordinary wall",
            "authored opaque lightfilter",
            "transformed inline glass",
            "sRGB texture transmission",
        ];
        // Toggle on -> off -> on against the SAME TLAS to verify restored baseline
        // classification and that toggling never requires stale caster data.
        for enabled in [true, false, true] {
            queue.write_buffer(
                &flags,
                0,
                bytemuck::cast_slice(&[u32::from(enabled), 0, 0, 0]),
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(rays.len() as u32, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let data = readback.slice(..).get_mapped_range().expect("GPU readback range");
            let expected = if enabled { &expected_on } else { &expected_off };
            for (i, actual) in bytemuck::cast_slice::<u8, [f32; 4]>(&data)
                .iter()
                .enumerate()
            {
                for channel in 0..3 {
                    assert!(
                        (actual[channel] - expected[i][channel]).abs() < 0.0001,
                        "{} enabled={enabled}: got {actual:?}, expected {:?}",
                        names[i],
                        expected[i]
                    );
                }
                println!(
                    "Glass acceptance enabled={enabled} {}: {:?}",
                    names[i],
                    &actual[..3]
                );
            }
            drop(data);
            readback.unmap();
        }
    });
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn raster_glass_gpu_transmission_acceptance() {
    use crate::renderer::lighting::glass_transmission::{
        pack_raster_glass_slot, raster_glass_triangles,
    };
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let base = include_str!("../../bsp.wgsl");
        let mut source = String::new();
        for name in ["PointLight", "RasterGlassTriangle", "RasterGlassSlot"] {
            source.push_str(rt_models::declaration(base, &format!("struct {name} ")).unwrap());
            source.push_str(";\n");
        }
        source.push_str(
            base.lines()
                .find(|line| line.starts_with("const LOCAL_SHADOW_NEAR:"))
                .unwrap(),
        );
        source.push_str(
            r#"
const ENABLE_LOCAL_SHADOWS: bool = true;
struct TestLighting { feature_flags: vec4<u32>, local_shadows: vec4<u32>, baked_sun_debug_mode: vec4<u32> };
@group(0) @binding(0) var<storage, read_write> results: array<vec4<f32>>;
@group(0) @binding(1) var<uniform> lighting_settings: TestLighting;
@group(0) @binding(2) var local_shadow_texture: texture_depth_cube_array;
@group(0) @binding(3) var local_shadow_sampler: sampler_comparison;
@group(0) @binding(17) var<storage, read> raster_glass_slots: array<RasterGlassSlot>;
"#,
        );
        for name in [
            "sample_local_shadow_visibility",
            "baked_local_moving_visibility",
            "local_light_shadow_visibility",
            "raster_glass_throughput",
            "local_light_shadow_rgb",
        ] {
            source.push_str(rt_models::declaration(base, &format!("fn {name}(")).unwrap());
            source.push('\n');
        }
        source.push_str(
            r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let reversed = id.x == 7u;
    let light_z = select(0.0, 6.0, reversed);
    let receiver_z = select(select(6.0, 1.0, id.x == 1u), 0.0, reversed);
    let light = PointLight(vec4<f32>(f32(id.x) * 8.0, 0.0, light_z, 20.0),
        vec4<f32>(1.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(f32(id.x + 1u), 0.0, 0.0, 0.0));
    let normal = vec3<f32>(0.0, 0.0, select(-1.0, 1.0, reversed));
    results[id.x] = vec4<f32>(local_light_shadow_rgb(light,
        vec3<f32>(f32(id.x) * 8.0, 0.0, receiver_z), normal), 1.0);
}
"#,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("raster glass acceptance"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("raster glass acceptance"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let red = [1.0, 0.2, 0.1];
        let blue = [0.1, 0.2, 1.0];
        let slots: Vec<_> = (0..8)
            .map(|i| {
                let x = i as f32 * 8.0;
                let mut panes = if i == 6 {
                    Vec::new()
                } else {
                    vec![pane(
                        x,
                        if i == 3 { 8.0 } else { 2.0 },
                        red,
                        true,
                        false,
                        128,
                    )]
                };
                if i == 2 {
                    panes.push(pane(x, 4.0, blue, true, false, 128));
                }
                let triangles = raster_glass_triangles(&panes);
                pack_raster_glass_slot(
                    [x, 0.0, if i == 7 { 6.0 } else { 0.0 }],
                    20.0,
                    &triangles,
                    &[],
                )
                .0
            })
            .collect();
        let slot_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&slots),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0u32, 0, 512, 0, 1, 8, 16, 0, 0, 0, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cubes = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("acceptance opaque depth cubemaps"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 48,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let cube_view = cubes.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 128,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 128,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: settings.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&cube_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 17,
                    resource: slot_buffer.as_entire_binding(),
                },
            ],
        });
        let mut clear = device.create_command_encoder(&Default::default());
        for face in 0..48 {
            let view = cubes.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: face,
                array_layer_count: Some(1),
                ..Default::default()
            });
            // Actual depth-comparison textures represent unobstructed rays or
            // opaque blockers. Both glass/opaque orders must suppress tint.
            let blocked = matches!(face / 6, 4 | 5);
            let _pass = clear.begin_render_pass(&wgpu::RenderPassDescriptor {
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(if blocked { 0.0 } else { 1.0 }),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
        }
        queue.submit([clear.finish()]);
        let transparency = 1.0 - 128.0 / 255.0;
        let red_t = red.map(|c| c * transparency);
        let blue_t = blue.map(|c| c * transparency);
        let stacked = std::array::from_fn(|i| red_t[i] * blue_t[i]);
        let expected_on = [
            red_t, [1.0; 3], stacked, [1.0; 3], [0.0; 3], [0.0; 3], [1.0; 3], red_t,
        ];
        let expected_off = [
            [1.0; 3], [1.0; 3], [1.0; 3], [1.0; 3], [0.0; 3], [0.0; 3], [1.0; 3], [1.0; 3],
        ];
        for enabled in [true, false] {
            queue.write_buffer(
                &settings,
                0,
                bytemuck::cast_slice(&[0u32, 0, if enabled { 512 } else { 0 }, 0, 1, 8, 16, 0]),
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(8, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 128);
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let data = readback.slice(..).get_mapped_range().expect("GPU readback range");
            let expected = if enabled { &expected_on } else { &expected_off };
            for (i, actual) in bytemuck::cast_slice::<u8, [f32; 4]>(&data)
                .iter()
                .enumerate()
            {
                for c in 0..3 {
                    assert!(
                        (actual[c] - expected[i][c]).abs() < 0.0001,
                        "raster lane {i} enabled={enabled}: {actual:?} != {:?}",
                        expected[i]
                    );
                }
                println!(
                    "Raster acceptance lane {i} enabled={enabled}: {:?}",
                    &actual[..3]
                );
            }
            drop(data);
            readback.unmap();
        }
    });
}
