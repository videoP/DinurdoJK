//! Native wave math and vertex integration diagnostics. Fixture textures are
//! test data, never replacements installed into the game's water presentation.
use super::material_tests::{read_texture, sample};
use jka_water::river::{
    material::{self, Motion},
    wave_advection::{self as wave, Height, Plan, Settings},
};
use wgpu::util::DeviceExt;

const INPUTS: usize = 9;
const OUTPUTS: usize = 39;
#[derive(Clone, Copy)]
struct Case {
    position: [f32; 2],
    velocity: [f32; 2],
    normal: [f32; 3],
    time: f32,
    distance: f32,
    settings: Settings,
    height: Height,
}
fn cases() -> Vec<Case> {
    let times = [-0.125, 0.0, 0.35, 1.25, 1.999, 2.0, 2.01, 8.0, 257.0, 513.0];
    times
        .into_iter()
        .enumerate()
        .map(|(i, time)| Case {
            position: [123.0 + i as f32 * 293.0, -321.0 - i as f32 * 93.0],
            velocity: if i == 0 {
                [0.0, 0.0]
            } else {
                [173.0 + i as f32 * 7.0, -47.0 + i as f32 * 2.0]
            },
            normal: if i % 2 == 0 {
                [0.0, 0.0, 1.0]
            } else {
                [0.25, 0.125, 0.875]
            },
            time,
            distance: if i == 2 {
                199.0
            } else if i == 3 {
                225.0
            } else {
                300.0
            },
            settings: Settings {
                angle_degrees: if i == 8 { 0.0 } else { 20.0 },
                magnitude_delta: if i == 9 { 0.0 } else { 70.0 },
                speed: if i == 5 { 1.75 } else { 1.0 },
                ..Settings::default()
            },
            height: Height::default(),
        })
        .collect()
}
pub(super) fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    mips: u32,
    mut value: impl FnMut(u32, u32, u32) -> [f32; 4],
) -> wgpu::Texture {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wave diagnostic fixture, not a production FFT substitute"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for mip in 0..mips {
        let [w, h] = size.map(|n| (n >> mip).max(1));
        let mut values = Vec::new();
        for y in 0..h {
            for x in 0..w {
                values.push(value(mip, x, y));
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 16),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
    tex
}

pub(super) fn native_random_oracle(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cycles: &[f32],
) -> (Vec<f32>, f32) {
    // Only native device dot/frac arithmetic, independent of the production
    // planner. FMA/rounding differences must not be disguised by replacing the
    // source hash or widening sample-result tolerances.
    let source = r#"
    @group(0) @binding(0) var<storage,read> cycles:array<vec4<f32>>;
    @group(0) @binding(1) var<storage,read_write> result:array<vec4<f32>>;
    @compute @workgroup_size(64) fn primitive(@builtin(global_invocation_id) id:vec3<u32>) {
        if id.x>=arrayLength(&cycles){return;}
        let pos=fract(vec2<f32>(cycles[id.x].x,0.0)/128.0)*128.0+vec2<f32>(-64.340622,-72.465622);
        result[id.x]=vec4<f32>(fract(dot(pos.xyx*pos.xyy,vec3<f32>(20.390625,60.703125,2.4281209))),atan2(cycles[id.x].z,cycles[id.x].y),0.0,0.0);
    }"#;
    let values: Vec<_> = cycles.iter().map(|v| [*v, 0.0, 0.0, 0.0]).collect();
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&values),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: values.len() as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("independent native random primitive"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("primitive"),
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
        pass.dispatch_workgroups((cycles.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    let result = super::read_buffer(device, queue, &output);
    (result.iter().map(|v| v[0]).collect(), result[0][1])
}

const PROBE: &str = r#"
@group(0) @binding(0) var detail_tex:texture_2d<f32>;
@group(0) @binding(1) var detail_sampler:sampler;
@group(0) @binding(2) var<storage,read> inputs:array<vec4<f32>>;
@group(0) @binding(3) var<storage,read_write> outputs:array<vec4<f32>>;
@group(0) @binding(4) var fluid_tex:texture_2d<f32>;
@group(0) @binding(5) var normal_tex:texture_2d<f32>;
fn combined(i:u32,previous:bool)->vec3<f32> {
    let b=i*9u;let q=inputs[b];let motion=RiverMotion(vec4<f32>(0.1,0.0,200.0,1.0),vec4<f32>(1.0,1.0,0.033,0.0));
    let time=select(q.z,q.z-0.033,previous);
    return river_live_grid_wpo(fluid_tex,normal_tex,detail_sampler,detail_tex,detail_sampler,inputs[b+8u].xy,q.xy,q.w,time,vec3<f32>(1.0,-2.0,3.0),vec3<f32>(0.0,0.0,1.0),motion,RiverAdvection(inputs[b+3u],inputs[b+4u]),RiverWaveHeight(inputs[b+5u],inputs[b+6u]),previous);
}
@compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
    let i=id.x;if i>=arrayLength(&inputs)/9u{return;}
    let b=i*9u;let o=i*39u;let q=inputs[b];let s=RiverAdvection(inputs[b+3u],inputs[b+4u]);
    let p=river_wave_plan(inputs[b+1u].xy,inputs[b+2u].xyz,q.z,s);
    let pp=river_wave_plan(inputs[b+1u].xy,inputs[b+2u].xyz,q.z-0.033,s);
    outputs[o]=vec4<f32>(p.times,p.angle_blend);outputs[o+1u]=vec4<f32>(p.weights,p.magnitude_blend);
    outputs[o+2u]=vec4<f32>(p.cycles,0.0);outputs[o+3u]=vec4<f32>(p.offsets,0.0);
    outputs[o+4u]=vec4<f32>(p.velocities[0],p.velocities[1]);outputs[o+5u]=vec4<f32>(p.velocities[2],p.velocities[3]);
    let value=river_wave_vertex(detail_tex,detail_sampler,q.xy,inputs[b+1u].xy,inputs[b+2u].xyz,q.z,s);
    let pv=river_wave_vertex(detail_tex,detail_sampler,q.xy,inputs[b+1u].xy,inputs[b+2u].xyz,q.z-0.033,s);
    let height=RiverWaveHeight(inputs[b+5u],inputs[b+6u]);
    outputs[o+6u]=value;outputs[o+7u]=vec4<f32>(river_advected_height(value.a,length(inputs[b+1u].xy),q.w,height),0.0,0.0,0.0);
    outputs[o+23u]=pv;outputs[o+24u]=vec4<f32>(river_advected_height(pv.a,length(inputs[b+1u].xy),q.w,height),0.0,0.0,0.0);
    outputs[o+25u]=vec4<f32>(pp.weights,pp.angle_blend);outputs[o+26u]=vec4<f32>(pp.magnitude_blend,0.0,0.0,0.0);
    for (var k=0u;k<4u;k++) {for (var j=0u;j<3u;j++) {
        outputs[o+8u+k*3u+j]=vec4<f32>(river_wave_uv(p,q.xy,s.brackets.w,k,j),s.wave.y,0.0);
        outputs[o+27u+k*3u+j]=vec4<f32>(river_wave_uv(pp,q.xy,s.brackets.w,k,j),s.wave.y,0.0);
    }}
    let footprint=inputs[b+7u];
    let lod=clamp(river_wave_mip(footprint.xy,footprint.zw,vec2<f32>(textureDimensions(detail_tex,0)))+s.wave.y,0.0,f32(textureNumLevels(detail_tex)));
    outputs[o+20u]=vec4<f32>(lod,0.0,0.0,0.0);
    outputs[o+21u]=vec4<f32>(combined(i,false),0.0);outputs[o+22u]=vec4<f32>(combined(i,true),0.0);
}
struct Varying { @builtin(position) position:vec4<f32>,@location(0) wpo:vec3<f32> }
@vertex fn vertex_probe(@builtin(vertex_index) i:u32)->Varying {
    let wpo=combined(i,false);let n=f32(arrayLength(&inputs)/9u);
    var out:Varying;out.position=vec4<f32>((f32(i)+0.5)/n*2.0-1.0+wpo.x*1e-6,wpo.y*1e-6,0.5+wpo.z*1e-6,1.0);out.wpo=wpo;return out;
}
@fragment fn pixel_probe(v:Varying)->@location(0) vec4<f32> {return vec4<f32>(v.wpo,0.0);}
"#;

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    check_detail(device, queue, None);
}
pub(super) fn check_native_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    detail: &wgpu::Texture,
) {
    check_detail(device, queue, Some(detail));
}
fn check_detail(device: &wgpu::Device, queue: &wgpu::Queue, native: Option<&wgpu::Texture>) {
    let cases = cases();
    let mut inputs = Vec::new();
    let mut cycles = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let s = c.settings;
        let h = c.height;
        inputs.extend([
            [c.position[0], c.position[1], c.time, c.distance],
            [c.velocity[0], c.velocity[1], 0.0, 0.0],
            [c.normal[0], c.normal[1], c.normal[2], 0.0],
            [
                s.angle_degrees,
                s.magnitude_delta,
                s.pulse_length,
                s.texture_scale,
            ],
            [s.speed, s.vertex_mip, 0.0, 0.0],
            [
                h.low_velocity_scale,
                h.velocity_min,
                h.velocity_max,
                h.power,
            ],
            [h.multiplier, h.camera_falloff, 0.0, 0.0],
            [0.125, 0.0, 0.0, 0.25],
            [(i as f32 + 0.5) / 16.0, 0.5, 0.0, 0.0],
        ]);
        for time in [c.time, c.time - 0.033] {
            for cycle in Plan::new(c.velocity, c.normal, time, s).cycles {
                if !cycles.contains(&cycle) {
                    cycles.push(cycle);
                }
            }
        }
    }
    let (random, zero_angle) = native_random_oracle(device, queue, &cycles);
    let undefined_zero_flow = !zero_angle.is_finite();
    assert_eq!(inputs.len(), cases.len() * INPUTS);
    let plan = |c: Case, time: f32| {
        Plan::with_random(c.velocity, c.normal, time, c.settings, |cycle| {
            random[cycles.iter().position(|v| *v == cycle).unwrap()]
        })
    };
    // Distinct, exactly half-representable values per mip/channel. This test
    // data exercises sample selection; it does not implement the FFT producer.
    let fixture = native.is_none().then(|| {
        texture(device, queue, [16, 8], 5, |m, x, y| {
            [
                x as f32 * 0.125,
                y as f32 * 0.25,
                0.5,
                m as f32 + x as f32 * 0.125 - y as f32 * 0.25,
            ]
        })
    });
    let detail = native.unwrap_or_else(|| fixture.as_ref().unwrap());
    let fluid = texture(device, queue, [16, 1], 1, |_, x, _| {
        let c = cases[(x as usize).min(cases.len() - 1)];
        [120.0 + x as f32, 100.0, c.velocity[0], c.velocity[1]]
    });
    let normal = texture(device, queue, [16, 1], 1, |_, x, _| {
        let n = cases[(x as usize).min(cases.len() - 1)].normal;
        [n[0], n[1], n[2], 0.0]
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("native FFT detail RT resource sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native wave plan, height and live-grid WPO probe"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", wave::vertex_shader(), PROBE).into()),
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&inputs),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (cases.len() * OUTPUTS * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let detail_view = detail.create_view(&Default::default());
    let fluid_view = fluid.create_view(&Default::default());
    let normal_view = normal.create_view(&Default::default());
    let bindings = [
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&detail_view),
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
        wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(&fluid_view),
        },
        wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::TextureView(&normal_view),
        },
    ];
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
        entries: &bindings,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    let actual = super::read_buffer(device, queue, &output);
    let mut current_wpo = Vec::new();
    for (i, c) in cases.iter().copied().enumerate() {
        let a = &actual[i * OUTPUTS..(i + 1) * OUTPUTS];
        if c.velocity == [0.0; 2] && undefined_zero_flow {
            // FXC's source atan2(0,0) produces NaN. Verify the production
            // planner retains that behavior, but do not assert a meaningful
            // sample or raster position for a native undefined expression.
            assert!(!a[0][3].is_finite());
            assert!(!a[4][0].is_finite() || !a[4][1].is_finite());
            eprintln!("native zero-flow wave direction is undefined on this compiler; defined-flow checks remain active");
            current_wpo.push([0.0; 4]);
            continue;
        }
        let p = plan(c, c.time);
        let pp = plan(c, c.time - 0.033);
        let expected = [
            [p.times[0], p.times[1], p.times[2], p.angle_blend],
            [p.weights[0], p.weights[1], p.weights[2], p.magnitude_blend],
            [p.cycles[0], p.cycles[1], p.cycles[2], 0.0],
            [p.offsets[0], p.offsets[1], p.offsets[2], 0.0],
            [
                p.velocities[0][0],
                p.velocities[0][1],
                p.velocities[1][0],
                p.velocities[1][1],
            ],
            [
                p.velocities[2][0],
                p.velocities[2][1],
                p.velocities[3][0],
                p.velocities[3][1],
            ],
        ];
        // Native atan2(0,0) is undefined and differs between DXC/Vulkan.
        // With zero magnitude its angle and upper-magnitude directions are
        // unused: magnitude_blend=0 selects the two zero-velocity brackets.
        // Check every defined/active component without assigning a direction.
        for row in 0..4 {
            for channel in 0..4 {
                if c.velocity == [0.0; 2] && row == 0 && channel == 3 {
                    continue;
                }
                super::compare(
                    &[[a[row][channel], 0.0, 0.0, 0.0]],
                    &[[expected[row][channel], 0.0, 0.0, 0.0]],
                    &format!("native three-phase bracket plan {i}/{row}/{channel}"),
                );
            }
        }
        for bracket in 0..4 {
            if c.velocity == [0.0; 2] && bracket % 2 == 1 {
                continue;
            }
            let row = 4 + bracket / 2;
            let channel = (bracket % 2) * 2;
            let v = p.velocities[bracket];
            let scale = (v[0] * v[0] + v[1] * v[1]).sqrt();
            let error =
                ((a[row][channel] - v[0]).powi(2) + (a[row][channel + 1] - v[1]).powi(2)).sqrt();
            // Bound vector error relative to its magnitude: native GPU trig
            // can leave a tiny component at a cardinal direction. A relative
            // bound on that almost-zero component has no useful meaning.
            // This does not change UV or sampled-result tolerances below.
            assert!(
                error.is_finite() && error <= 0.00001 + scale * 0.000003,
                "native bracket velocity {i}/{bracket}: error {error}, magnitude {scale}"
            );
        }
        let lod = wave::fragment_mip(
            [0.125, 0.0],
            [0.0, 0.25],
            [detail.width(), detail.height()],
            c.settings.vertex_mip,
            detail.mip_level_count(),
        );
        assert!((a[20][0] - lod).abs() < 1e-5);
        for (previous, p, base, sample_index, height_index, wpo_index) in
            [(false, p, 8, 6, 7, 21), (true, pp, 27, 23, 24, 22)]
        {
            let uv: Vec<_> = (0..12).map(|tap| a[base + tap]).collect();
            for (tap, q) in uv.iter().enumerate() {
                if c.velocity == [0.0; 2] && (tap / 3) % 2 == 1 {
                    continue; // Unused upper-magnitude bracket; see atan2 note.
                }
                let reference = p.uv(c.position, c.settings, tap / 3, tap % 3);
                for axis in 0..2 {
                    assert!(
                        (q[axis] - reference[axis]).abs() < 0.00002,
                        "wave UV {i}/{tap}/{axis}: {} != {}",
                        q[axis],
                        reference[axis]
                    );
                }
            }
            let taps = sample(device, queue, &detail, 0, &sampler, &uv);
            let mut at = 0usize;
            // GPU tap coordinates were checked independently above. Sampling
            // these exact coordinates isolates device filter precision from
            // CPU/GPU trig/FMA differences instead of guessing fractional bits.
            let value = p.sample(
                c.position,
                c.settings,
                [c.settings.vertex_mip; 3],
                |_, _| {
                    let v = taps[at];
                    at += 1;
                    v
                },
            );
            super::compare(
                &[a[sample_index]],
                &[value],
                "native twelve-tap bracketed advection",
            );
            let magnitude = (c.velocity[0] * c.velocity[0] + c.velocity[1] * c.velocity[1]).sqrt();
            let height = c.height.evaluate(value[3], magnitude, c.distance);
            super::compare(
                &[a[height_index]],
                &[[height, 0.0, 0.0, 0.0]],
                "native alpha and camera/velocity attenuation",
            );
            let result = material::grid_displacement(
                [120.0 + i as f32, 100.0, c.velocity[0], c.velocity[1]],
                c.normal,
                height,
                [1.0, -2.0, 3.0],
                [0.0, 0.0, 1.0],
                0.033,
                Motion::default(),
                previous,
            );
            let expected = [result[0], result[1], result[2], 0.0];
            super::compare(
                &[a[wpo_index]],
                &[expected],
                "native connected live-grid wave WPO",
            );
            if !previous {
                current_wpo.push(expected);
            }
        }
    }
    check_vertex(
        device,
        queue,
        &shader,
        &bindings,
        &current_wpo,
        u32::from(undefined_zero_flow),
    );
}

