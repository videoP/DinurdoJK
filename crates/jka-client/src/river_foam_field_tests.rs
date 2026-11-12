//! Native foam field producer (stages 4/16) against the CPU graph port.
//! Each frame's previous FoamGrid is the GPU's own previous output, sampled
//! by the independent SampleLevel oracle; stores use the half-store oracle.
use super::material_tests::{quantize, read_texture, sample};
use crate::river_foam::{Field, FieldInputs};
use jka_water::river::foam_field::{self, Fields, Frame, Placement, Settings, Setup};
use wgpu::util::DeviceExt;

fn run(device: &wgpu::Device, queue: &wgpu::Queue, wgsl: &str, input: &[[f32; 4]], outputs: usize) -> Vec<[f32; 4]> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("foam field diagnostic"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(input),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: outputs as u64 * 16,
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
        pass.dispatch_workgroups((input.size() as u32 / 16).div_ceil(64), 1, 1);
    }
    queue.submit(Some(encoder.finish()));
    super::read_buffer(device, queue, &output)
}

/// Native JacobianSimplex_ALU / curl in WGSL against the CPU port.
fn check_noise(device: &wgpu::Device, queue: &wgpu::Queue) {
    let offset = foam_field::offset_from_seed(0);
    let points: Vec<[f32; 4]> = (0..48)
        .map(|i| {
            let f = i as f32;
            let base = [[0.0, 0.0, 0.0], [12.5, -3.25, 7.0], [1239.4, 1235.1, 1234.6], [86405.2, 86401.0, 86400.3]][i % 4];
            [base[0] + (f * 0.731).sin() * 2.3, base[1] + (f * 1.37).cos() * 1.9, base[2] + f * 0.0913, 0.0]
        })
        .collect();
    let wgsl = format!(
        "{}\n{}",
        foam_field::library(),
        r#"
    @group(0) @binding(0) var<storage,read> points:array<vec4<f32>>;
    @group(0) @binding(1) var<storage,read_write> values:array<vec4<f32>>;
    @compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {
        if id.x>=arrayLength(&points) {return;}
        let j=river_foam_jacobian_simplex(points[id.x].xyz);
        values[4u*id.x]=j[0];values[4u*id.x+1u]=j[1];values[4u*id.x+2u]=j[2];
        values[4u*id.x+3u]=vec4<f32>(river_foam_curl(points[id.x].xyz,OFFSET),0.0);
    }"#
        .replace("OFFSET", &format!("vec3<f32>({:?},{:?},{:?})", offset[0], offset[1], offset[2]))
    );
    let gpu = run(device, queue, &wgsl, &points, points.len() * 4);
    for (i, p) in points.iter().enumerate() {
        let v = [p[0], p[1], p[2]];
        let j = foam_field::jacobian_simplex(v);
        let c = foam_field::curl(v, offset);
        let expected = [j[0], j[1], j[2], [c[0], c[1], c[2], 0.0]];
        // f32 rounding of skew/corner offsets scales with coordinate magnitude.
        let magnitude = |v: [f32; 3]| v.iter().fold(1.0f32, |m, x| m.max(x.abs()));
        let shifted = magnitude(std::array::from_fn(|k| v[k] + offset[k]));
        for (row, (actual, reference)) in gpu[4 * i..4 * i + 4].iter().zip(expected).enumerate() {
            // Curl evaluates at v + OffsetFromSeed, hundreds of units away.
            let magnitude = if row == 3 { shifted } else { magnitude(v) };
            for k in 0..4 {
                let tolerance = 1e-3 + 4e-6 * magnitude * (1.0 + reference[k].abs());
                assert!(
                    (actual[k] - reference[k]).abs() <= tolerance,
                    "native curl point {i} row {row} component {k}: GPU {} CPU {}",
                    actual[k],
                    reference[k]
                );
            }
        }
    }
}

