use blue_water_core::{QualityTier, WaveParameters};
use blue_water_gpu::{shader_composer, surface_waves::SurfaceWaves, WaveField};
use naga_oil::compose::{NagaModuleDescriptor, ShaderDefValue, ShaderType};

/// Measure the actual stochastically tiled displacement, not just the reduction's
/// input textures. In particular this catches an incorrect derivative packing,
/// a missing cascade, or a limiter that introduces new folds at its boundaries.
#[test]
fn extreme_hybrid_sea_reduces_folded_area_and_keeps_height() {
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
        field.set_crest_limits(true);
        let waves = SurfaceWaves::new(&device, &field);
        field.prepare_composite_crest_limits(&device, &waves);
        let mut encoder = device.create_command_encoder(&Default::default());
        field.update(
            &queue,
            &mut encoder,
            WaveParameters {
                significant_height: 55.8,
                peak_wavelength: 512.0,
                choppiness: 2.2543192,
                ..Default::default()
            },
            false,
            100.0,
            1.0 / 48.0,
            1.0,
            1.0,
        );
        waves.write(&queue, &field, 100.0, 1.0, 1.0);
        let source = format!(
            r#"
#import naval_ocean::waves::displacement
{}
@group(0) @binding(0) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let at = (vec2<f32>(id.xy) + vec2<f32>(0.37)) * 32.0 - vec2<f32>(2048.0);
    let h = 0.5;
    let raw = displacement(at, h);
    let dx = (displacement(at + vec2<f32>(h, 0.0), h) - displacement(at - vec2<f32>(h, 0.0), h)) / (2.0 * h);
    let dz = (displacement(at + vec2<f32>(0.0, h), h) - displacement(at - vec2<f32>(0.0, h), h)) / (2.0 * h);
    let wave = vec4<f32>(0.0);
    let motion = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    let a = jka_crest_scale(wave, motion, 1.0);
    let b = jka_crest_scale(wave, motion, 2.0);
    let off = jka_crest_scale(wave, motion, 0.0);
    let j0 = (1.0 + dx.x) * (1.0 + dz.z) - dx.z * dz.x;
    let j1 = (1.0 + dx.x * a) * (1.0 + dz.z * a) - dx.z * dz.x * a * a;
    let j2 = (1.0 + dx.x * b) * (1.0 + dz.z * b) - dx.z * dz.x * b * b;
    let limited = raw * vec3<f32>(b, 1.0, b);
    let i = (id.y * 128u + id.x) * 2u;
    result[i] = vec4<f32>(j0, j1, j2, a);
    result[i + 1u] = vec4<f32>(raw.y, limited.y, b, off);
}}
"#,
            include_str!("../src/crest_control.wgsl")
        );
        let mut composer = shader_composer::composer().unwrap();
        let module = composer
            .make_naga_module(NagaModuleDescriptor {
                source: &source,
                file_path: "Hybrid crest displacement regression",
                shader_type: ShaderType::Wgsl,
                shader_defs: [("CASCADES".into(), ShaderDefValue::UInt(3))].into(),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&composer)));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let bytes = 128 * 128 * 32;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(&empty), Some(waves.layout())],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &empty,
            entries: &[],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &empty_group, &[]);
            pass.set_bind_group(2, waves.group(&field), &[]);
            pass.dispatch_workgroups(16, 16, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, bytes);
        queue.submit(Some(encoder.finish()));
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let validation = scope.pop().await;
        assert!(
            validation.is_none(),
            "crest reduction GPU validation failed: {validation:?}"
        );
        let mapped = read.slice(..).get_mapped_range().unwrap();
        let samples: &[[f32; 8]] = bytemuck::cast_slice(&mapped);
        let folded = |column| samples.iter().filter(|s| s[column] < 0.0).count();
        let min_j = |column| {
            samples
                .iter()
                .map(|s| s[column])
                .fold(f32::INFINITY, f32::min)
        };
        eprintln!(
            "crest A/B: folded samples {:?}; min Jacobian {:?}; horizontal scales {:?}",
            [folded(0), folded(1), folded(2)],
            [min_j(0), min_j(1), min_j(2)],
            [samples[0][3], samples[0][6]]
        );
        assert!(samples.iter().flatten().all(|v| v.is_finite()));
        assert!(folded(0) > 20, "fixture must exercise the reported folding");
        assert!(
            folded(1) < folded(0) / 10 + 1,
            "limit must remove most folds"
        );
        assert!(
            folded(2) < folded(0) / 5 + 1,
            "spilling mode must bound the overlapping area"
        );
        assert!(min_j(2) > -0.25, "remaining overhangs must stay shallow");
        for sample in samples {
            assert_eq!(
                sample[4].to_bits(),
                sample[5].to_bits(),
                "vertical height must be preserved"
            );
            assert_eq!(
                sample[7], 1.0,
                "off must preserve the original displacement"
            );
        }
    });
}
