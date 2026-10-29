//! Independent CPU graph arithmetic plus hardware sampling/store oracles.
//! No diagnostic shader in this module is used as a production water material.
use crate::river_gpu::SurfaceFields;
use crate::river_material::{Presentation, Textures};
use jka_water::river::material::{self, Motion};
use wgpu::util::DeviceExt;

pub(super) fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    mip: u32,
) -> Vec<[f32; 4]> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("river half texture readback only"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
    @group(0) @binding(0) var tex:texture_2d<f32>;
    @group(0) @binding(1) var<storage,read_write> out_values:array<vec4<f32>>;
    @compute @workgroup_size(8,8) fn read(@builtin(global_invocation_id) id:vec3<u32>) {
        let size=textureDimensions(tex,0); if any(id.xy>=size) {return;}
        out_values[id.x+id.y*size.x]=textureLoad(tex,vec2<i32>(id.xy),0);
    }"#
            .into(),
        ),
    });
    let size = [(tex.width() >> mip).max(1), (tex.height() >> mip).max(1)];
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river half texture readback"),
        size: u64::from(size[0]) * u64::from(size[1]) * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("read native material texture"),
        layout: None,
        module: &shader,
        entry_point: Some("read"),
        compilation_options: Default::default(),
        cache: None,
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: mip,
        mip_level_count: Some(1),
        ..Default::default()
    });
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
                resource: buffer.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
    }
    queue.submit(Some(encoder.finish()));
    super::read_buffer(device, queue, &buffer)
}

pub(super) fn quantize(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    values: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    // Only a device typed-store oracle: CPU computes the normal blur; this
    // shader does not know its weights, normalization, border or input grid.
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("CPU expected normals before typed store"),
        contents: bytemuck::cast_slice(values),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("independent half store oracle"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("independent half store only"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
    @group(0) @binding(0) var<storage,read> values:array<vec4<f32>>;
    @group(0) @binding(1) var output_tex:texture_storage_2d<rgba16float,write>;
    @compute @workgroup_size(8,8) fn cast_half(@builtin(global_invocation_id) id:vec3<u32>) {
        let size=textureDimensions(output_tex);if any(id.xy>=size){return;}
        textureStore(output_tex,id.xy,values[id.x+id.y*size.x]);
    }"#
            .into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("cast_half"),
        compilation_options: Default::default(),
        cache: None,
    });
    let view = texture.create_view(&Default::default());
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
                resource: wgpu::BindingResource::TextureView(&view),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
    }
    queue.submit(Some(encoder.finish()));
    read_texture(device, queue, &texture, 0)
}

