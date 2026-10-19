use super::{create_ssr_visibility_bind_group, create_ssr_visibility_resources};
use crate::renderer::{create_targets, RenderTargets};

#[test]
fn depth_resolve_rebinds_replaced_targets_without_rebuilding_pipeline() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let clear_depth = |targets: &RenderTargets, value: f32| {
            let mut encoder = device.create_command_encoder(&Default::default());
            for view in [&targets.depth_view, &targets.ao_depth_view] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(value),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
            }
            queue.submit([encoder.finish()]);
        };
        for samples in [1, 4] {
            let make_targets = |width| {
                create_targets(
                    &device,
                    width,
                    8,
                    wgpu::TextureFormat::Rgba16Float,
                    samples,
                    false,
                    false,
                    false,
                    false,
                    false,
                    Default::default(),
                )
            };
            let original = make_targets(8);
            clear_depth(&original, 0.25);
            let (layout, pipeline, stale_group) =
                create_ssr_visibility_resources(&device, &original, samples).unwrap();
            // Bind groups keep the old textures alive after RenderTargets is dropped.
            drop(original);
            for width in [8, 16] {
                // Same-size replacement models surface recovery; a new size models resize.
                let replacement = make_targets(width);
                clear_depth(&replacement, 0.75);
                let output = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("depth resolve replacement regression"),
                    size: wgpu::Extent3d {
                        width,
                        height: 8,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let output_view = output.create_view(&Default::default());
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("resolved depth readback"),
                    size: 256 * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let resolve = |group: &wgpu::BindGroup| {
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
                            ..Default::default()
                        });
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, group, &[]);
                        pass.draw(0..3, 0..1);
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
                    let pixels: Vec<f32> = mapped
                        .chunks_exact(256)
                        .flat_map(|row| bytemuck::cast_slice::<u8, f32>(&row[..width as usize * 4]))
                        .copied()
                        .collect();
                    drop(mapped);
                    readback.unmap();
                    pixels
                };
                assert!(
                    resolve(&stale_group).iter().all(|&value| value == 0.25),
                    "old binding reproduces frozen pre-recovery depth"
                );
                let rebound = create_ssr_visibility_bind_group(&device, &layout, &replacement);
                assert!(
                    resolve(&rebound).iter().all(|&value| value == 0.75),
                    "same pipeline must read replacement depth after rebinding"
                );
                clear_depth(&replacement, 0.0);
                assert!(
                    resolve(&rebound).iter().all(|&value| value == 0.0),
                    "later sky-only frames must clear the old horizon without another rebind"
                );
            }
        }
        assert!(scope.pop().await.is_none(), "depth resolve GPU validation");
    });
}
