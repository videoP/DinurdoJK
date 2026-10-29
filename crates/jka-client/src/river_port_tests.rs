//! River-port GPU checks run through the client's existing dependency feature
//! set, so a focused water test does not rebuild a second graphics configuration.
use crate::river_gpu::{BoundaryInput, Kernel, Simulation};
use jka_water::river::boundary::{self, Controls, SourceBox, WaterInfo};
use jka_water::river::{
    stages, transport::Field, Geometry, Parameters, Presentation, Transport, Water, SHALLOW_SHADER,
};
#[path = "river_grid_mesh_tests.rs"]
mod grid_mesh_tests;
#[path = "river_material_tests.rs"]
mod material_tests;
#[path = "river_ocean_tests.rs"]
mod ocean_tests;
#[path = "river_sampling_tests.rs"]
mod sampling;
#[path = "river_surface_tests.rs"]
mod surface_tests;
#[path = "river_water_info_tests.rs"]
mod water_info_tests;
#[path = "river_wave_tests.rs"]
mod wave_tests;

fn read_state(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture) -> Vec<[f32; 4]> {
    assert_eq!(source.format(), wgpu::TextureFormat::R32Float);
    let dimensions = source.size();
    let row = (dimensions.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river test planar texture readback"),
        size: u64::from(row) * u64::from(dimensions.height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        source.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(dimensions.height),
            },
        },
        dimensions,
    );
    queue.submit(Some(encoder.finish()));
    let floats: Vec<_> = read_buffer(device, queue, &buffer)
        .into_iter()
        .flatten()
        .collect();
    (0..dimensions.width * dimensions.height)
        .map(|i| {
            let x = i % dimensions.width;
            let y = i / dimensions.width;
            std::array::from_fn(|c| {
                floats[(c * dimensions.height as usize + y as usize) * (row / 4) as usize
                    + x as usize]
            })
        })
        .collect()
}

#[test]
fn river_shader_validates() {
    let wave_vertex = jka_water::river::wave_advection::vertex_shader();
    for shader in [
        SHALLOW_SHADER,
        jka_water::river::capture::PREPARE_SHADER,
        crate::river_capture::RASTER_SHADER,
        crate::river_water_info_capture::SHADER,
        jka_water::river::grid_mesh::SHADER,
        jka_water::river::material::SHADER,
        jka_water::river::material::OUTPUT_SHADER,
        jka_water::river::wave_advection::SHADER,
        jka_water::river::ocean::SHADER,
        &wave_vertex,
    ] {
        let module = wgpu::naga::front::wgsl::parse_str(shader).expect("river WGSL parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("river WGSL validates");
    }
    for precision in [
        crate::river_water_info::Precision::Half,
        crate::river_water_info::Precision::Full,
    ] {
        let module = wgpu::naga::front::wgsl::parse_str(&precision.shader())
            .expect("native WaterInfo WGSL parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("native WaterInfo WGSL validates");
    }
}

#[test]
fn river_initialization_matches_serialized_niagara_graph() {
    use jka_water::river::control::{Controls as StepControls, Frame, Grid};
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../jka-water/src/river/control-reference.json"
    ))
    .unwrap();
    for row in data["vectors"].as_array().unwrap() {
        let dx = row["dx"].as_f64().unwrap() as f32;
        let grid = Grid::max_axis([dx * 16.0, dx * 8.0], 16).unwrap();
        let frame = Frame::new(
            grid,
            StepControls {
                substeps: row["substeps"].as_u64().unwrap() as u32,
                speed: row["speed"].as_f64().unwrap() as f32,
                ..StepControls::default()
            },
            0.016,
        )
        .unwrap();
        let p = frame.parameters();
        for (key, value) in [
            ("Emitter.SimDt", p.dt),
            ("Emitter.dx", p.dx),
            ("Emitter.DtOverDx", p.dt_over_dx()),
            ("Emitter.MaxDepth", p.max_depth),
            ("Emitter.MaxVelocity", p.max_velocity),
        ] {
            let expected = row["outputs"][key].as_f64().unwrap() as f32;
            assert!(
                (value - expected).abs() <= expected.abs() * 2e-6,
                "{key}: {value} != serialized-graph {expected}"
            );
        }
    }
}

fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<[f32; 4]> {
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("river test readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &read, 0, source.size());
    queue.submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    read.slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(20)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = read.slice(..).get_mapped_range().unwrap();
    mapped
        .chunks_exact(16)
        .map(|cell| {
            std::array::from_fn(|i| f32::from_le_bytes(cell[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect()
}

fn channels(water: &[Water]) -> Vec<[f32; 4]> {
    water
        .iter()
        .map(|w| [w.height, w.depth, w.velocity[0], w.velocity[1]])
        .collect()
}

fn compare(actual: &[[f32; 4]], expected: &[[f32; 4]], label: &str) {
    assert_eq!(actual.len(), expected.len());
    for (cell, (a, b)) in actual.iter().zip(expected).enumerate() {
        for (channel, (&value, &reference)) in a.iter().zip(b).enumerate() {
            let tolerance = 0.0003 + reference.abs() * 0.00003;
            assert!(
                value.is_finite() && (value - reference).abs() <= tolerance,
                "{label} cell {cell} channel {channel}: GPU {value}, CPU {reference}"
            );
        }
    }
}

fn reference_step(
    kernel: Kernel,
    dimensions: [usize; 2],
    water: &[Water],
    geo: &[Geometry],
    previous: &[f32],
    p: Parameters,
    t: Transport,
    render: Presentation,
    boundary: BoundaryInput<'_>,
    oracle: &sampling::Oracle<'_>,
) -> (Vec<Water>, Vec<Water>, Vec<[f32; 4]>) {
    let [nx, ny] = dimensions;
    let queries = std::cell::RefCell::new(Vec::new());
    if kernel == Kernel::AdvectWater {
        let collect = |uv: [f32; 2], channel: usize| {
            queries
                .borrow_mut()
                .push([uv[0], uv[1], channel as f32, 0.0]);
            0.0
        };
        let field = Field::new(nx, ny, water, geo)
            .unwrap()
            .with_filtered_sampler(&collect);
        for y in 0..ny as i32 {
            for x in 0..nx as i32 {
                field.advect(x, y, previous[y as usize * nx + x as usize], p, t);
            }
        }
    }
    let queries = queries.into_inner();
    let samples = oracle.samples(dimensions, water, &queries);
    let next = std::cell::Cell::new(0);
    let filtered = |uv: [f32; 2], channel: usize| {
        let i = next.get();
        assert_eq!(queries[i], [uv[0], uv[1], channel as f32, 0.0]);
        next.set(i + 1);
        samples[i]
    };
    let field = Field::new(nx, ny, water, geo)
        .unwrap()
        .with_filtered_sampler(&filtered);
    let mut output = Vec::new();
    let mut surface = Vec::new();
    let mut normals = Vec::new();
    for y in 0..ny as i32 {
        for x in 0..nx as i32 {
            let w = field.water(x, y);
            let info = boundary.water_info[y as usize * nx + x as usize];
            output.push(match kernel {
                Kernel::SourceAndSink => boundary::apply(
                    w,
                    field.geometry(x, y),
                    info,
                    [
                        boundary.controls.grid_min[0] + (x as f32 + 0.5) * p.dx,
                        boundary.controls.grid_min[1] + (y as f32 + 0.5) * p.dx,
                        boundary.controls.grid_min[2],
                    ],
                    boundary.boxes,
                    boundary.controls,
                ),
                Kernel::DistributeOverlap => {
                    let depth = w.depth.max(0.0).min(p.max_depth);
                    Water {
                        height: field.geometry(x, y).bottom + depth,
                        depth,
                        ..w
                    }
                }
                Kernel::MatchSpline => {
                    boundary::match_spline(w, field.geometry(x, y), info, p.dt, boundary.controls)
                }
                Kernel::ExtrapolateVelocity => field.extrapolate_velocity(x, y, t),
                Kernel::AdvectWater => {
                    field.advect(x, y, previous[y as usize * nx + x as usize], p, t)
                }
                Kernel::ExtrapolateHeight => field.extrapolate_height(x, y, t),
                Kernel::UpdateVelocity => Water {
                    velocity: stages::velocity(
                        stages::VelocityStencil {
                            water: w,
                            geometry: field.geometry(x, y),
                            left_water: field.water(x - 1, y),
                            left_geometry: field.geometry(x - 1, y),
                            down_water: field.water(x, y - 1),
                            down_geometry: field.geometry(x, y - 1),
                        },
                        p,
                    ),
                    ..w
                },
                Kernel::UpdateDepth => stages::depth(
                    stages::DepthStencil {
                        water: w,
                        geometry: field.geometry(x, y),
                        depth: [
                            field.water(x - 1, y).depth,
                            field.water(x + 1, y).depth,
                            field.water(x, y - 1).depth,
                            field.water(x, y + 1).depth,
                        ],
                        depth_two: [
                            field.water(x - 2, y).depth,
                            field.water(x + 2, y).depth,
                            field.water(x, y - 2).depth,
                            field.water(x, y + 2).depth,
                        ],
                        right_velocity_x: field.water(x + 1, y).velocity[0],
                        up_velocity_y: field.water(x, y + 1).velocity[1],
                    },
                    p,
                ),
                Kernel::PrepareSurface => {
                    let (state, visible) = field.present(x, y, render);
                    surface.push(visible);
                    state
                }
                Kernel::ComputeNormalsAndReset => {
                    let (state, normal) = field.normal_and_reset(x, y, p.dx);
                    normals.push(normal);
                    state
                }
            });
        }
    }
    assert_eq!(next.get(), queries.len());
    (output, surface, normals)
}

fn check_backend(backend: wgpu::Backends) {
    futures_lite::future::block_on(async {
        let mut descriptor = wgpu::InstanceDescriptor {
            backends: backend,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        };
        if backend == wgpu::Backends::DX12 {
            descriptor.backend_options.dx12.shader_compiler =
                if let Some(path) = std::env::var_os("RIVER_DXC_PATH") {
                    let path = std::path::PathBuf::from(path);
                    assert!(
                        path.is_absolute() && path.is_file(),
                        "RIVER_DXC_PATH must identify the compiler DLL explicitly"
                    );
                    std::eprintln!("river DX12 compiler: DXC {}", path.display());
                    wgpu::Dx12Compiler::DynamicDxc {
                        dxc_path: path.to_string_lossy().into_owned(),
                    }
                } else {
                    std::eprintln!("river DX12 compiler: Windows FXC");
                    wgpu::Dx12Compiler::Fxc
                };
        }
        let instance = wgpu::Instance::new(descriptor);
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("requested GPU backend");
        std::eprintln!("river stage check: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::FLOAT32_FILTERABLE,
                // Offscreen normal diagnostics retain three full-precision
                // outputs so typed stores cannot hide sampling differences.
                required_limits: wgpu::Limits {
                    max_color_attachment_bytes_per_sample: 48,
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        water_info_tests::check(&device, &queue);
        grid_mesh_tests::check(&device, &queue);
        material_tests::check(&device, &queue);
        wave_tests::check(&device, &queue);
        surface_tests::check(&device, &queue);
        ocean_tests::check(&device, &queue);
        let oracle = sampling::Oracle::new(&device, &queue);
        oracle.check_exact_centres_edges_and_layers();
        check_filter_fallbacks(&device, &queue, &oracle);
        let n = 13usize;
        let mut water = Vec::new();
        let mut geometry = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let bottom =
                    (x as f32 * 0.3).sin() * 20.0 + if x == 5 && y > 2 { 180.0 } else { 0.0 };
                let dry = (1..=3).contains(&x) && (1..=3).contains(&y);
                let depth = if dry {
                    0.0
                } else {
                    80.0 + ((x * 7 + y * 13) % 40) as f32
                };
                water.push(Water {
                    height: bottom + depth,
                    depth,
                    velocity: [
                        ((x * 19 + y * 3) % 90) as f32 - 45.0,
                        ((y * 17 + x * 5) % 60) as f32 - 30.0,
                    ],
                });
                geometry.push(Geometry {
                    bottom,
                    collider: if x == 5 && y > 2 { 1.0 } else { 0.0 },
                    collision_velocity: [3.0, -2.0],
                });
            }
        }
        let previous: Vec<_> = geometry.iter().map(|g| g.bottom).collect();
        let p = Parameters::cached_river_variant(20.0, 0.02, 980.0, 150.0, 1000.0);
        let t = Transport::default();
        let render = Presentation {
            half_width: 3,
            height_cutoff: 50.0,
            ..Presentation::default()
        };
        let info: Vec<_> = geometry
            .iter()
            .enumerate()
            .map(|(i, g)| WaterInfo {
                height: g.bottom + 100.0 + (i / n % 3) as f32 * 2.0,
                depth: if i % n < 2 { 0.0 } else { 100.0 },
                flow_velocity: [30.0, 4.0],
            })
            .collect();
        let boxes = [
            SourceBox {
                position: [-105.0, -105.0, 0.0],
                size: [60.0, 40.0, 240.0],
                angle: 0.25,
                sink: false,
            },
            SourceBox {
                position: [105.0, 105.0, 0.0],
                size: [35.0, 20.0, 100.0],
                angle: 0.0,
                sink: true,
            },
        ];
        let boundary = BoundaryInput {
            water_info: &info,
            boxes: &boxes,
            controls: Controls::cached_river_variant(
                0.033,
                [-(n as f32) * p.dx / 2.0, -(n as f32) * p.dx / 2.0, 0.0],
            ),
        };
        let mut simulation = Simulation::new(
            &device,
            [n as u32; 2],
            &water,
            &geometry,
            &previous,
            p,
            t,
            render,
            boundary,
        )
        .unwrap();
        assert!(simulation.output_fields().is_none());
        simulation = match simulation.freeze() {
            Err(original) => original,
            Ok(_) => panic!("an unprepared river must not freeze or lose its evolving state"),
        };
        // Check each stage through the production ping-pong groups. This catches
        // both numeric translations and incorrect cross-stage resource wiring.
        for kernel in Kernel::ALL {
            let (expected, visible, normals) = reference_step(
                kernel,
                [n, n],
                &water,
                &geometry,
                &previous,
                p,
                t,
                render,
                boundary,
                &oracle,
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            simulation.encode_kernel(&mut encoder, kernel);
            queue.submit(Some(encoder.finish()));
            compare(
                &read_state(&device, &queue, simulation.state()),
                &channels(&expected),
                kernel.entry(),
            );
            if !visible.is_empty() {
                compare(
                    &read_buffer(&device, &queue, simulation.surface()),
                    &channels(&visible),
                    "render surface",
                );
            }
            if !normals.is_empty() {
                compare(
                    &read_buffer(&device, &queue, simulation.normals()),
                    &normals,
                    "render normals",
                );
            }
            water = expected;
        }
        assert!(simulation.surface_ready());
        let outputs = simulation.output_fields().unwrap();
        let mut material = crate::river_material::Presentation::new(&device, &outputs).unwrap();
        drop(outputs);
        // Queue two whole cycles in one submission, without CPU readbacks or
        // queue uniform rewrites between stages/cycles.
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut final_surface = Vec::new();
        let mut final_normals = Vec::new();
        for _ in 0..2 {
            simulation.encode_hydraulics_and_surface(&mut encoder);
            material.encode(&mut encoder);
            for kernel in Kernel::ALL {
                let (next, visible, normals) = reference_step(
                    kernel,
                    [n, n],
                    &water,
                    &geometry,
                    &previous,
                    p,
                    t,
                    render,
                    boundary,
                    &oracle,
                );
                water = next;
                if !visible.is_empty() {
                    final_surface = visible;
                }
                if !normals.is_empty() {
                    final_normals = normals;
                }
            }
        }
        let material_textures = material
            .freeze()
            .ok()
            .expect("encoded native material fields");
        queue.submit(Some(encoder.finish()));
        compare(
            &read_state(&device, &queue, simulation.state()),
            &channels(&water),
            "queued cycles",
        );
        let live_bytes = simulation.logical_bytes();
        let frozen = simulation
            .freeze()
            .ok()
            .expect("prepared outputs can freeze");
        assert_eq!(frozen.dimensions, [n as u32; 2]);
        assert_eq!(frozen.logical_bytes(), (n * n * 32) as u64);
        assert!(frozen.logical_bytes() < live_bytes);
        compare(
            &read_buffer(&device, &queue, &frozen.surface),
            &channels(&final_surface),
            "frozen surface",
        );
        compare(
            &read_buffer(&device, &queue, &frozen.normals),
            &final_normals,
            "frozen normals",
        );
        material_tests::check_generated_outputs(&device, &queue, &material_textures, &frozen);
        assert!(Simulation::new(&device, [0, 1], &[], &[], &[], p, t, render, boundary).is_err());
        let mut invalid = p;
        invalid.dt = f32::NAN;
        assert!(Simulation::new(
            &device,
            [n as u32; 2],
            &water,
            &geometry,
            &previous,
            invalid,
            t,
            render,
            boundary,
        )
        .is_err());
        check_capture(&device, &queue);
        assert!(scope.pop().await.is_none(), "river GPU validation failed");
    });
}

fn check_filter_fallbacks(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    oracle: &sampling::Oracle<'_>,
) {
    // This peaked profile overshoots under Basic2DCubic at x=3.3. The source
    // must fall back to the hardware sample, including near/outside the edge.
    let dimensions = [10usize, 9];
    let water: Vec<_> = (0..90)
        .map(|i| {
            let depth = match i % 10 {
                3 => 1.0,
                4 => 0.9,
                _ => 0.25,
            };
            Water {
                height: depth,
                depth,
                velocity: [-3.0, 0.0],
            }
        })
        .collect();
    let geometry = vec![Geometry::default(); 90];
    let info = vec![WaterInfo::default(); 90];
    let previous = vec![0.0; 90];
    let p = Parameters::cached_river_variant(1.0, 0.1, 980.0, 100.0, 100.0);
    let boundary = BoundaryInput {
        water_info: &info,
        boxes: &[],
        controls: Controls::cached_river_variant(0.016, [-5.0, -4.5, 0.0]),
    };
    let t = Transport::default();
    let render = Presentation::default();
    let expected = reference_step(
        Kernel::AdvectWater,
        dimensions,
        &water,
        &geometry,
        &previous,
        p,
        t,
        render,
        boundary,
        oracle,
    )
    .0;
    let direct = oracle.samples(dimensions, &water, &[[0.38, 0.5, 1.0, 0.0]])[0];
    assert_eq!(
        expected[43].depth, direct,
        "overshoot must select the hardware bilinear value"
    );
    assert!(direct > 0.9 && direct < 1.0);
    let mut simulation = Simulation::new(
        device,
        [10, 9],
        &water,
        &geometry,
        &previous,
        p,
        t,
        render,
        boundary,
    )
    .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    simulation.encode_kernel(&mut encoder, Kernel::AdvectWater);
    queue.submit(Some(encoder.finish()));
    compare(
        &read_state(device, queue, simulation.state()),
        &channels(&expected),
        "hardware-filtered overshoot/edge fallback",
    );
}

fn check_capture(device: &wgpu::Device, queue: &wgpu::Queue) {
    let oracle = sampling::Oracle::new(device, queue);
    use crate::river_capture::{DepthCaptures, Mesh, MeshClass, Sides};
    use jka_water::river::capture::{self, Camera, Channel, Settings};
    let settings = Settings {
        dimensions: [8, 6],
        world_size: [8.0, 6.0],
        capture_width: 8.0,
        dx: 1.0,
        bottom_depth: -20.0,
        dilation: 1.0,
        cameras: std::array::from_fn(|i| Camera {
            height: if Channel::ALL[i].top_down() {
                100.0
            } else {
                -100.0
            },
            near: 0.0,
            far: 1000.0,
        }),
    };
    let resolution = 16u32;
    let mut captures = DepthCaptures::new(device, resolution, 8.0, settings.cameras).unwrap();
    assert_eq!(captures.logical_bytes(), 16 * 16 * 6 * 4);
    assert!(DepthCaptures::new(device, 0, 8.0, settings.cameras).is_err());
    let terrain = [
        [-4.0, -4.0, -11.0],
        [4.0, -4.0, -9.0],
        [4.0, 4.0, -9.0],
        [-4.0, 4.0, -11.0],
    ];
    let top_indices = [0, 1, 2, 0, 2, 3];
    let bridge = [
        [-1.0, -1.0, 8.0],
        [1.0, -1.0, 8.0],
        [1.0, 1.0, 8.0],
        [-1.0, 1.0, 8.0],
        [-1.0, -1.0, 5.0],
        [1.0, -1.0, 5.0],
        [1.0, 1.0, 5.0],
        [-1.0, 1.0, 5.0],
    ];
    // Underside winds the other way. A single-sided capture must select top
    // from above and underside from below, not just the closest arbitrary face.
    let bridge_indices = [0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6];
    let dilated = [
        [1.0, 0.0, 12.0],
        [2.0, 0.0, 12.0],
        [2.0, 1.0, 12.0],
        [1.0, 1.0, 12.0],
        [1.0, 0.0, 9.0],
        [2.0, 0.0, 9.0],
        [2.0, 1.0, 9.0],
        [1.0, 1.0, 9.0],
    ];
    let meshes = [
        Mesh {
            class: MeshClass::Landscape,
            positions: &terrain,
            indices: &top_indices,
            sides: Sides::CounterClockwise,
        },
        Mesh {
            class: MeshClass::Actors,
            positions: &bridge,
            indices: &bridge_indices,
            sides: Sides::CounterClockwise,
        },
        Mesh {
            class: MeshClass::DilatedActors,
            positions: &dilated,
            indices: &bridge_indices,
            sides: Sides::TwoSided,
        },
    ];
    let mut images: [Vec<f32>; 5] =
        std::array::from_fn(|_| vec![1000.0; (resolution * resolution) as usize]);
    for y in 0..resolution {
        for x in 0..resolution {
            let wx = (x as f32 + 0.5) * 8.0 / resolution as f32 - 4.0;
            let wy = (y as f32 + 0.5) * 8.0 / resolution as f32 - 4.0;
            let i = (y * resolution + x) as usize;
            images[0][i] = 110.0 - wx * 0.25;
            if wx.abs() < 1.0 && wy.abs() < 1.0 {
                images[1][i] = 92.0;
                images[2][i] = 105.0;
            }
            if wx > 1.0 && wx < 2.0 && wy > 0.0 && wy < 1.0 {
                images[3][i] = 88.0;
                images[4][i] = 109.0;
            }
        }
    }
    let contours = capture::project(
        settings,
        resolution,
        std::array::from_fn(|i| images[i].as_slice()),
    )
    .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    captures
        .encode_capture(device, &mut encoder, &meshes)
        .unwrap();
    // Same five captures, two distinct water levels and dilation settings.
    // Preparation and all solver stages queue together, with no CPU readback.
    let mut results = Vec::new();
    for level in [4.0, 10.0] {
        let local = Settings {
            dilation: if level < 5.0 { 0.0 } else { 1.0 },
            ..settings
        };
        let info = vec![
            WaterInfo {
                height: level,
                depth: 50.0,
                flow_velocity: [0.0; 2]
            };
            48
        ];
        let expected_geo = capture::geometry(local, &contours, &vec![level; 48]).unwrap();
        use jka_water::river::water_body;
        let body = water_body::from_evaluated_samples(
            &[0.0, 20.0].map(|distance| water_body::Sample {
                distance,
                position: [f64::from(distance) - 10.0, 0.0, f64::from(level)],
                tangent: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                width: 20.0,
                velocity: 0.0,
            }),
            water_body::Settings {
                dilation: 0.0,
                relative_yaw_degrees: 0.0,
                maximum_velocity: 1024.0,
            },
        )
        .unwrap();
        let ground = [
            [-8.0, -8.0, level - 50.0],
            [8.0, -8.0, level - 50.0],
            [8.0, 8.0, level - 50.0],
            [-8.0, 8.0, level - 50.0],
        ];
        let field = crate::river_water_info_capture::encode(
            device,
            &mut encoder,
            crate::river_water_info::Settings {
                dimensions: [8, 6],
                simulation_dimensions: [8, 6],
                zone: jka_water::river::water_info::Zone {
                    water_min: -64.0,
                    water_max: 64.0,
                    ground_min: -64.0,
                    capture_z: 128.0,
                    underground_offset: 64.0,
                },
                origin_z: 0.0,
                grid_min: [-4.0, -3.0],
                grid_size: [8.0, 6.0],
                zone_min: [-4.0, -3.0],
                zone_size: [8.0, 6.0],
                blur_radius: 1,
                precision: crate::river_water_info::Precision::Half,
            },
            [0.0; 3],
            &[crate::river_water_info_capture::Ground {
                positions: &ground,
                indices: &[0, 1, 2, 0, 2, 3],
                sides: crate::river_capture::Sides::CounterClockwise,
            }],
            &[crate::river_water_info_capture::Body {
                mesh: &body,
                local_to_capture: crate::river_water_info_capture::IDENTITY,
            }],
        )
        .unwrap();
        let bed = captures
            .encode_prepare_field(device, &mut encoder, local, &field)
            .unwrap();
        assert_eq!(bed.logical_bytes(), 48 * 20);
        let geo_buffer = bed.geometry().clone();
        let boxes = [
            SourceBox {
                position: [0.0; 3],
                size: [8.0, 6.0, 40.0],
                angle: 0.0,
                sink: false,
            },
            SourceBox {
                position: [0.0; 3],
                size: [2.0, 2.0, 100.0],
                angle: 0.3,
                sink: true,
            },
        ];
        let mut controls = Controls::cached_river_variant(0.016, [-4.0, -3.0, 0.0]);
        controls.match_spline = false;
        let boundary = BoundaryInput {
            water_info: &info,
            boxes: &boxes,
            controls,
        };
        use jka_water::river::control::{Controls as StepControls, Frame, Grid};
        let grid = Grid::max_axis([8.0, 6.0], 8).unwrap();
        let step_controls = StepControls {
            substeps: 3,
            speed: 0.25,
            ..StepControls::default()
        };
        let frame = Frame::new(grid, step_controls, controls.frame_dt).unwrap();
        let p = frame.parameters();
        let render = Presentation {
            half_width: 1,
            ..Presentation::default()
        };
        let mut simulation = Simulation::from_prepared_field(
            device,
            bed,
            field,
            p,
            Transport::default(),
            render,
            &boxes,
            controls,
        )
        .unwrap();
        let tracing = std::env::var_os("RIVER_TRACE_STAGES").is_some();
        if tracing {
            simulation.enable_trace(device);
        }
        let mut expected_stages = Vec::new();
        let mut water = vec![Water::default(); 48];
        let mut previous = vec![0.0; 48];
        let different = Frame::new(
            Grid::max_axis([16.0, 12.0], 8).unwrap(),
            StepControls {
                speed: 0.5,
                ..step_controls
            },
            controls.frame_dt,
        )
        .unwrap();
        assert!(simulation
            .encode_reference_frame(device, &mut encoder, different)
            .is_err());
        for iteration in 0..2 {
            let frame = if iteration == 0 {
                frame
            } else {
                Frame::new(
                    grid,
                    StepControls {
                        substeps: 4,
                        speed: 0.4,
                        ..step_controls
                    },
                    0.033,
                )
                .unwrap()
            };
            let p = frame.parameters();
            let boundary = BoundaryInput {
                controls: Controls {
                    frame_dt: frame.engine_delta(),
                    ..controls
                },
                ..boundary
            };
            simulation
                .encode_reference_frame(device, &mut encoder, frame)
                .unwrap();
            // Serialized emitter loops are 0..7 once, 8..12 NumSubsteps times,
            // then 13..17 once. Source/sink must not repeat inside the core.
            let schedule = [Kernel::SourceAndSink, Kernel::DistributeOverlap]
                .into_iter()
                .chain((0..frame.substeps()).flat_map(|_| {
                    [
                        Kernel::ExtrapolateVelocity,
                        Kernel::AdvectWater,
                        Kernel::ExtrapolateHeight,
                        Kernel::UpdateVelocity,
                        Kernel::UpdateDepth,
                        Kernel::MatchSpline,
                    ]
                }))
                .chain([Kernel::PrepareSurface, Kernel::ComputeNormalsAndReset]);
            for kernel in schedule {
                water = reference_step(
                    kernel,
                    [8, 6],
                    &water,
                    &expected_geo,
                    &previous,
                    p,
                    Transport::default(),
                    render,
                    boundary,
                    &oracle,
                )
                .0;
                if tracing {
                    expected_stages.push((kernel, water.clone()));
                }
            }
            previous = expected_geo.iter().map(|g| g.bottom).collect();
        }
        let expected_geometry = expected_geo
            .iter()
            .map(|g| {
                [
                    g.bottom,
                    g.collider,
                    g.collision_velocity[0],
                    g.collision_velocity[1],
                ]
            })
            .collect::<Vec<_>>();
        results.push((
            level,
            geo_buffer,
            expected_geometry,
            simulation,
            water,
            expected_stages,
        ));
    }
    queue.submit(Some(encoder.finish()));
    for (level, geo_buffer, expected_geo, mut simulation, water, expected_stages) in results {
        let trace = simulation.take_trace();
        assert_eq!(trace.len(), expected_stages.len());
        for (stage, ((kernel, buffer), (expected_kernel, expected))) in
            trace.into_iter().zip(expected_stages).enumerate()
        {
            assert_eq!(kernel, expected_kernel);
            let actual = read_state(device, queue, &buffer);
            compare(
                &actual,
                &channels(&expected),
                &format!("level {level}, traced stage {stage} {kernel:?}"),
            );
        }
        compare(
            &read_buffer(device, queue, &geo_buffer),
            &expected_geo,
            &format!("captured bed at water level {level}"),
        );
        compare(
            &read_state(device, queue, simulation.state()),
            &channels(&water),
            "capture into solver without readback",
        );
    }
}

#[test]
fn river_stages_match_cpu_reference_on_vulkan() {
    check_backend(wgpu::Backends::VULKAN);
}

#[test]
fn river_stages_match_cpu_reference_on_dx12() {
    check_backend(wgpu::Backends::DX12);
}

#[test]
#[ignore = "requires the local Tsangpo acceptance BSP; override TSANGPO_BSP to relocate it"]
fn tsangpo_map_resolves_real_river_metadata() {
    use jka_assets::bsp::river::{Inlet, Outlet};
    let path = std::env::var_os("TSANGPO_BSP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"C:\Games\JKA\gClient\base\maps\Tsangpo.bsp".into());
    let bsp =
        jka_assets::bsp::Bsp::parse(&std::fs::read(path).expect("Tsangpo acceptance BSP")).unwrap();
    let rivers = bsp.rivers();
    assert!(rivers.diagnostics.is_empty(), "{:?}", rivers.diagnostics);
    assert_eq!(rivers.rivers.len(), 1);
    let river = &rivers.rivers[0];
    assert_eq!(river.name, "tsangpo");
    assert_eq!(river.settings.cell_size, 32.0);
    assert_eq!(river.settings.japro_friction, 0.03);
    assert_eq!(river.sources.len(), 1);
    assert_eq!(river.sinks.len(), 1);
    assert_eq!(river.sources[0].inlet, Inlet::DischargeCfs(30000.0));
    assert_eq!(river.sinks[0].outlet, Outlet::Drain { rate: 0.2 });
    let mapped = crate::river_map::MapRiver::from_bsp(&bsp, river, 512)
        .expect("Tsangpo native authoring adapter");
    assert_eq!(mapped.domain.grid.dimensions, [512, 241]);
    assert_eq!(mapped.domain.origin[2], -560.0);
    assert_eq!(mapped.sources[0].sections_map.len(), 1);
    assert_eq!(mapped.sinks[0].sections_map.len(), 2);
    assert!((mapped.sources[0].endpoint.width - 2740.2547).abs() < 0.01);
    assert!((mapped.sinks[0].endpoint.width - 5938.6294).abs() < 0.01);
    assert_eq!(
        mapped.sources[0].requested,
        crate::river_map::RequestedInlet::DischargeCm3PerSecond(30000.0 * 28316.846592)
    );
    let boxes = mapped.height_boundary_boxes().unwrap();
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[0].position[2] + boxes[0].size[2] * 0.5, 14160.0);
    assert!(boxes[0].position[2] - boxes[0].size[2] * 0.5 < 0.0);
    assert!(boxes[0].size[0] > 0.0 && boxes[1].size[0] > 0.0);
    std::eprintln!("Tsangpo native endpoint widths {} / {} cm; source height {} cm local; requested CFS retained, not enforced by native height mode",boxes[0].size[0],boxes[1].size[0],boxes[0].position[2]+boxes[0].size[2]*0.5);
    std::eprintln!("Tsangpo river: {river:#?}");
    check_tsangpo_capture(&bsp, river);
}

/// Actual world geometry and explicit map material classification. This is a
/// capture acceptance check, not a hydraulic solve with invented spline data.
fn check_tsangpo_capture(bsp: &jka_assets::bsp::Bsp, river: &jka_assets::bsp::river::River) {
    use crate::river_capture::{BspCapture, DepthCaptures, MeshClass, Selection, Sides};
    use jka_water::river::{
        capture::{Camera, Channel, Settings},
        CENTIMETRES_PER_MAP_UNIT,
    };
    let world = bsp.world_mesh(4).unwrap();
    // These are Tsangpo's explicit terrain materials, inspected from the BSP.
    // Rock is an obstacle; sky/caulk/nodraw/river trigger geometry is excluded.
    let names = [
        "textures/tsangpo/terrain",
        "textures/tsangpo/terrain_3",
        "textures/tsangpo/terrain_0to1",
        "textures/tsangpo/terrain_1to3",
        "textures/tsangpo/terrain_1",
        "textures/tsangpo/terrain_0",
        "textures/tsangpo/terrain_0to2",
        "textures/tsangpo/terrain_2",
        "textures/tsangpo/terrain_0to3",
        "textures/tsangpo/terrain_1to2",
    ];
    let mut selection: Vec<_> = names
        .iter()
        .map(|shader| Selection {
            shader,
            class: MeshClass::Landscape,
            sides: Sides::Clockwise,
        })
        .collect();
    selection.push(Selection {
        shader: "textures/yavin/rock2",
        class: MeshClass::Actors,
        sides: Sides::Clockwise,
    });
    let mapped = crate::river_map::MapRiver::from_bsp(bsp, river, 512).unwrap();
    let origin = mapped
        .domain
        .origin
        .map(|v| (v / f64::from(CENTIMETRES_PER_MAP_UNIT)) as f32);
    let capture = BspCapture::from_world(bsp, &world, origin, &selection).unwrap();
    assert!(BspCapture::from_world(bsp, &world, origin, &[]).is_err());
    let extent = mapped.domain.grid.world_size;
    let dx = mapped.domain.grid.cell_size[0];
    let dimensions = mapped.domain.grid.dimensions;
    let count = (dimensions[0] * dimensions[1]) as usize;
    let [lo, hi] = capture.vertical_bounds();
    let headroom = river.settings.capture_headroom * CENTIMETRES_PER_MAP_UNIT;
    let cameras = std::array::from_fn(|i| Camera {
        height: if Channel::ALL[i].top_down() {
            hi + headroom
        } else {
            lo - headroom
        },
        near: 0.0,
        far: hi - lo + 2.0 * headroom,
    });
    let settings = Settings {
        dimensions,
        world_size: extent,
        capture_width: extent[0].max(extent[1]),
        dx,
        bottom_depth: lo - headroom,
        dilation: 0.0,
        cameras,
    };
    let info = vec![
        WaterInfo {
            height: 0.0,
            ..WaterInfo::default()
        };
        count
    ];
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut depths = DepthCaptures::new(&device, 512, settings.capture_width, cameras).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(depths
            .encode_prepare(&device, &mut encoder, settings, &info)
            .is_err());
        depths
            .encode_capture(&device, &mut encoder, &capture.meshes())
            .unwrap();
        let bed = depths
            .encode_prepare(&device, &mut encoder, settings, &info)
            .unwrap();
        queue.submit(Some(encoder.finish()));
        let geometry = read_buffer(&device, &queue, bed.geometry());
        assert!(geometry.iter().flatten().all(|v| v.is_finite()));
        let covered = geometry
            .iter()
            .filter(|g| g[0] > settings.floor() + 1.0)
            .count();
        assert!(
            covered > count / 4,
            "Tsangpo capture unexpectedly empty: {covered}/{count}"
        );
        let [bottom, top] = geometry
            .iter()
            .filter(|g| g[0] > settings.floor() + 1.0)
            .fold([f32::INFINITY, f32::NEG_INFINITY], |[a, b], g| {
                [a.min(g[0]), b.max(g[0])]
            });
        assert!(
            top - bottom > 1000.0,
            "Tsangpo terrain lost its elevation range"
        );
        std::eprintln!("Tsangpo actual-world capture: {}x{}, {covered}/{count} covered cells, local bed {bottom}..{top} cm; fixed diagnostic WaterInfo height=0, no hydraulic solve",dimensions[0],dimensions[1]);
        assert!(
            scope.pop().await.is_none(),
            "Tsangpo capture GPU validation failed"
        );
    });
}
