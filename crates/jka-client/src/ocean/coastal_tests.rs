use super::*;
use jka_assets::bsp::bathymetry::{Cell, Face, Grid};

fn tiled_bank() -> (Atlas, PromotedOcean) {
    let tile = |y: f32| {
        let mut cells = Vec::new();
        for _ in 0..16 {
            for x in 0..16 {
                let bed = if x < 5 {
                    -80.0
                } else {
                    (x as f32 - 10.0) * 4.0
                };
                cells.push(Cell {
                    class: if bed < 0.0 { Class::Wet } else { Class::Dry },
                    bed,
                    depth: (-bed).max(0.0),
                    bed_min: bed,
                    bed_max: bed,
                    ceiling: f32::NAN,
                    coverage: 5,
                    component: 1,
                });
            }
        }
        let mut fx = vec![
            Face {
                boundary: Boundary::Open,
                crest: -80.0
            };
            17 * 16
        ];
        for row in 0..16 {
            fx[row * 17 + 10].boundary = Boundary::Bank;
        }
        Grid {
            slot: 2,
            level: 0.0,
            origin: [-256.0, y],
            size: [16, 16],
            cell_size: 16.0,
            cells,
            faces_x: fx,
            faces_y: vec![
                Face {
                    boundary: Boundary::Open,
                    crest: -80.0
                };
                16 * 17
            ],
            components: 1,
            footprint: Vec::new(),
        }
    };
    let atlas = Atlas {
        grids: vec![tile(0.0), tile(256.0)],
        ..Default::default()
    };
    let mut ocean = PromotedOcean {
        index: 2,
        name: "coastal tile fixture".into(),
        surface: super::super::OceanSurface {
            plane_height: 0.0,
            minimum: [-256.0, -512.0],
            maximum: [0.0, 0.0],
        },
        heuristic: super::super::appearance::wave_heuristic(0.0, None),
        color: [0.1, 0.2, 0.3],
        roughness: None,
        source_deformation: None, source_alpha: None,
        swell_direction: None,
    };
    ocean.surface.plane_height = 0.0;
    ocean.heuristic.wavelength = 400.0;
    (atlas, ocean)
}

#[test]
fn coastal_adapter_stitches_tiles_and_seals_unknowns_and_walls() {
    let (mut atlas, ocean) = tiled_bank();
    // The last row of the first tile and first row of the next are contiguous.
    let a = 15 * 16 + 7;
    let b = 256 + 7;
    let prepared = prepare(&atlas, &ocean).unwrap();
    assert_eq!(prepared.cells[a].neighbours[3], b as u32);
    assert_eq!(prepared.cells[b].neighbours[2], a as u32);
    assert!(prepared.cells.iter().any(|c| c.metric[3] > 0.0));
    assert!(
        prepared.cells[10].position[2] > 0.5,
        "dry bank can be wetted"
    );
    assert_eq!(prepared.depth[10], 0.0);
    assert_eq!(
        prepared.cells[0].position[2], 0.0,
        "deep FFT stays outside solver"
    );
    atlas.grids[0].faces_y[16 * 16 + 7].boundary = Boundary::Wall;
    atlas.grids[1].faces_y[7].boundary = Boundary::Wall;
    let prepared = prepare(&atlas, &ocean).unwrap();
    assert_eq!(prepared.cells[a].neighbours[3], NONE);
    assert_eq!(prepared.cells[b].neighbours[2], NONE);
    atlas.grids[0].cells[a].class = Class::NoFloor;
    atlas.grids[0].cells[a].bed = f32::NAN;
    let prepared = prepare(&atlas, &ocean).unwrap();
    assert_eq!(prepared.cells[a].position[2], 0.0);
    assert_eq!(prepared.cells[a].position[3], 0.0);
    // Negative tile coordinates use the same wrapping hash as the shader.
    let index = hash(-1, 0) as usize & (prepared.lookup.len() - 1);
    assert_eq!(prepared.lookup[index][0], -1);
}

