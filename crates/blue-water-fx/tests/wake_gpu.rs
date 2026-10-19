//! The preserved wake field runs on the JKA device, and what it writes binds to the real upstream surface layout
//! (the `Wake` uniform's size and the display/atlas formats are validated by wgpu when the material group is built).
use blue_water_core::{QualityTier, WaveParameters};
use blue_water_fx::hull_profile::SimpleHull;
use blue_water_fx::ship_wake::{ShipWake, WakeCamera, WakeInputs, WakeShip, WakeSplash};
use blue_water_gpu::original_surface::{OriginalSurfaceBindings, OriginalSurfacePipeline, WakeResources};

fn half(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

#[test]
fn ship_wake_field_runs_and_binds_to_the_upstream_surface() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })
            .await
            .expect("GPU device");
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut wake = ShipWake::new(&device, 512, 1.0);
        let field = blue_water_gpu::WaveField::new(&device, QualityTier::High);
        let waves = blue_water_gpu::surface_waves::SurfaceWaves::new(&device, &field);
        let pipeline = OriginalSurfacePipeline::new(&device, &waves, wgpu::TextureFormat::Rgba16Float, 1, 16);
        let mut bindings = OriginalSurfaceBindings::new(&device, &queue, &pipeline);
        {
            let b = wake.bindings();
            bindings.set_wake(&device, &pipeline, Some(WakeResources {
                uniform: b.uniforms.clone(),
                displays: [b.displays[0].clone(), b.displays[1].clone()],
                trail_atlas: b.trail_atlas.clone(),
                torpedo_field: b.torpedo_field.clone(),
            }));
        }
        assert_eq!(bindings.wake_generation(), 1);
        let hull = SimpleHull::warship(115.0, 12.0, 4.2);
        let profile = hull.profile();
        // A destroyer steams straight at 15 m/s for 20 s, with a shell falling beside her after ten.
        let mut z = 0.0;
        let _ = WaveParameters::default();
        for frame in 0..600 {
            z -= 15.0 / 30.0;
            let ships = [WakeShip {
                id: "dd", position: [0.0, 0.0, z], heading: 0.0, pitch: 0.0, roll: 0.0, speed: 15.0,
                length: 115.0, beam: 12.0, draft: 4.2, center_x: 0.0, center_z: 0.0,
                forward_speed: 18.0, reverse_speed: 7.0, profile: Some(&profile),
            }];
            let splashes = if frame == 300 {
                vec![WakeSplash { sequence: 1, x: 40.0, z: z - 20.0, caliber_m: 0.38 }]
            } else { Vec::new() };
            wake.update(&WakeInputs {
                ships: &ships, dt: 1.0 / 30.0, splashes: &splashes, torpedoes: &[],
                camera: WakeCamera { eye: [0.0, 30.0, z + 150.0], forward: [0.0, -0.2, -1.0], fov_y: 0.9, zoom: 1.0, drawing_height: 1080.0 },
                sea_height: 2.0,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            wake.encode(&device, &queue, &mut encoder);
            queue.submit(Some(encoder.finish()));
        }
        let n = 512u32;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wake display readback"), size: u64::from(n) * u64::from(n) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: wake.field.as_ref().unwrap().display_texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 8), rows_per_image: Some(n) } },
            wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("wake submission");
        assert!(scope.pop().await.is_none(), "wake GPU validation error");
        let bytes = read.slice(..).get_mapped_range().expect("wake map");
        let values: Vec<f32> = bytes.chunks_exact(2).map(|c| half(u16::from_le_bytes([c[0], c[1]]))).collect();
        let peak = |channel: usize| values.iter().skip(channel).step_by(4).fold(0.0f32, |m, v| m.max(v.abs()));
        let (height, foam) = (peak(0), peak(3));
        println!("wake display peaks: height {height:.4} m, slope {:.4}/{:.4}, foam {foam:.4}", peak(1), peak(2));
        assert!(height > 0.02, "a destroyer's wake raised nothing ({height})");
    });
}
