//! The original world-space wave-height sampler, bounded to promoted water.
use crate::{original_surface::OriginalSurfaceBindings, shader_composer};
use naga_oil::compose::{NagaModuleDescriptor, ShaderDefValue, ShaderType};

pub const SPAN: f32 = 256.0;
pub const TEXELS: u32 = 256;
pub struct RainHeights {
    pub view: wgpu::TextureView,
    camera_layout: wgpu::BindGroupLayout,
    wake_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WaveField,
        original_surface::{OriginalSurfacePipeline, SurfaceBoundary, SurfaceCamera},
        surface_waves::SurfaceWaves,
    };
    use bytemuck::Zeroable;
    #[test]
    fn splash_heights_follow_fft_planes_and_water_boundaries() {
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
            let mut field = WaveField::new(&device, blue_water_core::QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            let pipeline = OriginalSurfacePipeline::new(
                &device,
                &waves,
                wgpu::TextureFormat::Rgba16Float,
                1,
                16,
            );
            let surface = OriginalSurfaceBindings::new(&device, &queue, &pipeline);
            let mut camera = SurfaceCamera::zeroed();
            camera.position_time = [0., 2., 0., 0.];
            surface.write_camera(&queue, &camera);
            let mut boundary = SurfaceBoundary::zeroed();
            boundary.bounds = [-2000., -2000., 2000., 2000.];
            boundary.scale = [40., 0.75, 0., 0.];
            surface.write_boundary(&queue, &boundary);
            let heights = RainHeights::new(&device, waves.layout());
            let read_surface = heights.surface(&device, &surface);
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (TEXELS * TEXELS * 8) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let half = |bits: u16| {
                let sign = if bits & 0x8000 == 0 { 1. } else { -1. };
                let exponent = (bits >> 10) & 31;
                let mantissa = (bits & 1023) as f32;
                sign * if exponent == 0 {
                    mantissa * 2f32.powi(-24)
                } else {
                    (1. + mantissa / 1024.) * 2f32.powi(exponent as i32 - 15)
                }
            };
            let mut original = Vec::new();
            for eye_y in [2., 5.] {
                let mut encoder = device.create_command_encoder(&Default::default());
                field.update(
                    &queue,
                    &mut encoder,
                    blue_water_core::WaveParameters::default(),
                    true,
                    1.,
                    1. / 60.,
                    1.,
                    1.,
                );
                waves.write(&queue, &field, 1., 1., 1.);
                heights.begin(&queue, &mut encoder, [0., eye_y, 0.]);
                heights.encode(&mut encoder, &read_surface, waves.group(&field));
                encoder.copy_texture_to_buffer(
                    heights.view.texture().as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(TEXELS * 8),
                            rows_per_image: Some(TEXELS),
                        },
                    },
                    wgpu::Extent3d {
                        width: TEXELS,
                        height: TEXELS,
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
                let mapped = read.slice(..).get_mapped_range().unwrap();
                let data = bytemuck::cast_slice::<u8, u16>(&mapped);
                let admitted: Vec<f32> = data
                    .chunks_exact(4)
                    .filter(|p| p[3] != 0)
                    .map(|p| half(p[0]))
                    .collect();
                assert_eq!(
                    admitted.len(),
                    10000,
                    "only the promoted 100 m square admits splashes"
                );
                if eye_y == 2. {
                    let low = admitted.iter().copied().fold(f32::INFINITY, f32::min);
                    let high = admitted.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    assert!(
                        high - low > 0.1,
                        "splashes must sample actual crests and troughs"
                    );
                    original = admitted;
                } else {
                    for (a, b) in original.iter().zip(admitted) {
                        assert!(
                            (b - a - 3.).abs() < 0.025,
                            "water-plane offset must match weather coordinates"
                        );
                    }
                }
                drop(mapped);
                read.unmap();
            }
            assert!(scope.pop().await.is_none());
        });
    }
}
pub struct SurfaceRead {
    camera: wgpu::BindGroup,
    wake: wgpu::BindGroup,
}
impl RainHeights {
    pub fn new(device: &wgpu::Device, waves: &wgpu::BindGroupLayout) -> Self {
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain height camera"),
            entries: &[uniform(0)],
        });
        let wake_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain height wake and boundary"),
            entries: &[
                uniform(5),
                texture(6),
                texture(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(16),
            ],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain height output"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("native rain wave heights and water admission"),
                size: wgpu::Extent3d {
                    width: TEXELS,
                    height: TEXELS,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native rain patch"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let original =
            include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/rain_heights.wgsl");
        let store = "textureStore(heights_out, vec2<i32>(id.xy), vec4<f32>(height_at(xz) + wake_height(xz), 0.0, 0.0, 1.0));";
        assert_eq!(original.matches(store).count(), 1);
        let source=original.replace("centre: vec4<f32>,", "centre: vec4<f32>,\n    eye: vec4<f32>,")
            .replace("#import naval_ocean::waves::height_at", "#import naval_ocean::waves::height_at\n#import naval::camera::camera")
            .replace(store,"if !jka_water_contains(xz) { return; }\n    let relative_height = height_at(xz) + wake_height(xz) + rain_patch.eye.x - camera.position.y;\n    textureStore(heights_out, vec2<i32>(id.xy), vec4<f32>(relative_height, 0.0, 0.0, 1.0));");
        let source = format!("{source}\n{}", include_str!("surface_boundary.wgsl"));
        let mut composer = shader_composer::composer().expect("rain height modules");
        let module = composer
            .make_naga_module(NagaModuleDescriptor {
                source: &source,
                file_path: "JKA native rain heights",
                shader_type: ShaderType::Wgsl,
                shader_defs: [("CASCADES".into(), ShaderDefValue::UInt(3))]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native rain height sampler"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("native rain height sampler"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[
                        Some(&camera_layout),
                        Some(&wake_layout),
                        Some(waves),
                        Some(&layout),
                    ],
                    immediate_size: 0,
                }),
            ),
            module: &module,
            entry_point: Some("rain_heights_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            view,
            camera_layout,
            wake_layout,
            params,
            group,
            pipeline,
        }
    }
    pub fn surface(&self, device: &wgpu::Device, surface: &OriginalSurfaceBindings) -> SurfaceRead {
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: surface.camera.as_entire_binding(),
            }],
        });
        let wake = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.wake_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: surface.wake_resources().uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&surface.wake_resources().displays[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&surface.wake_resources().displays[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&surface.wake_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: surface.boundary.as_entire_binding(),
                },
            ],
        });
        SurfaceRead { camera, wake }
    }
    pub fn begin(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, eye: [f32; 3]) {
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[eye[0], eye[2], SPAN, TEXELS as f32, eye[1], 0., 0., 0.]),
        );
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear rain height admission"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface: &SurfaceRead,
        waves: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("sample splash wave heights"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &surface.camera, &[]);
        pass.set_bind_group(1, &surface.wake, &[]);
        pass.set_bind_group(2, waves, &[]);
        pass.set_bind_group(3, &self.group, &[]);
        pass.dispatch_workgroups(TEXELS.div_ceil(8), TEXELS.div_ceil(8), 1);
    }
}