#[test]
fn coastal_live_budget_excludes_unused_offshore_tiles_and_remaps_seams() {
    let (mut atlas, ocean) = tiled_bank();
    let reference = prepare(&atlas, &ocean).unwrap();
    // More than the old 256-tile live limit; all the extra bathymetry is deep.
    for y in 0..260 {
        let mut grid = atlas.grids[0].clone();
        grid.origin = [1024.0, y as f32 * 256.0];
        for cell in &mut grid.cells {
            cell.class = Class::Wet;
            cell.bed = -128.0;
            cell.bed_min = -128.0;
            cell.bed_max = -128.0;
            cell.depth = 128.0;
        }
        atlas.grids.push(grid);
    }
    // Put unused tiles between the two active ones to exercise remapping.
    atlas.grids.swap(1, 200);
    let packed = prepare(&atlas, &ocean).unwrap();
    assert_eq!(packed.cells.len(), 512);
    assert_eq!(packed.active, reference.active);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&packed.cells),
        bytemuck::cast_slice::<_, u8>(&reference.cells)
    );
    assert_eq!(packed.depth, reference.depth);
    assert_eq!(packed.lookup, reference.lookup);
}

#[test]
fn coastal_screened_pool_rejects_wavelength_beyond_baked_depth() {
    let (mut atlas, mut ocean) = tiled_bank();
    atlas
        .shoreline_bands
        .push(jka_assets::bsp::bathymetry::ShorelineBand {
            slot: ocean.index as u32,
            full_tiles: 5000,
            retained_tiles: 2,
            max_depth: 38.4,
            patch_radius_cells: 0,
        });
    assert!(prepare(&atlas, &ocean).is_ok());
    ocean.heuristic.wavelength = 800.0;
    assert!(prepare(&atlas, &ocean)
        .err()
        .unwrap()
        .contains("screened shoreline band"));
}

fn shallow_patch_fixture() -> (Atlas, PromotedOcean) {
    let (mut atlas, ocean) = tiled_bank();
    for grid in &mut atlas.grids {
        for cell in &mut grid.cells {
            if cell.depth > 20.0 {
                cell.bed = -16.0;
                cell.depth = 16.0;
                cell.bed_min = -16.0;
                cell.bed_max = -16.0;
            }
        }
    }
    atlas
        .shoreline_bands
        .push(jka_assets::bsp::bathymetry::ShorelineBand {
            slot: 2,
            full_tiles: 5000,
            retained_tiles: 2,
            max_depth: 38.4,
            patch_radius_cells: 2,
        });
    (atlas, ocean)
}

#[test]
fn coastal_bank_patch_has_accounted_fft_reservoir_in_shallow_water() {
    let (mut atlas, ocean) = shallow_patch_fixture();
    let prepared = prepare(&atlas, &ocean).unwrap();
    assert!(prepared.cell_indirection);
    assert_eq!(prepared.cells.len(), 160);
    assert_eq!(prepared.tiled_coverage, 512);
    let find = |x: usize| {
        let address =
            prepared.lookup[(hash(-1, 0) & prepared.lookup_mask) as usize][2] as usize + x;
        prepared.lookup[address / 4][address % 4] as u32
    };
    assert_eq!(
        find(0),
        NONE,
        "far shallows remain FFT without a solver allocation"
    );
    let ring = find(6) as usize;
    let core = find(7) as usize;
    let bank = find(10) as usize;
    assert!(
        prepared.cells[ring].metric[3] > 0.0,
        "shallow cut uses an explicit FFT source"
    );
    assert_eq!(
        prepared.cells[ring].position[3], 0.0,
        "FFT reservoir presents FFT"
    );
    assert_eq!(
        prepared.cells[ring].neighbours[1], core as u32,
        "reservoir shares a real conservative face with the patch"
    );
    assert_eq!(prepared.cells[core].neighbours[0], ring as u32);
    assert!(prepared.cells[core].position[3] > 0.0 && prepared.cells[core].position[3] < 1.0);
    assert!(
        prepared.cells[bank].position[2] > 0.5,
        "finite dry bank stays active"
    );
    for grid in &mut atlas.grids {
        for y in 0..16 {
            grid.cells[y * 16 + 5].class = Class::NoFloor;
            grid.cells[y * 16 + 5].bed = f32::NAN;
        }
    }
    assert!(
        prepare(&atlas, &ocean)
            .err()
            .unwrap()
            .contains("validated offshore FFT reservoir"),
        "unknown offshore continuation rejects the whole pool"
    );
}

