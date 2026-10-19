use blue_water_sky::{Sky, SkyQuality, SkyScene, SkyTargets, SkyView, math::Vec3};

#[test]
fn original_atmosphere_precomputes_and_draws_finite_day_and_twilight() {
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
        let mut sky = Sky::new(
            &device,
            &queue,
            SkyQuality::Low,
            SkyTargets {
                color: wgpu::TextureFormat::Rgba16Float,
                depth: None,
            },
        );
        let sky_mean = blue_water_sky::SkyMeanSampler::new(&device);
        let mean_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("original sky mean readback"), size: 16, mapped_at_creation: false,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        });
        let extent = wgpu::Extent3d {
            width: 32,
            height: 16,
            depth_or_array_layers: 1,
        };
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Original atmosphere GPU fixture"),
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
            label: Some("Atmosphere readback"),
            size: 256 * 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = SkyView::looking_at(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 800.0, 1000.0),
            52.0,
            [32, 16],
        );
        let mut means = Vec::new();
        let mut sky_references = Vec::new();
        for elevation in [35.0, -8.0] {
            sky.apply(&SkyScene::north_atlantic(elevation, 135.0).cloudless());
            let mut encoder = device.create_command_encoder(&Default::default());
            sky.update(&queue, &mut encoder, &view, 1.0 / 60.0);
            sky_mean.encode(&mut encoder, sky.environment());
            encoder.copy_buffer_to_buffer(&sky_mean.buffer, 0, &mean_read, 0, 16);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Original atmosphere dome fixture"),
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
                sky.draw_dome(&mut pass);
            }
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(16),
                    },
                },
                extent,
            );
            let atlas = &sky.environment().texture;
            let atlas_size = atlas.size();
            let atlas_row = (atlas_size.width * 8).div_ceil(256) * 256;
            let atlas_read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Original sky environment readback"),
                size: u64::from(atlas_row * atlas_size.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                atlas.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &atlas_read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(atlas_row),
                        rows_per_image: Some(atlas_size.height),
                    },
                },
                atlas_size,
            );
            queue.submit([encoder.finish()]);
            atlas_read
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
            read.slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
            mean_read.slice(..).map_async(wgpu::MapMode::Read, |result| result.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let mut sum = 0.0f64;
            {
                let bytes = read.slice(..).get_mapped_range().unwrap();
                for pixel in bytes.chunks_exact(8) {
                    for channel in 0..3 {
                        let bits = u16::from_le_bytes(
                            pixel[channel * 2..channel * 2 + 2].try_into().unwrap(),
                        );
                        assert_ne!(bits & 0x7c00, 0x7c00, "finite atmosphere radiance");
                        assert!(
                            bits & 0x8000 == 0 || bits & 0x7fff == 0,
                            "nonnegative atmosphere radiance"
                        );
                        let exponent = i32::from((bits >> 10) & 31);
                        let mantissa = f64::from(bits & 1023) / 1024.0;
                        sum += if exponent == 0 {
                            mantissa * 2.0f64.powi(-14)
                        } else {
                            (1.0 + mantissa) * 2.0f64.powi(exponent - 15)
                        };
                    }
                }
            }
            read.unmap();
            {
                let bytes = mean_read.slice(..).get_mapped_range().unwrap();
                let reference = bytemuck::cast_slice::<u8, f32>(&bytes);
                assert!(reference.iter().all(|v| v.is_finite() && *v > 0.0), "sky reference and clear luminance: {reference:?}");
                sky_references.push(reference[3]);
            }
            mean_read.unmap();
            {
                let atlas_bytes = atlas_read.slice(..).get_mapped_range().unwrap();
                for bytes in atlas_bytes.chunks_exact(2) {
                    let half = u16::from_le_bytes(bytes.try_into().unwrap());
                    assert_ne!(
                        half & 0x7c00,
                        0x7c00,
                        "finite environment atlas at every prefilter level"
                    );
                }
            }
            atlas_read.unmap();
            means.push(sum / (32.0 * 16.0 * 3.0));
            assert!(
                sky.environment().generation > 0,
                "original environment baked"
            );
        }
        assert!(
            means[0] > means[1] * 2.0 && means[1] > 0.0,
            "day/twilight radiance: {means:?}"
        );
        assert!(sky_references[0] > sky_references[1] * 2.0, "actual sky reference follows dusk: {sky_references:?}");
        assert!(scope.pop().await.is_none(), "atmosphere GPU validation");
    });
}
