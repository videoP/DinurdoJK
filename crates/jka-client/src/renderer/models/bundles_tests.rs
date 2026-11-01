//! Headless GPU parity test of the production bundle recorder/state restore.
use super::*;
use wgpu::util::DeviceExt;

fn plan(
    slot: &mut Slot,
    device: &wgpu::Device,
    resources: &Resources,
    commands: &[Command],
) -> Update {
    slot.scratch.clear();
    slot.scratch.extend_from_slice(commands);
    slot.update(device, resources.clone())
}

fn pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    resources: &Resources,
    commands: &[Command],
    slot: Option<&Slot>,
) -> Vec<u8> {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bundle parity color"),
        size: wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: resources.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bundle parity depth"),
        size: color.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 32,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bundle parity render"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
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
        // Scissor/viewport remain pass-local across bundle execution.
        pass.set_viewport(1.0, 1.0, 30.0, 30.0, 0.0, 1.0);
        pass.set_scissor_rect(2, 2, 28, 28);
        if let Some(slot) = slot {
            let (bundle, count, last_texture) = slot
                .executable(
                    &resources.camera,
                    &resources.pipeline,
                    &resources.vertices,
                    &resources.indices,
                )
                .unwrap();
            assert_eq!(count as usize, commands.len());
            execute_and_restore_model_state(
                &mut pass,
                bundle,
                &resources.camera,
                &resources.pipeline,
                &resources.vertices,
                &resources.indices,
                last_texture,
            );
        } else {
            pass.set_pipeline(&resources.pipeline);
            pass.set_vertex_buffer(0, resources.vertices.slice(..));
            pass.set_index_buffer(resources.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_bind_group(0, &resources.camera, &[]);
            for command in commands {
                pass.set_bind_group(1, &command.texture, &[]);
                pass.draw_indexed(command.indices.clone(), command.base_vertex, 0..1);
            }
        }
        // Uses state retained by the previous draws. This catches an incorrect
        // restore after execute_bundles, rather than rebinding it in the test.
        pass.draw_indexed(0..3, 0, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(32),
            },
        },
        color.size(),
    );
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let data = readback.slice(..).get_mapped_range().expect("GPU readback range");
    data.chunks_exact(256)
        .flat_map(|row| row[..128].iter().copied())
        .collect()
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn model_bundles_preserve_pixels_live_updates_and_resource_invalidation() {
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
        let layout = |visibility| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        };
        let camera_layout = layout(wgpu::ShaderStages::VERTEX);
        let material_layout = layout(wgpu::ShaderStages::FRAGMENT);
        let uniform = |values: [f32; 4], layout: &wgpu::BindGroupLayout| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&values),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            (buffer, group)
        };
        let (camera_buffer, camera) = uniform([0.0; 4], &camera_layout);
        let (color_buffer, red) = uniform([1.0, 0.0, 0.0, 1.0], &material_layout);
        let (_, green) = uniform([0.0, 1.0, 0.0, 1.0], &material_layout);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                r#"
                @group(0) @binding(0) var<uniform> offset: vec4<f32>;
                @group(1) @binding(0) var<uniform> color: vec4<f32>;
                @vertex fn vs(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
                    return vec4<f32>(p.xy + offset.xy, p.z, 1.0);
                }
                @fragment fn fs() -> @location(0) vec4<f32> { return color; }
                @fragment fn fs_swapped() -> @location(0) vec4<f32> { return color.yzxw; }
            "#
                .into(),
            ),
        });
        let pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        }],
                    })],
                },
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format(),
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let vertices = [
            [-0.85f32, -0.7, 0.5],
            [-0.1, -0.7, 0.5],
            [-0.475, 0.7, 0.5],
            [0.1, -0.7, 0.5],
            [0.85, -0.7, 0.5],
            [0.475, 0.7, 0.5],
        ];
        let vertex_buffer = || {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            })
        };
        let mut resources = Resources {
            camera,
            vertices: vertex_buffer(),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0u32, 1, 2, 3, 4, 5]),
                usage: wgpu::BufferUsages::INDEX,
            }),
            pipeline: pipeline("fs"),
            format: wgpu::TextureFormat::Rgba8Unorm,
            samples: 1,
        };
        let mut commands: Vec<_> = (0..40)
            .map(|i| Command {
                texture: if i % 2 == 0 {
                    red.clone()
                } else {
                    green.clone()
                },
                indices: if i % 2 == 0 { 0..3 } else { 3..6 },
                base_vertex: 0,
            })
            .collect();
        let mut slot = Slot::default();
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Changed
        );
        assert!(slot.bundle.is_none());
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Built
        );
        assert_eq!(plan(&mut slot, &device, &resources, &commands), Update::Hit);
        let initial = pixels(&device, &queue, &resources, &commands, Some(&slot));
        assert_eq!(
            initial,
            pixels(&device, &queue, &resources, &commands, None)
        );
        assert!(initial
            .chunks_exact(4)
            .any(|pixel| pixel[..3] == [0, 255, 0]));

        // Changed uniform and geometry bytes remain live behind stable handles.
        queue.write_buffer(
            &camera_buffer,
            0,
            bytemuck::bytes_of(&[0.1f32, 0.0, 0.0, 0.0]),
        );
        queue.write_buffer(
            &color_buffer,
            0,
            bytemuck::bytes_of(&[0.0f32, 0.0, 1.0, 1.0]),
        );
        let mut shifted = vertices;
        shifted[0][0] += 0.1;
        queue.write_buffer(&resources.vertices, 0, bytemuck::cast_slice(&shifted));
        assert_eq!(plan(&mut slot, &device, &resources, &commands), Update::Hit);
        let changed = pixels(&device, &queue, &resources, &commands, Some(&slot));
        assert_eq!(
            changed,
            pixels(&device, &queue, &resources, &commands, None)
        );
        assert_ne!(initial, changed);

        // Exact material, range, base-vertex and draw-order changes invalidate.
        commands[0].texture = green.clone();
        commands[0].indices = 0..3;
        commands[0].base_vertex = 3;
        commands.swap(38, 39);
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Changed
        );
        assert!(slot.bundle.is_none());
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Built
        );
        assert_eq!(
            pixels(&device, &queue, &resources, &commands, Some(&slot)),
            pixels(&device, &queue, &resources, &commands, None)
        );

        // Replaced buffers/pipelines/cameras are different objects even with
        // identical layouts and lengths. Never execute the previous recording.
        let old_vertices = resources.vertices.clone();
        resources.vertices = vertex_buffer();
        assert!(slot
            .executable(
                &resources.camera,
                &resources.pipeline,
                &resources.vertices,
                &resources.indices
            )
            .is_none());
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Changed
        );
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Built
        );
        assert_ne!(old_vertices, resources.vertices);
        resources.pipeline = pipeline("fs_swapped");
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Changed
        );
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Built
        );
        resources.camera = uniform([0.0; 4], &camera_layout).1;
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Changed
        );
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands),
            Update::Built
        );
        assert_eq!(
            pixels(&device, &queue, &resources, &commands, Some(&slot)),
            pixels(&device, &queue, &resources, &commands, None)
        );

        // Candidate keys cover targets/MSAA; clearing/small modes stay direct.
        let mut changed_target = resources.clone();
        changed_target.samples = 4;
        assert_eq!(
            plan(&mut slot, &device, &changed_target, &commands),
            Update::Changed
        );
        assert!(slot.bundle.is_none());
        changed_target.format = wgpu::TextureFormat::Bgra8Unorm;
        assert_eq!(
            plan(&mut slot, &device, &changed_target, &commands),
            Update::Changed
        );
        assert_eq!(
            plan(&mut slot, &device, &resources, &commands[..2]),
            Update::Small
        );
        assert!(slot.bundle.is_none());
        let mut cache = ModelBundles::default();
        cache.slots[0] = slot;
        cache.clear();
        assert!(cache
            .slots
            .iter()
            .all(|slot| slot.resources.is_none() && slot.bundle.is_none()));
    });
}
