//! Headless GPU smoke test for sparse (long-distance) rivers: spawn on a
//! synthetic sloped bed, run render frames through the real encode path and
//! fail on any wgpu validation error. Skips when no adapter is available.
use super::*;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::yield_now();
    }
}

/// A 4096 x 1024 unit plane falling 0.05 units per unit along +X, in 64-unit
/// quads, plus a raised bank strip along both long edges.
fn sloped_bed() -> Vec<jka_assets::bsp::river::CollisionSurface> {
    let z = |x: f32, y: f32| 200.0 - 0.05 * x + if !(128.0..=896.0).contains(&y) { 40.0 } else { 0.0 };
    let mut out = Vec::new();
    let step = 64.0;
    for iy in 0..16 {
        for ix in 0..64 {
            let (x0, y0) = (ix as f32 * step, iy as f32 * step);
            let (x1, y1) = (x0 + step, y0 + step);
            let p = |x: f32, y: f32| [x, y, z(x, y)];
            for triangle in [[p(x0, y0), p(x1, y0), p(x1, y1)], [p(x0, y0), p(x1, y1), p(x0, y1)]] {
                out.push(jka_assets::bsp::river::CollisionSurface { triangle, solid_bottom_z: None, wall: false });
            }
        }
    }
    out
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.ok()?;
        if !adapter.features().contains(wgpu::Features::FLOAT32_FILTERABLE) {
            return None;
        }
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::FLOAT32_FILTERABLE,
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .ok()
    })
}

