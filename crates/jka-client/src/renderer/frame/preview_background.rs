//! An installed-map image behind the existing 3D Profile preview.
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
@group(0) @binding(0) var backdrop:texture_2d<f32>;
@group(0) @binding(1) var linear_sampler:sampler;
@group(0) @binding(2) var<uniform> crop:vec4<f32>;
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vs(@builtin(vertex_index) index:u32)->Vertex {
    let points=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    let p=points[index]; var v:Vertex;
    v.position=vec4(p,0.0,1.0); v.uv=vec2(p.x*0.5+0.5,0.5-p.y*0.5);return v;
}
@fragment fn fs(v:Vertex)->@location(0) vec4<f32> {
    let uv=(v.uv-vec2(0.5))*crop.xy+vec2(0.5);
    return vec4(textureSample(backdrop,linear_sampler,uv).rgb*0.72,1.0);
}
"#;

pub(in crate::renderer) struct PreviewBackground {
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    crop: wgpu::Buffer,
    shader: wgpu::ShaderModule,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    samples: u32,
    image_size: [u32; 2],
}
fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Profile backdrop layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Profile backdrop"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: crate::renderer::DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}
impl PreviewBackground {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        samples: u32,
        size: [u32; 2],
        rgba: &[u8],
    ) -> Result<Self, String> {
        if size.iter().any(|n| *n == 0 || *n > 2048)
            || rgba.len() != size[0] as usize * size[1] as usize * 4
        {
            return Err("invalid Profile background dimensions".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Profile map image"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            texture.size(),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Profile backdrop bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let crop = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Profile backdrop crop"),
            contents: bytemuck::cast_slice(&[1.0f32, 1.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Profile backdrop"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: crop.as_entire_binding(),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Profile backdrop"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = pipeline(device, &layout, &shader, format, samples);
        Ok(Self {
            layout,
            group,
            crop,
            shader,
            pipeline,
            format,
            samples,
            image_size: size,
        })
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        samples: u32,
        viewport: [u32; 2],
    ) {
        if self.format != format || self.samples != samples {
            self.pipeline = pipeline(device, &self.layout, &self.shader, format, samples);
            self.format = format;
            self.samples = samples;
        }
        let image_aspect = self.image_size[0] as f32 / self.image_size[1] as f32;
        let view_aspect = viewport[0].max(1) as f32 / viewport[1].max(1) as f32;
        let scale = if view_aspect > image_aspect {
            [1.0, image_aspect / view_aspect, 0.0, 0.0]
        } else {
            [view_aspect / image_aspect, 1.0, 0.0, 0.0]
        };
        queue.write_buffer(&self.crop, 0, bytemuck::cast_slice(&scale));
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn profile_backdrop_draws_inside_preview_and_survives_msaa_change() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let format = wgpu::TextureFormat::Rgba8Unorm;
            assert!(PreviewBackground::new(&device, &queue, format, 1, [0, 1], &[]).is_err());
            assert!(PreviewBackground::new(&device, &queue, format, 1, [1, 1], &[255]).is_err());
            let mut background =
                PreviewBackground::new(&device, &queue, format, 1, [1, 1], &[255, 0, 0, 255])
                    .unwrap();
            for samples in [1, 4] {
                background.prepare(&device, &queue, format, samples, [24, 24]);
                let desc = wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 32,
                        height: 32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                };
                let target = device.create_texture(&desc);
                let target_view = target.create_view(&Default::default());
                let msaa = (samples > 1).then(|| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        sample_count: samples,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        ..desc
                    })
                });
                let msaa_view = msaa.as_ref().map(|t| t.create_view(&Default::default()));
                let depth = device.create_texture(&wgpu::TextureDescriptor {
                    format: crate::renderer::DEPTH_FORMAT,
                    sample_count: samples,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    ..desc
                });
                let depth_view = depth.create_view(&Default::default());
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: msaa_view.as_ref().unwrap_or(&target_view),
                            resolve_target: msaa_view.as_ref().map(|_| &target_view),
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
                    pass.set_viewport(4.0, 4.0, 24.0, 24.0, 0.0, 1.0);
                    pass.set_scissor_rect(4, 4, 24, 24);
                    background.draw(&mut pass);
                }
                let pixels = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 32,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &target,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &pixels,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(32),
                        },
                    },
                    target.size(),
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                pixels
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = tx.send(result);
                    });
                device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                rx.recv().unwrap().unwrap();
                let data = pixels.slice(..).get_mapped_range().unwrap();
                assert_eq!(&data[..4], &[0, 0, 0, 255]);
                let center = 16 * 256 + 16 * 4;
                assert!((data[center] as i32 - 184).abs() <= 1);
                assert_eq!(&data[center + 1..center + 4], &[0, 0, 255]);
            }
        });
    }
}
