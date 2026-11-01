//! Native foam graph arithmetic, cubic boundaries and actual HDR resource use.
//! Field textures below are diagnostic inputs; the foam simulator is not replaced.
use super::material_tests::{read_texture, sample};
use super::wave_tests::{native_random_oracle, texture};
use jka_water::river::{
    foam::{self, Attributes, Settings},
    material::Motion,
    surface,
    wave_advection::{self as wave, Plan},
};
use wgpu::util::DeviceExt;

fn packed(s: Settings) -> [[f32; 4]; 7] {
    [
        [
            s.mask_fit,
            s.mask_power,
            s.scattering_fit,
            s.scattering_power,
        ],
        [
            s.low_velocity_scale,
            s.velocity_min,
            s.velocity_max,
            s.normal_flatness,
        ],
        [s.gain, s.power, s.black_point_to_mult, s.render_fit],
        [s.opacity, s.roughness, s.specular, 0.0],
        [s.base_color[0], s.base_color[1], s.base_color[2], 0.0],
        s.scattering_color,
        [
            s.scattering_velocity_min,
            s.scattering_velocity_max,
            0.0,
            0.0,
        ],
    ]
}
const DEFAULTS: &str = r#"
fn foam_settings()->RiverFoamSettings {
    return RiverFoamSettings(vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(0.0,25.0,100.0,0.3),
        vec4<f32>(1.0,1.0,1.0,0.25),vec4<f32>(1.0,0.1,0.5,0.0),
        vec4<f32>(0.95,1.0,0.9,0.0),vec4<f32>(0.427083,1.0,0.67074,1.0),vec4<f32>(100.0,500.0,0.0,0.0));
}
"#;
fn field_value(x: u32, y: u32) -> [f32; 4] {
    let f = if (x + 2 * y) % 3 == 0 {
        0.0
    } else {
        (x + 3 * y) as f32 / 16.0
    };
    [
        f,
        x as f32 / 8.0,
        y as f32 / 4.0,
        (x as f32 - y as f32) / 16.0,
    ]
}
fn clamped(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}
const INCOMING: Attributes = Attributes {
    base_color: [0.05, 0.07, 0.09],
    roughness: 0.8,
    specular: 0.2,
    opacity: 0.35,
};

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    let field = texture(device, queue, [9, 7], 1, |_, x, y| field_value(x, y));
    let sampler = clamped(device);
    let uvs = [
        [-0.2, 0.5],
        [0.0, 0.0],
        [0.5, 0.5],
        [0.41, 0.63],
        [1.0, 1.0],
        [1.2, 0.4],
        [0.16, 0.25],
        [0.82, 0.7],
    ];
    let queries: Vec<_> = uvs.iter().map(|v| [v[0], v[1], 0.0, 0.0]).collect();
    let edge = sample(device, queue, &field, 0, &sampler, &queries);
    let mut inputs = Vec::<[f32; 4]>::new();
    let mut expected = Vec::new();
    for i in 0..24 {
        let mut s = Settings::default();
        if i >= 8 {
            s.mask_fit = 0.75;
            s.mask_power = 1.5;
            s.scattering_fit = 0.5;
            s.scattering_power = 0.75;
            s.low_velocity_scale = 0.25;
            s.black_point_to_mult = 0.6;
            s.gain = 1.3;
            s.power = 1.5;
            s.opacity = 0.7;
        }
        let f = [0.0, 0.1, 0.5, 1.0, 2.0, 4.0][i % 6];
        let speed = [0.0, 25.0, 62.5, 100.0, 250.0, 500.0][i / 4];
        let detail = [0.25, 0.75, 0.875, [0.0, 0.125, 0.5, 0.875][i % 4]];
        let uv = uvs[i % 8];
        inputs.push([f, speed, uv[0], uv[1]]);
        inputs.push(detail);
        inputs.extend(packed(s));
        inputs.push([
            INCOMING.base_color[0],
            INCOMING.base_color[1],
            INCOMING.base_color[2],
            INCOMING.roughness,
        ]);
        inputs.push([INCOMING.specular, INCOMING.opacity, 0.0, 0.0]);
        let out = foam::present(f, detail, speed, s);
        let a = foam::attributes(INCOMING, out, s);
        let field_ref = foam::cubic_field(
            [9, 7],
            uv,
            |p| field_value(p[0] as u32, p[1] as u32),
            |_| edge[i % 8],
        );
        expected.extend([
            [out.normal[0], out.normal[1], out.normal[2], out.mask],
            [
                out.scattering[0],
                out.scattering[1],
                out.scattering[2],
                out.scattering_mask,
            ],
            [out.total, out.opacity, a.roughness, a.specular],
            [a.base_color[0], a.base_color[1], a.base_color[2], a.opacity],
            field_ref,
        ]);
    }
    let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("selected native foam graph diagnostic"),source:wgpu::ShaderSource::Wgsl(format!("{}\n{}",foam::fragment_shader(),r#"
    @group(0) @binding(0) var field:texture_2d<f32>;
    @group(0) @binding(1) var clamp_sampler:sampler;
    @group(0) @binding(2) var<storage,read> inputs:array<vec4<f32>>;
    @group(0) @binding(3) var<storage,read_write> outputs:array<vec4<f32>>;
    @compute @workgroup_size(32) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
        let i=id.x*11u;let o=id.x*5u;if i>=arrayLength(&inputs){return;}
        let p=inputs[i];let s=RiverFoamSettings(inputs[i+2u],inputs[i+3u],inputs[i+4u],inputs[i+5u],inputs[i+6u],inputs[i+7u],inputs[i+8u]);
        let f=river_foam_present(p.x,inputs[i+1u],p.y,s);
        let a=river_foam_attributes(RiverFoamAttributes(inputs[i+9u].xyz,inputs[i+9u].w,inputs[i+10u].x,inputs[i+10u].y),f,s);
        outputs[o]=vec4<f32>(f.normal,f.mask);outputs[o+1u]=vec4<f32>(f.scattering,f.scattering_mask);
        outputs[o+2u]=vec4<f32>(f.total,f.opacity,a.roughness,a.specular);outputs[o+3u]=vec4<f32>(a.base_color,a.opacity);
        outputs[o+4u]=river_foam_field(field,clamp_sampler,p.zw);
    }"#).into())});
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&inputs),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: expected.len() as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let view = field.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    compare(
        &super::read_buffer(device, queue, &output),
        &expected,
        3e-5,
        "native foam graph/cubic",
    );
}

