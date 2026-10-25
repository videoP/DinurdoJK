//! Bounded normal-only ripple stamps, inspired by Cheap Interactive Water's
//! expanding normal sprites. Redrawn into a small world-space atlas; no solver.
use bytemuck::{Pod, Zeroable};

pub const SIZE: u32 = 256;
pub const LAYERS: u32 = 8;
pub const MAX_STAMPS: usize = 64;
pub const SPAN: f32 = 32.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Stamp {
    // Centre UV, outer radius / span, ring radius in metres.
    pub position: [f32; 4],
    // Outer radius in metres, ring width, signed normal strength, spare.
    pub shape: [f32; 4],
}

pub struct RippleAtlas {
    pub view: wgpu::TextureView,
    layers: Vec<wgpu::TextureView>,
    instances: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
}

impl RippleAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cheapwater local ripple normals"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layers = (0..LAYERS)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cheapwater bounded ripple stamps"),
            size: (LAYERS as usize * MAX_STAMPS * std::mem::size_of::<Stamp>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cheapwater expanding ripple normals"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lake_ripples.wgsl").into()),
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cheapwater ripple stamps"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Stamp>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x4, 1=>Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState {
                        color: add,
                        alpha: add,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            view,
            layers,
            instances,
            pipeline,
        }
    }

    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: usize,
        stamps: &[Stamp],
    ) {
        let stamps = &stamps[..stamps.len().min(MAX_STAMPS)];
        let Some(view) = self.layers.get(layer) else {
            return;
        };
        // Each layer owns its buffer slice: later queue writes cannot replace
        // an earlier layer's inputs before submission.
        let start = (layer * MAX_STAMPS * std::mem::size_of::<Stamp>()) as u64;
        if !stamps.is_empty() {
            queue.write_buffer(&self.instances, start, bytemuck::cast_slice(stamps));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Cheapwater local wake normals"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if !stamps.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(
                0,
                self.instances
                    .slice(start..start + (MAX_STAMPS * std::mem::size_of::<Stamp>()) as u64),
            );
            pass.draw(0..6, 0..stamps.len() as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lake_ripple_stamps_have_balanced_slopes_separate_layers_and_clear_cleanly() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let atlas = RippleAtlas::new(&device);
            let stamp = Stamp {
                position: [0.5, 0.5, 1.25 / SPAN, 1.0],
                shape: [1.25, 0.25, 0.03, 0.0],
            };
            let mut opposite = stamp;
            opposite.shape[2] *= -2.0;
            let row = SIZE * 8;
            let layer_bytes = (row * SIZE) as u64;
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ripple atlas readback"),
                size: layer_bytes * 2,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            atlas.encode(&queue, &mut encoder, 0, &[stamp]);
            atlas.encode(&queue, &mut encoder, 1, &[opposite]);
            for layer in 0..2 {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: atlas.view.texture(),
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: layer_bytes * layer as u64,
                            bytes_per_row: Some(row),
                            rows_per_image: Some(SIZE),
                        },
                    },
                    wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                );
            }
            queue.submit(Some(encoder.finish()));
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let data = read.slice(..).get_mapped_range().unwrap();
            let channel = |offset: usize| {
                half::f16::from_bits(u16::from_le_bytes(
                    data[offset..offset + 2].try_into().unwrap(),
                ))
                .to_f32()
            };
            let mut positive = 0;
            let mut negative = 0;
            let mut peak = 0;
            for pixel in 0..(SIZE * SIZE) as usize {
                for c in 0..2 {
                    let a = channel(pixel * 8 + c * 2);
                    let b = channel(layer_bytes as usize + pixel * 8 + c * 2);
                    if a > 0.01 {
                        positive += 1;
                        peak = pixel;
                    }
                    if a < -0.01 {
                        negative += 1;
                    }
                    assert!(
                        (b + 2.0 * a).abs() < 0.001,
                        "layer writes must not replace each other's instances"
                    );
                }
            }
            assert!(
                positive > 10 && negative > 10,
                "ring normals must contain balanced positive and negative slopes"
            );
            drop(data);
            read.unmap();
            let mut encoder = device.create_command_encoder(&Default::default());
            atlas.encode(&queue, &mut encoder, 0, &[]);
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: atlas.view.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: peak as u32 % SIZE,
                        y: peak as u32 / SIZE,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let data = read.slice(..).get_mapped_range().unwrap();
            assert_eq!(
                &data[..8],
                &[0; 8],
                "an empty layer must clear previous ripples"
            );
            drop(data);
            read.unmap();
            assert!(scope.pop().await.is_none(), "ripple atlas GPU validation");
        });
    }
}