struct CpuFields {
    dimensions: [u32; 2],
    water: Vec<[f32; 4]>,
    normals: Vec<[f32; 4]>,
}
impl CpuFields {
    // Ideal clamped bilinear; production queries are texel centres, where it
    // is exact. Only the zero-weight depth-difference query is off-centre.
    fn bilinear(&self, values: &[[f32; 4]], unit: [f32; 2]) -> [f32; 4] {
        let p: [f32; 2] = std::array::from_fn(|i| unit[i] * self.dimensions[i] as f32 - 0.5);
        let cell = p.map(|v| v.floor());
        let t: [f32; 2] = std::array::from_fn(|i| p[i] - cell[i]);
        let at = |dx: f32, dy: f32| {
            let x = (cell[0] + dx).clamp(0.0, self.dimensions[0] as f32 - 1.0) as usize;
            let y = (cell[1] + dy).clamp(0.0, self.dimensions[1] as f32 - 1.0) as usize;
            values[y * self.dimensions[0] as usize + x]
        };
        let (a, b, c, d) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * t[0];
            let bottom = c[k] + (d[k] - c[k]) * t[0];
            top + (bottom - top) * t[1]
        })
    }
}
impl Fields for CpuFields {
    fn depth(&self, unit: [f32; 2]) -> f32 {
        self.bilinear(&self.water, unit)[1]
    }
    fn velocity(&self, unit: [f32; 2]) -> [f32; 2] {
        let v = self.bilinear(&self.water, unit);
        [v[2], v[3]]
    }
    fn normal(&self, unit: [f32; 2]) -> [f32; 3] {
        let n = self.bilinear(&self.normals, unit);
        [n[0], n[1], n[2]]
    }
}

fn state_texture(device: &wgpu::Device, queue: &wgpu::Queue, dims: [u32; 2], cells: &[[f32; 4]]) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("foam test hydraulic layers"),
        size: wgpu::Extent3d {
            width: dims[0],
            height: dims[1],
            depth_or_array_layers: 4,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::river_gpu::WATER_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    for layer in 0..4 {
        let values: Vec<f32> = cells.iter().map(|c| c[layer]).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(dims[0] * 4),
                rows_per_image: Some(dims[1]),
            },
            wgpu::Extent3d {
                width: dims[0],
                height: dims[1],
                depth_or_array_layers: 1,
            },
        );
    }
    texture
}