fn compare(actual: &[[f32; 4]], expected: &[[f32; 4]], error: f32, label: &str) {
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
        for c in 0..4 {
            assert!(
                a[c].is_finite() && (a[c] - b[c]).abs() <= error,
                "{label} sample {i} component {c}: GPU {}, reference {}",
                a[c],
                b[c]
            );
        }
    }
}

fn archive() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../jka-water/src/river/foam-detail.zip"),
    )
    .expect("regenerate actual native foam detail with scripts/ue-river-foam-texture.py")
}
fn half(bits: u16) -> f32 {
    // All native source components are finite 0..1. Exact powers of two keep
    // subnormal values exact too, without depending on a particular half crate.
    let e = (bits >> 10) & 31;
    let m = bits & 1023;
    if e == 0 {
        m as f32 * 2.0f32.powi(-24)
    } else {
        (1.0 + m as f32 / 1024.0) * 2.0f32.powi(e as i32 - 15)
    }
}
fn check_uploaded_mips(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    detail: &crate::river_foam::Detail,
    bytes: &[u8],
) {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    for mip in 0..12 {
        let size = 2048u32 >> mip;
        let mut data = Vec::new();
        zip.by_index(mip).unwrap().read_to_end(&mut data).unwrap();
        let mut queries = Vec::new();
        let mut expected = Vec::new();
        for i in 0..128u32 {
            let x = if i == 0 {
                0
            } else if i == 1 {
                size - 1
            } else {
                (i * 127 + 17) % size
            };
            let y = if i == 0 {
                0
            } else if i == 1 {
                size - 1
            } else {
                (i * 277 + 3) % size
            };
            queries.push([
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
                mip as f32,
                0.0,
            ]);
            let offset = (x + y * size) as usize * 8;
            expected.push(std::array::from_fn(|c| {
                half(u16::from_le_bytes([
                    data[offset + c * 2],
                    data[offset + c * 2 + 1],
                ]))
            }));
        }
        assert_eq!(
            sample(device, queue, &detail.texture, 0, &detail.sampler, &queries),
            expected,
            "actual foam upload mip {mip}"
        );
    }
}

