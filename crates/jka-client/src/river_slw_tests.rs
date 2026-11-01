//! Single Layer Water WGSL against the CPU port, plus the generated
//! PreintegratedGF resource through the independent SampleLevel oracle.
//! Inputs are synthetic diagnostics, not scene captures or light values.
use super::material_tests::sample;
use crate::river_slw::PreintegratedGf;
use jka_water::river::slw::{self, Parameters, VolumeInputs};
use wgpu::util::DeviceExt;

const KERNEL: &str = r#"
@group(0) @binding(0) var<storage,read> cases:array<vec4<f32>>;
@group(0) @binding(1) var<storage,read_write> values:array<vec4<f32>>;
const IN:u32=16u; const OUT:u32=13u;
@compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&cases)/IN {return;}
    let i=id.x*IN; let o=id.x*OUT; let p=river_slw_parameters();
    let s=river_slw_surface(p,cases[i].x,cases[i].y,cases[i].z);
    let c=river_slw_coefficients(p,cases[i+1u].xyz,cases[i+2u].xy,cases[i].y);
    let ior=river_slw_ior(s.specular);
    let ray=river_slw_refract(cases[i+3u].xyz,cases[i+4u].xyz,1.0/ior);
    let env=river_slw_env_brdf(vec3<f32>(0.08*s.specular),cases[i+9u].xy);
    let b=river_slw_bilateral(cases[i+9u].zw,cases[i+12u].yz,cases[i+10u],cases[i+11u],cases[i].w,cases[i+12u].x);
    let d=river_slw_uv_distortion(cases[i+13u].xyz,cases[i+14u].xyz,s.refraction,vec2<f32>(cases[i+13u].w,cases[i+14u].w),
        cases[i+15u],b.scene_depth,cases[i].w);
    let v=river_slw_volume(c,cases[i].w,cases[i+1u].w,cases[i+8u].xyz,cases[i+5u].w,cases[i+2u].z,cases[i+2u].w,
        cases[i+3u].xyz,cases[i+4u].xyz,ior,cases[i+5u].xyz,cases[i+6u].xyz,cases[i+7u].xyz,env,cases[i+3u].w,
        vec3<f32>(1.0),cases[i+4u].w);
    values[o]=vec4<f32>(s.specular,s.roughness,s.opacity,s.refraction.x);
    values[o+1u]=vec4<f32>(c.scattering,c.phase_g);
    values[o+2u]=vec4<f32>(c.absorption,ior);
    values[o+3u]=ray;
    values[o+4u]=vec4<f32>(env,river_slw_schlick_phase(c.phase_g,dot(-cases[i+5u].xyz,ray.xyz)));
    values[o+5u]=b.weights;
    values[o+6u]=vec4<f32>(b.scene_depth,d,s.base_color.r);
    values[o+7u]=vec4<f32>(v.luminance,0.0);
    values[o+8u]=vec4<f32>(v.water_to_scene,0.0);
    values[o+9u]=vec4<f32>(v.water_to_scene_to_light,0.0);
    values[o+10u]=b.refraction_mask;
    let direct=river_slw_direct(cases[i+4u].xyz,cases[i+3u].xyz,-cases[i+5u].xyz,s.base_color,vec3<f32>(0.08*s.specular),s.roughness);
    values[o+11u]=vec4<f32>(direct.diffuse,0.0);
    values[o+12u]=vec4<f32>(direct.specular,0.0);
}"#;

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|x| x / l)
}

