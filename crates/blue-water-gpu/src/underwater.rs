//! Preserved Blue Water probe, waterline/composite, and suspended motes.
//! JKA supplies the ray's bounded length in water, in metres.
use crate::{original_surface::{OriginalSurfaceBindings, SurfaceAppearance, SurfaceCamera}, shader_composer};
use bytemuck::{Pod, Zeroable};

/// Optical path multiplier used by the preserved underwater composite.
/// Keep CPU absorption culling matched to this value.
pub const UNDERWATER_CLARITY: f32 = 0.15;

pub struct UnderwaterPipelines {
    camera_layout: wgpu::BindGroupLayout,
    wake_layout: wgpu::BindGroupLayout,
    probe_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    probe: wgpu::ComputePipeline,
    dim: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    motes: wgpu::RenderPipeline,
}

fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty: wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None,
    }, count: None }
}
fn texture(binding: u32, visibility: wgpu::ShaderStages, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty: wgpu::BindingType::Texture {
        sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
    }, count: None }
}
fn sampler(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}
fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some("Blue Water underwater parameters"), size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}
fn entry_buffer(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
}
fn entry_texture(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}
fn entry_sampler(binding: u32, sampler: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::Sampler(sampler) }
}

impl UnderwaterPipelines {
    pub fn new(device: &wgpu::Device, waves: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, samples: u32) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water underwater camera"), entries: &[uniform(0, both | compute)],
        });
        let wake_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water underwater sea read"), entries: &[
                uniform(5, compute), texture(6, compute, filterable), texture(7, compute, filterable),
                sampler(8, compute), uniform(16, compute),
            ],
        });
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water underwater probe output"), entries: &[uniform(0, compute),
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: compute,
                    ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water underwater material"), entries: &[
                uniform(0, both), texture(1, fragment, wgpu::TextureSampleType::Depth),
                texture(2, both, filterable), sampler(3, both),
                texture(4, both, wgpu::TextureSampleType::Float { filterable: false }),
                uniform(16, wgpu::ShaderStages::VERTEX),
            ],
        });
        let mut composer = shader_composer::composer().expect("Blue Water underwater modules");
        let module = |composer: &mut naga_oil::compose::Composer, name| {
            let module = shader_composer::underwater_module(composer, name)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(composer)));
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name),
                source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)) })
        };
        let probe_module = module(&mut composer, "probe");
        let probe_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water underwater probe"),
            bind_group_layouts: &[Some(&camera_layout), Some(&wake_layout), Some(waves), Some(&probe_layout)], immediate_size: 0,
        });
        let probe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Blue Water original underwater probe"), layout: Some(&probe_pipeline_layout),
            module: &probe_module, entry_point: Some("probe_main"), compilation_options: Default::default(), cache: None,
        });
        let composite_module = module(&mut composer, "composite");
        let mote_module = module(&mut composer, "motes");
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water underwater composite"),
            bind_group_layouts: &[Some(&camera_layout), Some(&material_layout)], immediate_size: 0,
        });
        let pipeline = |module: &wgpu::ShaderModule, vertex, fragment, source, destination| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fragment), layout: Some(&composite_layout),
                vertex: wgpu::VertexState { module, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module, entry_point: Some(fragment), compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, write_mask: wgpu::ColorWrites::ALL,
                        blend: Some(wgpu::BlendState { color: wgpu::BlendComponent { src_factor: source, dst_factor: destination,
                            operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add } }) })] }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
            })
        };
        let dim = pipeline(&composite_module, "vertex", "underwater_dim", wgpu::BlendFactor::Zero, wgpu::BlendFactor::Src);
        let light = pipeline(&composite_module, "vertex", "underwater_light", wgpu::BlendFactor::One, wgpu::BlendFactor::One);
        let motes = pipeline(&mote_module, "mote_vertex", "mote_fragment", wgpu::BlendFactor::One, wgpu::BlendFactor::One);
        Self { camera_layout, wake_layout, probe_layout, material_layout, probe, dim, light, motes }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeParams {
    inverse_projection: [[f32; 4]; 4], camera_world: [[f32; 4]; 4], camera: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WaterParams {
    prefix: [[f32; 4]; 5], water: blue_water_core::optics::WaterField,
}

pub struct UnderwaterBindings {
    camera: wgpu::BindGroup,
    wake: wgpu::BindGroup,
    probe: wgpu::BindGroup,
    material: wgpu::BindGroup,
    probe_params: wgpu::Buffer,
    water_params: wgpu::Buffer,
}

impl UnderwaterBindings {
    pub fn new(device: &wgpu::Device, pipelines: &UnderwaterPipelines, surface: &OriginalSurfaceBindings,
        scene_depth: &wgpu::TextureView, water_distance: &wgpu::TextureView) -> Self {
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water underwater camera"), layout: &pipelines.camera_layout,
            entries: &[entry_buffer(0, &surface.camera)],
        });
        let wake = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water underwater wake/boundary"), layout: &pipelines.wake_layout,
            entries: &[entry_buffer(5, &surface.wake_resources().uniform), entry_texture(6, &surface.wake_resources().displays[0]),
                entry_texture(7, &surface.wake_resources().displays[1]), entry_sampler(8, &surface.wake_sampler), entry_buffer(16, &surface.boundary)],
        });
        let probe_params = buffer(device, std::mem::size_of::<ProbeParams>() as u64);
        let water_params = buffer(device, std::mem::size_of::<WaterParams>() as u64);
        let probe = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water original underwater probe"), layout: &pipelines.probe_layout,
            entries: &[entry_buffer(0, &probe_params), entry_texture(1, &surface.probe_view)],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Blue Water underwater probe sampler"), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, ..Default::default()
        });
        let material = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water original underwater material"), layout: &pipelines.material_layout,
            entries: &[entry_buffer(0, &water_params), entry_texture(1, scene_depth),
                entry_texture(2, &surface.probe_view), entry_sampler(3, &sampler), entry_texture(4, water_distance),
                entry_buffer(16, &surface.boundary)],
        });
        Self { camera, wake, probe, material, probe_params, water_params }
    }

    pub fn write(&self, queue: &wgpu::Queue, camera: &SurfaceCamera, appearance: SurfaceAppearance,
        sun: [f32; 3], sun_color: [f32; 3], near_surface: bool) {
        let probe = ProbeParams {
            inverse_projection: camera.inverse_projection, camera_world: camera.inverse_view,
            camera: [camera.position_time[0], camera.position_time[1], camera.position_time[2],
                if near_surface { camera.near } else { 0.0 }],
        };
        queue.write_buffer(&self.probe_params, 0, bytemuck::bytes_of(&probe));
        let mut water = WaterParams::zeroed();
        water.prefix[0] = [appearance.pigment[0], appearance.pigment[1], appearance.pigment[2], 0.0];
        water.prefix[1] = [appearance.absorption[0], appearance.absorption[1], appearance.absorption[2], UNDERWATER_CLARITY];
        water.prefix[2] = [appearance.transmission[0], appearance.transmission[1], appearance.transmission[2], 0.0];
        water.prefix[3] = [sun[0], sun[1], sun[2], 0.0];
        water.prefix[4] = [sun_color[0], sun_color[1], sun_color[2], 0.0];
        queue.write_buffer(&self.water_params, 0, bytemuck::bytes_of(&water));
    }

    pub fn probe(&self, encoder: &mut wgpu::CommandEncoder, pipelines: &UnderwaterPipelines, waves: &wgpu::BindGroup) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blue Water original underwater probe"), timestamp_writes: None,
        });
        pass.set_pipeline(&pipelines.probe);
        pass.set_bind_group(0, &self.camera, &[]);
        pass.set_bind_group(1, &self.wake, &[]);
        pass.set_bind_group(2, waves, &[]);
        pass.set_bind_group(3, &self.probe, &[]);
        pass.dispatch_workgroups(16, 9, 1);
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, pipelines: &'a UnderwaterPipelines, motes: bool) {
        pass.set_bind_group(0, &self.camera, &[]);
        pass.set_bind_group(1, &self.material, &[]);
        pass.set_pipeline(&pipelines.dim);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&pipelines.light);
        pass.draw(0..3, 0..1);
        if motes {
            pass.set_pipeline(&pipelines.motes);
            pass.draw(0..6, 0..768);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{original_surface::{OriginalSurfacePipeline, SurfaceBoundary}, surface_waves::SurfaceWaves};

    #[test]
    fn underwater_probe_and_composite_respect_dry_pixels_and_bounded_paths() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let field = crate::WaveField::new(&device, blue_water_core::QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            waves.write(&queue, &field, 0.0, 0.0, 1.0);
            let surface_pipeline = OriginalSurfacePipeline::new(&device, &waves, wgpu::TextureFormat::Rgba8Unorm, 1, 16);
            let surface = OriginalSurfaceBindings::new(&device, &queue, &surface_pipeline);
            let pipelines = UnderwaterPipelines::new(&device, waves.layout(), wgpu::TextureFormat::Rgba8Unorm, 1);
            let size = wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 };
            let target = |format| device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Underwater wet/dry readback fixture"), size, mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST, view_formats: &[],
            });
            let color = target(wgpu::TextureFormat::Rgba8Unorm);
            let color_view = color.create_view(&Default::default());
            let depth = target(wgpu::TextureFormat::Depth32Float).create_view(&Default::default());
            let distance = target(wgpu::TextureFormat::R32Float);
            let distance_view = distance.create_view(&Default::default());
            let scene = crate::scene_copy::SceneCopy::new(&device, 16, 16, 1);
            let bindings = UnderwaterBindings::new(&device, &pipelines, &surface, &scene.depth, &distance_view);
            let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256 * 17,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            for case in 0..8 {
                let wet = case == 1 || case == 2 || case == 5 || case == 7;
                let position = glam::Vec3::new(0.0, if case == 0 { 2.0 } else if case == 5 { 0.0 } else { -2.0 }, 0.0);
                let view = glam::Mat4::look_to_rh(position, -glam::Vec3::Z, glam::Vec3::Y);
                let projection = glam::Mat4::perspective_rh(1.0, 1.0, 1000.0, 0.025);
                let vp = projection * view;
                let camera = SurfaceCamera {
                    view: view.to_cols_array_2d(), inverse_view: view.inverse().to_cols_array_2d(),
                    projection: projection.to_cols_array_2d(), inverse_projection: projection.inverse().to_cols_array_2d(),
                    view_projection: vp.to_cols_array_2d(), inverse_view_projection: vp.inverse().to_cols_array_2d(),
                    position_time: [position.x, position.y, position.z, 0.0], viewport: [16.0, 16.0, 0.0625, 0.0625],
                    near: 0.025, far: 1000.0, tan_half_fov_y: 0.5_f32.tan(), aspect: 1.0,
                    ..SurfaceCamera::zeroed()
                };
                surface.write_camera(&queue, &camera);
                let boundary = SurfaceBoundary { bounds: if case == 3 { [4000.0, 4000.0, 8000.0, 8000.0] }
                    else { [-4000.0, -4000.0, 4000.0, 4000.0] }, scale: [40.0, 0.75, 0.0, 0.0], ..SurfaceBoundary::zeroed() };
                let boundary = SurfaceBoundary {
                    depth: if case == 6 { [-1.0,1.0,0.0,0.0] }
                        else if case == 7 { [-3.0,1.0,0.0,0.0] } else { [0.0;4] },
                    ..boundary
                };
                surface.write_boundary(&queue, &boundary);
                let appearance = SurfaceAppearance { pigment: [0.0; 3], ..Default::default() };
                bindings.write(&queue, &camera, appearance, [0.0, 1.0, 0.0], [1.0; 3], case != 4);
                let path = if case == 1 { 0.0f32 } else { 10.0f32 };
                queue.write_texture(distance.as_image_copy(), bytemuck::cast_slice(&[path; 256]),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64), rows_per_image: Some(16) }, size);
                let mut encoder = device.create_command_encoder(&Default::default());
                bindings.probe(&mut encoder, &pipelines, waves.group(&field));
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color_view, depth_slice: None, resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                        ..Default::default()
                    });
                }
                scene.capture(&device, &mut encoder, &color_view, &depth);
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color_view, depth_slice: None, resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth,
                            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
                        ..Default::default()
                    });
                    bindings.draw(&mut pass, &pipelines, true);
                }
                encoder.copy_texture_to_buffer(color.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(16) } }, size);
                encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo {
                    texture: surface.probe_view.texture(), mip_level: 0, origin: wgpu::Origin3d { x: 64, y: 36, z: 0 }, aspect: wgpu::TextureAspect::All,
                }, wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout {
                    offset: 256 * 16, bytes_per_row: Some(256), rows_per_image: Some(1) } },
                    wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
                queue.submit([encoder.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                {
                    let bytes = read.slice(..).get_mapped_range().unwrap();
                    let probe = u16::from_le_bytes(bytes[4096..4098].try_into().unwrap());
                    assert_eq!(probe & 0x8000 == 0, wet, "case {case}: probe sign");
                    for (channel, absorption) in appearance.absorption.into_iter().enumerate() {
                        let attenuated = (-absorption * 10.0 * 0.15).exp();
                        let expected = if case == 2 || case == 7 { attenuated }
                            else if case == 5 { attenuated * 0.15 + appearance.transmission[channel] * 0.85 }
                            else { 1.0 };
                        let actual = f32::from(bytes[8 * 256 + 8 * 4 + channel]) / 255.0;
                        assert!((actual - expected).abs() < 0.008, "case {case}, channel {channel}: {actual} != {expected}");
                        if case == 5 {
                            // A horizontal lens crossing the calm surface:
                            // dry sky above, tinted meniscus, attenuated water below.
                            assert_eq!(bytes[8 * 4 + channel], 255, "dry waterline side");
                            let below = f32::from(bytes[15 * 256 + 8 * 4 + channel]) / 255.0;
                            assert!((below - attenuated).abs() < 0.008, "wet waterline side");
                        }
                    }
                }
                read.unmap();
            }
            assert!(scope.pop().await.is_none(), "underwater GPU validation");
        });
    }
}
