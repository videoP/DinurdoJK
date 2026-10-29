//! Geometry/vertex-fetch diagnostics only; this shader is not a water material.
use crate::river_grid_mesh;
use jka_water::river::grid_mesh::GridMesh;

fn words(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<u32> {
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river mesh test readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &read, 0, source.size());
    queue.submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    read.slice(..).map_async(wgpu::MapMode::Read, move |v| {
        tx.send(v).unwrap();
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(20)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = read.slice(..).get_mapped_range().unwrap();
    mapped
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

#[test]
fn river_grid_mesh_counts_match_selected_native_graph() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/grid-mesh-reference.json"
    ))
    .unwrap();
    for row in reference["vectors"].as_array().unwrap() {
        let cells = std::array::from_fn(|i| row["dimensions"][i].as_u64().unwrap() as u32);
        let mesh = GridMesh::new(cells, [2300.0, 1100.0]).unwrap();
        let outputs = &row["outputs"];
        for (name, value) in [
            ("Emitter.VertexCount", mesh.vertex_count()),
            ("Emitter.TriangleCount", mesh.index_count() / 3),
            ("Emitter.NumVerticesX", mesh.vertex_dimensions()[0]),
            ("Emitter.NumVerticesY", mesh.vertex_dimensions()[1]),
        ] {
            assert_eq!(
                u64::from(value),
                outputs[name].as_u64().unwrap(),
                "{cells:?} {name}"
            );
        }
    }
}

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut encoder = device.create_command_encoder(&Default::default());
    // Queue different resets together: bindings/controls must not alias a
    // later mesh. Odd rectangles exercise rounded workgroups in both axes.
    let meshes: Vec<_> = [[1, 1], [7, 3], [3, 7], [17, 9]]
        .into_iter()
        .map(|cells| {
            river_grid_mesh::encode(
                device,
                &mut encoder,
                GridMesh::new(cells, [2300.0, 1100.0]).unwrap(),
            )
            .unwrap()
        })
        .collect();
    queue.submit(Some(encoder.finish()));
    for mesh in &meshes {
        let indices = words(device, queue, &mesh.indices);
        let expected: Vec<_> = (0..mesh.geometry.index_count() / 6)
            .flat_map(|i| mesh.geometry.quad(i).unwrap())
            .collect();
        assert_eq!(
            indices,
            expected,
            "native triangle order {:?}",
            mesh.geometry.cells()
        );
        let vertices = words(device, queue, &mesh.vertices);
        assert_eq!(vertices.len() as u64 * 4, mesh.geometry.vertex_bytes());
        for (i, got) in vertices.chunks_exact(8).enumerate() {
            let expected = mesh.geometry.vertex(i as u32).unwrap().words();
            for j in 0..8 {
                if [0, 1, 5, 6].contains(&j) {
                    let (a, b) = (f32::from_bits(got[j]), f32::from_bits(expected[j]));
                    assert!(
                        (a - b).abs() <= if j < 2 { 0.00025 } else { 2e-7 },
                        "vertex {i} component {j}: {a} != {b}"
                    );
                } else {
                    assert_eq!(got[j], expected[j], "packed vertex {i} component {j}");
                }
            }
        }
    }
    check_vertex_fetch(device, queue, &meshes[1]);
    // No allocation should precede rejecting unsupported GPU size/dispatch.
    let mut encoder = device.create_command_encoder(&Default::default());
    let large = GridMesh::new([10_000, 10_000], [1.0, 1.0]).unwrap();
    assert!(river_grid_mesh::encode(device, &mut encoder, large).is_err());
}

fn check_vertex_fetch(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &river_grid_mesh::Mesh) {
    // Eight pixels per native grid cell. Align raster vertices to whole pixels
    // so this checks attribute interpolation without assuming an implementation's
    // subpixel snapping/precision for vertices at fractional pixel coordinates.
    assert_eq!(mesh.geometry.cells(), [7, 3]);
    const WIDTH: usize = 56;
    const HEIGHT: usize = 24;
    const ROW: usize = 1024;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label:Some("river mesh attribute diagnostic only"),source:wgpu::ShaderSource::Wgsl(r#"
        struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) check: f32 }
        @vertex fn vs(@location(0) p:vec3<f32>, @location(1) tx:vec4<f32>, @location(2) tz:vec4<f32>, @location(3) uv:vec2<f32>, @location(4) color:vec4<f32>) -> Out {
            var o:Out; o.position=vec4<f32>(p.xy*vec2<f32>(2.0/2300.0,2.0/1100.0),p.z,1.0);
            o.uv=uv; o.check=dot(tx,vec4<f32>(1.0,1.0,1.0,-1.0))+dot(tz,vec4<f32>(1.0,1.0,1.0,-1.0))+dot(color,vec4<f32>(1.0));
            return o;
        }
        @fragment fn fs(o:Out) -> @location(0) vec4<f32> { return vec4<f32>(o.uv,o.check,1.0); }
        "#.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("river mesh attribute diagnostic pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(river_grid_mesh::vertex_layout())],
        },
        primitive: wgpu::PrimitiveState {
            front_face: wgpu::FrontFace::Cw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let size = wgpu::Extent3d {
        width: WIDTH as u32,
        height: HEIGHT as u32,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("river mesh diagnostic target"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("river mesh diagnostic draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.geometry.index_count(), 0, 0..1);
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river mesh diagnostic pixels"),
        size: (ROW * HEIGHT) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ROW as u32),
                rows_per_image: Some(HEIGHT as u32),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));
    let pixels = words(device, queue, &buffer);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let got: [f32; 4] =
                std::array::from_fn(|i| f32::from_bits(pixels[y * (ROW / 4) + x * 4 + i]));
            let expected = [
                (x as f32 + 0.5) / WIDTH as f32,
                1.0 - (y as f32 + 0.5) / HEIGHT as f32,
                8.0,
                1.0,
            ];
            for i in 0..4 {
                assert!(
                    (got[i] - expected[i]).abs() < 2e-6,
                    "native mesh vertex fetch {x},{y}: {got:?} != {expected:?}"
                );
            }
        }
    }
}