fn cases() -> Vec<[[f32; 4]; 16]> {
    let mut out = Vec::new();
    for k in 0..48u32 {
        let f = k as f32;
        let hash = |s: f32| ((f * 12.9898 + s * 78.233).sin() * 43758.547).fract().abs();
        let n_dot_v = [0.9, 0.5, 0.21, 0.2299, 0.05, -0.1][k as usize % 6];
        let sign = if k % 5 == 3 { -1.0 } else { 1.0 };
        let surface_depth = 200.0 + 900.0 * hash(1.0);
        let behind = surface_depth + [0.0, 12.0, 29.0, 400.0, 2500.0, -40.0][k as usize % 6];
        let n = unit([0.3 * hash(2.0) - 0.15, 0.2 * hash(3.0) - 0.1, 1.0]);
        let v = unit([hash(4.0) - 0.5, hash(5.0) - 0.5, 0.2 + hash(6.0)]);
        let light = unit([hash(7.0) - 0.5, hash(8.0) - 0.5, -1.0]);
        let mask = |s: f32| if hash(s) < 0.2 { 0.3 } else { 128.0 / 255.0 + hash(s + 0.5) * 127.0 / 255.0 };
        out.push([
            [n_dot_v, sign, 300.0 + 8000.0 * hash(9.0), surface_depth],
            [0.4 * hash(10.0), 0.9 * hash(11.0), 0.6 * hash(12.0), behind],
            [180.0 * hash(13.0) - 90.0, 120.0 * hash(14.0) - 60.0, 50.0, 50.0 - 300.0 * hash(15.0)],
            [v[0], v[1], v[2], hash(16.0)],
            [n[0], n[1], n[2], if k % 7 == 6 { 0.0 } else { 0.5 + 0.5 * hash(17.0) }],
            [light[0], light[1], light[2], 0.25 + hash(18.0)],
            [30.0 * hash(19.0), 28.0 * hash(20.0), 25.0 * hash(21.0), 0.0],
            [3.0 * hash(22.0), 3.5 * hash(23.0), 4.0 * hash(24.0), 0.0],
            [hash(25.0) * 2.0, hash(26.0), hash(27.0) * 0.5, 0.0],
            [hash(28.0), hash(29.0), 0.1 + 0.8 * hash(30.0), 0.1 + 0.8 * hash(31.0)],
            [surface_depth + 600.0 * hash(32.0) - 200.0, surface_depth + 300.0, surface_depth - 90.0, surface_depth + 1.0],
            [mask(33.0), mask(34.0), mask(35.0), mask(36.0)],
            [20.0 * hash(37.0), 640.0, 360.0, 0.0],
            [n[0], n[1], n[2], 0.5 + 0.5 * hash(38.0)],
            [0.0, 0.0, 1.0, 0.5 + 0.5 * hash(39.0)],
            [1.0 + hash(40.0), 16.0 / 9.0, 640.0, 360.0],
        ]);
    }
    out
}

