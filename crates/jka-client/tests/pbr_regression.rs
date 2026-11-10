//! Runs independently of the client unit-test harness.
use futures_lite::future::block_on;
use wgpu::util::DeviceExt;

#[path = "../src/renderer/reflections/cubemap_view.rs"]
mod cubemap_view;

#[test]
fn capture_cameras_match_rend2_cube_lookup_at_centers_and_corners() {
    use glam::{Vec3, Vec4};
    let origin = Vec3::new(-3000.0, 110.0, 1900.0);
    for face in 0..6 { for [u,v] in [[0.0,0.0],[-0.8,-0.5],[0.6,0.4]] {
        // Independent face equations from prefilterEnvMap.glsl, in JKA axes.
        let d = match face {
            0 => Vec3::new(1.0,-v,-u), 1 => Vec3::new(-1.0,-v,u),
            2 => Vec3::new(u,1.0,v), 3 => Vec3::new(u,-1.0,-v),
            4 => Vec3::new(u,-v,1.0), _ => Vec3::new(-u,-v,-1.0),
        };
        let renderer_ray = Vec3::new(d.x,d.z,-d.y);
        let (matrix, _) = cubemap_view::view_projection(origin, face);
        let clip: Vec4 = matrix * (origin+renderer_ray*40.0).extend(1.0);
        assert!((clip.x/clip.w-u).abs() < 1e-4, "face {face}: horizontal orientation");
        assert!((clip.y/clip.w+v).abs() < 1e-4, "face {face}: vertical orientation");
        assert!((clip.z/clip.w-0.1).abs() < 1e-4, "reversed-Z near plane");
    }}
}

#[test]
fn rend2_cubemap_filter_faces_energy_and_brdf_on_gpu() {
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let source = include_str!("../src/renderer/reflections/cubemap.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production Rend2 cubemap filters"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = |entry| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: None,
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_fullscreen"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        });
        let filter = pipeline("fs_prefilter");
        let brdf_pipeline = pipeline("fs_brdf");
        let colors = [[255,0,0,255], [0,255,0,255], [0,0,255,255], [255,255,0,255], [255,0,255,255], [0,255,255,255]];
        let cube = |constant| {
            let pixels: Vec<u8> = (0..6).flat_map(|face| {
                let color = if constant { [64,128,192,255] } else { colors[face] };
                color.repeat(64)
            }).collect();
            let texture = device.create_texture_with_data(&queue, &wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 6 }, mip_level_count: 1,
                sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            }, wgpu::util::TextureDataOrder::LayerMajor, &pixels);
            texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() })
        };
        let colored = cube(false);
        let constant = cube(true);
        let target = |size, layers| device.create_texture(&wgpu::TextureDescriptor {
            label: None, size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let filtered = target(1, 8);
        let lut = target(128, 1);
        let lut_view = lut.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let mut encoder = device.create_command_encoder(&Default::default());
        for layer in 0..8 {
            let params: [f32; 4] = [1.0, (layer % 6) as f32, if layer == 7 { 1.0 } else { 0.0 }, 0.75];
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None,
                contents: bytemuck::cast_slice(&params), usage: wgpu::BufferUsages::UNIFORM });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &filter.get_bind_group_layout(0), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(if layer < 6 { &colored } else { &constant }) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
            ] });
            let view = filtered.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer, array_layer_count: Some(1), ..Default::default() });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&filter); pass.set_bind_group(0, &group, &[]); pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &lut_view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&brdf_pipeline); pass.draw(0..3, 0..1);
        }
        let read_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var faces: texture_2d_array<f32>;