/// RGBA16f texture holding device typed half stores of `values`.
fn half_texture(device: &wgpu::Device, queue: &wgpu::Queue, dims: [u32; 2], values: &[[f32; 4]]) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("foam test half NormalGrid"),
        size: wgpu::Extent3d {
            width: dims[0],
            height: dims[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(values),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("foam test half store only"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
    @group(0) @binding(0) var<storage,read> values:array<vec4<f32>>;
    @group(0) @binding(1) var output_tex:texture_storage_2d<rgba16float,write>;
    @compute @workgroup_size(8,8) fn store(@builtin(global_invocation_id) id:vec3<u32>) {
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
        entry_point: Some("store"),
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
        pass.dispatch_workgroups(dims[0].div_ceil(8), dims[1].div_ceil(8), 1);
    }
    queue.submit(Some(encoder.finish()));
    texture
}

fn trace(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, field: &Field) -> wgpu::Texture {
    let source = field.foam();
    let copy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("foam stage trace"),
        size: source.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: source.format(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    encoder.copy_texture_to_texture(source.as_image_copy(), copy.as_image_copy(), source.size());
    copy
}

/// CPU stage 16 for one frame, given the GPU's own previous output texture.
fn expected_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    setup: &Setup,
    frame: Frame,
    fields: &CpuFields,
    previous: &wgpu::Texture,
    depth_range: [f32; 2],
) -> (Vec<[f32; 4]>, Vec<f32>) {
    let [w, h] = setup.dimensions;
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let st = setup.settings;
    let offset = foam_field::offset_from_seed(st.curl_seed);
    let mut units = Vec::new();
    let mut emits = Vec::new();
    let mut index_error = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let unit = foam_field::execution_unit(setup, [x, y]);
            let a = foam_field::advection_unit(setup, frame, [x, y], fields.velocity(unit));
            units.push([a[0], a[1], 0.0, 0.0]);
            emits.push(foam_field::emit_amount_with_depth_range(foam_field::emission_inputs(setup, frame, unit, fields), depth_range));
            // Propagate check_noise's validated curl tolerance at this noise
            // input magnitude through NoiseOffset into the advection index.
            let m = setup.unit_to_world;
            let magnitude = (0..3).fold(1.0f32, |acc, j| {
                let world = unit[0] * m[0][j] + unit[1] * m[1][j] + m[3][j];
                acc.max((world * st.noise_frequency + st.noise_time_scale * frame.time + offset[j]).abs())
            });
            let noise = foam_field::noise_offset(setup, frame, unit);
            let curl = noise[0].abs().max(noise[1].abs()) / st.noise_strength;
            let curl_tolerance = 1e-3 + 4e-6 * magnitude * (1.0 + curl);
            index_error.push(st.advection_multiplier * frame.delta / setup.foam_dx * st.noise_strength * curl_tolerance);
        }
    }
    let advected = sample(device, queue, previous, 0, &sampler, &units);
    let texels = read_texture(device, queue, previous, 0);
    let mut values = Vec::new();
    let mut spread = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        values.push([foam_field::store_value(setup, frame, advected[i][0], emits[i]), 0.0, 0.0, 0.0]);
        // Hardware filter weights are fixed point (one weight step), plus the
        // bilinear change over the propagated index uncertainty on each axis.
        let p = [unit[0] * w as f32 - 0.5, unit[1] * h as f32 - 0.5].map(|v| v.floor() as i32);
        let reach = index_error[i].ceil() as i32;
        let mut range = (f32::MAX, f32::MIN);
        for dy in -reach..2 + reach {
            for dx in -reach..2 + reach {
                let x = (p[0] + dx).clamp(0, w as i32 - 1) as u32;
                let y = (p[1] + dy).clamp(0, h as i32 - 1) as u32;
                let v = texels[(y * w + x) as usize][0];
                range = (range.0.min(v), range.1.max(v));
            }
        }
        spread.push((range.1 - range.0) * (1.0 / 128.0 + 2.0 * index_error[i]));
    }
    (quantize(device, queue, setup.dimensions, &values), spread)
}

fn compare_frame(gpu: &[[f32; 4]], expected: &[[f32; 4]], spread: &[f32], label: &str) {
    for (i, (a, e)) in gpu.iter().zip(expected).enumerate() {
        let tolerance = 2e-4 + 2e-3 * e[0].abs() + spread[i];
        assert!(
            a[0].is_finite() && (a[0] - e[0]).abs() <= tolerance && a[1..] == [0.0; 3],
            "{label} cell {i}: GPU {:?} CPU {}",
            a,
            e[0]
        );
    }
}

fn fixture_state(variant: u32, dims: [u32; 2]) -> Vec<[f32; 4]> {
    let mut cells = Vec::new();
    for y in 0..dims[1] {
        for x in 0..dims[0] {
            let (depth, vx, vy) = if variant == 0 {
                (
                    if (x + y) % 7 == 0 { 0.0 } else { 120.0 + 40.0 * ((x * 3 + y * 5) % 11) as f32 },
                    60.0 + 25.0 * ((x * 7 + y * 3) % 9) as f32 - 15.0 * y as f32,
                    -40.0 + 30.0 * ((x * 5 + y * 11) % 7) as f32,
                )
            } else {
                (
                    200.0 + 30.0 * ((x + 2 * y) % 9) as f32,
                    -80.0 + 20.0 * ((3 * x + y) % 10) as f32,
                    50.0 - 12.0 * ((x + 4 * y) % 8) as f32,
                )
            };
            cells.push([depth + 10.0, depth, vx, vy]);
        }
    }
    cells
}

