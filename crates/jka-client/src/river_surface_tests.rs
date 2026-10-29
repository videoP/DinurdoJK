//! Selected normal/mask arithmetic and real fragment-derivative diagnostics.
//! Fixture foam/scene pins are diagnostic data, never production substitutes.
use super::material_tests::{read_texture, sample};
use super::wave_tests::{native_random_oracle, texture};
use jka_water::river::{
    material::Motion,
    surface,
    wave_advection::{self as wave, Height, Plan, Settings},
};
use wgpu::util::DeviceExt;

const COMMON: &str = r#"
fn motion()->RiverMotion {return RiverMotion(vec4<f32>(0.1,0.0,200.0,1.0),vec4<f32>(1.0,1.0,0.033,0.0));}
fn height()->RiverWaveHeight {return RiverWaveHeight(vec4<f32>(0.1,0.0,400.0,3.0),vec4<f32>(5.0,200.0,0.0,0.0));}
"#;

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    check_arithmetic(device, queue);
    let detail = texture(device, queue, [32, 16], 6, |m, x, y| {
        [
            x as f32 / 32.0 - 0.5,
            y as f32 / 16.0 - 0.5,
            0.5 + m as f32 / 16.0,
            m as f32,
        ]
    });
    check_native_texture(device, queue, &detail);
}

fn check_arithmetic(device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut inputs = Vec::<[f32; 4]>::new();
    let mut expected = Vec::new();
    for i in 0..24 {
        let base = match i % 4 {
            0 => [0.0, 0.0, 1.0],
            1 => [0.25, -0.125, 0.875],
            2 => [0.0, 0.0, -1.0],
            _ => [0.0, 0.0, 0.0],
        };
        let wpo = [0.3, -0.4, 0.8];
        let detail = [-0.25, 0.15, 0.9];
        let foam = [0.2, -0.3, (0.87f32).sqrt()];
        let magnitude = [0.0, 25.0, 100.0, 200.0, 400.0, 600.0][i % 6];
        let total = [0.0, 0.5, 1.0][i % 3];
        let foam_opacity = [0.0, 0.25, 1.0][(i / 3) % 3];
        let mask = if i >= 20 { 0.0 } else { 1.0 };
        let uv = [[0.5, 0.5], [0.125, 0.25], [0.1249, 0.5], [0.8751, 0.5]][i % 4];
        let depth = [-1.0, 0.0, 1e-8, 1.01e-8, 1.0, 10.0][i % 6];
        let opacity = [0.0, 0.5, 1.0][i % 3];
        let (pixel, custom) = [(1.0, 2.0), (1.0, 1.0), (1.0, 1.000005), (2.0, 1.0)][(i / 4) % 4];
        let tiny = [0.0, 0.001, 0.002, 0.004][i % 4];
        let v4 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
        inputs.extend([
            v4(base),
            v4(wpo),
            v4(detail),
            v4(foam),
            [total, foam_opacity, mask, magnitude],
            [uv[0], uv[1], depth, opacity],
            [8.0, 4.0, pixel, custom],
            [tiny, 0.0, 0.0, 0.0],
        ]);
        let n = surface::normal(
            base,
            wpo,
            detail,
            magnitude,
            Motion::default(),
            Height::default(),
            foam,
            total,
            foam_opacity,
            mask,
        );
        let m = surface::masks(depth, opacity, uv, [8, 4], 0.25, pixel, custom);
        expected.extend([
            v4(n),
            [m.water_opacity, m.simulation_mask, m.material_mask, 0.0],
            v4(surface::blend_normals(base, detail)),
            v4(surface::safe_normalize([tiny, 0.0, 0.0])),
        ]);
    }
    let source = format!(
        "{}\n{}\n{}",
        surface::fragment_shader(),
        COMMON,
        r#"
    @group(0) @binding(0) var<storage,read> inputs:array<vec4<f32>>;
    @group(0) @binding(1) var<storage,read_write> outputs:array<vec4<f32>>;
    @compute @workgroup_size(32) fn arithmetic(@builtin(global_invocation_id) id:vec3<u32>) {
        let i=id.x*8u;let o=id.x*4u;if i>=arrayLength(&inputs){return;}
        let s=inputs[i+4u];let uv=inputs[i+5u];let d=inputs[i+6u];
        let n=river_surface_normal(inputs[i].xyz,inputs[i+1u].xyz,inputs[i+2u].xyz,s.w,motion(),height(),inputs[i+3u].xyz,s.x,s.y,s.z);
        let m=river_surface_masks(uv.z,uv.w,uv.xy,d.xy,0.25,d.z,d.w);
        outputs[o]=vec4<f32>(n,0.0);outputs[o+1u]=vec4<f32>(m.water_opacity,m.simulation_mask,m.material_mask,0.0);
        outputs[o+2u]=vec4<f32>(river_blend_normals(inputs[i].xyz,inputs[i+2u].xyz),0.0);
        outputs[o+3u]=vec4<f32>(river_safe_normalize(inputs[i+7u].xyz),0.0);
    }"#
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native normal/mask graph arithmetic"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("arithmetic"),
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
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    compare(
        &super::read_buffer(device, queue, &output),
        &expected,
        2e-5,
        "normal/mask arithmetic",
    );
}

fn compare(actual: &[[f32; 4]], expected: &[[f32; 4]], tolerance: f32, label: &str) {
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
        for c in 0..4 {
            assert!(
                a[c].is_finite() && (a[c] - b[c]).abs() <= tolerance,
                "{label} pixel {i} channel {c}: GPU {}, reference {}",
                a[c],
                b[c]
            );
        }
    }
}

