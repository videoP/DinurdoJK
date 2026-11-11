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
                out.push(jka_assets::bsp::river::CollisionSurface { triangle, solid_bottom_z: None });
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

fn run(mode: SourceMode) {
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
    let mut river = LiveRiver::spawn_sparse(&device, &queue, &collision, source, |_| None, None).expect("sparse spawn");
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
    assert!(sparse.simulation.solver_frames() > 0, "no solver frame ran");
    assert!(sparse.presented && river.surface().is_some(), "presentation window was never filled");
    assert!(sparse.simulation.resident_tiles() > first, "residency never grew from occupancy readback");
}

#[test]
fn sparse_river_level_source_runs_without_validation_errors() {
    run(SourceMode::WaterLevel);
}

#[test]
fn sparse_river_discharge_source_runs_without_validation_errors() {
    run(SourceMode::Discharge);
}
