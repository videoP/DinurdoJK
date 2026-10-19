use blue_water_core::{QualityTier, WaveParameters};
use blue_water_gpu::{shore::ShoreMap, surface_waves::SurfaceWaves, WaveField};
use naga_oil::compose::{NagaModuleDescriptor, ShaderDefValue, ShaderType};
use std::sync::Arc;

#[test]
fn upstream_coast_changes_real_waves_and_moves_surf_without_leaking_between_pools() {
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
        let mut field = WaveField::new(&device, QualityTier::High);
        let mut encoder = device.create_command_encoder(&Default::default());
        field.update(
            &queue,
            &mut encoder,
            WaveParameters {
                peak_wavelength: 70.,
                significant_height: 1.8,
                wind_speed: 9.,
                ..Default::default()
            },
            false,
            3.,
            0.02,
            1.,
            1.,
        );
        queue.submit(Some(encoder.finish()));
        let open = SurfaceWaves::new(&device, &field);
        let map = |depth: f32| {
            Arc::new(ShoreMap::build(
                vec![-depth; 17 * 17],
                17,
                17,
                10.,
                [-80., -80.],
                0.,
            ))
        };
        let shallow = SurfaceWaves::with_shore(&device, &queue, &field, map(0.03));
        let deep = SurfaceWaves::with_shore(&device, &queue, &field, map(10000.));
        let land = (0..17 * 17)
            .map(|i| if i % 17 < 8 { 1. } else { -10000. })
            .collect();
        let lee_map = Arc::new(ShoreMap::build(land, 17, 17, 10., [-80., -80.], 0.));
        let lee = SurfaceWaves::with_shore(&device, &queue, &field, Arc::clone(&lee_map));
        let source = r#"
#import naval_ocean::waves::{displacement, shore_at, surf}
@group(0) @binding(0) var<storage,read_write> result: array<vec4<f32>,2>;
@compute @workgroup_size(1)
fn main() {
 let at=vec2<f32>(40.0,0.0); let s=shore_at(at);
 result[0]=vec4<f32>(displacement(at,0.5),s.scales.x);
 result[1]=vec4<f32>(s.depth,s.arriving,surf(at,s,0.01,0.01));
}
"#;
        let mut composer = blue_water_gpu::shader_composer::composer().unwrap();
        let module = composer
            .make_naga_module(NagaModuleDescriptor {
                source,
                file_path: "coast GPU fixture",
                shader_type: ShaderType::Wgsl,
                shader_defs: [("CASCADES".into(), ShaderDefValue::UInt(3))].into(),
                ..Default::default()
            })
            .unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("coast fixture"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &empty,
            entries: &[],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(&empty), Some(open.layout())],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let sample = |field: &WaveField, waves: &SurfaceWaves, time: f64| {
            waves.write(&queue, &field, time, 1., 1.);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.set_bind_group(1, &empty_group, &[]);
                pass.set_bind_group(2, waves.group(&field), &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 32);
            queue.submit(Some(encoder.finish()));
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap();
            let mut values = [0f32; 8];
            values.copy_from_slice(bytemuck::cast_slice(&bytes));
            drop(bytes);
            read.unmap();
            values
        };
        let a = sample(&field, &open, 0.);
        let b = sample(&field, &shallow, 0.);
        let c = sample(&field, &deep, 0.);
        let d = sample(&field, &lee, 0.);
        let magnitude = |v: [f32; 8]| v[..3].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(magnitude(a) > 1e-4);
        assert!(
            magnitude(b) < magnitude(a) * 0.15,
            "shoaling must change actual displacement: {a:?} / {b:?}"
        );
        assert!(
            (c[3] - 1.).abs() < 0.01,
            "deep open sea retains amplitude: {c:?}"
        );
        assert!(
            d[3] < c[3] * 0.8,
            "upwind land must shelter long waves: {d:?} / {c:?}"
        );
        assert!(
            (b[4] - 0.03).abs() < 0.001 && b[6] > 0.,
            "shallow broken water produces surf: {b:?}"
        );
        let surf: Vec<_> = (0..8)
            .map(|i| sample(&field, &shallow, i as f64)[6])
            .collect();
        let lo = surf.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = surf.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi - lo > 0.02, "surf must move over time: {surf:?}");
        assert_eq!(
            sample(&field, &open, 0.),
            a,
            "returning to open bindings must restore the baseline exactly"
        );
        // Regression: authored 512 m waves under 1.1 m/s wind are almost wholly
        // removed by upstream fetch. Depth-only must retain motion and slider updates.
        let mut weak = WaveField::new(&device, QualityTier::High);
        let params = WaveParameters {
            significant_height: 45.3,
            peak_wavelength: 512.,
            wind_speed: 1.1,
            ..Default::default()
        };
        let update = |field: &mut WaveField, params, time| {
            let mut encoder = device.create_command_encoder(&Default::default());
            field.update(&queue, &mut encoder, params, false, time, 0.02, 1., 1.);
            queue.submit(Some(encoder.finish()));
        };
        update(&mut weak, params, 3.);
        let sheltered = SurfaceWaves::with_shore(&device, &queue, &weak, Arc::clone(&lee_map));
        let depth_only =
            SurfaceWaves::with_shore_shelter(&device, &queue, &weak, Arc::clone(&lee_map), false);
        let limited = SurfaceWaves::with_shore_shelter(&device, &queue, &weak, map(0.03), false);
        let sheltered_value = sample(&weak, &sheltered, 3.);
        let restored = sample(&weak, &depth_only, 3.);
        assert!(
            sheltered_value[3] < 0.005,
            "weak wind removes long waves: {sheltered_value:?}"
        );
        assert!(
            (restored[3] - 1.).abs() < 0.01 && magnitude(restored) > 0.01,
            "depth only restores deep waves: {restored:?}"
        );
        assert!(
            sample(&weak, &limited, 3.)[3] < 0.005,
            "depth-only still limits shallow waves"
        );
        update(&mut weak, params, 4.);
        let animated = sample(&weak, &depth_only, 4.);
        assert!(
            (animated[1] - restored[1]).abs() > 0.001,
            "coastal field keeps animating"
        );
        update(
            &mut weak,
            WaveParameters {
                significant_height: 22.65,
                ..params
            },
            4.,
        );
        let resized = sample(&weak, &depth_only, 4.);
        assert!((resized[5]*2.-animated[5]).abs() < 0.01 && resized[1].abs() < animated[1].abs()*0.8,
            "height slider updates arriving height and real displacement: {animated:?} / {resized:?}");
        let stats = weak.surface_stats();
        let cpu = stats.sample_shore(&lee_map, [40., 0.], 1., 0.);
        assert!(
            (cpu.scales[0] - f64::from(sample(&weak, &sheltered, 4.)[3])).abs() < 0.001,
            "diagnostic agrees with GPU"
        );
        assert!(scope.pop().await.is_none(), "coastal GPU validation error");
    });
}