const FRAGMENT: &str = r#"
@group(0) @binding(0) var fluid:texture_2d<f32>;
@group(0) @binding(1) var normals:texture_2d<f32>;
@group(0) @binding(2) var normal_sampler:sampler;
@group(0) @binding(3) var detail:texture_2d<f32>;
@group(0) @binding(4) var detail_sampler:sampler;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(p[i],0.0,1.0);
}
struct FragmentResult {@location(0) normal:vec4<f32>,@location(1) wpo:vec4<f32>,@location(2) detail:vec4<f32>}
@fragment fn fragment(@builtin(position) pixel:vec4<f32>)->FragmentResult {
    // Panel boundaries are aligned with 2x2 derivative quads. Velocity is an
    // affine live field; derivatives agree for coarse and fine instructions.
    let panel=u32(pixel.x)/8u;
    let step=array<f32,4>(1.0,16.0,128.0,1024.0)[panel];
    let position=vec2<f32>(pixel.x*step-300.0,pixel.y*step+200.0);
    let uv=vec2<f32>(0.25)+pixel.xy/vec2<f32>(64.0,32.0);
    let wpo_settings=RiverAdvection(vec4<f32>(20.0,70.0,2.0,0.0005),vec4<f32>(1.75,2.0,0.0,0.0));
    let detail_settings=RiverAdvection(vec4<f32>(20.0,70.0,2.0,0.0005),vec4<f32>(1.75,0.0,0.0,0.0));
    let f=river_live_field(fluid,uv);let base=textureSampleLevel(normals,normal_sampler,uv,0.0).xyz;
    let a=river_wave_fragment(detail,detail_sampler,position,f.zw,base,0.35,wpo_settings);
    let b=river_wave_fragment(detail,detail_sampler,position,f.zw,base,0.35,detail_settings);
    let n=river_live_grid_normal(fluid,normals,normal_sampler,detail,detail_sampler,uv,position,0.35,motion(),wpo_settings,detail_settings,height(),vec3<f32>(0.2,-0.3,0.9327379),f32(panel)*0.25,0.6,1.0);
    return FragmentResult(vec4<f32>(n,1.0),a,b);
}
"#;