#[test]
fn coastal_compact_patches_still_reject_an_oversized_live_domain() {
    let (mut atlas, ocean) = shallow_patch_fixture();
    let template = atlas.grids[0].clone();
    atlas.grids.clear();
    atlas.shoreline_bands[0].patch_radius_cells = 8;
    atlas.shoreline_bands[0].retained_tiles = 800;
    for y in 0..400 {
        let mut bank = template.clone();
        bank.origin[1] = y as f32 * 256.0;
        let mut offshore = bank.clone();
        offshore.origin[0] -= 256.0;
        for c in &mut offshore.cells {
            c.class = Class::Wet;
            c.bed = -16.0;
            c.depth = 16.0;
            c.bed_min = -16.0;
            c.bed_max = -16.0;
        }
        for face in offshore.faces_x.iter_mut().chain(&mut offshore.faces_y) {
            face.boundary = Boundary::Open;
            face.crest = -16.0;
        }
        atlas.grids.extend([offshore, bank]);
    }
    assert!(
        prepare(&atlas, &ocean)
            .err()
            .unwrap()
            .contains("live 65536-cell budget"),
        "compaction cannot raise the fixed live cell cap"
    );
}

#[test]
fn coastal_patch_rejects_a_cut_through_unbaked_source_coverage() {
    let (mut atlas, ocean) = shallow_patch_fixture();
    for grid in &mut atlas.grids {
        grid.footprint = vec![
            [[-256.0, 0.0], [0.0, 0.0], [0.0, 1024.0]],
            [[-256.0, 0.0], [0.0, 1024.0], [-256.0, 1024.0]],
        ];
    }
    assert!(
        prepare(&atlas, &ocean)
            .err()
            .unwrap()
            .contains("unbaked footprint tile"),
        "an artificial allocation edge cannot seal a physical patch"
    );
}

