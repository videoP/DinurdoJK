//! Opaque scene copy for the original ocean shader. Depth remains reversed-Z
//! and sampleable, independently of JKA's linear-depth optical capture.

pub struct SceneCopy {
    pub key: (u32, u32, u32),
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    depth_pipeline: wgpu::RenderPipeline,
}

impl SceneCopy {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, samples: u32) -> Self {
        let target = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let color = target(
            "Blue Water opaque color copy",
            wgpu::TextureFormat::Rgba16Float,
        );
        let depth = target(
            "Blue Water opaque reversed depth copy",
            wgpu::TextureFormat::Depth32Float,
        );
        let entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: samples > 1,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water scene capture input"),
            entries: &[
                entry(0, wgpu::TextureSampleType::Float { filterable: false }),
                entry(1, wgpu::TextureSampleType::Depth),
            ],
        });
        let declarations = if samples > 1 {
            "@group(0) @binding(0) var color_in: texture_multisampled_2d<f32>;\n\
             @group(0) @binding(1) var depth_in: texture_depth_multisampled_2d;"
        } else {
            "@group(0) @binding(0) var color_in: texture_2d<f32>;\n\
             @group(0) @binding(1) var depth_in: texture_depth_2d;"
        };
        // Select the color belonging to the nearest reversed-depth sample at
        // MSAA silhouettes, so the sampled color and geometry stay paired.
        let read = if samples > 1 {
            format!(
                "var nearest = 0.0; var sample = 0;\n\
                for (var i = 0; i < {samples}; i += 1) {{\n\
                    let d = textureLoad(depth_in, p, i);\n\
                    if d > nearest {{ nearest = d; sample = i; }}\n\
                }}\n\
                return Output(textureLoad(color_in, p, sample), nearest);"
            )
        } else {
            "return Output(textureLoad(color_in, p, 0), textureLoad(depth_in, p, 0));".to_owned()
        };
        let read_depth = if samples > 1 {
            format!("var nearest = 0.0; for (var i = 0; i < {samples}; i += 1) {{ nearest = max(nearest, textureLoad(depth_in, p, i)); }} return nearest;")
        } else {
            "return textureLoad(depth_in, p, 0);".to_owned()
        };
        let source = format!("{declarations}\n\
            struct Output {{ @location(0) color: vec4<f32>, @builtin(frag_depth) depth: f32 }}\n\
            @vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{\n\
                let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));\n\
                return vec4<f32>(p[i], 0.0, 1.0);\n\
            }}\n\
            @fragment fn fragment(@builtin(position) position: vec4<f32>) -> Output {{\n\
                let p = vec2<i32>(position.xy); {read}\n\
            }}\n\
            @fragment fn fragment_depth(@builtin(position) position: vec4<f32>) -> @builtin(frag_depth) f32 {{\n\
                let p = vec2<i32>(position.xy); {read_depth}\n\
            }}");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water scene copy"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water scene copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let color_targets = [Some(wgpu::TextureFormat::Rgba16Float.into())];
        let create_pipeline = |depth_only: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Blue Water scene copy"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if depth_only {
                        "fragment_depth"
                    } else {
                        "fragment"
                    }),
                    compilation_options: Default::default(),
                    targets: if depth_only { &[] } else { &color_targets },
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = create_pipeline(false);
        let depth_pipeline = create_pipeline(true);
        Self {
            key: (width, height, samples),
            color,
            depth,
            layout,
            pipeline,
            depth_pipeline,
        }
    }

    pub fn capture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        self.capture_impl(device, encoder, color, depth, false);
    }

    /// Resolve reversed depth without reading or writing scene color. The color
    /// view only fills the shared bind-group layout and is unused by this shader.
    pub fn capture_depth(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        self.capture_impl(device, encoder, color, depth, true);
    }

    fn capture_impl(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        depth_only: bool,
    ) {
        let input = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water scene copy input"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
            ],
        });
        let color_attachment = [Some(wgpu::RenderPassColorAttachment {
            view: &self.color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blue Water opaque scene copy"),
            color_attachments: if depth_only { &[] } else { &color_attachment },
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(if depth_only {
            &self.depth_pipeline
        } else {
            &self.pipeline
        });
        pass.set_bind_group(0, &input, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_copy_preserves_color_and_reversed_depth_with_and_without_msaa() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            for samples in [1, 4] {
                let input = |format| {
                    device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: Some("Blue Water copy test input"),
                            size: wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: samples,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        })
                        .create_view(&Default::default())
                };
                let color = input(wgpu::TextureFormat::Rgba16Float);
                let depth = input(wgpu::TextureFormat::Depth32Float);
                let copy = SceneCopy::new(&device, 1, 1, samples);
                let read = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Blue Water copy test readback"),
                    size: 512,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                for depth_only in [false, true] {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &color,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: if depth_only { 1.0 } else { 0.25 },
                                        g: 0.5,
                                        b: 0.75,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &depth,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(if depth_only {
                                            0.3
                                        } else {
                                            0.7
                                        }),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            ..Default::default()
                        });
                    }
                    if depth_only {
                        copy.capture_depth(&device, &mut encoder, &color, &depth);
                    } else {
                        copy.capture(&device, &mut encoder, &color, &depth);
                    }
                    for (view, offset, aspect) in [
                        (&copy.color, 0, wgpu::TextureAspect::All),
                        (&copy.depth, 256, wgpu::TextureAspect::DepthOnly),
                    ] {
                        encoder.copy_texture_to_buffer(
                            wgpu::TexelCopyTextureInfo {
                                texture: view.texture(),
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect,
                            },
                            wgpu::TexelCopyBufferInfo {
                                buffer: &read,
                                layout: wgpu::TexelCopyBufferLayout {
                                    offset,
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
                    }
                    queue.submit(Some(encoder.finish()));
                    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    let mapped = read.slice(..).get_mapped_range().unwrap();
                    let actual: &[u16] = bytemuck::cast_slice(&mapped[..8]);
                    assert_eq!(
                        actual,
                        &[0x3400, 0x3800, 0x3a00, 0x3c00],
                        "color samples={samples}"
                    );
                    let actual = f32::from_le_bytes(mapped[256..260].try_into().unwrap());
                    assert!(
                        (actual - if depth_only { 0.3 } else { 0.7 }).abs() < 1e-6,
                        "depth samples={samples}: {actual}"
                    );
                    drop(mapped);
                    read.unmap();
                }
            }
            assert!(
                scope.pop().await.is_none(),
                "Blue Water scene copy validation"
            );
        });
    }
}