const FRAGMENT: &str = r#"
@group(0) @binding(0) var fluid:texture_2d<f32>;
@group(0) @binding(1) var normals:texture_2d<f32>;
@group(0) @binding(2) var field:texture_2d<f32>;
@group(0) @binding(3) var foam_detail:texture_2d<f32>;
@group(0) @binding(4) var ocean:texture_2d<f32>;
@group(0) @binding(5) var wrap_sampler:sampler;
@group(0) @binding(6) var clamp_sampler:sampler;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));return vec4<f32>(p[i],0.0,1.0);
}
struct Result {@location(0) normal:vec4<f32>,@location(1) scattering:vec4<f32>,@location(2) attributes:vec4<f32>}
@fragment fn fragment(@builtin(position) pixel:vec4<f32>)->Result {
    let panel=u32(pixel.x)/4u;let delta=array<f32,4>(1.0,16.0,128.0,1024.0)[panel];
    let position=pixel.xy*delta+vec2<f32>(123.0,-321.0);let uv=pixel.xy/vec2<f32>(16.0,8.0);
    let foam_advection=RiverAdvection(vec4<f32>(20.0,70.0,2.0,0.001),vec4<f32>(1.0,0.0,0.0,0.0));
    let wpo_advection=RiverAdvection(vec4<f32>(20.0,70.0,2.0,0.0005),vec4<f32>(1.0,2.0,0.0,0.0));
    let detail_advection=RiverAdvection(vec4<f32>(20.0,70.0,2.0,0.0005),vec4<f32>(1.0,0.0,0.0,0.0));
    let f=river_live_grid_foam(fluid,normals,wrap_sampler,field,clamp_sampler,foam_detail,wrap_sampler,uv,position,0.35,foam_advection,foam_settings());
    let n=river_live_grid_normal(fluid,normals,wrap_sampler,ocean,wrap_sampler,uv,position,0.35,motion(),wpo_advection,detail_advection,height(),f.normal,f.total,1.0,1.0);
    let a=river_foam_attributes(RiverFoamAttributes(vec3<f32>(0.05,0.07,0.09),0.8,0.2,0.35),f,foam_settings());
    return Result(vec4<f32>(n,f.total),vec4<f32>(f.scattering,f.mask),vec4<f32>(a.base_color,a.opacity));
}
"#;