#[test]
#[ignore = "requires installed RaceArena / Racepack4 / Racepack6 assets"]
fn coastal_large_pool_runtime_maps() {
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["racearena_pro", "racepack4", "racepack6"] {
        let map = crate::scene::prepare_with_options(
            &workspace.join("target/release/base"),
            Some(&workspace.join("target/release/japro")),
            name,
            crate::scene::MapPrepareOptions {
                ocean: true,
                grass: false,
                voxel_probe_gi: false,
                client_physics: false,
                gen_normal_maps: false,
                steam_audio: false,
                planar_reflections: false,
                planar_environment: false,
                ..Default::default()
            },
        )
        .expect("prepare real large-pool map");
        let entries =
            super::super::surfaces::collect(&map.vertices, &map.batches, &map.pvs_batches);
        let faces: Vec<_> = entries.iter().map(|e| e.face).collect();
        let triangles: Vec<_> = entries.iter().map(|e| e.triangles.clone()).collect();
        let footprints = super::super::build_water_footprints(
            &faces,
            &triangles,
            super::super::WATER_FOOTPRINT_TILE_SIZE,
        );
        if std::env::var_os("JKA_COASTAL_SCREEN_REPORT").is_some() {
            let mut assets = jka_assets::pk3::AssetSearchPath::open_game(
                &workspace.join("target/release/base"),
                Some(&workspace.join("target/release/japro")),
            )
            .unwrap();
            let asset = assets
                .read(&format!("maps/{name}.bsp"), jka_assets::bsp::MAX_FILE_BYTES)
                .unwrap()
                .unwrap();
            let bsp = jka_assets::bsp::Bsp::parse(&asset.bytes).unwrap();
            let geometry = jka_assets::bsp::bathymetry::Geometry::from_bsp(&bsp).unwrap();
            let footprint = &footprints[0];
            let exact: Vec<_> = footprint
                .triangles
                .iter()
                .map(|t| t.map(|[x, z]| [x, -z]))
                .collect();
            let regions: Vec<_> = footprint
                .tiles
                .iter()
                .map(|t| jka_assets::bsp::bathymetry::Region {
                    slot: 0,
                    level: footprint.plane,
                    triangles: t.triangles.iter().map(|&i| exact[i as usize]).collect(),
                    bounds: Some([[t.minimum[0], -t.maximum[1]], [t.maximum[0], -t.minimum[1]]]),
                })
                .collect();
            let report = geometry
                .shoreline_screening_report(
                    &regions,
                    super::super::bathymetry::SHORELINE_SCREENING_DEPTH,
                )
                .unwrap();
            let directory = workspace.join("target/coastal-large-pool-analysis");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(format!("{name}-screen.txt")), &report).unwrap();
            eprintln!("{name}: {report}");
        }
        let job = map
            .coastal_bathymetry
            .as_ref()
            .expect("coastal geometry job");
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let status = job.status.lock().unwrap();
            match &*status {
                super::super::bathymetry::Status::Ready {
                    atlas,
                    total_ms,
                    cache_hit,
                    ..
                } => {
                    let decoded = Atlas::decode(&atlas.encode([11, 12]), [11, 12]).unwrap();
                    assert_eq!(decoded.shoreline_bands.len(), atlas.shoreline_bands.len());
                    eprintln!(
                        "{name}: {} grids; {total_ms:.2} ms; cache_hit={cache_hit}",
                        atlas.grids.len()
                    );
                    for (slot, footprint) in footprints
                        .iter()
                        .take(super::super::OCEAN_MAX_SURFACES)
                        .enumerate()
                    {
                        if faces[footprint.members[0]].authored_ocean.is_some() {
                            continue;
                        }
                        let representative = footprint
                            .members
                            .iter()
                            .copied()
                            .max_by(|&a, &b| {
                                let area = |i: usize| {
                                    (faces[i].maximum[0] - faces[i].minimum[0])
                                        * (faces[i].maximum[1] - faces[i].minimum[1])
                                };
                                area(a).total_cmp(&area(b))
                            })
                            .unwrap();
                        let primary = entries[representative].source;
                        let source = map
                            .batches
                            .iter()
                            .chain(&map.pvs_batches)
                            .find(|s| {
                                s.vertices == primary.vertices
                                    && s.water
                                    && !s.texture_is_lightmap
                                    && !matches!(s.tc_gen, jka_assets::shader::TcGen::Environment)
                                    && s.texture.is_some()
                            })
                            .unwrap_or(primary);
                        let appearance = super::super::appearance::from_stage(
                            source,
                            &map.vertices,
                            &map.textures,
                        );
                        let ocean = PromotedOcean {
                            index: slot,
                            name: name.into(),
                            surface: super::super::OceanSurface {
                                plane_height: footprint.plane,
                                minimum: footprint.minimum,
                                maximum: footprint.maximum,
                            },
                            heuristic: appearance.heuristic,
                            color: [0.1; 3],
                            roughness: None,
                            source_deformation: None, source_alpha: None,
                            swell_direction: None,
                        };
                        let baked = atlas.grids.iter().filter(|g| g.slot == slot as u32).count();
                        let result = match prepare(atlas, &ocean) {
                            Ok(p) => {
                                if name == "racearena_pro" && slot == 0 {
                                    assert!(footprint.tiles.len() * 256 > MAX_BAKED_CELLS);
                                    assert!(p.cell_indirection);
                                    assert!(
                                        p.tiled_coverage > MAX_CELLS,
                                        "real pool must exercise cell compaction"
                                    );
                                    assert!(p.cells.len() <= MAX_CELLS && p.shoreline > 0);
                                }
                                format!("LIVE {} stored cells / {} tile-addressed / {} active / {} bank cells", p.cells.len(),p.tiled_coverage,p.active,p.shoreline)
                            }
                            Err(e) => {
                                assert!(
                                    name != "racearena_pro" || slot != 0,
                                    "huge installed RaceArena pool must be eligible: {e}"
                                );
                                format!("FFT: {e}")
                            }
                        };
                        eprintln!("{name} Pool {}: level {}u; full {} tiles / {} baked tiles; wavelength {:.2}u; {result}",
                            slot + 1, footprint.plane, footprint.tiles.len(), baked, ocean.heuristic.wavelength);
                    }
                    for warning in &atlas.warnings {
                        eprintln!("{name}: {warning}");
                    }
                    break;
                }
                super::super::bathymetry::Status::Failed(e) => panic!("{name}: {e}"),
                _ => {}
            }
            drop(status);
            assert!(Instant::now() < deadline, "{name} bake timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
#[ignore = "requires installed Racepack6 assets in target/release/base and japro"]
fn coastal_racepack6_pool3_runtime_adapter() {
    use std::path::Path;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let base = workspace.join("target/release/base");
    let game = workspace.join("target/release/japro");
    let map = crate::scene::prepare_with_options(
        &base,
        Some(&game),
        "racepack6",
        crate::scene::MapPrepareOptions {
            ocean: true,
            grass: false,
            voxel_probe_gi: false,
            client_physics: false,
            gen_normal_maps: false,
            steam_audio: false,
            planar_reflections: false,
            planar_environment: false,
            ..Default::default()
        },
    )
    .expect("prepare Racepack6");
    let entries = super::super::surfaces::collect(&map.vertices, &map.batches, &map.pvs_batches);
    let faces: Vec<_> = entries.iter().map(|e| e.face).collect();
    let triangles: Vec<_> = entries.iter().map(|e| e.triangles.clone()).collect();
    let footprint = super::super::build_water_footprints(
        &faces,
        &triangles,
        super::super::WATER_FOOTPRINT_TILE_SIZE,
    )
    .remove(2);
    assert!(
        (footprint.plane + 1312.0).abs() < 1.0,
        "matches user's Pool 3"
    );
    let representative = footprint
        .members
        .iter()
        .copied()
        .max_by(|&a, &b| {
            let area = |i: usize| {
                (faces[i].maximum[0] - faces[i].minimum[0])
                    * (faces[i].maximum[1] - faces[i].minimum[1])
            };
            area(a).total_cmp(&area(b))
        })
        .unwrap();
    let primary = entries[representative].source;
    let source = map
        .batches
        .iter()
        .chain(&map.pvs_batches)
        .find(|s| {
            s.vertices == primary.vertices
                && s.water
                && !s.texture_is_lightmap
                && !matches!(s.tc_gen, jka_assets::shader::TcGen::Environment)
                && s.texture.is_some()
        })
        .unwrap_or(primary);
    let appearance = super::super::appearance::from_stage(source, &map.vertices, &map.textures);
    let ocean = PromotedOcean {
        index: 2,
        name: "racepack6 Pool 3".into(),
        surface: super::super::OceanSurface {
            plane_height: footprint.plane,
            minimum: footprint.minimum,
            maximum: footprint.maximum,
        },
        heuristic: appearance.heuristic,
        color: [0.1; 3],
        roughness: None,
        source_deformation: None, source_alpha: None,
        swell_direction: None,
    };
    let job = map.coastal_bathymetry.as_ref().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let status = job.status.lock().unwrap();
        if let super::super::bathymetry::Status::Ready {
            atlas, cache_hit, ..
        } = &*status
        {
            let prepared = prepare(atlas, &ocean)
                .expect("actual user's Pool 3 must have a safe shallow-bank interface");
            let encoded = atlas.encode([123, 456]);
            let decoded = Atlas::decode(&encoded, [123, 456])
                .expect("many sparse tiles must load from cache");
            assert_eq!(decoded.grids.len(), atlas.grids.len());
            println!("Racepack6 Pool 3: {} tiled cells, {} active, {} shallow bank cells, wavelength {:.3}u; cache_hit={cache_hit}; {} total atlas tiles",
                prepared.cells.len(),prepared.active,prepared.shoreline,ocean.heuristic.wavelength,atlas.grids.len());
            assert!(prepared.shoreline > 0);
            assert!(prepared.active < prepared.cells.len());
            break;
        }
        if let super::super::bathymetry::Status::Failed(e) = &*status {
            panic!("{e}");
        }
        drop(status);
        assert!(
            std::time::Instant::now() < deadline,
            "Racepack6 bathymetry timeout"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn read_f32(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Buffer) -> Vec<f32> {
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Coastal test readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &read, 0, source.size());
    let submission = queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    read.slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    receive.recv().unwrap().unwrap();
    let values = bytemuck::cast_slice::<u8, f32>(&read.slice(..).get_mapped_range().expect("GPU readback range")).to_vec();
    read.unmap();
    values
}

fn sample_render_field(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &Gpu,
    pool: f32,
) -> Vec<f32> {
    // Compile the production sampler, including hash/world/JKA conversion.
    let bsp = include_str!("../bsp.wgsl");
    let compute = include_str!("../coastal_compute.wgsl");
    let uniform = &compute[compute.find("struct OceanRenderSettings").unwrap()
        ..compute.find("struct Params").unwrap()];
    let sampler = &bsp[bsp.find("struct CoastalParams").unwrap()
        ..bsp.find("// Bind the persistent field").unwrap()];
    let source=format!("{uniform}\n{}\n@group(0) @binding(3) var<uniform> ocean:OceanRenderSettings;\n@group(0) @binding(12) var<storage,read_write> result:array<vec4<f32>>;\n@compute @workgroup_size(1) fn probe() {{let s=sample_coastal(vec3<f32>(-136.0,0.0,-256.0));result[0]=s.field;result[1]=vec4<f32>(s.slope,0.0,0.0);}}",sampler.replace("@group(6)","@group(0)"));
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Production coastal sampling probe"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut wave = OceanRenderUniform::zeroed();
    wave.authored_plane[3] = 1.0;
    wave.spray_planes[0][2] = pool;
    let wave = buffer(
        device,
        "Probe pool identity",
        bytemuck::bytes_of(&wave),
        wgpu::BufferUsages::UNIFORM,
    );
    let result = buffer(
        device,
        "Probe samples",
        &[0; 32],
        wgpu::BufferUsages::STORAGE,
    );
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            super::super::bg_buffer(3, &wave),
            super::super::bg_buffer(9, &gpu.render.params),
            super::super::bg_buffer(10, &gpu.render.lookup),
            super::super::bg_buffer(11, &gpu.render.surface),
            super::super::bg_buffer(12, &result),
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit([encoder.finish()]);
    read_f32(device, queue, &result)
}

fn rectangular_fixture(case: usize) -> (Prepared, jka_water::coastal::Solver) {
    use jka_water::coastal::{Domain, Grid as SolverGrid, Solver};
    let (nx, ny) = (32usize, 16usize);
    let n = nx * ny;
    let mut bed = vec![-1.0; n];
    let mut depth = vec![1.0; n];
    let mut active = vec![true; n];
    let mut wall_x = vec![false; n];
    let wall_y = vec![false; n];
    for y in 0..ny {
        for x in 0..nx {
            let k = y * nx + x;
            match case {
                1 => {
                    bed[k] = -1.0 + x as f32 * 0.01;
                    depth[k] = -bed[k];
                }
                2 => {
                    depth[k] += 0.02 * (-((x as f32 - 8.0) / 3.0).powi(2)).exp();
                    wall_x[k] = x == 15;
                }
                3 => {
                    depth[k] = if x < 12 { 1.0 } else { 0.0 };
                    active[k] = !(x == 23 && y > 3 && y < 12);
                }
                4 => {
                    bed[k] = if x > 16 { 0.1 } else { -1.0 };
                    depth[k] = (-bed[k]).max(0.0);
                }
                _ => {}
            }
        }
    }
    let domain = Domain::with_geometry(
        SolverGrid::new(nx, ny, 0.4, 0.4).unwrap(),
        bed.clone(),
        active.clone(),
        wall_x,
        wall_y,
    )
    .unwrap();
    let mut cells = Vec::new();
    for k in 0..n {
        let x = k % nx;
        let y = k / nx;
        let west = if x > 0 && domain.x_face_open(k - 1) {
            (k - 1) as u32
        } else {
            NONE
        };
        let east = if domain.x_face_open(k) {
            (k + 1) as u32
        } else {
            NONE
        };
        let south = if y > 0 && domain.y_face_open(k - nx) {
            (k - nx) as u32
        } else {
            NONE
        };
        let north = if domain.y_face_open(k) {
            (k + nx) as u32
        } else {
            NONE
        };
        cells.push(CellGpu {
            neighbours: [west, east, south, north],
            metric: [
                bed[k],
                bed[k].max(bed[(k + 1).min(n - 1)]),
                bed[k].max(bed[(k + nx).min(n - 1)]),
                0.0,
            ],
            position: [
                x as f32 * 16.0,
                y as f32 * 16.0,
                if active[k] { 1.0 } else { 0.0 },
                if active[k] { 1.0 } else { 0.0 },
            ],
        });
    }
    let solver = Solver::new(domain, depth.clone()).unwrap();
    let prepared = Prepared {
        slot: 2,
        level: 0.0,
        spacing: 16.0,
        cells,
        depth,
        lookup: vec![[0; 4]; 4],
        lookup_mask: 3,
        cell_indirection: false,
        tiled_coverage: n,
        max_depth: 1.1,
        active: n,
        shoreline: 1,
    };
    (prepared, solver)
}

#[test]
#[ignore = "requires a Vulkan GPU; compares the live sparse shader with the independently validated CPU solver"]
fn coastal_gpu_matches_cpu_conservation_wall_and_wet_front() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        println!("Coastal GPU validation: {:?}", adapter.get_info());
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fft = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 3,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = fft.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        for case in 0..5 {
            let (prepared, mut cpu) = rectangular_fixture(case);
            let initial_volume = cpu.volume();
            let initialization_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut gpu = Gpu::new(&device, prepared, &view, &sampler, 0.0);
            if let Some(error) = initialization_scope.pop().await {
                panic!("coastal GPU initialization: {error}");
            }
            for step in 0..80 {
                cpu.step(f64::from(gpu.dt));
                let mut encoder = device.create_command_encoder(&Default::default());
                gpu.encode(&queue, &mut encoder, gpu.dt, &OceanRenderUniform::zeroed());
                queue.submit([encoder.finish()]);
                if step == 0 {
                    let fields = read_f32(&device, &queue, &gpu.state);
                    let limits = read_f32(&device, &queue, &gpu.limiter);
                    let flux = read_f32(&device, &queue, &gpu.limited);
                    for k in 0..cpu.depth().len() {
                        assert!(
                            (fields[k * 4] - cpu.fields().velocity_x[k]).abs() < 2e-5,
                            "case {case} x velocity {k}"
                        );
                        assert!(
                            (fields[k * 4 + 1] - cpu.fields().velocity_y[k]).abs() < 2e-5,
                            "case {case} y velocity {k}"
                        );
                        assert!(
                            (limits[k] - cpu.fields().limiter[k]).abs() < 2e-5,
                            "case {case} donor limiter {k}"
                        );
                        assert!(
                            (flux[k * 2] - cpu.fields().flux_x[k]).abs() < 2e-5,
                            "case {case} shared X flux {k}"
                        );
                        assert!(
                            (flux[k * 2 + 1] - cpu.fields().flux_y[k]).abs() < 2e-5,
                            "case {case} shared Y flux {k}"
                        );
                    }
                }
            }
            let depth = read_f32(&device, &queue, &gpu.depths[gpu.front]);
            let max_error = depth
                .iter()
                .zip(cpu.depth())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(max_error < 2e-5, "case {case}: max depth error {max_error}");
            assert!(depth.iter().all(|h| h.is_finite() && *h >= 0.0));
            let volume = depth.iter().map(|&h| f64::from(h) * 0.16).sum::<f64>();
            let ppm = (volume - initial_volume).abs() / initial_volume * 1e6;
            assert!(ppm < 20.0, "case {case}: {ppm} ppm volume error");
            println!("case {case}: depth max error {max_error:.9} m; volume error {ppm:.5} ppm");
        }
        // A source-accounted FFT target drives actual bank wetting across two
        // atlas tiles. It also validates sparse global neighbour indexing.
        for compact in [false, true] {
            let (atlas, ocean) = if compact {
                shallow_patch_fixture()
            } else {
                tiled_bank()
            };
            let prepared = prepare(&atlas, &ocean).unwrap();
            // Scatter the packed GPU cells into an independently indexed dense
            // CPU domain. Do not give the reference the GPU neighbour remap.
            let dense: Vec<usize> = prepared
                .cells
                .iter()
                .map(|c| {
                    ((c.position[1] / 16.0).floor() as usize) * 16
                        + ((c.position[0] + 256.0) / 16.0).floor() as usize
                })
                .collect();
            let bank = dense.iter().position(|&k| k == 10).unwrap();
            let mut active = vec![false; 512];
            let mut bed = vec![0.0; 512];
            let mut initial_depth = vec![0.0; 512];
            let mut wall_x = vec![true; 512];
            let mut wall_y = vec![true; 512];
            for (k, c) in prepared.cells.iter().enumerate() {
                let d = dense[k];
                active[d] = c.position[2] > 0.5;
                bed[d] = c.metric[0];
                initial_depth[d] = prepared.depth[k];
                wall_x[d] = c.neighbours[1] == NONE;
                wall_y[d] = c.neighbours[3] == NONE;
                for (axis, &q) in c.neighbours.iter().enumerate() {
                    if q != NONE {
                        let offset = [-1i32, 1, -16, 16][axis];
                        assert_eq!(
                            dense[q as usize] as i32,
                            d as i32 + offset,
                            "packed face must reference the same physical CPU cell"
                        );
                    }
                }
            }
            let domain = jka_water::coastal::Domain::with_geometry(
                jka_water::coastal::Grid::new(16, 32, 0.4, 0.4).unwrap(),
                bed,
                active,
                wall_x,
                wall_y,
            )
            .unwrap();
            let targets: Vec<_> = prepared
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.metric[3] > 0.0)
                .map(|(k, c)| jka_water::coastal::BoundaryTarget {
                    cell: dense[k],
                    surface: 0.04,
                    rate: f64::from(c.metric[3]),
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    transverse_velocity_blend: 1.0,
                })
                .collect();
            let mut cpu = jka_water::coastal::Solver::new(domain, initial_depth).unwrap();
            let mut gpu = Gpu::new(&device, prepared, &view, &sampler, 1.6);
            let mut wave = OceanRenderUniform::zeroed();
            wave.ocean_info[2] = 40.0;
            wave.swell[3] = 0.04;
            wave.swell_motion[0] = 1.0;
            wave.swell_motion[3] = -std::f32::consts::FRAC_PI_2;
            for _ in 0..400 {
                cpu.step_with_boundaries(f64::from(gpu.dt), &targets);
                let mut encoder = device.create_command_encoder(&Default::default());
                gpu.encode(&queue, &mut encoder, gpu.dt, &wave);
                queue.submit([encoder.finish()]);
            }
            let depth = read_f32(&device, &queue, &gpu.depths[gpu.front]);
            let max_error = depth
                .iter()
                .enumerate()
                .map(|(k, a)| (a - cpu.depth()[dense[k]]).abs())
                .fold(0.0f32, f32::max);
            assert!(
                max_error < 2e-5,
                "FFT interface max depth error {max_error}"
            );
            assert!(
                depth[bank] > 0.0005,
                "FFT-fed water must wet the initially dry bank"
            );
            let samples = read_f32(&device, &queue, &gpu.render.surface);
            assert!(
                samples[4 + bank * 16 + 5] > 0.02,
                "reconstructed bank must present positive water depth"
            );
            let sample = sample_render_field(&device, &queue, &gpu, 2.0);
            assert!(
                (sample[3] - if compact { 1.0 / 3.0 } else { 1.0 }).abs() < 0.001,
                "shader must find negative-X tiles across their shared seam: {sample:?}"
            );
            assert!(
                sample[0] > 0.01 && sample[1] > 0.0,
                "shader must see the solved free surface"
            );
            let wrong_pool = sample_render_field(&device, &queue, &gpu, 3.0);
            assert_eq!(
                wrong_pool[3], 0.0,
                "another pool must not sample this body's field"
            );
            let diagnostics = cpu.diagnostics();
            let gpu_source = samples[4..]
                .chunks_exact(16)
                .map(|c| f64::from(c[12]))
                .sum::<f64>();
            let gpu_volume = samples[4..]
                .chunks_exact(16)
                .map(|c| f64::from(c[13]) * 0.16)
                .sum::<f64>();
            assert!(
                (gpu_source - diagnostics.accounted_source_volume).abs() < 3e-6,
                "FFT source accounting must match the CPU reference"
            );
            assert!(
                (gpu_volume - diagnostics.volume).abs() < 3e-6,
                "source-accounted live volume must match the CPU reference"
            );
            assert_eq!(diagnostics.non_finite, 0);
            println!("FFT target / two-tile wetting: max depth error {max_error:.9} m; wetted-bank depth {:.6} m; source volume {:.9} m3",depth[bank],diagnostics.accounted_source_volume);
            let mut encoder = device.create_command_encoder(&Default::default());
            gpu.encode(&queue, &mut encoder, 2.0, &wave);
            queue.submit([encoder.finish()]);
            assert!(
                gpu.dropped_time > 1.9,
                "large frame gaps must drop debt instead of unbounded catch-up"
            );
            wave.swell[3] = 1e6;
            let mut encoder = device.create_command_encoder(&Default::default());
            gpu.encode(&queue, &mut encoder, gpu.dt, &wave);
            queue.submit([encoder.finish()]);
            let failed = read_f32(&device, &queue, &gpu.render.surface);
            assert_ne!(
                failed[0].to_bits(),
                0,
                "excessive forcing must latch a numerical failure"
            );
            assert_eq!(
                sample_render_field(&device, &queue, &gpu, 2.0)[3],
                0.0,
                "unsafe state must fall back to FFT immediately on the GPU"
            );
        }
        assert!(
            scope.pop().await.is_none(),
            "live coastal pipelines and bindings must validate"
        );
    });
}
