//! Diagnostic: dome radiance at night, printed per scenario. `cargo test -p blue-water-sky --test night_probe -- --nocapture`
use blue_water_sky::{Sky, SkyAngles, SkyQuality, SkyScene, SkyTargets, SkyView, math::Vec3};

fn half(bits: u16) -> f32 {
    let exponent = i32::from((bits >> 10) & 31);
    let mantissa = f32::from(bits & 1023) / 1024.0;
    let value = if exponent == 0 {
        mantissa * 2.0f32.powi(-14)
    } else {
        (1.0 + mantissa) * 2.0f32.powi(exponent - 15)
    };
    if bits & 0x8000 != 0 { -value } else { value }
}

#[test]
fn night_dome_radiance() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })
            .await
            .expect("GPU device");
        let mut sky = Sky::new(
            &device,
            &queue,
            SkyQuality::High,
            SkyTargets { color: wgpu::TextureFormat::Rgba16Float, depth: None },
        );
        let sky_mean = blue_water_sky::SkyMeanSampler::new(&device);
        let mean_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("night probe sky mean"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (w, h) = (64u32, 36u32);
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("night probe"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("night probe readback"),
            size: 512 * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Looking up about 60 degrees, as in the screenshot.
        let view = SkyView::looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1000.0, 600.0), 90.0, [w, h]);
        let cases: [(&str, f64, Option<f64>, f64); 8] = [
            ("sun -45 moon +10 full", -45.0, Some(10.0), 1.0),
            ("sun -45 moon 0 full", -45.0, Some(0.0), 1.0),
            ("sun -45 moon -2 full", -45.0, Some(-2.0), 1.0),
            ("sun -45 moon -4 full", -45.0, Some(-4.0), 1.0),
            ("sun -45 moon -6 full", -45.0, Some(-6.0), 1.0),
            ("sun -45 moon -8 full", -45.0, Some(-8.0), 1.0),
            ("sun -45 moon -4 half", -45.0, Some(-4.0), 0.5),
            ("sun -45 moon -15 full", -45.0, Some(-15.0), 1.0),
        ];
        for (label, sun, moon, phase) in cases {
            let mut scene = SkyScene::north_atlantic(sun, 135.0).cloudless();
            scene.moon_phase = phase;
            if label.contains("I0") { scene.sun_intensity = 0.0; }
            scene.moon = moon.map(|elevation| SkyAngles { elevation, azimuth: 315.0 });
            sky.apply(&scene);
            let mut encoder = device.create_command_encoder(&Default::default());
            sky.update(&queue, &mut encoder, &view, 1.0 / 60.0);
            sky_mean.encode(&mut encoder, sky.environment());
            encoder.copy_buffer_to_buffer(&sky_mean.buffer, 0, &mean_read, 0, 16);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("night probe"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &output_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                sky.draw_dome(&mut pass);
            }
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(512), rows_per_image: Some(h) },
                },
                extent,
            );
            queue.submit([encoder.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            mean_read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
            let mut sum = [0.0f64; 3];
            let mut min = f32::MAX;
            let mut max = 0.0f32;
            let mut centre = [0.0f32; 3];
            {
                let bytes = read.slice(..).get_mapped_range().unwrap();
                for y in 0..h {
                    for x in 0..w {
                        let at = (y * 512 + x * 8) as usize;
                        let mut px = [0.0f32; 3];
                        for c in 0..3 {
                            px[c] = half(u16::from_le_bytes([bytes[at + c * 2], bytes[at + c * 2 + 1]]));
                            sum[c] += f64::from(px[c]);
                        }
                        let lum = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
                        min = min.min(lum);
                        max = max.max(lum);
                        if x == w / 2 && y == h / 2 {
                            centre = px;
                        }
                    }
                }
            }
            read.unmap();
            let mean: [f32; 4] = {
                let bytes = mean_read.slice(..).get_mapped_range().unwrap();
                let values = bytemuck::cast_slice::<u8, f32>(&bytes);
                [values[0], values[1], values[2], values[3]]
            };
            mean_read.unmap();
            let n = f64::from(w * h);
            println!(
                "{label:40} mean rgb ({:.5}, {:.5}, {:.5})  lum min {:.5} max {:.4}  centre {:?}  SKYMEAN {:?} clear_log2 {:.3}",
                sum[0] / n, sum[1] / n, sum[2] / n, min, max, centre, mean, mean[3].max(1e-6).log2()
            );
        }
    });
}
