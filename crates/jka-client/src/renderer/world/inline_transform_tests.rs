use super::*;
use crate::renderer::posed_inline_vertices;

fn vertex(i: usize) -> GpuVertex {
    GpuVertex {
        position: [
            (i as f32 * 0.3).sin() * 11.0,
            (i % 11) as f32 - 4.0,
            i as f32 * -0.13,
        ],
        normal: if i % 7 == 0 {
            [0.0; 3]
        } else {
            [0.3, -0.4, 0.5]
        },
        uv: [0.25, 0.75],
        lightmap_uv: [0.125, 0.875],
        color: [0.1, 0.2, 0.3, 0.4],
        alpha_cutoff: 0.625,
    }
}

fn poses() -> [InlineModelInstance; 4] {
    [
        InlineModelInstance::compiled(1),
        InlineModelInstance {
            model: 1,
            origin: [2109.0, -1837.0, -851.0],
            axis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            is_portal_entity: false,
        },
        InlineModelInstance {
            model: 1,
            origin: [-17.0, 83.0, 11.0],
            axis: [[1.6, 1.2, 0.0], [-1.2, 1.6, 0.0], [0.0, 0.0, 2.0]],
            is_portal_entity: true,
        },
        InlineModelInstance::compiled(1),
    ]
}

#[test]
fn inline_transform_shader_validates() {
    let module = wgpu::naga::front::wgsl::parse_str(SHADER)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(SHADER)));
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn inline_transform_workers_match_cpu_and_reuse_storage() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(3)
        .build()
        .unwrap();
    let base: Vec<_> = (0..17003).map(vertex).collect();
    let mut scratch = Vec::new();
    let mut retained_capacity = 0;
    // Cross chunk boundaries, shrink/grow within retained storage, empty a
    // model, and restore its compiled pose after translation/rotation/scale.
    for count in [17003, 7, 8193, 0, 129, 17003] {
        for pose in poses() {
            let (expected, minimum, maximum) = posed_inline_vertices(&base[..count], pose);
            let bounds = super::super::geometry::posed_inline_vertices_workers(
                &base[..count],
                pose,
                &mut scratch,
                &pool,
            );
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&scratch),
                bytemuck::cast_slice::<_, u8>(&expected)
            );
            assert_eq!(bounds, (minimum, maximum));
            if retained_capacity != 0 {
                assert_eq!(scratch.capacity(), retained_capacity);
            }
            retained_capacity = scratch.capacity();
        }
    }
}

#[test]
fn inline_transform_bounds_contain_cpu_vertices() {
    let base: Vec<_> = (0..129).map(vertex).collect();
    let (_, minimum, maximum) = posed_inline_vertices(&base, InlineModelInstance::compiled(1));
    for pose in poses() {
        let (minimum, maximum) = posed_bounds((minimum, maximum), pose);
        let (posed, _, _) = posed_inline_vertices(&base, pose);
        for vertex in posed {
            for axis in 0..3 {
                assert!(
                    vertex.position[axis] >= minimum[axis]
                        && vertex.position[axis] <= maximum[axis]
                );
            }
        }
    }
}

/// Actual GPU readback checks packed vertex stride, unaligned mover regions,
/// multiple instances of a shared BSP handle, non-geometry attributes, zero
/// normals, editor scale, and repeated absolute (never cumulative) transforms.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn inline_transform_gpu_matches_cpu() {
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
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let base: Vec<_> = (0..129).map(vertex).collect();
        // Five static vertices before the first mover force a nonzero prefix
        // when its storage binding is rounded down to the adapter's alignment.
        let models: Vec<_> = [5, 134]
            .into_iter()
            .map(|start| InlineModelGpu {
                pvs_visible: None,
                pvs_bounds_dirty: true,
                pvs_area_rejected: false,
                model: 1,
                gpu_vertex_start: start,
                base: base.clone(),
                rt_indices: Vec::new(),
                rt_mask_sources: Vec::new(),
                raster_glass_local: Vec::new(),
                coarse_batches: Vec::new(),
                full_batches: Vec::new(),
                uploaded: None,
            })
            .collect();
        let mut expected: Vec<_> = (0..268).map(vertex).collect();
        let output = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Inline transform parity world vertices"),
            contents: bytemuck::cast_slice(&expected),
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        });
        let mut gpu = InlineTransformGpu::new(&device, &output, &models);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: output.size(),
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        });
        for pose in poses() {
            for (slot, model) in models.iter().enumerate() {
                let mut pose = pose;
                pose.origin[0] += slot as f32 * 113.0;
                let resource = gpu.models[slot].as_mut().unwrap();
                resource.update(&queue, pose);
                let (posed, _, _) = posed_inline_vertices(&base, pose);
                // Only positions/normals change; the existing world's current
                // AO, UVs and colours must survive GPU posing.
                for (i, posed) in posed.iter().enumerate() {
                    let dst = &mut expected[model.gpu_vertex_start as usize + i];
                    dst.position = posed.position;
                    dst.normal = posed.normal;
                }
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: None,
                });
                pass.set_pipeline(&gpu.pipeline);
                for resource in gpu.models.iter().flatten() {
                    pass.set_bind_group(0, &resource.bind_group, &[]);
                    pass.dispatch_workgroups(resource.count.div_ceil(64), 1, 1);
                }
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output.size());
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
            {
                let mapped = readback.slice(..).get_mapped_range().unwrap();
                let actual: &[GpuVertex] = bytemuck::cast_slice(&mapped);
                for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                    for axis in 0..3 {
                        assert!(
                            (actual.position[axis] - expected.position[axis]).abs() <= 0.0005,
                            "vertex {i} axis {axis}: {:?} != {:?}",
                            actual.position,
                            expected.position
                        );
                        assert!((actual.normal[axis] - expected.normal[axis]).abs() <= 0.00001);
                    }
                    assert_eq!(
                        (
                            actual.uv,
                            actual.lightmap_uv,
                            actual.color,
                            actual.alpha_cutoff
                        ),
                        (
                            expected.uv,
                            expected.lightmap_uv,
                            expected.color,
                            expected.alpha_cutoff
                        )
                    );
                }
            }
            readback.unmap();
        }
        assert!(errors.pop().await.is_none());
    });
}