pub fn check(device: &wgpu::Device, queue: &wgpu::Queue) {
    check_noise(device, queue);
    let dims = [13u32, 11u32];
    let grid = jka_water::river::control::Grid::max_axis([260.0, 220.0], 13).unwrap();
    assert_eq!(grid.dimensions, dims);
    let placement = Placement::unrotated([3086.25, -2468.125, 100.0], &grid).unwrap();
    let setup = Setup::native(&grid, placement, foam_field::native_sim_res([260.0, 220.0], 13));
    assert_eq!(setup.sim_res, [13.0, 11.0]);
    assert_eq!(setup.settings, Settings::default());
    let states = [fixture_state(0, dims), fixture_state(1, dims)];
    let state = [0, 1].map(|i| state_texture(device, queue, dims, &states[i]));
    let raw_normals: Vec<[f32; 4]> = (0..dims[0] * dims[1])
        .map(|i| {
            let (x, y) = ((i % dims[0]) as f32, (i / dims[0]) as f32);
            let tilt = if (i * 7) % 5 == 0 { 0.6 } else { 0.25 };
            let v = [(0.7 * x).sin() * tilt, (0.5 * y).cos() * 0.25, 1.0];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l, 0.0]
        })
        .collect();
    let normal_grid = half_texture(device, queue, dims, &raw_normals);
    let normals = read_texture(device, queue, &normal_grid, 0);
    let mut field = Field::new(
        device,
        FieldInputs {
            state: &state,
            normal_grid: &normal_grid,
            blocks: None,
        },
        &setup,
    )
    .unwrap();
    assert_eq!(field.logical_bytes(), 2 * 13 * 11 * 8 + 160);
    let frames = [
        (0usize, Frame::new(0.016, 1234.5)),
        (1, Frame::new(0.033, 1234.533)),
        (1, Frame::new(0.05, 1234.583)),
        (0, Frame::new(0.0, 1234.583)),
        (0, Frame::new(0.02, 86400.25)),
    ];
    // Reset after an earlier animated frame, then queue every frame in one
    // submission. Traces are in-stream copies; readback follows submission.
    let mut encoder = device.create_command_encoder(&Default::default());
    field.encode_frame(device, &mut encoder, 0, frames[0].1).unwrap();
    field.encode_reset(&mut encoder);
    let mut traces = vec![trace(device, &mut encoder, &field)];
    for (s, frame) in frames {
        field.encode_frame(device, &mut encoder, s, frame).unwrap();
        traces.push(trace(device, &mut encoder, &field));
    }
    queue.submit(Some(encoder.finish()));
    assert!(read_texture(device, queue, &traces[0], 0).iter().all(|v| *v == [0.0; 4]), "Init Foam must zero FoamGrid");
    let mut peak = 0.0f32;
    for (k, (s, frame)) in frames.into_iter().enumerate() {
        let fields = CpuFields {
            dimensions: dims,
            water: states[s].clone(),
            normals: normals.clone(),
        };
        let (expected, spread) = expected_frame(device, queue, &setup, frame, &fields, &traces[k], foam_field::WATER_DEPTH.0);
        let gpu = read_texture(device, queue, &traces[k + 1], 0);
        compare_frame(&gpu, &expected, &spread, &format!("foam frame {k}"));
        peak = gpu.iter().fold(peak, |m, v| m.max(v[0]));
    }
    assert!(peak > 0.05, "fixture must exercise emission and advection, peak {peak}");
    // Changing the generation depth keeps old foam and changes only the next
    // frame's emission; queued native/default frames above stay independent.
    let mut encoder = device.create_command_encoder(&Default::default());
    let previous = trace(device, &mut encoder, &field);
    field.set_minimum_depth(2.5);
    let frame = Frame::new(0.016, 86400.266);
    field.encode_frame(device, &mut encoder, 0, frame).unwrap();
    let result = trace(device, &mut encoder, &field);
    queue.submit(Some(encoder.finish()));
    let fields = CpuFields { dimensions: dims, water: states[0].clone(), normals: normals.clone() };
    let (expected, spread) = expected_frame(device, queue, &setup, frame, &fields, &previous, foam_field::authored_depth_range(2.5));
    compare_frame(&read_texture(device, queue, &result, 0), &expected, &spread, "live authored shallow foam depth");
    // Frozen hydraulics keep only the final state's binding.
    field.retain_state(0);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(field.encode_frame(device, &mut encoder, 1, frames[0].1).is_err());
    assert!(field.encode_frame(device, &mut encoder, 0, Frame::new(f32::NAN, 0.0)).is_err());
    drop(state);
    field.encode_frame(device, &mut encoder, 0, frames[1].1).unwrap();
    field.encode_reset(&mut encoder);
    queue.submit(Some(encoder.finish()));
    assert!(read_texture(device, queue, field.foam(), 0).iter().all(|v| *v == [0.0; 4]));
    let mut bad = setup;
    bad.dimensions = [12, 11];
    let state = [0, 1].map(|i| state_texture(device, queue, dims, &states[i]));
    assert!(Field::new(
        device,
        FieldInputs {
            state: &state,
            normal_grid: &normal_grid,
            blocks: None,
        },
        &bad,
    )
    .is_err());
}

