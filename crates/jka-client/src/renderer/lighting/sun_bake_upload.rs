//! Prepare pipelines off-thread; install only a finished result at a frame boundary.
use crate::renderer::{GpuImage, TextureData};
use wgpu::util::DeviceExt;

pub(super) struct MaskUpload {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 2],
    sampler: wgpu::Sampler,
}

impl MaskUpload {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("baked sun alpha installation"),
            entries: &[
                texture(0, false),
                texture(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("baked sun alpha installation"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("baked sun alpha installation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sun_mask.wgsl").into()),
        });
        let pipelines = [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ]
        .map(|format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("baked sun alpha installation"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: crate::pipeline_cache::get().as_ref(),
            })
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("baked sun mask bilinear sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            layout,
            pipelines,
            sampler,
        }
    }

    pub(super) fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        images: &[GpuImage],
        bases: &[TextureData],
        masks: &[Vec<u8>],
    ) -> Vec<GpuImage> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("install completed baked sun masks"),
        });
        let output = images
            .iter()
            .zip(bases)
            .zip(masks)
            .map(|((image, base), mask)| {
                let mask_texture = device.create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor {
                        label: Some("completed baked sun flux"),
                        size: wgpu::Extent3d {
                            width: base.width,
                            height: base.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::R8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    mask,
                );
                let mask_view = mask_texture.create_view(&Default::default());
                let format = if base.srgb {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("lightmap with completed baked sun flux"),
                    size: image._texture.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("baked sun alpha installation"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&image.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&mask_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("install baked sun alpha, preserve lightmap RGB"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipelines[usize::from(base.srgb)]);
                    pass.set_bind_group(0, &group, &[]);
                    pass.draw(0..3, 0..1);
                }
                GpuImage::new(texture, view)
            })
            .collect();
        queue.submit([encoder.finish()]);
        output
    }
}
