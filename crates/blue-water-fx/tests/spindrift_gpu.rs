//! Spindrift composes against the real surface group layouts, the JKA boundary gate compiles in, and a gale over a
//! settled field draws wisps (and calm air draws none).
use blue_water_core::{QualityTier, WaveParameters};
use blue_water_fx::spindrift::{PoolDraw, Spindrift, SpindriftFrame};
use blue_water_gpu::original_surface::{
    OriginalSurfaceBindings, OriginalSurfacePipeline, SurfaceBoundary, SurfaceCamera,
};
use glam::{Mat4, Vec3};

const SIZE: u32 = 256;

fn camera() -> SurfaceCamera {
    let position = Vec3::new(0.0, 6.0, 0.0);
    let view = Mat4::look_to_rh(position, Vec3::new(0.15, -0.12, -1.0).normalize(), Vec3::Y);
    let (near, far) = (0.1_f32, 20_000.0_f32);
    let fov = 0.9_f32;
    // Reversed-Z perspective: near maps to 1, far to 0.
    let f = 1.0 / (fov * 0.5).tan();
    let projection = Mat4::from_cols_array(&[
        f, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, near / (far - near), -1.0, 0.0, 0.0,
        near * far / (far - near), 0.0,
    ]);
    let view_projection = projection * view;
    SurfaceCamera {
        view: view.to_cols_array_2d(),
        projection: projection.to_cols_array_2d(),
        view_projection: view_projection.to_cols_array_2d(),
        inverse_view: view.inverse().to_cols_array_2d(),
        inverse_projection: projection.inverse().to_cols_array_2d(),
        inverse_view_projection: view_projection.inverse().to_cols_array_2d(),
        position_time: [position.x, position.y, position.z, 0.0],
        viewport: [SIZE as f32, SIZE as f32, 1.0 / SIZE as f32, 1.0 / SIZE as f32],
        near,
        far,
        tan_half_fov_y: (fov * 0.5).tan(),
        aspect: 1.0,
        dt: 1.0 / 60.0,
        frame: 0,
        curvature: 0.0,
        padding: 0,
    }
}

fn render(device: &wgpu::Device, queue: &wgpu::Queue, wind: f64) -> (u32, u32) {
    let mut field = blue_water_gpu::WaveField::new(device, QualityTier::High);
    let parameters = WaveParameters { significant_height: 6.0, wind_speed: wind, peak_wavelength: 80.0, ..Default::default() };
    for step in 0..180 {
        let mut encoder = device.create_command_encoder(&Default::default());
        let t = f64::from(step) / 30.0;
        field.update(queue, &mut encoder, parameters, true, t, 1.0 / 30.0, 1.0, 6.0);
        queue.submit(Some(encoder.finish()));
    }
    let waves = blue_water_gpu::surface_waves::SurfaceWaves::new(device, &field);
    waves.write(queue, &field, 6.0, 1.0, 1.0);
    let pipeline = OriginalSurfacePipeline::new(device, &waves, wgpu::TextureFormat::Rgba16Float, 1, 16);
    let bindings = OriginalSurfaceBindings::new(device, queue, &pipeline);
    bindings.write_camera(queue, &camera());
    let mut boundary: SurfaceBoundary = bytemuck::Zeroable::zeroed();
    boundary.bounds = [-1e8, -1e8, 1e8, 1e8];
    boundary.scale = [1.0, 0.75, 0.0, 0.0];
    bindings.write_boundary(queue, &boundary);
    let spindrift = Spindrift::new(
        device, &pipeline.camera_layout, waves.layout(), &pipeline.environment_layout,
        wgpu::TextureFormat::Rgba16Float, 1,
    );
    let target = |format, label| device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color = target(wgpu::TextureFormat::Rgba16Float, "color");
    let depth = target(wgpu::TextureFormat::Depth32Float, "depth");
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    // The soft-particle backdrop is a separate single-sample copy, never the pass's own attachment.
    let backdrop = target(wgpu::TextureFormat::Depth32Float, "backdrop depth");
    let backdrop_view = backdrop.create_view(&Default::default());
    let foam = blue_water_gpu::foam_texture::FoamMap::new(device, queue);
    let (cloud_view, cloud_sampler, cloud_uniform) = bindings.cloud_resources();
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spindrift test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        spindrift.draw(
            device, queue, &mut pass,
            &SpindriftFrame { wind_speed: wind, wind_direction: parameters.wind_direction, time: 6.0,
                significant_height: 6.0, flash: 0.0, eye: [0.0, 0.0], aerial: false },
            &backdrop_view, (&foam.view, &foam.sampler), waves.group(&field),
            &[PoolDraw { waves: None, camera: &bindings.camera_group, boundary: bindings.boundary(),
                cloud: (cloud_view, cloud_sampler, cloud_uniform), environment: &bindings.environment_group }],
        );
    }
    let bytes_per_row = SIZE * 8;
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("spindrift readback"), size: u64::from(bytes_per_row * SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(SIZE) } },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    let data = read.slice(..).get_mapped_range().expect("map");
    let lit = data.chunks_exact(8).filter(|px| px[..6].iter().any(|b| *b != 0)).count() as u32;
    (lit, SIZE * SIZE)
}

#[test]
fn a_gale_draws_wisps_and_calm_air_draws_none() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })
            .await.expect("GPU device");
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (calm, _) = render(&device, &queue, 8.0);
        let (gale, total) = render(&device, &queue, 22.0);
        if let Some(error) = scope.pop().await { panic!("spindrift GPU validation error: {error}"); }
        println!("spindrift lit pixels: calm {calm}, gale {gale} of {total}");
        assert_eq!(calm, 0);
        assert!(gale > 0, "a 22 m/s gale drew no spray");
    });
}