fn check_vertex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shader: &wgpu::ShaderModule,
    bindings: &[wgpu::BindGroupEntry<'_>],
    expected: &[[f32; 4]],
    first_vertex: u32,
) {
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("native live WPO vertex-stage diagnostic only"),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_probe"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::PointList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("pixel_probe"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let entries: Vec<_> = bindings
        .iter()
        .filter(|b| b.binding != 3)
        .cloned()
        .collect();
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vertex WPO diagnostic, not a water optical shader"),
        size: wgpu::Extent3d {
            width: expected.len() as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(first_vertex..expected.len() as u32, 0..1);
    }
    queue.submit(Some(encoder.finish()));
    super::compare(
        &read_texture(device, queue, &target, 0),
        expected,
        "native advection and displacement executed in vertex stage",
    );
}

#[test]
fn river_wave_settings_match_selected_native_parent() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/wave-advection-reference.json"
    ))
    .unwrap();
    let values = &data["effective_parameters"];
    let s = Settings::default();
    let h = Height::default();
    for (name, value) in [
        ("BracketAngleDelta", s.angle_degrees),
        ("BracketMagnitudeDelta", s.magnitude_delta),
        ("PulseLength", s.pulse_length),
        ("WPOWaveScale", s.texture_scale),
        ("WaveSpeed", s.speed),
        ("WPOWaveMIPBias", s.vertex_mip),
        ("LowVelocityWPOWaveScale", h.low_velocity_scale),
        ("WPOWaveAttenMin", h.velocity_min),
        ("WPOWaveAttenMax", h.velocity_max),
        ("LowVelocityWPOWaveAttenuationPower", h.power),
        ("WPOWaveHeightMult", h.multiplier),
        ("AdvectedWPOWaveCameraFalloff", h.camera_falloff),
    ] {
        assert_eq!(value, values[name].as_f64().unwrap() as f32, "{name}");
    }
}
