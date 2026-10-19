use super::*;
use crate::renderer::{AutoExposureState, AutoExposureUniform, PostUniform, block_on, weather};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[test]
fn native_exposure_preserves_night_and_limits_daylight_glare() {
    block_on(async {
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
        let mut exposure = NativeExposure::new(&device);
        let pipeline_error = scope.pop().await;
        assert!(
            pipeline_error.is_none(),
            "native exposure pipeline creation: {pipeline_error:?}"
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let scene = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native exposure regression"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = scene.create_view(&Default::default());
        let state = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&AutoExposureState::zeroed()),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let settings = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        });
        let read_values = |encoder: wgpu::CommandEncoder| {
            queue.submit([encoder.finish()]);
            read.slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let values: [f32; 4] =
                bytemuck::cast_slice::<u8, f32>(&read.slice(..).get_mapped_range().unwrap())
                    .try_into()
                    .unwrap();
            read.unmap();
            values
        };
        let mut meter = |sky: f32, clear: f32, value: f32, bright: bool, under: bool| {
            let mut pixels = vec![[value, value, value, 1.0]; 256];
            if bright {
                pixels[..26].fill([8.0, 8.0, 8.0, 1.0]);
            }
            queue.write_texture(
                scene.as_image_copy(),
                bytemuck::cast_slice(&pixels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(16),
                },
                scene.size(),
            );
            queue.write_buffer(
                &exposure.sky_mean.buffer,
                0,
                bytemuck::cast_slice(&[sky, sky, sky, clear]),
            );
            queue.write_buffer(&state, 0, bytemuck::bytes_of(&AutoExposureState::zeroed()));
            queue.write_buffer(
                &settings,
                0,
                bytemuck::bytes_of(&AutoExposureUniform {
                    params: [1.0, 1.0, -1.0, 2.0],
                    adaptation: [0.18, 100.0, 100.0, if under { 1.0 } else { 0.0 }],
                }),
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            exposure.adapt(&device, &mut encoder, &view, &state, &settings);
            encoder.copy_buffer_to_buffer(&state, 0, &read, 0, 16);
            read_values(encoder)
        };
        assert!(
            meter(1.0, 1.0, 1.0, false, false)[2].abs() < 0.001,
            "sky keeps fixed exposure"
        );
        let dark_day = meter(1.0, 1.0, 0.125, false, false);
        assert!(
            (dark_day[2] - 2.0).abs() < 0.001,
            "day opening bounded at two stops: {dark_day:?}"
        );
        assert!(
            (meter(0.25, 1.0, 0.25, false, false)[2] - 1.63).abs() < 0.01,
            "overcast meters against clear sky"
        );
        assert!(
            (meter(1.0, 1.0, 0.125, true, false)[2] + 1.0).abs() < 0.001,
            "brightest tenth closes for glare"
        );
        let night = meter(0.005, 0.01, 0.001, false, false);
        let cloud_night = meter(0.001, 0.01, 0.001, false, false);
        assert_eq!(night[2], 0.0, "night cannot brighten into daylight");
        assert_eq!(
            night[3], cloud_night[3],
            "cloud cover does not alter night grading"
        );
        assert_eq!(
            meter(1.0, 1.0, 0.125, false, true)[2],
            0.0,
            "underwater returns to fixed exposure"
        );
        let missing = meter(0.0, 0.0, 0.001, false, false);
        assert_eq!(missing[2], 0.0);
        assert_eq!(
            missing[3], 0.0,
            "missing sky reference cannot leave night grading enabled"
        );

        // Exercise the real post function: nearby world shadows lift, the sky
        // dims, distant water joins the sky, and underwater open deep stays world.
        let mut source = weather::with_weather_surface(&weather::with_post_settings(include_str!(
            "../../../post.wgsl"
        )));
        source.push_str(
            r#"
@group(1) @binding(1) var<storage, read_write> result: array<f32>;
@compute @workgroup_size(1)
fn night_probe() {
    result[0] = native_night_readability(vec3<f32>(0.01), 1000.0).r;
    result[1] = native_night_readability(vec3<f32>(0.01), 1000000.0).r;
    result[2] = native_night_readability(vec3<f32>(0.01), 480000.0).r;
    result[3] = native_night_readability(vec3<f32>(1.0), 1000.0).r;
}
"#,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native night-grade regression"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("night_probe"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut post = PostUniform::zeroed();
        post.exposure_controls[1] = 1.0;
        post.underwater[0] = -1e30;
        let post_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&post),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        queue.write_buffer(&state, 0, bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 1.0]));
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            mapped_at_creation: false,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: post_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 24,
                    resource: state.as_entire_binding(),
                },
            ],
        });
        let out_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            }],
        });
        let grade = || {
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.set_bind_group(1, &out_group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 16);
            read_values(encoder)
        };
        let graded = grade();
        assert!(graded[0] > 0.02 && graded[1] < 0.01);
        assert_eq!(graded[1], graded[2], "distant water and sky stay joined");
        assert_eq!(graded[3], 1.0, "highlights unchanged");
        post.underwater[0] = 0.0;
        queue.write_buffer(&post_buffer, 0, bytemuck::bytes_of(&post));
        let submerged = grade();
        assert_eq!(
            submerged[0], submerged[1],
            "open deep and underside share the world grade"
        );
        assert!(
            scope.pop().await.is_none(),
            "native exposure GPU validation"
        );
    });
}