pub fn check_native_texture(device: &wgpu::Device, queue: &wgpu::Queue, ocean: &wgpu::Texture) {
    let bytes = archive();
    let detail = crate::river_foam::Detail::from_archive(device, queue, &bytes).unwrap();
    assert_eq!(detail.logical_bytes(), 44_739_240);
    check_uploaded_mips(device, queue, &detail, &bytes);
    assert!(crate::river_foam::Detail::from_archive(device, queue, b"not a foam archive").is_err());
    // One fluid texel per diagnostic pixel keeps every sampled UV at a cell
    // centre. A two-cell fixture would fade velocity via native OOB zero loads.
    let fluid = texture(device, queue, [16, 8], 1, |_, _, _| {
        [120.0, 80.0, 200.0, -50.0]
    });
    let normals = texture(device, queue, [2, 2], 1, |_, _, _| {
        [0.25, -0.125, 0.875, 0.0]
    });
    let field = texture(device, queue, [9, 7], 1, |_, x, y| field_value(x, y));
    let clamped = clamped(device);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("actual foam HDR + FFT fragment graph"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}\n{}\n{}",
                foam::fragment_shader(),
                super::surface_tests::COMMON,
                DEFAULTS,
                FRAGMENT
            )
            .into(),
        ),
    });
    let targets: Vec<_> = (0..3)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("foam graph diagnostic, not SLW lighting"),
                size: wgpu::Extent3d {
                    width: 16,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        })
        .collect();
    let color_targets: Vec<_> = (0..3)
        .map(|_| {
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &color_targets,
        }),
        multiview_mask: None,
        cache: None,
    });
    let views: Vec<_> = [&fluid, &normals, &field, &detail.texture, ocean]
        .iter()
        .map(|t| t.create_view(&Default::default()))
        .collect();
    let mut bindings: Vec<_> = views
        .iter()
        .enumerate()
        .map(|(i, v)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(v),
        })
        .collect();
    bindings.extend([
        wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::Sampler(&detail.sampler),
        },
        wgpu::BindGroupEntry {
            binding: 6,
            resource: wgpu::BindingResource::Sampler(&clamped),
        },
    ]);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &bindings,
    });
    let target_views: Vec<_> = targets
        .iter()
        .map(|t| t.create_view(&Default::default()))
        .collect();
    let attachments: Vec<_> = target_views
        .iter()
        .map(|v| {
            Some(wgpu::RenderPassColorAttachment {
                view: v,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &attachments,
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    queue.submit(Some(encoder.finish()));
    let base = [0.25, -0.125, 0.875];
    let (random, _) = native_random_oracle(device, queue, &[0.0]);
    let plan = Plan::with_random(
        [200.0, -50.0],
        base,
        0.35,
        wave::Settings::default(),
        |_| random[0],
    );
    let mut foam_queries = Vec::new();
    let mut ocean_queries = Vec::new();
    let mut field_queries = Vec::new();
    for y in 0..8 {
        for x in 0..16 {
            let pixel = [x as f32 + 0.5, y as f32 + 0.5];
            let delta = [1.0, 16.0, 128.0, 1024.0][x / 4];
            let position = [pixel[0] * delta + 123.0, pixel[1] * delta - 321.0];
            field_queries.push([pixel[0] / 16.0, pixel[1] / 8.0, 0.0, 0.0]);
            for variant in 0..3 {
                let (tex, s, queries) = match variant {
                    0 => (&detail.texture, foam::advection(), &mut foam_queries),
                    1 => (ocean, wave::Settings::default(), &mut ocean_queries),
                    _ => (ocean, surface::detail_advection(), &mut ocean_queries),
                };
                let mip = wave::fragment_mip(
                    [delta * s.texture_scale, 0.0],
                    [0.0, delta * s.texture_scale],
                    [tex.width(), tex.height()],
                    s.vertex_mip,
                    tex.mip_level_count(),
                );
                for b in 0..4 {
                    for phase in 0..3 {
                        let uv = plan.uv(position, s, b, phase);
                        queries.push([uv[0], uv[1], mip, 0.0]);
                    }
                }
            }
        }
    }
    let foam_taps = sample(
        device,
        queue,
        &detail.texture,
        0,
        &detail.sampler,
        &foam_queries,
    );
    let ocean_taps = sample(device, queue, ocean, 0, &detail.sampler, &ocean_queries);
    let field_edges = sample(device, queue, &field, 0, &clamped, &field_queries);
    let mut expected = [Vec::new(), Vec::new(), Vec::new()];
    for i in 0..128 {
        let mut q = i * 12;
        let detail_value = plan.sample([0.0; 2], foam::advection(), [0.0; 3], |_, _| {
            let v = foam_taps[q];
            q += 1;
            v
        });
        let mut q = i * 24;
        let mut fft = || {
            plan.sample([0.0; 2], wave::Settings::default(), [0.0; 3], |_, _| {
                let v = ocean_taps[q];
                q += 1;
                v
            })
        };
        let a = fft();
        let b = fft();
        let uv = [field_queries[i][0], field_queries[i][1]];
        let field_value_ref = foam::cubic_field(
            [9, 7],
            uv,
            |p| field_value(p[0] as u32, p[1] as u32),
            |_| field_edges[i],
        )[0];
        let f = foam::present(
            field_value_ref,
            detail_value,
            200.0f32.hypot(-50.0),
            Settings::default(),
        );
        let n = surface::normal(
            base,
            [a[0], a[1], a[2]],
            [b[0], b[1], b[2]],
            200.0f32.hypot(-50.0),
            Motion::default(),
            wave::Height::default(),
            f.normal,
            f.total,
            1.0,
            1.0,
        );
        let attrs = foam::attributes(INCOMING, f, Settings::default());
        expected[0].push([n[0], n[1], n[2], f.total]);
        expected[1].push([f.scattering[0], f.scattering[1], f.scattering[2], f.mask]);
        expected[2].push([
            attrs.base_color[0],
            attrs.base_color[1],
            attrs.base_color[2],
            attrs.opacity,
        ]);
    }
    for i in 0..3 {
        compare(
            &read_texture(device, queue, &targets[i], 0),
            &expected[i],
            0.0008,
            "actual native foam/FFT fragment",
        );
    }
}

#[test]
fn river_foam_defaults_match_native_selected_primary_graph() {
    let j: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/foam-reference.json"
    ))
    .unwrap();
    let texture: serde_json::Value = serde_json::from_str(crate::river_foam::REFERENCE).unwrap();
    for (backend, name) in [
        (wgpu::Backend::Dx12, "Dx12"),
        (wgpu::Backend::Vulkan, "Vulkan"),
    ] {
        let actual = crate::river_foam::explicit_mip_filter(backend).unwrap();
        assert_eq!(
            texture["sampler_mips"][name],
            match actual {
                wgpu::MipmapFilterMode::Linear => "linear",
                wgpu::MipmapFilterMode::Nearest => "nearest",
            }
        );
    }
    let s = Settings::default();
    let a = foam::advection();
    for (name, v) in [
        ("FoamWithNormalTextureScale", a.texture_scale),
        ("FoamSpeed", a.speed),
        ("Foam Mask Fit 0-1", s.mask_fit),
        ("Foam Mask Power", s.mask_power),
        ("Foam Scattering Area Fit 0-1", s.scattering_fit),
        ("Foam Scattering Area Power", s.scattering_power),
        ("LowVelocityFoamScale", s.low_velocity_scale),
        ("FoamAttenMin", s.velocity_min),
        ("FoamAttenMax", s.velocity_max),
        ("FoamWithNormalGain", s.gain),
        ("FoamWithNormalPower", s.power),
        ("Black Point to Mult Blend", s.black_point_to_mult),
        ("Foam Render Fit 0-1", s.render_fit),
        ("Foam Normal Flatness", s.normal_flatness),
        ("FoamOpacity", s.opacity),
        ("Foam Roughness", s.roughness),
        ("Foam Specular", s.specular),
        ("Scattering Min Velocity", s.scattering_velocity_min),
        ("Scattering Max Velocity", s.scattering_velocity_max),
    ] {
        assert_eq!(
            j["effective_parameters"][name].as_f64().unwrap() as f32,
            v,
            "{name}"
        );
    }
    // The base tint intentionally differs from Epic's archived native default.
    assert_eq!(s.base_color, [0.95, 1.0, 0.9]);
    for (name, color) in [("Foam Scattering Color", s.scattering_color)] {
        let native = j["parent_vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["ParameterInfo"]["Name"] == name)
            .unwrap();
        for c in 0..4 {
            assert_eq!(
                native["ParameterValue"][c].as_f64().unwrap() as f32,
                color[c]
            );
        }
    }
}