/// Device interpolation oracle, with CPU-generated explicit UVs. It neither
/// computes mip coordinates nor performs material field/displacement logic.
pub(super) fn sample(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    mip: u32,
    sampler: &wgpu::Sampler,
    uv: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("independent material SampleLevel oracle"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
    @group(0) @binding(0) var tex:texture_2d<f32>;
    @group(0) @binding(1) var samp:sampler;
    @group(0) @binding(2) var<storage,read> queries:array<vec4<f32>>;
    @group(0) @binding(3) var<storage,read_write> values:array<vec4<f32>>;
    @compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
        if id.x>=arrayLength(&queries){return;}
        values[id.x]=textureSampleLevel(tex,samp,queries[id.x].xy,queries[id.x].z);
    }"#
            .into(),
        ),
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(uv),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: uv.len() as u64 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: mip,
        mip_level_count: None,
        ..Default::default()
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
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
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
        pass.dispatch_workgroups((uv.len() as u32).div_ceil(64), 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    super::read_buffer(device, queue, &output)
}

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    for size in [[7u32, 3u32], [1, 9], [1, 1]] {
        let count = (size[0] * size[1]) as usize;
        let mut surface: Vec<[f32; 4]> = (0..count)
            .map(|i| {
                [
                    i as f32 * 13.0 + 0.25,
                    (i % 5) as f32 * 0.5,
                    i as f32 * 4.0 - 30.0,
                    20.0 - i as f32 * 2.0,
                ]
            })
            .collect();
        surface[0][0] = 16000.25; // Non-half input; both adjacent-conversion modes round down here.
        let normals: Vec<[f32; 4]> = (0..count)
            .map(|i| {
                [
                    ((i % 3) as f32 - 1.0) * 0.25,
                    ((i / 3 % 3) as f32 - 1.0) * 0.25,
                    0.5,
                    0.0,
                ]
            })
            .collect();
        let make = |v: &[[f32; 4]]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("river material diagnostic source"),
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let fields = SurfaceFields {
            dimensions: size,
            surface: make(&surface),
            normals: make(&normals),
        };
        let mut presentation = match Presentation::new(device, &fields).unwrap().freeze() {
            Err(p) => p,
            Ok(_) => panic!("unencoded fields must not freeze"),
        };
        assert!(presentation.textures().is_none());
        let mut encoder = device.create_command_encoder(&Default::default());
        presentation.encode(&mut encoder);
        // Repeat without allocating resources, then freeze before submission.
        presentation.encode(&mut encoder);
        let textures = match presentation.freeze() {
            Ok(t) => t,
            Err(_) => panic!("encoded fields should freeze"),
        };
        drop(fields);
        queue.submit(Some(encoder.finish()));
        assert_eq!(textures.surface.format(), wgpu::TextureFormat::Rgba16Float);
        assert_eq!(textures.normals.mip_level_count(), 1);
        let actual = read_texture(device, queue, &textures.surface, 0);
        surface[0][0] = 16000.0;
        assert_eq!(actual, surface, "native surface typed-store boundary");
        let expected = material::blur_normals(size.map(|n| n as usize), &normals).unwrap();
        let expected = quantize(device, queue, size, &expected);
        let actual_normals = read_texture(device, queue, &textures.normals, 0);
        super::compare(
            &actual_normals,
            &expected,
            "native half NormalGrid -> blur -> half NormalsRT",
        );
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut bytes = u64::from(size[0]) * u64::from(size[1]) * 16;
        for mip in 1..textures.surface.mip_level_count() {
            let dims = size.map(|n| (n >> mip).max(1));
            let uv: Vec<_> = (0..dims[1])
                .flat_map(|y| {
                    (0..dims[0]).map(move |x| {
                        [
                            (x as f32 + 0.5) * (1.0 / dims[0] as f32),
                            (y as f32 + 0.5) * (1.0 / dims[1] as f32),
                            0.0,
                            0.0,
                        ]
                    })
                })
                .collect();
            let expected = sample(device, queue, &textures.surface, mip - 1, &clamp, &uv);
            let expected = quantize(device, queue, dims, &expected);
            assert_eq!(
                read_texture(device, queue, &textures.surface, mip),
                expected,
                "native odd/one-axis mip {mip}"
            );
            bytes += u64::from(dims[0]) * u64::from(dims[1]) * 8;
        }
        assert_eq!(textures.logical_bytes(), bytes);
        check_displacement(device, queue, &textures, &surface);
    }
}

