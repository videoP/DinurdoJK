use crate::{
    app::{event_workers, App},
    console::{
        cvars::{CvarDef, CvarFlags, CvarType, CvarValue},
        LatchScope,
    },
};
pub(super) const CVARS: &[CvarDef<App>] = &[
CvarDef::new("cg_remoteSmoothing", "0", "0..4", "Live remote players AND spectator-follow POV: 0 original; 1 buffered linear/hold; 2 buffered linear + bounded extrapolation; 3 Hermite + extrapolation; 4 jaMME-inspired commandTime-aware Hermite (fallback linear). Visual-only, session-only.", CvarType::Int { min: 0, max: 4 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::mode() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_mode(mode as u8);
    if let Some(session) = app.game_session.as_mut() { session.remote_smoothing.sync_from_cvar(); }
    app.push_console_line(format!("^3cg_remoteSmoothing:^7 mode {mode} (0=original, 1=buffer+hold, 2=buffer+extrap, 3=buffer+Hermite+extrap, 4=jaMME-inspired commandTime tangents). Includes live spectator follow; recorded demos unchanged."));
    Ok(())
}),
CvarDef::new("cg_remoteBufferMs", "50", "0..100", "Extra visual-only snapshot interpolation delay (ms), 0 for low latency; used when cg_remoteBufferAuto=0. Session only.", CvarType::Int { min: 0, max: 100 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::buffer_ms() as i32), |_app, value| {
    let CvarValue::Int(ms) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_buffer_ms(ms as u32);
    Ok(())
}),
CvarDef::new("cg_remoteBufferAuto", "0", "0|1", "0 fixed cg_remoteBufferMs; 1 adaptive visual buffer from server snapshot cadence and arrival jitter (up to 100 ms). Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::cgame::remote_smoothing::buffer_auto()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_buffer_auto(enabled);
    Ok(())
}),
CvarDef::new("cg_remoteExtrapMaxMs", "35", "0..100", "Maximum visual-only extrapolation beyond the latest remote snapshot for smoothing modes 2/3/4. 0 disables extrapolation. Session only.", CvarType::Int { min: 0, max: 100 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::extrap_max_ms() as i32), |_app, value| {
    let CvarValue::Int(ms) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_extrap_max_ms(ms as u32);
    Ok(())
}),
CvarDef::new("cg_remoteSmoothDebug", "0", "0..2", "0 off; 1 one-second smoothing/underrun/correction summary; 2 also logs per remote player. No render overhead/readbacks. Session only.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::debug() as i32), |_app, value| {
    let CvarValue::Int(level) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_debug(level as u8);
    Ok(())
}),
CvarDef::new("cg_remoteSmoothStats", "0", "0|1", "Write 1 to print an immediate remote visual history/jitter/underrun report to the in-game console. Read-back remains 0 (one-shot command cvar).", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(false), |app, value| {
    let CvarValue::Bool(print) = value else { unreachable!() };
    if print {
        if let Some(session) = app.game_session.as_ref() {
            app.push_console_line(session.remote_smoothing.summary());
        } else {
            app.push_console_line("^3[REMOTE SMOOTH]^7 No current game session.");
        }
    }
    Ok(())
}),
CvarDef::new("r_riverDebug", "0", "0|1|2", "Live spawnriver diagnostics: 0 off, 1 one-second solver/draw status, 2 detailed spawn geometry and CPU encoding timings. Session-only; no GPU readbacks or stalls.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::river_runtime::debug_mode() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::river_runtime::set_debug_mode(mode as u8);
    app.push_console_line(format!("^3r_riverDebug:^7 {}", match mode {
        0 => "OFF",
        1 => "ON (river progress and draw eligibility every second)",
        _ => "VERBOSE (plus source bed geometry and CPU encode timings; no GPU stalls)",
    }));
    Ok(())
}),
CvarDef::new("r_riverPause", "0", "0|1", "Live river A/B: 1 stops the solver (no simulated time passes) while the water keeps drawing, with FFT detail and foam, from the last solved state. Compare GPU cost with r_gpuTimings 1 ([JKA RIVER PERF] sim/solve/draw) or plain FPS. Session-only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::paused()), |app, value| {
    let CvarValue::Bool(paused) = value else { unreachable!() };
    crate::river_runtime::set_paused(paused);
    app.push_console_line(format!("^3r_riverPause:^7 {}", if paused { "ON (solver stopped; water still drawn)" } else { "OFF (solver running)" }));
    Ok(())
}),
CvarDef::new("r_riverSmoothing", "2", "0|1|2|3", "River render-height smoothing (Unreal Grid2D_SW_ExtrapolateRenderHeight SmoothingMethod): 0 none, 1 edge-preserving (neighbours past the height cutoff are ignored, keeps pourovers sharp), 2 Unreal's River default (ramps across drops), 3 plain distance blur. Applies live.", CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(crate::river_runtime::smoothing_method())), |app, value| {
    let CvarValue::Int(method) = value else { unreachable!() };
    crate::river_runtime::set_smoothing_method(method as u8);
    app.push_console_line(format!("^3r_riverSmoothing:^7 {}", match method { 0 => "0 none", 1 => "1 edge-preserving", 2 => "2 Unreal River default", _ => "3 plain blur" }));
    Ok(())
}),
CvarDef::new("r_riverSmoothingCutoff", "200", "map units >= 0", "River smoothing height cutoff in map units (Unreal SmoothingHeightCutoff, 500 cm = 200 units). Method 1 ignores neighbours this much higher or lower; method 2 weights them fully. 0 forces the plain blur, as upstream.", CvarType::Float { min: 0.0, max: 100000.0 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Float(crate::river_runtime::smoothing_cutoff_cm() / jka_water::river::CENTIMETRES_PER_MAP_UNIT), |app, value| {
    let CvarValue::Float(units) = value else { unreachable!() };
    crate::river_runtime::set_smoothing_cutoff_cm(units * jka_water::river::CENTIMETRES_PER_MAP_UNIT);
    app.push_console_line(format!("^3r_riverSmoothingCutoff:^7 {units} map units"));
    Ok(())
}),
CvarDef::new("r_riverBedClamp", "1", "0|1", "River pourovers (not an Unreal feature): 1 keeps the drawn water on or above the bed it covers and lifts steep mesh vertices to the upper water, so water falls in front of a ledge instead of sliding under it; 0 the plain Unreal surface.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::bed_clamp()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::river_runtime::set_bed_clamp(enabled);
    app.push_console_line(format!("^3r_riverBedClamp:^7 {}", if enabled { "ON" } else { "OFF (Unreal surface)" }));
    Ok(())
}),
CvarDef::new("r_riverWhitewater", "1", "0|1", "River whitewater spray (Unreal SW_River_Secondary_Emitter): 50000 GPU sprites recycled through the view, emitted where the simulated surface curves sharply (pourovers, drops) in fast water deeper than the foam minimum depth. 0 frees the particles.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::whitewater()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::river_runtime::set_whitewater(enabled);
    app.push_console_line(format!("^3r_riverWhitewater:^7 {}", if enabled { "ON" } else { "OFF" }));
    Ok(())
}),
CvarDef::new("r_riverWaterMesh", "1", "0|1", "Sparse rivers beyond the camera window: 1 draws the Unreal Water plugin's water mesh quadtree (LOD tiles over the wet simulation tiles, read straight from the simulation), 0 draws only the window (r_riverWindow).", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_far::enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_far::set_enabled(enabled);
    app.push_console_line(format!("^3r_riverWaterMesh:^7 {}", if enabled { "ON (water mesh beyond the window)" } else { "OFF (window only)" }));
    Ok(())
}),
CvarDef::new("r_riverBlockCull", "1", "0|1", "River camera window A/B (not Unreal): 1 draws only the 16x16-cell blocks of the window grid that can show water, skipping dry riverbed and unloaded void; 0 draws every block (the full grid). The image is identical; compare GPU cost.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_blocks::enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_blocks::set_enabled(enabled);
    app.push_console_line(format!("^3r_riverBlockCull:^7 {}", if enabled { "ON (wet blocks only)" } else { "OFF (full window grid)" }));
    Ok(())
}),
CvarDef::new("r_riverAdaptiveMesh", "0", "0|1", "River mesh A/B (renderer only; the solver and riverbed do not change): 0 one vertex per solver cell everywhere; 1 fewer triangles for calm water: per 16x16-cell block, from the water's turbulence (surface curvature and flow speed), no distance term. Turbulent water keeps the tile resolution (the maximum); calmer water drops to 2/4/8-cell triangles; banks stay at least 4-cell. Each block holds its score (fast rise, ~0.5 s fall) and coarsens only past a margin, so it does not flicker. Block borders keep one vertex per cell (no cracks). Tune with r_riverMeshTurbulence; inspect with r_showtris 128.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_blocks::adaptive()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_blocks::set_adaptive(enabled);
    app.push_console_line(format!("^3r_riverAdaptiveMesh:^7 {} (renderer only, solver unchanged)", if enabled { "B by turbulence" } else { "A fixed" }));
    Ok(())
}),
CvarDef::new("r_riverMeshTurbulence", "1", "0..8", "r_riverAdaptiveMesh 1 sensitivity: multiplies the measured turbulence. Higher keeps more blocks at the tile resolution; lower lets more water use the coarse meshes. 0 makes all wet water the coarsest mesh (banks 4-cell).", CvarType::Float { min: 0.0, max: 8.0 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Float(jka_river::river_blocks::sensitivity()), |app, value| {
    let CvarValue::Float(sensitivity) = value else { unreachable!() };
    jka_river::river_blocks::set_sensitivity(sensitivity);
    app.push_console_line(format!("^3r_riverMeshTurbulence:^7 {sensitivity}"));
    Ok(())
}),
CvarDef::new("r_riverMaterialLOD", "0", "0|1|2", "River fragment shading A/B (session only): 0 native 12-sample advection and bicubic foam; 1 screen-space filtered 3-sample advection at coarse pixel footprints; 2 also switches subpixel foam-field filtering to bilinear. Applies to window and distant water, base and reflection composite. Simulation, WPO, and SSR unchanged.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(crate::river_runtime::material_lod())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::river_runtime::set_material_lod(mode as u8);
    app.push_console_line(format!("^3r_riverMaterialLOD:^7 {}", match mode { 0 => "0 native (A)", 1 => "1 adaptive advected wave sampling", _ => "2 adaptive waves and foam filtering" }));
    Ok(())
}),
CvarDef::new("r_riverMaterialLODCutoff", "2", "0..8", "r_riverMaterialLOD 1/2 cutoff in texture mips (bias included): pixels at or past it take the cheap 3-sample wave path. Lower = cheap path reaches nearer water (0 = everywhere). Foam (mode 2) shifts with it (4 = 1.5 foam cells per pixel, never below 1). Session only.", CvarType::Float { min: 0.0, max: 8.0 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Float(crate::river_runtime::material_lod_cutoff()), |app, value| {
    let CvarValue::Float(mip) = value else { unreachable!() };
    crate::river_runtime::set_material_lod_cutoff(mip);
    app.push_console_line(format!("^3r_riverMaterialLODCutoff:^7 {:.2} mips", crate::river_runtime::material_lod_cutoff()));
    Ok(())
}),
CvarDef::new("r_riverWire", "0", "0|1", "River wireframe (r_showtris river bit): 0 only where water draws; 1 the whole window grid and water-mesh tiles, including dry riverbed and unloaded cells (flat at the solver base height).", CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(jka_river::river_blocks::wire_mode())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    jka_river::river_blocks::set_wire_mode(mode as u8);
    app.push_console_line(format!("^3r_riverWire:^7 {}", if mode == 1 { "1 full grid (dry and unloaded cells too)" } else { "0 drawn water only" }));
    Ok(())
}),
CvarDef::new("r_riverLodMorph", "1", "0|1", "Water mesh LOD morphing (Unreal r.Water.WaterMesh.LODMorphEnabled): 1 vertices slide toward the next LOD with distance so LOD changes and T-junctions do not pop or crack; 0 off.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_far::lod_morph()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_far::set_lod_morph(enabled);
    app.push_console_line(format!("^3r_riverLodMorph:^7 {}", if enabled { "ON" } else { "OFF" }));
    Ok(())
}),
CvarDef::new("r_riverSparse", "1", "0|1", "Long-distance rivers (applies to rivers placed afterwards): 0 the native single rectangle of the authored radius; 1 sparse tiles that grow along the wet channel over the whole map, drawn through a camera window of r_riverWindow cells. Same Unreal hydraulic stages either way.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_tiles::sparse_enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_tiles::set_sparse_enabled(enabled);
    app.push_console_line(format!("^3r_riverSparse:^7 {} (re-place the river to apply)", if enabled { "ON (sparse tiles)" } else { "OFF (native rectangle)" }));
    Ok(())
}),
CvarDef::new("r_riverCaptureMode", "1", "0|1", "River bed capture A/B (applies to rivers placed afterwards): 0 legacy, all collision below the source level is Landscape and anything whose solid starts above it is dropped; 1 BSP-aware, a flood from the source finds the reachable floor (Landscape) and puts bridge decks, roofs and arches above it into Unreal's actor top/underside captures, so water passes under a bridge that sits below the source level. Same hydraulic solver either way. r_riverDebug 1 prints the classification counts and CPU time.", CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(jka_river::river_reach::capture_mode())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    jka_river::river_reach::set_capture_mode(mode as u8);
    app.push_console_line(format!("^3r_riverCaptureMode:^7 {} (re-place the river to apply)", if mode == 1 { "1 BSP-aware (actors above the reachable bed)" } else { "0 legacy (source-level clip)" }));
    Ok(())
}),
CvarDef::new("r_riverAdvectDepth", "0", "0|1", "River mass conservation (applies to rivers placed afterwards): 0 (not Unreal) moves water depth only by the solver's conservative face fluxes, so basins fill to the source level; 1 Unreal's AdvectWater also advects depth semi-Lagrangian, which deletes water where flow converges or slows (a pond under a cascade stalls part-filled). riverdump replays a frame both ways.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_tiles::advect_depth()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_tiles::set_advect_depth(enabled);
    app.push_console_line(format!("^3r_riverAdvectDepth:^7 {} (re-place the river to apply)", if enabled { "1 Unreal depth advection (loses water)" } else { "0 conservative" }));
    Ok(())
}),
CvarDef::new("r_riverTileSize","32", "16|32|64|128", "Sparse river tile side in cells (power of two). Smaller tiles follow narrow channels more tightly; larger tiles need fewer activations.", CvarType::Int { min: 16, max: 128 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(jka_river::river_tiles::tile_size() as i32), |app, value| {
    let CvarValue::Int(cells) = value else { unreachable!() };
    jka_river::river_tiles::set_tile_size(cells as u32);
    app.push_console_line(format!("^3r_riverTileSize:^7 {} cells", jka_river::river_tiles::tile_size()));
    Ok(())
}),
CvarDef::new("r_riverTileGuard", "2", "1..4", "Sparse river guard ring in tiles kept resident around wet tiles. Wider rings let fast-fill run more solver frames per occupancy readback.", CvarType::Int { min: 1, max: 4 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(jka_river::river_tiles::guard_tiles() as i32), |app, value| {
    let CvarValue::Int(tiles) = value else { unreachable!() };
    jka_river::river_tiles::set_guard_tiles(tiles as u32);
    app.push_console_line(format!("^3r_riverTileGuard:^7 {} tiles", jka_river::river_tiles::guard_tiles()));
    Ok(())
}),
CvarDef::new("r_riverTileSchedule", "0", "0|1|2", "Sparse river solver tile scheduling A/B (live, exact at any solver frame): 0 native, every stage over every resident tile; 1 the same through a GPU slot worklist and indirect dispatch (parity check); 2 dormant-dry: skip resident tiles that are completely dry and still with no water, source or body within the halo. Every tile that runs still takes every native substep; this is not local time stepping. r_riverDebug 1 prints scheduled/resident tiles.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(jka_river::river_tiles::tile_schedule())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    jka_river::river_tiles::set_tile_schedule(mode as u8);
    let mode = jka_river::river_tiles::tile_schedule();
    app.push_console_line(format!("^3r_riverTileSchedule:^7 {} {}", mode, jka_river::river_tiles::tile_schedule_name(mode)));
    Ok(())
}),
CvarDef::new("r_riverStorageMode", "2", "0|1|2", "Sparse river pool layout A/B, applied when a river spawns (respawn to switch). Default 2. 0 native (reference): 100 bytes/cell, PreSim history, WaterInfo and FP32 normals stored. 1 lean: history and the always-dry WaterInfo derived, no PreSim snapshot pass and no identity MatchSpline pass per substep, all stages in one compute pass, workgroup-memory surface smoothing, and tiles that stay dry outside every guard ring are released (recommitted when water returns); 80 bytes/cell, bitwise identical water, about 35% less solver GPU time measured. 2 packed: 1 plus octahedral-packed normals; 68 bytes/cell, identical water, normals within ~1e-4 rad. FP32 hydraulics in every mode. r_riverDebug 1 prints the layout and pool size.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(jka_river::river_tiles::storage_mode())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    jka_river::river_tiles::set_storage_mode(mode as u8);
    let mode = jka_river::river_tiles::storage_mode();
    app.push_console_line(format!("^3r_riverStorageMode:^7 {} {} (next river spawn)", mode, jka_river::river_tiles::storage_mode_name(mode)));
    Ok(())
}),
CvarDef::new("r_riverWindow", "512", "128..2048", "Sparse river presentation window in cells around the camera (rounded to whole tiles). The whole river is simulated; only this window is drawn.", CvarType::Int { min: 128, max: 2048 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(jka_river::river_tiles::window_cells() as i32), |app, value| {
    let CvarValue::Int(cells) = value else { unreachable!() };
    jka_river::river_tiles::set_window_cells(cells as u32);
    app.push_console_line(format!("^3r_riverWindow:^7 {} cells", jka_river::river_tiles::window_cells()));
    Ok(())
}),
CvarDef::new("cm_collisionMerge", "0", "0|1", "Experimental collision union: 0 exact OpenJK BSP collision, 1 merge perfectly flush solid box brushes at map load. Live A/B switch; affects local movement prediction and may differ from vanilla servers.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_movement::collision_merge_enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_movement::set_collision_merge_enabled(enabled);
    app.push_console_line(format!("^3cm_collisionMerge:^7 {}. BSP solid-box unions are prebuilt on map load; no reload needed. Online prediction parity is not guaranteed.", if enabled { "ON" } else { "OFF (stock OpenJK)" }));
    Ok(())
}),
CvarDef::new("cg_debugEvents", "0", "0|1|2|3", "Event diagnostics: 1 accepted+status, 2 adds suppressed candidates, 3 adds entity/resource detail.", CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None, |app| CvarValue::Int(app.cg_debug_events.into()), |app, value| {
let CvarValue::Int(mode) = value else { unreachable!() };
let mode = mode as u8;
                app.cg_debug_events = mode;
                app.push_console_line(match mode {
                    0 => "^3cg_debugEvents:^7 OFF".to_owned(),
                    1 => "^3cg_debugEvents:^7 accepted events + current dispatch status".to_owned(),
                    2 => {
                        "^3cg_debugEvents:^7 accepted events + suppressed duplicate/zero candidates"
                            .to_owned()
                    }
                    _ => "^3cg_debugEvents:^7 verbose entity/resource diagnostics".to_owned(),
                });

Ok(())
}),
CvarDef::new("cg_asyncAssets", if crate::asset_jobs::DEFAULT_ASYNC_ENABLED { "1" } else { "0" }, "0|1", "Runtime asset cache misses (player/saber/weapon Ghoul2 models): 1 load on background workers and keep the previous or default model until ready, 0 old synchronous registration. Developer A/B switch.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::asset_jobs::async_enabled()), |app, value| {
let CvarValue::Bool(enabled) = value else { unreachable!() };


                crate::asset_jobs::set_async_enabled(enabled);
                let workers = crate::asset_jobs::pool().map_or(0, |pool| pool.worker_count());
                app.push_console_line(if enabled {
                    format!("^3cg_asyncAssets:^7 ON ({workers} asset worker thread(s); cache misses never block the frame)")
                } else {
                    "^3cg_asyncAssets:^7 OFF (synchronous registration on the presenting thread)".to_owned()
                });

Ok(())
}),
CvarDef::new("cg_eventWorkers", "1", "0|1", "Event semantic preparation: 0 CGame thread, 1 dedicated worker pool (default); ordered side effects remain on CGame.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |app| CvarValue::Bool(app.cg_event_workers), |app, value| {
let CvarValue::Bool(enabled) = value else { unreachable!() };


                app.cg_event_workers = enabled;
                let workers = if enabled {
                    event_workers::worker_count()
                } else {
                    0
                };
                app.push_console_line(if enabled {
                    format!("^3cg_eventWorkers:^7 ON ({workers} pool thread(s); single-event batches stay inline)")
                } else {
                    "^3cg_eventWorkers:^7 OFF (event preparation runs on the CGame thread)".to_owned()
                });
                app.publish_ui();

Ok(())
}),
];