fn expected(p: &Parameters, c: &[[f32; 4]; 16]) -> Vec<[f32; 4]> {
    let xyz = |v: [f32; 4]| [v[0], v[1], v[2]];
    let s = slw::surface(p, c[0][0], c[0][1], c[0][2]);
    let k = slw::coefficients(p, xyz(c[1]), [c[2][0], c[2][1]], c[0][1]);
    let ior = slw::water_ior(s.specular);
    let ray = slw::water_refract(xyz(c[3]), xyz(c[4]), 1.0 / ior);
    let ray4 = match ray {
        Some(r) => [r[0], r[1], r[2], 1.0],
        None => unreachable!("air-to-water refraction cannot be total internal reflection"),
    };
    let env = slw::env_brdf([0.08 * s.specular; 3], [c[9][0], c[9][1]]);
    let b = slw::bilateral([c[9][2], c[9][3]], [c[12][1], c[12][2]], c[10], c[11], c[0][3], c[12][0]);
    let d = slw::water_uv_distortion(xyz(c[13]), xyz(c[14]), s.refraction, [c[13][3], c[14][3]], c[15], b.scene_depth, c[0][3]);
    let l = xyz(c[5]);
    let v = slw::volume_lighting(&VolumeInputs {
        coefficients: k,
        water_surface_depth: c[0][3],
        behind_depth: c[1][3],
        behind_luminance: xyz(c[8]),
        one_over_pre_exposure: c[5][3],
        surface_z: c[2][2],
        behind_z: c[2][3],
        camera_vector: xyz(c[3]),
        normal: xyz(c[4]),
        ior,
        light_direction: l,
        sun_illuminance: xyz(c[6]),
        ambient_illuminance: xyz(c[7]),
        env_brdf: env,
        directional_shadow: c[3][3],
        light_function: [1.0; 3],
        water_visibility: c[4][3],
    });
    let phase = slw::schlick_phase(k.phase_g, -l[0] * ray4[0] - l[1] * ray4[1] - l[2] * ray4[2]);
    let direct = slw::direct(xyz(c[4]), xyz(c[3]), l.map(|x| -x), s.base_color, [0.08 * s.specular; 3], s.roughness);
    vec![
        [s.specular, s.roughness, s.opacity, s.refraction[0]],
        [k.scattering[0], k.scattering[1], k.scattering[2], k.phase_g],
        [k.absorption[0], k.absorption[1], k.absorption[2], ior],
        ray4,
        [env[0], env[1], env[2], phase],
        b.weights,
        [b.scene_depth, d[0], d[1], s.base_color[0]],
        [v.luminance[0], v.luminance[1], v.luminance[2], 0.0],
        [v.water_to_scene[0], v.water_to_scene[1], v.water_to_scene[2], 0.0],
        [v.water_to_scene_to_light[0], v.water_to_scene_to_light[1], v.water_to_scene_to_light[2], 0.0],
        b.refraction_mask,
        [direct.0[0], direct.0[1], direct.0[2], 0.0],
        [direct.1[0], direct.1[1], direct.1[2], 0.0],
    ]
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((h >> 10) & 0x1f);
    let mantissa = f32::from(h & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Classic indirect field: CPU half conversion read back by the device, and
/// the WGSL decode against the CPU decode at several normals.
fn check_classic_ambient(device: &wgpu::Device, queue: &wgpu::Queue) {
    use crate::river_slw::{classic_ambient_texels, classic_indirect, f32_to_f16, ClassicAmbient, ADAPTER_SHADER};
    let dims = [7u32, 5u32];
    let texels = classic_ambient_texels(
        dims,
        |x, y| [x as f32 * 40.0, y as f32 * 40.0, 16.0],
        |p| {
            (p[0] != 120.0 || p[1] != 80.0).then(|| crate::scene::ClassicEntityLight {
                ambient: [20.0 + p[0] * 0.3, 35.0, 50.0 + p[1] * 0.2],
                directed: [180.0, 140.0 + p[0] * 0.1, 90.0],
                direction: crate::scene::render_position(
                    [(p[0] * 0.01).cos(), (p[1] * 0.02).sin(), 0.6 - p[1] / 200.0]
                        .map(|v| v / (1.0f32 + 0.36).sqrt()),
                ),
                baked_sun: [60.0, 40.0, 20.0],
                ..Default::default()
            })
        },
        Some([0.0, 1.0, 0.0]),
    );
    let field = ClassicAmbient::upload(device, queue, &texels).unwrap();
    assert_eq!(field.logical_bytes(), 7 * 5 * 16);
    let normals = [[0.0, 0.0, 1.0], [0.6, 0.0, 0.8], [-0.48, 0.6, 0.64]];
    let cases: Vec<[f32; 4]> = (0..dims[0] * dims[1])
        .flat_map(|i| normals.map(|n| [n[0], n[1], n[2], i as f32]))
        .collect();
    let wgsl = format!(
        "{ADAPTER_SHADER}\n{}",
        r#"
    @group(0) @binding(0) var<storage,read> cases:array<vec4<f32>>;
    @group(0) @binding(1) var<storage,read_write> values:array<vec4<f32>>;
    @group(0) @binding(2) var ambient_tex:texture_2d<f32>;
    @group(0) @binding(3) var directed_tex:texture_2d<f32>;
    @compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
        if id.x>=arrayLength(&cases) {return;}
        let i=u32(cases[id.x].w);let w=textureDimensions(ambient_tex).x;
        let cell=vec2<i32>(i32(i%w),i32(i/w));
        let a=textureLoad(ambient_tex,cell,0);let d=textureLoad(directed_tex,cell,0);
        values[3u*id.x]=a;values[3u*id.x+1u]=d;
        values[3u*id.x+2u]=vec4<f32>(river_slw_classic_indirect(a,d,cases[id.x].xyz),0.0);
    }"#
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("river classic indirect diagnostic"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&cases),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: cases.len() as u64 * 48,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let (va, vd) = (
        field.ambient.create_view(&Default::default()),
        field.directed.create_view(&Default::default()),
    );
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&va) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&vd) },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    let gpu = super::read_buffer(device, queue, &output);
    let stored = |v: [f32; 4]| v.map(|x| f16_to_f32(f32_to_f16(x)));
    assert_eq!(texels.ambient[2 * 7 + 3], [0.0; 4], "missing lightgrid sample stores zero indirect light");
    for (n, case) in cases.iter().enumerate() {
        let i = case[3] as usize;
        let (a, d) = (stored(texels.ambient[i]), stored(texels.directed[i]));
        assert_eq!(gpu[3 * n], a, "device half decode of ambient texel {i}");
        assert_eq!(gpu[3 * n + 1], d, "device half decode of directed texel {i}");
        let expected = classic_indirect(a, d, [case[0], case[1], case[2]]);
        for k in 0..3 {
            assert!((gpu[3 * n + 2][k] - expected[k]).abs() <= 1e-5 + 1e-5 * expected[k].abs(),
                "classic indirect texel {i} normal {:?}: GPU {} CPU {}", &case[..3], gpu[3 * n + 2][k], expected[k]);
        }
    }
}

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    check_classic_ambient(device, queue);
    let p = Parameters::default();
    let all = cases();
    let flat: Vec<[f32; 4]> = all.iter().flatten().copied().collect();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Single Layer Water diagnostic"),
        source: wgpu::ShaderSource::Wgsl(
            format!("{}\n{}\n{}", slw::SHADER, crate::river_slw::parameters_shader(&p), KERNEL).into(),
        ),
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&flat),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: all.len() as u64 * 13 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((all.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    let gpu = super::read_buffer(device, queue, &output);
    let rows = ["surface", "scattering", "absorption", "refract", "env/phase", "weights", "depth/distortion", "luminance",
                "to scene", "to light", "mask", "direct diffuse", "direct specular"];
    let mut exercised = [false; 5];
    for (n, c) in all.iter().enumerate() {
        let e = expected(&p, c);
        exercised[0] |= e[0][2] == 1.0;
        exercised[1] |= e[5] == [0.0; 4];
        exercised[2] |= e[6][1] != 0.0;
        exercised[3] |= e[7][0] > 0.0;
        exercised[4] |= e[12][0] > 0.0;
        for (row, (a, b)) in gpu[n * 13..n * 13 + 13].iter().zip(&e).enumerate() {
            for k in 0..4 {
                // GGX at roughness 0.02 peaks steeply; allow the device's
                // pow/FMA rounding there.
                let relative = if row == 12 { 2e-4 } else { 2e-5 };
                let tolerance = 2e-6 + relative * b[k].abs();
                assert!(
                    a[k].is_finite() && (a[k] - b[k]).abs() <= tolerance,
                    "SLW case {n} {} [{k}]: GPU {} CPU {}",
                    rows[row],
                    a[k],
                    b[k]
                );
            }
        }
    }
    assert_eq!(exercised, [true; 5], "fixture must cover underside, rejected samples, refraction, scattering and sun specular");

    // Generated PreintegratedGF through the device bilinear-clamp sampler.
    let gf = PreintegratedGf::new(device, queue);
    let table = slw::preintegrated_gf();
    let texel = |x: i32, y: i32| {
        let t = table[(y.clamp(0, 31) * 128 + x.clamp(0, 127)) as usize];
        [f32::from(t[0]) / 65535.0, f32::from(t[1]) / 65535.0]
    };
    let uvs: Vec<[f32; 4]> = (0..17)
        .flat_map(|i| (0..5).map(move |j| [i as f32 / 16.0, j as f32 / 4.0, 0.0, 0.0]))
        .chain([[0.5 / 128.0, 0.5 / 32.0, 0.0, 0.0], [127.5 / 128.0, 31.5 / 32.0, 0.0, 0.0]])
        .collect();
    let sampled = sample(device, queue, &gf.texture, 0, &gf.sampler, &uvs);
    for (uv, s) in uvs.iter().zip(&sampled) {
        let p = [uv[0] * 128.0 - 0.5, uv[1] * 32.0 - 0.5];
        let c = p.map(|v| v.floor());
        let t = [p[0] - c[0], p[1] - c[1]];
        let (x, y) = (c[0] as i32, c[1] as i32);
        for k in 0..2 {
            let top = texel(x, y)[k] + (texel(x + 1, y)[k] - texel(x, y)[k]) * t[0];
            let bottom = texel(x, y + 1)[k] + (texel(x + 1, y + 1)[k] - texel(x, y + 1)[k]) * t[0];
            let ideal = top + (bottom - top) * t[1];
            // Hardware fixed-point filter weights bound the deviation.
            assert!((s[k] - ideal).abs() <= 2.0 / 256.0, "PreintegratedGF {uv:?}[{k}]: GPU {} ideal {ideal}", s[k]);
        }
    }
    assert_eq!(
        [sampled[85][0], sampled[85][1]],
        texel(0, 0),
        "texel centre must return the generated unorm value exactly"
    );
}