fn check_displacement(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    textures: &Textures,
    surface: &[[f32; 4]],
) {
    let uv = [
        [0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [-0.25, 0.75, 0.0, 0.0],
        [0.31, 0.63, 0.0, 0.0],
        [1.3, 0.4, 0.0, 0.0],
    ];
    let normals = sample(
        device,
        queue,
        &textures.normals,
        0,
        &textures.normal_sampler,
        &uv,
    );
    let motion = Motion::default();
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for (i, at) in uv.iter().enumerate() {
        let fluid = material::live_field(
            textures.dimensions.map(|n| n as usize),
            surface,
            [at[0], at[1]],
        )
        .unwrap();
        let height = i as f32 * 0.75 - 1.25;
        let incoming = [1.0, -2.0, 3.0];
        let previous_height = height - 0.5;
        let previous_incoming = [0.5, -1.5, 2.5];
        let axis = [0.25, 0.5, 2.0];
        let delta = 0.033;
        let evaluate = |height, incoming, previous| {
            material::grid_displacement(
                fluid,
                [normals[i][0], normals[i][1], normals[i][2]],
                height,
                incoming,
                axis,
                delta,
                motion,
                previous,
            )
        };
        let current = evaluate(height, incoming, false);
        let previous = evaluate(previous_height, previous_incoming, true);
        inputs.extend([
            [at[0], at[1], height, previous_height],
            [incoming[0], incoming[1], incoming[2], 0.0],
            [
                previous_incoming[0],
                previous_incoming[1],
                previous_incoming[2],
                0.0,
            ],
            [axis[0], axis[1], axis[2], 0.0],
            [
                motion.low_velocity_scale,
                motion.attenuation_min,
                motion.attenuation_max,
                motion.wave_amount,
            ],
            [
                motion.previous_velocity_multiplier,
                motion.wave_speed,
                delta,
                0.0,
            ],
        ]);
        expected.extend([
            fluid,
            [current[0], current[1], current[2], 0.0],
            [previous[0], previous[1], previous[2], 0.0],
        ]);
    }
    let source = format!(
        "{}\n{}",
        material::SHADER,
        r#"
    @group(0) @binding(0) var fluid_tex:texture_2d<f32>;
    @group(0) @binding(1) var normal_tex:texture_2d<f32>;
    @group(0) @binding(2) var normal_sampler:sampler;
    @group(0) @binding(3) var<storage,read> inputs:array<vec4<f32>>;
    @group(0) @binding(4) var<storage,read_write> outputs:array<vec4<f32>>;
    @compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
        let i=id.x;if i>=arrayLength(&inputs)/6u{return;}
        let at=inputs[i*6u];let fluid=river_live_field(fluid_tex,at.xy);
        let normal=textureSampleLevel(normal_tex,normal_sampler,at.xy,0.0).xyz;
        let motion=RiverMotion(inputs[i*6u+4u],inputs[i*6u+5u]);
        let current=river_grid_displacement(fluid,normal,at.z,inputs[i*6u+1u].xyz,inputs[i*6u+3u].xyz,motion,false);
        let previous=river_grid_displacement(fluid,normal,at.w,inputs[i*6u+2u].xyz,inputs[i*6u+3u].xyz,motion,true);
        outputs[i*3u]=fluid;outputs[i*3u+1u]=vec4<f32>(current,0.0);outputs[i*3u+2u]=vec4<f32>(previous,0.0);
    }"#
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native live material function diagnostic"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
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
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let fluid = textures.surface.create_view(&Default::default());
    let normal = textures.normals.create_view(&Default::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fluid),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&normal),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&textures.normal_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
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
    super::compare(
        &super::read_buffer(device, queue, &output),
        &expected,
        "native live loads and current/previous material displacement",
    );
}

pub fn check_generated_outputs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    textures: &Textures,
    fields: &SurfaceFields,
) {
    let raw = super::read_buffer(device, queue, &fields.surface);
    let expected = quantize(device, queue, fields.dimensions, &raw);
    assert_eq!(
        read_texture(device, queue, &textures.surface, 0),
        expected,
        "solver output -> half material surface without CPU roundtrip"
    );
    let raw = super::read_buffer(device, queue, &fields.normals);
    let grid = quantize(device, queue, fields.dimensions, &raw);
    let blurred = material::blur_normals(fields.dimensions.map(|n| n as usize), &grid).unwrap();
    let expected = quantize(device, queue, fields.dimensions, &blurred);
    super::compare(
        &read_texture(device, queue, &textures.normals, 0),
        &expected,
        "solver output -> half normal grid -> blur -> half normal RT",
    );
}

#[test]
fn river_material_motion_defaults_match_native_parent_overrides() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/presentation-reference.json"
    ))
    .unwrap();
    let motion = Motion::default();
    for (key, value) in [
        ("LowVelocityWaveScale", motion.low_velocity_scale),
        ("WaveAttenMin", motion.attenuation_min),
        ("WaveAttenMax", motion.attenuation_max),
        ("WaveAmount", motion.wave_amount),
        (
            "PreviousFrameVelocityMult",
            motion.previous_velocity_multiplier,
        ),
        ("WaveSpeed", motion.wave_speed),
    ] {
        assert_eq!(
            value,
            reference["effective_live_scalars"][key].as_f64().unwrap() as f32,
            "{key}"
        );
    }
}