@group(0) @binding(1) var lut: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> result: array<vec4<f32>, 11>;
@compute @workgroup_size(1) fn main() {
    for (var i = 0; i < 8; i += 1) { result[i] = textureLoad(faces, vec2(0), i, 0); }
    result[8] = textureLoad(lut, vec2(64,64), 0);
    result[9] = textureLoad(lut, vec2(16,112), 0);
    result[10] = textureLoad(lut, vec2(112,16), 0);
}
"#.into()) });
        let read_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &read_shader,
            entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 176,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 176,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let faces = filtered.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &read_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&faces) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&lut_view) },
            wgpu::BindGroupEntry { binding: 2, resource: output.as_entire_binding() },
        ] });
        { let mut pass = encoder.begin_compute_pass(&Default::default()); pass.set_pipeline(&read_pipeline);
            pass.set_bind_group(0, &group, &[]); pass.dispatch_workgroups(1,1,1); }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 176); queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        let actual: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
        for (value, color) in actual[..6].iter().zip(colors) {
            for channel in 0..3 { assert!((value[channel] - f32::from(color[channel])/255.0).abs() < 0.002, "face: {value:?} != {color:?}"); }
            assert!((value[3]-0.75).abs() < 0.001);
        }
        for value in &actual[6..8] { for (value, expected) in value.iter().zip([64.0/255.0,128.0/255.0,192.0/255.0,0.75]) {
            assert!((value-expected).abs() < 0.002, "convolution must preserve constant radiance");
        }}
        // Independent f64 scalar port of R_CreateEnvBrdfLUT/GSmithCorrelated.
        for (sample, (x,y)) in actual[8..].iter().zip([(64,64),(16,112),(112,16)]) {
            let r = (x as f64+0.5)/128.0; let nv = (y as f64+0.5)/128.0;
            let vx = (1.0-nv*nv).sqrt(); let m2 = r.powi(4); let mut expected = [0.0,0.0];
            for i in 0..1024u32 {
                let phi = std::f64::consts::TAU * f64::from(i)/1024.0;
                let e2 = f64::from(i.reverse_bits()) / 4294967296.0;
                let hz = ((1.0-e2)/(1.0+(m2-1.0)*e2)).sqrt();
                let hx = (1.0-hz*hz).sqrt()*phi.cos(); let vh = (vx*hx+nv*hz).max(0.0);
                let nl = (2.0*(vx*hx+nv*hz)*hz-nv).max(0.0);
                if nl > 0.0 { let vis = 0.5/(nl*(nv*(nv-nv*m2)+m2).sqrt()+nv*(nl*(nl-nl*m2)+m2).sqrt());
                    let energy = nl*vis*4.0*vh/hz; let f=(1.0-vh).powi(5);
                    expected[0] += energy*(1.0-f)/1024.0; expected[1] += energy*f/1024.0;
                }
            }
            for c in 0..2 { assert!((f64::from(sample[c])-expected[c]).abs() < 0.003, "LUT {x},{y}: {sample:?} != {expected:?}"); }
        }
    });
}

