use super::create_cull_debug_pipeline;
use crate::renderer::{GpuVertex, DEPTH_FORMAT};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

/// Exercise the real shader and pipeline. Many full-screen, through-wall PVS
/// layers must have exactly the same coverage/color as one; each reason must
/// remain readable when all reason classes overlap.
#[test]
fn cull_debug_gpu_overdraw_is_bounded_and_hiz_filter_excludes_pvs() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout_entry = |ty| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[layout_entry(wgpu::BufferBindingType::Uniform)],
        });
        let reason_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[layout_entry(wgpu::BufferBindingType::Storage {
                read_only: true,
            })],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout), Some(&reason_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(include_str!("../../cull_debug.wgsl").into()),
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let all = create_cull_debug_pipeline(&device, &layout, &shader, format, 1, false);
        let hiz = create_cull_debug_pipeline(&device, &layout, &shader, format, 1, true);
        let mut camera = [0.0_f32; 20];
        for i in [0, 5, 10, 15] {
            camera[i] = 1.0;
        }
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&camera),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let reasons = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 512 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = |layout: &wgpu::BindGroupLayout, buffer: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            })
        };
        let camera_group = group(&camera_layout, &camera_buffer);
        let reason_group = group(&reason_layout, &reasons);
        let mut vertices = [GpuVertex::zeroed(); 3];
        for (v, position) in
            vertices
                .iter_mut()
                .zip([[-1.0, -1.0, 0.5], [3.0, -1.0, 0.5], [-1.0, 3.0, 0.5]])
        {
            v.position = position;
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let texture = |format, usage| {
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
                usage,
                view_formats: &[],
            })
        };
        let output = texture(
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let output_view = output.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let draw = |pipeline: &wgpu::RenderPipeline, codes: &[u32]| {
            queue.write_buffer(&reasons, 0, bytemuck::cast_slice(codes));
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &output_view,
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
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &camera_group, &[]);
                pass.set_bind_group(1, &reason_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..3, 0..codes.len() as u32);
            }
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(8),
                    },
                },
                output.size(),
            );
            queue.submit([encoder.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            let pixels: Vec<[u8; 4]> = mapped
                .chunks_exact(256)
                .flat_map(|row| row[..32].chunks_exact(4).map(|p| p.try_into().unwrap()))
                .collect();
            drop(mapped);
            readback.unmap();
            pixels
        };
        let colored = |pixels: &[[u8; 4]]| pixels.iter().filter(|p| p[..3] != [0, 0, 0]).count();
        let one_pvs = draw(&all, &[3]);
        let many_pvs = draw(&all, &[3; 512]);
        assert_eq!(
            one_pvs, many_pvs,
            "PVS overdraw must never accumulate color/opacity"
        );
        assert_eq!(
            colored(&many_pvs),
            4,
            "PVS can cover only 1/16 of the screen"
        );
        let codes: Vec<u32> = (0..512).map(|i| i % 4 + 1).collect();
        assert_eq!(
            colored(&draw(&all, &codes)),
            16,
            "all reasons combined must leave 3/4 of the scene untouched"
        );
        assert_eq!(
            colored(&draw(&hiz, &[3; 512])),
            0,
            "Hi-Z-only must exclude PVS geometry"
        );
        let filtered = draw(&hiz, &codes);
        assert_eq!(
            filtered,
            draw(&hiz, &[2; 512]),
            "PVS/frustum/area layers must not obscure Hi-Z"
        );
        assert_eq!(colored(&filtered), 4);
        assert_eq!(
            colored(&draw(&all, &[0; 512])),
            0,
            "visible batches are not rejection overlays"
        );
        assert!(scope.pop().await.is_none(), "cull overlay GPU validation");
    });
}
