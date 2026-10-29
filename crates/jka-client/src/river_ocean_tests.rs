//! Native FFT stages, typed stores, reset and immutable frame dataflow.
use super::material_tests::{quantize, read_texture, sample};
use crate::river_ocean::Ocean;
use jka_water::river::ocean::{self, Parameters, SIZE};
use wgpu::util::DeviceExt;
const N: usize = SIZE as usize;
const PROBE: &str = r#"
@group(0) @binding(7) var<storage,read> probe_input:array<vec4<f32>>;
@group(0) @binding(8) var<storage,read_write> probe_output:array<vec4<f32>>;
@compute @workgroup_size(256) fn fft_probe(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) t:u32) {
    let initial=array<vec2<f32>,2>(probe_input[group.x*512u+t].xy,probe_input[group.x*512u+t+256u].xy);
    let v=ocean_fft(t,initial);
    probe_output[group.x*512u+t]=vec4<f32>(v[0],0.0,0.0);
    probe_output[group.x*512u+t+256u]=vec4<f32>(v[1],0.0,0.0);
}
@compute @workgroup_size(64) fn spectrum_probe(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&probe_input){return;}
    let q=vec2<u32>(probe_input[id.x].xy);let uv=ocean_noise_uv(q.x,q.y);var noise:array<vec4<f32>,4>;
    for(var i=0u;i<4u;i++) {
        noise[i]=textureSampleLevel(noise_tex,noise_sampler,uv[i],0.0);
        probe_output[id.x*9u+i]=vec4<f32>(uv[i],0.0,0.0);
        probe_output[id.x*9u+4u+i]=noise[i];
    }
    let v=ocean_spectrum(q.x,q.y,ocean.frame.x,ocean,noise);
    probe_output[id.x*9u+8u]=vec4<f32>(v[0],v[1]);
}
"#;
fn run(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    module: &wgpu::ShaderModule,
    entry: &str,
    input: &[[f32; 4]],
    count: usize,
    groups: u32,
    other: &[(u32, wgpu::BindingResource<'_>)],
) -> Vec<[f32; 4]> {
    let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(input),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: count as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut entries: Vec<_> = other
        .iter()
        .map(|(b, r)| wgpu::BindGroupEntry {
            binding: *b,
            resource: r.clone(),
        })
        .collect();
    entries.extend([
        wgpu::BindGroupEntry {
            binding: 7,
            resource: source.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 8,
            resource: output.as_entire_binding(),
        },
    ]);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut p = encoder.begin_compute_pass(&Default::default());
        p.set_pipeline(&pipeline);
        p.set_bind_group(0, &group, &[]);
        p.dispatch_workgroups(groups, 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    super::read_buffer(device, queue, &output)
}
fn check_fft(device: &wgpu::Device, queue: &wgpu::Queue, module: &wgpu::ShaderModule) {
    let mut input = vec![[0.0; 4]; N * 3];
    input[0] = [1.0, -2.0, 0.0, 0.0];
    for v in &mut input[N..N * 2] {
        v[0] = 1.0;
    }
    for (at, v) in [
        (7, [0.25, -0.5]),
        (31, [-0.75, 0.125]),
        (255, [0.375, 0.0625]),
        (511, [-0.125, 0.5]),
    ] {
        input[N * 2 + at] = [v[0], v[1], 0.0, 0.0];
    }
    let actual = run(
        device,
        queue,
        module,
        "fft_probe",
        &input,
        input.len(),
        3,
        &[],
    );
    // Independent positive-sign f64 DFT, including bins across the 256-thread
    // exchange boundary. Sparse inputs keep the reference inexpensive.
    let mut expected = vec![[0.0; 4]; input.len()];
    for line in 0..3 {
        for k in 0..N {
            let mut sum = [0.0; 2];
            for j in 0..N {
                let v = input[line * N + j];
                if v[0] == 0.0 && v[1] == 0.0 {
                    continue;
                }
                let a = 2.0 * std::f64::consts::PI * (k * j) as f64 / N as f64;
                sum[0] += v[0] as f64 * a.cos() - v[1] as f64 * a.sin();
                sum[1] += v[0] as f64 * a.sin() + v[1] as f64 * a.cos();
            }
            expected[line * N + k] = [sum[0] as f32, sum[1] as f32, 0.0, 0.0];
        }
    }
    super::compare(
        &actual,
        &expected,
        "native shared-memory FFT versus independent positive DFT",
    );
}
fn spectrum(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    module: &wgpu::ShaderModule,
    ocean: &Ocean,
    age: f32,
    p: Parameters,
) -> Vec<[f32; 4]> {
    let data = [
        p.gravity,
        p.amplitude,
        p.time_scale,
        p.length,
        p.wind_speed,
        p.wind_angle,
        p.alignment as f32,
        p.slosh,
        age,
        0.0,
        0.0,
        0.0,
    ];
    let controls = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&data),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let queries: Vec<_> = [0, 1, 255, 256, 257, 511]
        .into_iter()
        .flat_map(|row| (0..256).map(move |t| [row as f32, t as f32, 0.0, 0.0]))
        .collect();
    let result = run(
        device,
        queue,
        module,
        "spectrum_probe",
        &queries,
        queries.len() * 9,
        (queries.len() as u32).div_ceil(64),
        &[
            (0, controls.as_entire_binding()),
            (1, wgpu::BindingResource::TextureView(&ocean.noise)),
            (2, wgpu::BindingResource::Sampler(&ocean.noise_sampler)),
        ],
    );
    for (i, q) in queries.iter().enumerate() {
        let row = q[0] as u32;
        let t = q[1] as u32;
        let a = &result[i * 9..(i + 1) * 9];
        let uv = ocean::noise_uv(row, t);
        let noise = std::array::from_fn(|j| a[j + 4]);
        for j in 0..4 {
            assert_eq!(&a[j][..2], &uv[j]);
            super::compare(
                &[a[j + 4]],
                &[ocean::sample_noise(uv[j])],
                "native G8 R001 wrap SampleLevel",
            );
        }
        let v = ocean::spectrum_pair(row, t, age, p, noise);
        super::compare(
            &[a[8]],
            &[[v[0][0], v[0][1], v[1][0], v[1][1]]],
            "native Phillips phase/sign/mask coefficients",
        );
    }
    result
}
fn half_ulp(x: f32) -> f32 {
    let exponent = ((x.abs().to_bits() >> 23) & 255) as i32 - 127;
    2.0f32.powi(exponent.max(-14) - 10)
}
fn check_axis(actual: &[[f32; 4]], expected: &[[f32; 4]], l1: f32, label: &str) {
    for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
        for c in 0..2 {
            // Two half-store ULPs plus a transform error envelope scaled by
            // the input L1 norm. Never scale by a nearly cancelled output.
            let bound = 2.0 * half_ulp(b[c]) + l1 * 0.000003;
            assert!(
                a[c].is_finite() && (a[c] - b[c]).abs() <= bound,
                "{label} {i}/{c}: GPU {}, reference {}, bound {bound}",
                a[c],
                b[c]
            );
        }
        assert_eq!(a[2..], [0.0, 0.0]);
    }
}
pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    let ocean = Ocean::new(device, queue).unwrap();
    assert_eq!(ocean.texture.mip_level_count(), 10);
    assert_eq!(ocean.texture.format(), wgpu::TextureFormat::Rgba16Float);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native FFT primitive diagnostics"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", ocean::SHADER, PROBE).into()),
    });
    check_fft(device, queue, &module);
    let age = 0.35;
    let p = Parameters::default();
    let coefficients = spectrum(device, queue, &module, &ocean, age, p);
    spectrum(
        device,
        queue,
        &module,
        &ocean,
        -0.125,
        Parameters {
            wind_speed: -55.0,
            wind_angle: -30.0,
            alignment: 25,
            slosh: 0.25,
            ..p
        },
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    ocean.encode_reset(&mut encoder);
    queue.submit(Some(encoder.finish()));
    assert!(read_texture(device, queue, &ocean.texture, 0)
        .iter()
        .all(|v| *v == [0.0, 0.0, 1.0, 0.0]));
    let snapshot = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("first immutable ocean frame diagnostic"),
        size: ocean.texture.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let first = ocean.prepare_frame(device, age, p).unwrap();
    let second = ocean.prepare_frame(device, 0.8, p).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    first.encode(&mut encoder);
    encoder.copy_texture_to_texture(
        ocean.texture.as_image_copy(),
        snapshot.as_image_copy(),
        snapshot.size(),
    );
    second.encode(&mut encoder);
    queue.submit(Some(encoder.finish()));
    let before = read_texture(device, queue, &snapshot, 0);
    let after = read_texture(device, queue, &ocean.texture, 0);
    assert!(
        before
            .iter()
            .zip(&after)
            .map(|(a, b)| (a[3] - b[3]).abs() as f64)
            .sum::<f64>()
            > 1.0,
        "native FFT animation must remain active independently of hydraulics"
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    first.encode(&mut encoder);
    queue.submit(Some(encoder.finish()));
    let detail = read_texture(device, queue, &ocean.texture, 0);
    assert_eq!(
        before, detail,
        "two queued ages must retain immutable settings"
    );
    let x = read_texture(device, queue, &ocean.x, 0);
    let y = read_texture(device, queue, &ocean.y, 0);
    for (r, row) in [0, 1, 255, 256, 257, 511].into_iter().enumerate() {
        let mut initial = vec![[0.0; 2]; N];
        for t in 0..256 {
            let c = coefficients[(r * 256 + t) * 9 + 8];
            initial[t] = [c[0], c[1]];
            initial[t + 256] = [c[2], c[3]];
        }
        let l1 = initial.iter().map(|v| v[0].abs() + v[1].abs()).sum::<f32>();
        let expected: Vec<_> = ocean::fft_line(&initial)
            .iter()
            .map(|v| [v[0], v[1], 0.0, 0.0])
            .collect();
        check_axis(
            &x[row * N..(row + 1) * N],
            &expected,
            l1,
            "native X transform/half store",
        );
    }
    let mut expected_y = vec![[0.0; 4]; N * N];
    let mut l1 = vec![0.0; N];
    for col in 0..N {
        let initial: Vec<_> = (0..N)
            .map(|row| [x[col + row * N][0], x[col + row * N][1]])
            .collect();
        l1[col] = initial.iter().map(|v| v[0].abs() + v[1].abs()).sum();
        for (row, v) in ocean::fft_line(&initial).iter().enumerate() {
            expected_y[col + row * N] = [v[0], v[1], 0.0, 0.0];
        }
    }
    for col in 0..N {
        let actual: Vec<_> = (0..N).map(|r| y[col + r * N]).collect();
        let expected: Vec<_> = (0..N).map(|r| expected_y[col + r * N]).collect();
        check_axis(&actual, &expected, l1[col], "native Y transform/half store");
    }
    let normals: Vec<_> = (0..N * N)
        .map(|i| {
            let row = i / N;
            let col = i % N;
            ocean::normal(
                y[i][0],
                y[(col + 1) % N + row * N][0],
                y[col + (row + 1) % N * N][0],
            )
        })
        .collect();
    let quantized = quantize(device, queue, [SIZE, SIZE], &normals);
    for (i, (a, b)) in detail.iter().zip(&quantized).enumerate() {
        assert_eq!(
            a[3], y[i][0],
            "native alpha is RealPart, not a normal component"
        );
        for c in 0..3 {
            assert!(
                a[c].is_finite() && (a[c] - b[c]).abs() <= half_ulp(b[c]) + 0.000001,
                "native normal/store {i}/{c}: {} != {}",
                a[c],
                b[c]
            );
        }
    }
    let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    for mip in 1..10 {
        let size = SIZE >> mip;
        let queries: Vec<_> = (0..size * size)
            .map(|i| {
                [
                    (i % size) as f32 / size as f32 + 0.5 / size as f32,
                    (i / size) as f32 / size as f32 + 0.5 / size as f32,
                    0.0,
                    0.0,
                ]
            })
            .collect();
        let taps = sample(
            device,
            queue,
            &ocean.texture,
            mip - 1,
            &mip_sampler,
            &queries,
        );
        let expected = quantize(device, queue, [size, size], &taps);
        assert_eq!(
            read_texture(device, queue, &ocean.texture, mip),
            expected,
            "native FFT linear mip {mip}"
        );
    }
    assert!(ocean.logical_bytes() < 8 * 1024 * 1024);
    super::wave_tests::check_native_texture(device, queue, &ocean.texture);
    super::surface_tests::check_native_texture(device, queue, &ocean.texture);
    // Reset after animation must clear old amplitudes, not merely creation data.
    let mut encoder = device.create_command_encoder(&Default::default());
    ocean.encode_reset(&mut encoder);
    queue.submit(Some(encoder.finish()));
    assert!(read_texture(device, queue, &ocean.texture, 0)
        .iter()
        .all(|v| *v == [0.0, 0.0, 1.0, 0.0]));
}
#[test]
fn river_ocean_defaults_match_native_cached_emitter() {
    let j: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/ocean-reference.json"
    ))
    .unwrap();
    let p = Parameters::default();
    for (name, v) in [
        ("gravity", p.gravity),
        ("PhillipsAmplitude", p.amplitude),
        ("TimeScale", p.time_scale),
        ("OceanLengthScale", p.length),
        ("windSpeed", p.wind_speed),
        ("windAngle", p.wind_angle),
        ("windAlignement", p.alignment as f32),
        ("Slosh", p.slosh),
    ] {
        assert_eq!(j["parameters"][name].as_f64().unwrap() as f32, v, "{name}");
    }
}