#[test]
fn river_foam_minimum_depth_preserves_native_ratio_and_can_allow_shallow_rapids() {
    assert_eq!(foam_field::authored_depth_range(150.0), [150.0, 500.0]);
    let inputs = foam_field::EmissionInputs { curl: 3.0, curvature: 0.05, velocity: [300.0, 0.0], water_depth: 10.0, depth_difference: 0.0 };
    assert_eq!(foam_field::emit_amount(inputs), 0.0);
    assert!(foam_field::emit_amount_with_depth_range(inputs, foam_field::authored_depth_range(2.5)) > 0.0);
    assert!(foam_field::emit_amount_with_depth_range(inputs, foam_field::authored_depth_range(0.0)).is_finite());
    assert_eq!(foam_field::emit_amount_with_depth_range(foam_field::EmissionInputs { curl: 0.0, curvature: 0.0, ..inputs }, foam_field::authored_depth_range(0.0)), 0.0, "removing the depth gate must still require turbulence");
}

/// Wiring against the real Simulation arrays and material NormalGrid.
pub fn check_simulation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    simulation: &crate::river_gpu::Simulation,
    normal_grid: &wgpu::Texture,
    dx: f32,
) {
    let dims = [normal_grid.width(), normal_grid.height()];
    let world = dims.map(|n| n as f32 * dx);
    let grid = jka_water::river::control::Grid::max_axis(world, dims[0].max(dims[1])).unwrap();
    assert_eq!(grid.dimensions, dims);
    let setup = Setup::native(
        &grid,
        Placement::unrotated([0.0, 0.0, 0.0], &grid).unwrap(),
        dims.map(|n| n as f32),
    );
    let mut field = Field::new(
        device,
        FieldInputs {
            state: simulation.state_textures(),
            normal_grid,
            blocks: None,
        },
        &setup,
    )
    .unwrap();
    let index = simulation.state_index();
    let frame = Frame::new(0.033, 12.5);
    let mut encoder = device.create_command_encoder(&Default::default());
    field.encode_reset(&mut encoder);
    let previous = trace(device, &mut encoder, &field);
    field.encode_frame(device, &mut encoder, index, frame).unwrap();
    queue.submit(Some(encoder.finish()));
    let fields = CpuFields {
        dimensions: dims,
        water: super::read_state(device, queue, simulation.state()),
        normals: read_texture(device, queue, normal_grid, 0),
    };
    let (expected, spread) = expected_frame(device, queue, &setup, frame, &fields, &previous, foam_field::WATER_DEPTH.0);
    compare_frame(&read_texture(device, queue, field.foam(), 0), &expected, &spread, "real river foam");
}