#[test]
fn pbr_regression_lightgrid_and_rend2_baked_lighting_on_gpu() {
    // Execute the actual helpers from both production shader families. The
    // first case is the caboose surface hit from castingcouch viewpos
    // (-3825 1703 19), yaw 75, pitch 9. Its lightgrid is elsewhere in the BSP.
    block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        println!("PBR regression GPU: {}", adapter.get_info().name);
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        for base in [include_str!("../src/bsp.wgsl"), include_str!("../src/bsp_lean.wgsl")] {
            let start = base.find("fn pbr_lightgrid_uvw(").unwrap();
            let end = base.find("fn sample_vertex_static_lightgrid(").unwrap();
            let lighting_start = base.find("fn rend2_baked_color(").unwrap();
            let lighting_end = base.find("fn static_baked_pbr(").unwrap();
            let transfer_start = base.find("fn legacy_rgb_encode(").unwrap();
            let transfer_end = base.find("fn classic_three_stage_rgb(").unwrap();
            let source = format!("{}\n{}\n{}\n{}", &base[start..end], &base[lighting_start..lighting_end], &base[transfer_start..transfer_end], r#"
@group(0) @binding(0) var<storage, read_write> result: array<vec4<f32>, 16>;
@compute @workgroup_size(1) fn main() {
    result[0] = vec4(pbr_lightgrid_uvw(vec3(-3699.33, 2172.0, -57.9),
        vec3(7872.0, -192.0, 384.0), vec3(1.0/64.0, 1.0/64.0, 1.0/128.0), vec3(8.0, 152.0, 2.0)), 1.0);
    result[1] = vec4(pbr_lightgrid_uvw(vec3(128.0, 64.0, 128.0),
        vec3(0.0), vec3(1.0/64.0, 1.0/64.0, 1.0/128.0), vec3(8.0, 152.0, 2.0)), 1.0);
    result[2] = vec4(pbr_lightgrid_uvw(vec3(1000000.0),
        vec3(0.0), vec3(1.0/64.0), vec3(8.0, 152.0, 2.0)), 1.0);
    result[3] = vec4(pbr_lightgrid_direction(vec3(1.0, 0.5, 0.5), vec3(0.0, 1.0, 0.0)), 1.0);
    result[4] = vec4(pbr_lightgrid_direction(vec3(0.5, 1.0, 0.5), vec3(0.0, 1.0, 0.0)), 1.0);
    result[5] = vec4(pbr_lightgrid_direction(vec3(0.5), vec3(0.0, 0.0, 1.0)), 1.0);
    // Neutral normals preserve diffuse energy; grazing reconstruction retains
    // the unused ambient share instead of applying lightgrid brightness twice.
    result[6] = vec4(rend2_baked_light_factors(1.0, 1.0), 1.0);
    result[7] = vec4(rend2_baked_light_factors(0.1, 0.2), 1.0);
    result[8] = vec4(rend2_baked_light_factors(0.0, 0.0), 1.0);
    result[9] = vec4(rend2_baked_light_factors(0.8, 0.2), 1.0);
    // Reference lightall GGX/Smith values include PI radiance and N.L.
    result[10] = vec4(rend2_baked_specular_lobe(0.5, 1.0, 1.0, 1.0));
    result[11] = vec4(rend2_baked_specular_lobe(0.5, 0.0, 1.0, 1.0));
    result[12] = vec4(rend2_baked_specular_lobe(0.2, 0.8, 0.3, 0.6));
    result[13] = vec4(rend2_baked_specular_lobe(1.0, 1.0, 0.5, 0.5));
    // A reference LDR metal with a strong GGX highlight. Encode the inputs,
    // combine diffuse + specular once, and decode the finished operation.
    result[14] = vec4(rend2_baked_color(vec3(0.21404114), vec3(0.21404114), vec3(0.1), vec3(0.7), true), 1.0);
    result[15] = vec4(rend2_baked_color(vec3(0.21404114), vec3(0.21404114), vec3(0.1), vec3(0.7), false), 1.0);
}
"#);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("PBR lightgrid regressions"), source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: None, module: &shader, entry_point: Some("main"),
                compilation_options: Default::default(), cache: None,
            });
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 256, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 256, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() }],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 256);
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |status| tx.send(status).unwrap());
            device.poll(wgpu::PollType::Wait { submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().unwrap();
            let actual: &[[f32; 4]] = bytemuck::cast_slice(&bytes);
            let ldr_highlight = ((0.725_f32 + 0.055) / 1.055).powf(2.4);
            let hdr_highlight = 0.21404114 * 0.21404114 * 0.1 + 0.7;
            let expected = [
                [0.0625, 37.4375 / 152.0, 0.25, 1.0],
                [2.5 / 8.0, 1.5 / 152.0, 0.75, 1.0],
                [7.5 / 8.0, 151.5 / 152.0, 0.75, 1.0],
                [0.0, 1.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0],
                [1.0, 0.0, 1.0, 1.0], [4.0, 0.6, 1.4, 1.0],
                [4.0, 1.0, 1.0, 1.0], [1.25, 0.0, 0.25, 1.0],
                [1.0; 4], [0.0625; 4], [0.17245133; 4], [0.25; 4],
                [ldr_highlight, ldr_highlight, ldr_highlight, 1.0],
                [hdr_highlight, hdr_highlight, hdr_highlight, 1.0],
            ];
            for (sample, expected) in actual.iter().zip(expected) {
                for (value, expected) in sample.iter().zip(expected) {
                    assert!((value - expected).abs() < 1e-5, "{sample:?} != {expected:?}");
                }
            }
        }
    });
}