fn run(mode: SourceMode, storage: crate::river_tiles::Storage) {
    let Some((device, queue)) = device() else {
        eprintln!("no FLOAT32_FILTERABLE GPU adapter; sparse river GPU test skipped");
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let collision = sloped_bed();
    let source = LocalSource {
        center: [300.0, 512.0, 185.0],
        radius: 256.0,
        water_level: 205.0,
        source_width: 64.0,
        resolution: 64,
        rate: 10.0,
        mode,
        discharge_cfs: (mode == SourceMode::Discharge).then_some(200.0),
    };
    crate::river_tiles::override_storage_for_thread(Some(storage));
    let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
    crate::river_tiles::override_storage_for_thread(None);
    let first = match &river.solver {
        Solver::Sparse(sparse) => sparse.simulation.resident_tiles(),
        Solver::Dense(_) => unreachable!(),
    };
    for frame in 0..240 {
        // Walk the camera downstream so the window moves at least once.
        let eye = [300.0 + frame as f32 * 12.0, 512.0, 260.0];
        river.update_view(&device, &queue, eye, |_| None, None);
        // A player hull wading downstream through the source pool for a while,
        // then gone: exercises stamp, restore-after-move and final restore.
        let bodies: Vec<MapBody> = if (30..180).contains(&frame) {
            let x = 300.0 + (frame - 30) as f32 * 2.0;
            vec![MapBody { mins: [x - 15.0, 497.0, 176.0], maxs: [x + 15.0, 527.0, 240.0], velocity: [120.0, 0.0, 0.0] }]
        } else {
            Vec::new()
        };
        river.set_bodies(&device, &queue, &bodies, 1.0);
        // Whitewater ViewRecycler camera looking downstream from the eye.
        river.set_spray_view(Some(crate::river_secondary::SprayView {
            position_cm: eye.map(|v| v * jka_water::river::CENTIMETRES_PER_MAP_UNIT),
            right: [0.0, -1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            forward: [1.0, 0.0, 0.0],
            m00: 1.0,
            m11: 16.0 / 9.0,
            aspect: 16.0 / 9.0,
        }));
        let mut encoder = device.create_command_encoder(&Default::default());
        river.encode_frame(&device, &mut encoder, 1.0 / 60.0, frame as f64 / 60.0, None, &mut |_, _| {}).expect("sparse frame");
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
    }
    // The water mesh quadtree over the wet tiles, from a camera far upstream
    // (the window does not cover the downstream reach).
    let surface = crate::river_surface::RiverSurface::new(&device, &queue).expect("river surface resources");
    let always = |_c: [f64; 3], _e: [f64; 3]| true;
    let view = crate::river_far::FarView { eye_cm: [0.0, 512.0 * 2.5, 400.0 * 2.5], frustum: &always, lod_morphing: true };
    river.prepare_far(&device, &queue, &surface, &view, &mut |_| None, None);
    queue.submit(None);
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU water mesh upload");
    let far = river.far.as_ref().expect("sparse river builds a water mesh");
    eprintln!("water mesh: leaves {} nodes {} depth {} instances {} per density {:?}", far.stats.leaves, far.stats.nodes, far.stats.tree_depth, far.stats.instances, far.draws);
    let error = block_on(scope.pop());
    assert!(error.is_none(), "sparse river validation error: {error:?}");
    let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
    eprintln!(
        "sparse river {mode:?}: tiles {first} -> {} (queued {}), solver frames {}, throttled {}, window {:?}",
        sparse.simulation.resident_tiles(),
        sparse.simulation.queued_tiles(),
        sparse.simulation.solver_frames(),
        sparse.throttled_frames,
        sparse.window.origin_cell(),
    );
    assert!(river.spray.as_ref().is_some_and(|spray| spray.ready()), "whitewater emitter never ran");
    assert!(sparse.simulation.solver_frames() > 0, "no solver frame ran");
    assert!(sparse.presented && river.surface().is_some(), "presentation window was never filled");
    assert!(sparse.simulation.resident_tiles() > first, "residency never grew from occupancy readback");
}

/// r_riverTileSchedule A/B: the same scenario (level source, a body wading
/// through and leaving) under 0 native, 1 worklist and 2 dormant-dry must
/// leave a bitwise identical pool. Mode 2 must also have skipped work.
#[test]
fn sparse_river_tile_schedule_matches_native_bitwise() {
    let Some((device, queue)) = device() else {
        eprintln!("no FLOAT32_FILTERABLE GPU adapter; sparse river schedule test skipped");
        return;
    };
    if device.limits().max_storage_buffers_per_shader_stage < 12 {
        eprintln!("adapter has fewer than 12 storage buffers per stage; schedule test skipped");
        return;
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let collision = sloped_bed();
    let run = |mode: u8| {
        let source = LocalSource {
            center: [300.0, 512.0, 185.0],
            radius: 256.0,
            water_level: 205.0,
            source_width: 64.0,
            resolution: 64,
            rate: 10.0,
            mode: SourceMode::WaterLevel,
            discharge_cfs: None,
        };
        let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
        let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
        sparse.schedule_override = Some(mode);
        for frame in 0..200 {
            let bodies: Vec<MapBody> = if (20..120).contains(&frame) {
                let x = 300.0 + (frame - 20) as f32 * 2.0;
                vec![MapBody { mins: [x - 15.0, 497.0, 176.0], maxs: [x + 15.0, 527.0, 240.0], velocity: [120.0, 0.0, 0.0] }]
            } else {
                Vec::new()
            };
            river.set_bodies(&device, &queue, &bodies, 1.0);
            let mut encoder = device.create_command_encoder(&Default::default());
            river.encode_frame(&device, &mut encoder, 1.0 / 60.0, frame as f64 / 60.0, None, &mut |_, _| {}).expect("sparse frame");
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
        }
        let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
        let simulation = &sparse.simulation;
        eprintln!(
            "schedule {mode}: tiles {} solver frames {} {}",
            simulation.resident_tiles(),
            simulation.solver_frames(),
            simulation.tile_schedule_report().unwrap_or_else(|| "schedule=native".into()),
        );
        (simulation.read_pool(&device, &queue), simulation.solver_frames(), simulation.tile_schedule_skipped())
    };
    let (native, frames, _) = run(0);
    let wet = native.values().filter(|cells| cells.chunks_exact(4).any(|c| c[0][1] > 0.0)).count();
    assert!(wet > 0, "no wet tile: the scenario does not exercise the solver");
    for mode in [1u8, 2] {
        let (pool, mode_frames, skipped) = run(mode);
        assert_eq!(mode_frames, frames, "schedule {mode} ran a different number of solver frames");
        assert_eq!(pool.keys().collect::<Vec<_>>(), native.keys().collect::<Vec<_>>(), "schedule {mode} resident set differs");
        for (tile, cells) in &native {
            let other = &pool[tile];
            if let Some(i) = (0..cells.len()).find(|&i| cells[i].map(f32::to_bits) != other[i].map(f32::to_bits)) {
                let field = ["state", "surface", "normals", "history"][i % 4];
                panic!("schedule {mode} differs from native at tile {tile:?} lane {} {field}: {:?} vs {:?}", i / 4, other[i], cells[i]);
            }
        }
        let skipped = skipped.expect("schedule stats were sampled");
        eprintln!("schedule {mode}: bitwise identical over {} tiles; skipped {:.1}% of resident slot-frames", native.len(), 100.0 * skipped);
        if mode == 1 {
            assert_eq!(skipped, 0.0, "worklist mode must schedule every resident slot");
        } else {
            assert!(skipped > 0.0, "dormant-dry never skipped a tile in this scenario");
        }
    }
    let error = block_on(scope.pop());
    assert!(error.is_none(), "sparse river schedule validation error: {error:?}");
}

/// r_riverStorageMode A/B: native storage (0) against lean (1) and packed
/// (2) pools, under the native dispatch and the dormant-dry schedule, for a
/// level source with a wading body and for a CFS inlet. State, PreSim history
/// (derived from the bed in 1/2) and surface must be bitwise identical; normals
/// bitwise in mode 1 and within the octahedral codec's error in mode 2.
#[test]
fn sparse_river_storage_modes_match_native() {
    use crate::river_tiles::Storage;
    let Some((device, queue)) = device() else {
        eprintln!("no FLOAT32_FILTERABLE GPU adapter; sparse river storage test skipped");
        return;
    };
    if device.limits().max_storage_buffers_per_shader_stage < 12 {
        eprintln!("adapter has fewer than 12 storage buffers per stage; storage test skipped");
        return;
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let collision = sloped_bed();
    let run = |source_mode: SourceMode, storage: Storage, schedule: u8| {
        let source = LocalSource {
            center: [300.0, 512.0, 185.0],
            radius: 256.0,
            water_level: 205.0,
            source_width: 64.0,
            resolution: 64,
            rate: 10.0,
            mode: source_mode,
            discharge_cfs: (source_mode == SourceMode::Discharge).then_some(200.0),
        };
        crate::river_tiles::override_storage_for_thread(Some(storage));
        let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
        crate::river_tiles::override_storage_for_thread(None);
        let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
        assert_eq!(sparse.simulation.storage(), storage, "spawn kept the requested layout");
        sparse.schedule_override = Some(schedule);
        for frame in 0..200 {
            let bodies: Vec<MapBody> = if (20..120).contains(&frame) {
                let x = 300.0 + (frame - 20) as f32 * 2.0;
                vec![MapBody { mins: [x - 15.0, 497.0, 176.0], maxs: [x + 15.0, 527.0, 240.0], velocity: [120.0, 0.0, 0.0] }]
            } else {
                Vec::new()
            };
            river.set_bodies(&device, &queue, &bodies, 1.0);
            let mut encoder = device.create_command_encoder(&Default::default());
            river.encode_frame(&device, &mut encoder, 1.0 / 60.0, frame as f64 / 60.0, None, &mut |_, _| {}).expect("sparse frame");
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
        }
        let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
        let simulation = &sparse.simulation;
        (simulation.read_pool(&device, &queue), simulation.solver_frames(), simulation.logical_bytes())
    };
    for source_mode in [SourceMode::WaterLevel, SourceMode::Discharge] {
        let (native, frames, native_bytes) = run(source_mode, Storage::Native, 0);
        let wet = native.values().filter(|cells| cells.chunks_exact(4).any(|c| c[0][1] > 0.0)).count();
        assert!(wet > 0, "no wet tile: the scenario does not exercise the solver");
        for (storage, schedule) in [(Storage::Lean, 0u8), (Storage::Packed, 0), (Storage::Lean, 2), (Storage::Packed, 2)] {
            let (pool, mode_frames, bytes) = run(source_mode, storage, schedule);
            let label = format!("{source_mode:?} storage {} schedule {schedule}", storage.name());
            assert_eq!(mode_frames, frames, "{label}: different solver frame count");
            assert_eq!(pool.keys().collect::<Vec<_>>(), native.keys().collect::<Vec<_>>(), "{label}: resident set differs");
            assert!(bytes < native_bytes, "{label}: pool did not shrink ({bytes} vs {native_bytes})");
            let mut worst_normal = 0.0f32;
            for (tile, cells) in &native {
                let other = &pool[tile];
                for (i, (a, b)) in cells.iter().zip(other).enumerate() {
                    let field = ["state", "surface", "normals", "history"][i % 4];
                    if field == "normals" && storage.packed_normals() {
                        let error = (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max);
                        worst_normal = worst_normal.max(error);
                        assert!(error <= 2e-4, "{label}: normal error {error} at tile {tile:?} lane {}: {b:?} vs {a:?}", i / 4);
                    } else if a.map(f32::to_bits) != b.map(f32::to_bits) {
                        panic!("{label}: differs from native at tile {tile:?} lane {} {field}: {b:?} vs {a:?}", i / 4);
                    }
                }
            }
            eprintln!(
                "{label}: identical over {} tiles; pool {:.2} -> {:.2} MiB; worst normal component error {worst_normal:e}",
                native.len(),
                native_bytes as f64 / 1048576.0,
                bytes as f64 / 1048576.0,
            );
        }
    }
    let error = block_on(scope.pop());
    assert!(error.is_none(), "sparse river storage validation error: {error:?}");
}

#[test]
fn sparse_river_level_source_runs_without_validation_errors() {
    run(SourceMode::WaterLevel, crate::river_tiles::Storage::Native);
}

#[test]
fn sparse_river_discharge_source_runs_without_validation_errors() {
    // Packed pool: the window gather and the water mesh decode its normals.
    run(SourceMode::Discharge, crate::river_tiles::Storage::Packed);
}

/// r_riverCaptureMode 1 on the GPU: a stepped channel descending along +X
/// with a bridge deck below the source level but above the local floor. The
/// deck goes to the actor captures with a per-cell reference, so water must
/// pass under it (mode 0 keeps the deck as ground and stops the river).
#[test]
fn sparse_capture_mode_one_passes_water_under_a_bridge() {
    use jka_assets::bsp::river::{CaptureBrush, CaptureFace, CaptureGeometry};
    let Some((device, queue)) = device() else {
        eprintln!("no FLOAT32_FILTERABLE GPU adapter; sparse capture mode test skipped");
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut geometry = CaptureGeometry::default();
    let mut slab = |mins: [f32; 3], maxs: [f32; 3]| {
        let corners = [[mins[0], mins[1]], [maxs[0], mins[1]], [maxs[0], maxs[1]], [mins[0], maxs[1]]];
        let start = geometry.brush_faces.len() as u32;
        for (z, up) in [(maxs[2], true), (mins[2], false)] {
            let q = corners.map(|c| [c[0], c[1], z]);
            for triangle in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                geometry.brush_faces.push(CaptureFace { triangle, up, sky: false });
            }
        }
        geometry.brushes.push(CaptureBrush {
            planes: vec![
                ([1.0, 0.0, 0.0], maxs[0]),
                ([-1.0, 0.0, 0.0], -mins[0]),
                ([0.0, 1.0, 0.0], maxs[1]),
                ([0.0, -1.0, 0.0], -mins[1]),
                ([0.0, 0.0, 1.0], maxs[2]),
                ([0.0, 0.0, -1.0], -mins[2]),
            ],
            mins,
            maxs,
            footprint: corners.to_vec(),
            faces: start..geometry.brush_faces.len() as u32,
        });
    };
    for i in 0..16 {
        let x = i as f32 * 128.0;
        slab([x, 256.0, 0.0], [x + 128.0, 768.0, 200.0 - 2.0 * i as f32]);
        // Banks along both sides of the channel.
        slab([x, 0.0, 0.0], [x + 128.0, 256.0, 260.0]);
        slab([x, 768.0, 0.0], [x + 128.0, 1024.0, 260.0]);
    }
    // Deck from bank to bank over floor 190: its underside is below the
    // source level 210, its top above it, so mode 0 keeps it as a dam.
    slab([640.0, 0.0, 200.0], [768.0, 1024.0, 220.0]);
    let source = LocalSource {
        center: [300.0, 512.0, 196.0],
        radius: 256.0,
        water_level: 210.0,
        source_width: 64.0,
        resolution: 64,
        rate: 10.0,
        mode: SourceMode::WaterLevel,
        discharge_cfs: None,
    };
    // Mode 0 input for the same brushes: their tops with the solid's bottom.
    let legacy: Vec<_> = geometry
        .brushes
        .iter()
        .flat_map(|b| {
            geometry.brush_faces[b.faces.start as usize..b.faces.end as usize]
                .iter()
                .filter(|f| f.up)
                .map(|f| jka_assets::bsp::river::CollisionSurface { triangle: f.triangle, solid_bottom_z: Some(b.mins[2]), wall: false })
        })
        .collect();
    let past_bridge = |capture: Option<&CaptureGeometry>| {
        let mut river = LiveRiver::spawn_sparse(&device, &queue, &legacy, capture, source, |_| None, None).expect("sparse spawn");
        for frame in 0..300 {
            let mut encoder = device.create_command_encoder(&Default::default());
            river.encode_frame(&device, &mut encoder, 1.0 / 60.0, frame as f64 / 60.0, None, &mut |_, _| {}).expect("sparse frame");
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
        }
        let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
        let simulation = &sparse.simulation;
        let pool = simulation.read_pool(&device, &queue);
        let tile = simulation.lattice().tile() as usize;
        let min_map = simulation.bins().min_map();
        let cell = f64::from(sparse.dx_cm) / f64::from(jka_water::river::CENTIMETRES_PER_MAP_UNIT);
        pool.iter()
            .flat_map(|(t, cells)| {
                cells.chunks_exact(4).enumerate().map(move |(lane, c)| {
                    let x = min_map[0] + ((t[0] as usize * tile + lane % tile) as f64 + 0.5) * cell;
                    (x, c[0][1])
                })
            })
            .filter(|&(x, depth)| x > 800.0 && depth > 0.0)
            .count()
    };
    let legacy_wet = past_bridge(None);
    let classified_wet = past_bridge(Some(&geometry));
    eprintln!("wet cells past the bridge: mode 0 {legacy_wet}, mode 1 {classified_wet}");
    let error = block_on(scope.pop());
    assert!(error.is_none(), "sparse capture mode 1 validation error: {error:?}");
    assert_eq!(legacy_wet, 0, "mode 0 should stop at a deck whose solid starts below the source level");
    assert!(classified_wet > 0, "mode 1: water never passed under the bridge");
}

/// Headless sparse solver benchmark (ignored; GPU timestamps, not a pass/fail
/// test). Level source on the sloped bed, warmed until the river has spread,
/// then timed per pass for each storage layout and tile schedule:
/// scripts\fast-cargo.ps1 -CargoArgs @('test','--profile','fast','--offline','--locked','-p','jka-river','--lib','--','--ignored','sparse_river_solver_benchmark','--nocapture','--test-threads','1')
#[test]
#[ignore]
fn sparse_river_solver_benchmark() {
    use crate::river_tiles::Storage;
    let wanted = wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let Some((device, queue, name)) = block_on(async {
        // The game's instance (render_thread.rs): no indirect-call validation,
        // which otherwise adds a validation dispatch per indirect dispatch.
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        let instance = wgpu::Instance::new(desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })
            .await
            .ok()?;
        if !adapter.features().contains(wanted) {
            return None;
        }
        let name = format!("{} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_features: wanted, required_limits: adapter.limits(), ..Default::default() })
            .await
            .ok()?;
        Some((device, queue, name))
    }) else {
        eprintln!("no adapter with timestamp queries; benchmark skipped");
        return;
    };
    eprintln!("adapter: {name}");
    let collision = sloped_bed();
    // Alternating, so GPU clock drift does not favour one layout.
    // (layout, r_riverTileSchedule, tile cells, guard tiles).
    // RIVER_BENCH_CONFIGS=residency compares tile/guard layouts instead.
    let configs: Vec<(Storage, u8, u32, u32)> = if std::env::var("RIVER_BENCH_CONFIGS").is_ok_and(|v| v == "residency") {
        vec![(Storage::Packed, 0, 32, 2), (Storage::Packed, 0, 16, 2), (Storage::Packed, 0, 16, 3), (Storage::Packed, 0, 16, 4), (Storage::Packed, 0, 64, 1), (Storage::Packed, 0, 32, 1)]
    } else {
        vec![(Storage::Native, 0, 32, 2), (Storage::Packed, 0, 32, 2), (Storage::Native, 0, 32, 2), (Storage::Packed, 0, 32, 2), (Storage::Packed, 2, 32, 2)]
    };
    let timed_frames = 240;
    let default_tile = crate::river_tiles::tile_size();
    let default_guard = crate::river_tiles::guard_tiles();
    // Equal simulated time for every layout: warm (fast-fill rate) until this
    // many solver frames ran, then time at the game's 1x rate.
    let warm_solver_frames: u64 = std::env::var("RIVER_BENCH_WARM").ok().and_then(|v| v.parse().ok()).unwrap_or(6000);
    for (storage, schedule, tile, guard) in configs {
        crate::river_tiles::set_tile_size(tile);
        crate::river_tiles::set_guard_tiles(guard);
        let source = LocalSource {
            center: [300.0, 512.0, 185.0],
            radius: 256.0,
            water_level: 205.0,
            source_width: 64.0,
            resolution: 64,
            rate: 10.0,
            mode: SourceMode::WaterLevel,
            discharge_cfs: None,
        };
        crate::river_tiles::override_storage_for_thread(Some(storage));
        let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
        crate::river_tiles::override_storage_for_thread(None);
        crate::river_tiles::set_tile_size(default_tile);
        crate::river_tiles::set_guard_tiles(default_guard);
        let solve_set = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 });
        let solve_resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut per_stage: std::collections::BTreeMap<&'static str, (f64, u32)> = std::collections::BTreeMap::new();
        let mut solve_ms = 0.0;
        let mut solver_frames_timed = 0;
        // Bracket-only frames first (no per-pass queries), then profiled ones.
        let mut bracket = (0.0, 0);
        let mut samples: Vec<f64> = Vec::new();
        let mut warm_frames = u32::MAX;
        let mut throttled_before = 0;
        for frame in 0.. {
            if warm_frames == u32::MAX {
                let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
                if sparse.simulation.solver_frames() >= warm_solver_frames || frame >= 20_000 {
                    warm_frames = frame;
                    throttled_before = sparse.throttled_frames;
                    river.source.rate = 1.0;
                }
            }
            if warm_frames != u32::MAX && frame >= warm_frames + 2 * timed_frames {
                break;
            }
            let timed = frame >= warm_frames;
            if frame == warm_frames {
                let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
                sparse.schedule_override = Some(schedule);
            }
            if frame == warm_frames + timed_frames {
                bracket = (solve_ms, solver_frames_timed);
                solve_ms = 0.0;
                solver_frames_timed = 0;
                let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
                sparse.simulation.enable_stage_profile(&device);
            }
            let before = match &river.solver {
                Solver::Sparse(sparse) => sparse.simulation.solver_frames(),
                Solver::Dense(_) => unreachable!(),
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            let mut stamp = |encoder: &mut wgpu::CommandEncoder, end: bool| {
                if timed {
                    encoder.write_timestamp(&solve_set, u32::from(end));
                }
            };
            river.encode_frame(&device, &mut encoder, 1.0 / 60.0, frame as f64 / 60.0, None, &mut stamp).expect("sparse frame");
            let read = timed.then(|| {
                let read = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.resolve_query_set(&solve_set, 0..2, &solve_resolve, 0);
                encoder.copy_buffer_to_buffer(&solve_resolve, 0, &read, 0, 16);
                read
            });
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
            let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
            if let Some(read) = read {
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("solve stamp");
                let view = read.slice(..).get_mapped_range().expect("mapped");
                let ticks: &[u64] = bytemuck::cast_slice(&view);
                let ms = ticks[1].wrapping_sub(ticks[0]) as f64 * f64::from(queue.get_timestamp_period()) * 1e-6;
                let ran = sparse.simulation.solver_frames() - before;
                solve_ms += ms;
                solver_frames_timed += ran;
                let stages = sparse.simulation.take_stage_profile(&device, &queue);
                if stages.is_empty() && ran > 0 {
                    samples.push(ms / ran as f64);
                }
                for (label, ms) in stages {
                    let entry = per_stage.entry(label).or_default();
                    entry.0 += ms;
                    entry.1 += 1;
                }
            }
        }
        let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
        let frames = solver_frames_timed.max(1) as f64;
        let staged: f64 = per_stage.iter().filter(|(l, _)| **l != "gather_window").map(|(_, (ms, _))| ms).sum();
        eprintln!(
            "\n== storage {} schedule {schedule} tile {tile} guard {guard}: {} resident tiles ({} cells), pool {:.2} MiB, warm {warm_frames} render frames, {solver_frames_timed} solver frames timed (1x), throttled render frames warm {throttled_before} timed {} ==",
            storage.name(),
            sparse.simulation.resident_tiles(),
            sparse.simulation.resident_tiles() as u32 * tile * tile,
            sparse.simulation.logical_bytes() as f64 / 1048576.0,
            sparse.throttled_frames - throttled_before,
        );
        if let Some(report) = sparse.simulation.tile_schedule_report() {
            eprintln!("  {report}");
        }
        samples.sort_by(f64::total_cmp);
        let pick = |q: f64| samples.get(((samples.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
        eprintln!(
            "  UNPROFILED solve mean {:.4} median {:.4} p10 {:.4} p90 {:.4} ms/solver frame over {} solver frames",
            bracket.0 / bracket.1.max(1) as f64,
            pick(0.5),
            pick(0.1),
            pick(0.9),
            bracket.1,
        );
        eprintln!("  profiled solve {:.4} ms/solver frame (passes {:.4}, rest = gaps/collisions/schedule/uploads)", solve_ms / frames, staged / frames);
        for (label, (ms, count)) in &per_stage {
            eprintln!("  {label:28} {:.4} ms/solver frame  {:.4} ms/pass  {count} passes", ms / frames, ms / f64::from(*count));
        }
    }
}

/// Probe (ignored): after the source stops, do once-wet tiles return to the
/// exact dry fixed point the occupancy kernel tests? Long-dry tile eviction
/// is only useful if they do.
#[test]
#[ignore]
fn sparse_river_drained_tiles_return_to_rest() {
    let Some((device, queue)) = device() else { return };
    let collision = sloped_bed();
    let source = LocalSource {
        center: [300.0, 512.0, 185.0],
        radius: 256.0,
        water_level: 205.0,
        source_width: 64.0,
        resolution: 64,
        rate: 10.0,
        mode: SourceMode::WaterLevel,
        discharge_cfs: None,
    };
    let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
    for frame in 0..4000u32 {
        if frame == 600 {
            let Solver::Sparse(sparse) = &mut river.solver else { unreachable!() };
            sparse.simulation.set_height_source(false, 0.0);
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        river.encode_frame(&device, &mut encoder, 1.0 / 60.0, f64::from(frame) / 60.0, None, &mut |_, _| {}).expect("sparse frame");
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
        if frame % 250 == 0 || frame == 3999 {
            let Solver::Sparse(sparse) = &river.solver else { unreachable!() };
            let s = &sparse.simulation;
            eprintln!("frame {frame}: solver frames {} resident {} occupied {}", s.solver_frames(), s.resident_tiles(), s.wet_tiles().len());
        }
    }
}

/// Long-dry tile eviction must be invisible to the hydraulics. The same
/// timeline (source on, off until tiles dry out and are evicted, on again so
/// the water returns over evicted tiles) runs with and without eviction,
/// driven by solver-frame count so both stop at the same frame. Every tile of
/// the evicting run is bitwise identical to the resident run, and every tile
/// only the resident run holds is at the dry fixed point.
#[test]
fn sparse_river_eviction_matches_resident_run() {
    use crate::river_tiles::Storage;
    let Some((device, queue)) = device() else {
        eprintln!("no FLOAT32_FILTERABLE GPU adapter; sparse river eviction test skipped");
        return;
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let collision = sloped_bed();
    const OFF_AT: u64 = 3600;
    const ON_AT: u64 = 20000;
    const END: u64 = 24000;
    let run = |evict: bool| {
        let source = LocalSource {
            center: [300.0, 512.0, 185.0],
            radius: 256.0,
            water_level: 205.0,
            source_width: 64.0,
            resolution: 64,
            rate: 10.0,
            mode: SourceMode::WaterLevel,
            discharge_cfs: None,
        };
        crate::river_tiles::override_storage_for_thread(Some(Storage::Packed));
        let river = LiveRiver::spawn_sparse(&device, &queue, &collision, None, source, |_| None, None).expect("sparse spawn");
        crate::river_tiles::override_storage_for_thread(None);
        let step = Frame::new(river.grid, river.controls, 1.0 / 60.0).expect("frame").hydraulic_duration();
        let frame = Frame::new(river.grid, river.controls, step as f32).expect("frame");
        let Solver::Sparse(mut sparse) = river.solver else { unreachable!() };
        let sim = &mut sparse.simulation;
        sim.set_eviction(evict);
        let (mut most, mut evicted_at_on) = (0usize, 0u64);
        let mut middle = None;
        while sim.solver_frames() < END {
            let mut encoder = device.create_command_encoder(&Default::default());
            sim.update_residency(&device, &mut encoder).expect("residency");
            let mut moved = false;
            for _ in 0..10 {
                let n = sim.solver_frames();
                if n >= END || sim.frames_available() == 0 || (n == ON_AT && middle.is_none()) {
                    break;
                }
                sim.set_height_source(!(OFF_AT..ON_AT).contains(&n), 0.0);
                sim.encode_reference_frame(&device, &mut encoder, frame).expect("solver frame");
                moved = true;
            }
            if moved {
                sim.encode_occupancy(&device, &mut encoder);
            }
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frame");
            most = most.max(sim.resident_tiles());
            if sim.solver_frames() == ON_AT && middle.is_none() {
                evicted_at_on = sim.evicted_tiles();
                middle = Some(sim.read_pool(&device, &queue));
            }
        }
        eprintln!(
            "eviction {evict}: solver frames {} resident {} (peak {most}) evicted {} (before the source returned {evicted_at_on}) pool {:.2} MiB",
            sim.solver_frames(),
            sim.resident_tiles(),
            sim.evicted_tiles(),
            sim.logical_bytes() as f64 / 1048576.0,
        );
        (middle.expect("reached the source's return"), sim.read_pool(&device, &queue), evicted_at_on)
    };
    type PoolCells = std::collections::BTreeMap<[i32; 2], Vec<[f32; 4]>>;
    let compare = |when: &str, resident: &PoolCells, evicting: &PoolCells| {
        let mut extra = 0;
        for (tile, cells) in resident {
            match evicting.get(tile) {
                Some(other) => {
                    if let Some(i) = (0..cells.len()).find(|&i| cells[i].map(f32::to_bits) != other[i].map(f32::to_bits)) {
                        let field = ["state", "surface", "normals", "history"][i % 4];
                        panic!("{when}: evicting run differs at tile {tile:?} lane {} {field}: {:?} vs {:?}", i / 4, other[i], cells[i]);
                    }
                }
                None => {
                    extra += 1;
                    for c in cells.chunks_exact(4) {
                        let bed = c[3][0];
                        assert_eq!(c[0], [bed, 0.0, 0.0, 0.0], "{when}: evicted tile {tile:?} was not at rest in the resident run");
                        assert_eq!(c[1], [bed, 0.0, 0.0, 0.0], "{when}: evicted tile {tile:?} surface was not at rest");
                        assert_eq!(c[2], [0.0, 0.0, 1.0, 0.0], "{when}: evicted tile {tile:?} normal was not at rest");
                    }
                }
            }
        }
        assert!(evicting.keys().all(|t| resident.contains_key(t)), "{when}: evicting run holds a tile the resident run never needed");
        eprintln!("eviction {when}: {} shared tiles bitwise identical; {extra} tiles released and at rest", evicting.len());
        extra
    };
    let (resident_middle, resident_end, _) = run(false);
    let (evicting_middle, evicting_end, evicted_at_on) = run(true);
    assert!(evicted_at_on > 0, "no tile was evicted while the source was off");
    assert!(compare("source off", &resident_middle, &evicting_middle) > 0, "no tile was out when the source returned");
    compare("end", &resident_end, &evicting_end);
    let error = block_on(scope.pop());
    assert!(error.is_none(), "sparse river eviction validation error: {error:?}");
}