pub(super) fn check_native_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    detail: &wgpu::Texture,
) {
    let fluid = texture(device, queue, [8, 8], 1, |_, x, y| {
        [
            120.0,
            80.0,
            153.0 + x as f32 * 18.0 + y as f32 * 7.0,
            -73.0 + x as f32 * 3.0 + y as f32 * 15.0,
        ]
    });
    let normals = texture(device, queue, [8, 8], 1, |_, _, _| {
        [0.25, -0.125, 0.875, 0.0]
    });
    let normal_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let detail_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native detail normals in actual fragment stage"),
        source: wgpu::ShaderSource::Wgsl(
            format!("{}\n{}\n{}", surface::fragment_shader(), COMMON, FRAGMENT).into(),
        ),
    });
    let targets: Vec<_> = (0..3)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("normal diagnostic only, not optical water"),
                size: wgpu::Extent3d {
                    width: 32,
                    height: 16,
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
    let views: Vec<_> = targets
        .iter()
        .map(|t| t.create_view(&Default::default()))
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
    let fluid_view = fluid.create_view(&Default::default());
    let normal_view = normals.create_view(&Default::default());
    let detail_view = detail.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fluid_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&normal_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&normal_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&detail_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&detail_sampler),
            },
        ],
    });
    let attachments: Vec<_> = views
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
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
    let values = read_texture(device, queue, &fluid, 0);
    let (random, _) = native_random_oracle(device, queue, &[0.0]);
    let settings = Settings {
        speed: 1.75,
        ..Default::default()
    };
    let base = [0.25, -0.125, 0.875];
    let mut fluids = Vec::new();
    let mut plans = Vec::new();
    let mut queries = Vec::new();
    for y in 0..16 {
        for x in 0..32 {
            let pixel = [x as f32 + 0.5, y as f32 + 0.5];
            let uv = [0.25 + pixel[0] / 64.0, 0.25 + pixel[1] / 32.0];
            let f = jka_water::river::material::live_field([8, 8], &values, uv).unwrap();
            let step = [1.0, 16.0, 128.0, 1024.0][x / 8];
            let position = [pixel[0] * step - 300.0, pixel[1] * step + 200.0];
            let p = Plan::with_random([f[2], f[3]], base, 0.35, settings, |_| random[0]);
            // Independent analytic derivatives of the original affine velocity.
            let vx = [18.0 / 8.0, 3.0 / 8.0];
            let vy = [7.0 / 4.0, 15.0 / 4.0];
            for bias in [2.0, 0.0] {
                let mips = p.times.map(|t| {
                    wave::fragment_mip(
                        [
                            (step - vx[0] * t) * settings.texture_scale,
                            -vx[1] * t * settings.texture_scale,
                        ],
                        [
                            -vy[0] * t * settings.texture_scale,
                            (step - vy[1] * t) * settings.texture_scale,
                        ],
                        [detail.width(), detail.height()],
                        bias,
                        detail.mip_level_count(),
                    )
                });
                for b in 0..4 {
                    for phase in 0..3 {
                        let uv = p.uv(position, settings, b, phase);
                        queries.push([uv[0], uv[1], mips[phase], 0.0]);
                    }
                }
            }
            fluids.push(f);
            plans.push(p);
        }
    }
    let taps = sample(device, queue, detail, 0, &detail_sampler, &queries);
    let mut expected = [Vec::new(), Vec::new(), Vec::new()];
    for i in 0..512 {
        let p = plans[i];
        let position = [0.0; 2];
        let mut tap = i * 24;
        let mut get = || {
            p.sample(position, settings, [0.0; 3], |_, _| {
                let v = taps[tap];
                tap += 1;
                v
            })
        };
        // The callback deliberately consumes independent direct SampleLevel taps;
        // planner UVs here do not feed the hardware oracle a second time.
        let a = get();
        let b = get();
        let f = fluids[i];
        let total = (i % 32 / 8) as f32 * 0.25;
        let n = surface::normal(
            base,
            [a[0], a[1], a[2]],
            [b[0], b[1], b[2]],
            f[2].hypot(f[3]),
            Motion::default(),
            Height::default(),
            [0.2, -0.3, 0.9327379],
            total,
            0.6,
            1.0,
        );
        expected[0].push([n[0], n[1], n[2], 1.0]);
        expected[1].push(a);
        expected[2].push(b);
    }
    for i in 0..3 {
        // Direct taps cover all 24 coordinates, derivative-selected fractional
        // mips and final basis/attenuation, including actual native FFT RGB.
        compare(
            &read_texture(device, queue, &targets[i], 0),
            &expected[i],
            0.001,
            "native fragment FFT/normal graph",
        );
    }
}

#[test]
fn river_surface_defaults_match_selected_native_parent() {
    let j: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/surface-reference.json"
    ))
    .unwrap();
    let s = surface::detail_advection();
    let motion = Motion::default();
    assert_eq!(s.vertex_mip, 0.0);
    for (name, v) in [
        ("NormalTextureScale", s.texture_scale),
        ("WaveSpeed", s.speed),
        ("LowVelocityWaveScale", motion.low_velocity_scale),
        ("WaveAttenMin", motion.attenuation_min),
        ("WaveAttenMax", motion.attenuation_max),
        ("WaveAmount", motion.wave_amount),
    ] {
        assert_eq!(
            j["effective_parameters"][name].as_f64().unwrap() as f32,
            v,
            "{name}"
        );
    }
}
