use crate::{
    app::{event_workers, App},
    console::{
        cvars::{CvarDef, CvarFlags, CvarType, CvarValue},
        LatchScope,
    },
};
pub(super) const CVARS: &[CvarDef<App>] = &[
CvarDef::new("r_ninjaFog", "0", "0|1", "Fluid Ninja ground fog. Enable, then use ninjafog in a loaded map. 0 clears the volume. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fog_debug::enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    if enabled && !jka_ninja::INCLUDED {
        app.push_console_line("^3[NINJA FOG]^7 Fluid Ninja is not included in this build".to_owned());
    } else {
        crate::ninja_fog_debug::set_enabled(enabled);
    }
    Ok(())
}),
CvarDef::new("r_shaderPromotion", "1", "0|1", "Apply authored q3map_dinurdoPromoted overrides (hide or swap map shaders such as seabed caustics) while their promotion system (ocean, river, water, fire, smoke) is active. 0 restores the authored shaders. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::app::shader_promotion::enabled()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::app::shader_promotion::set_enabled(enabled);
    Ok(())
}),
CvarDef::new("r_ninjaFirePromote","0", "0|1", "Replace supported stationary ET_FX flame layers with Ninja fire and Burning Man sparks. Smoke is controlled separately by r_ninjaSmokePromote. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::promote()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    if enabled && !jka_ninja::INCLUDED {
        app.push_console_line("^3[NINJA FIRE]^7 Fluid Ninja is not included in this build".to_owned());
        return Ok(());
    }
    crate::ninja_fire_debug::set_promote(enabled);
    if let Some(session) = app.game_session.as_mut() {
        session.weapon_fx.set_runner_culling(app.video.fx_occlusion == 1 || crate::ninja_fire_debug::promotion_enabled());
    }
    if enabled && crate::ninja_fire_debug::brushes_wanted() {
        if let Some(map) = app.game_session.as_ref().and_then(|session| session.map_name.clone()) {
            app.ensure_ninja_fire_brushes(&map);
        }
    }
    Ok(())
}),
CvarDef::new("r_ninjaSmokePromote", "0", "0|1", "Promote supported upright particle smoke and steam from continuous stationary ET_FX emitters to Ninja smoke. The two nearest eligible sources use volumes once the GPU is ready. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::promote_smoke()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    if enabled && !jka_ninja::INCLUDED {
        app.push_console_line("^3[NINJA SMOKE]^7 Fluid Ninja is not included in this build".to_owned());
        return Ok(());
    }
    crate::ninja_fire_debug::set_promote_smoke(enabled);
    if let Some(session) = app.game_session.as_mut() {
        session.weapon_fx.set_runner_culling(app.video.fx_occlusion == 1 || crate::ninja_fire_debug::promotion_enabled());
    }
    Ok(())
}),
CvarDef::new("r_ninjaFireEmbers", "0", "0|1", "Fire_Smoke_Explosive flamethrower-turret embers (bright streaks and soft settling dots, colliding with solid map brushes) from every Ninja fire source: placed, promoted and Burning Man. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::embers()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ninja_fire_debug::set_embers(enabled);
    if enabled && jka_ninja::INCLUDED {
        if let Some(map) = app.game_session.as_ref().and_then(|session| session.map_name.clone()) {
            app.ensure_ninja_fire_brushes(&map);
        }
    }
    Ok(())
}),
CvarDef::new("r_ninjaFireHeatHaze", "0", "0|1", "Heat haze: the scene behind Ninja surface flames shimmers through scrolling curl noise, masked by the flame field's own density. Costs only the pixels under the flame quads; nothing while no flame is on screen. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::heat_haze()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ninja_fire_debug::set_heat_haze(enabled);
    Ok(())
}),
CvarDef::new("r_ninjaFireFlicker", "1", "0|1", "Animate the ninjafireon point light with the existing two-octave smooth flicker curve. 0 restores the authored steady intensity. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::flicker()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ninja_fire_debug::set_flicker(enabled);
    Ok(())
}),
CvarDef::new("r_ninjaFireAirflow", "0", "0|1", "Thin the outer Ninja flame and brighten its ember-red core in fast air, with a brief flare as the airflow subsides. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::airflow_response()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ninja_fire_debug::set_airflow_response(enabled);
    Ok(())
}),
CvarDef::new("r_ninjaFireCollision", "1", "0|1", "Fluid Ninja MeshField collision against solid BSP world brushes. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ninja_fire_debug::collision()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ninja_fire_debug::set_collision(enabled);
    if enabled && jka_ninja::INCLUDED {
        if let Some(map) = app.game_session.as_ref().and_then(|session| session.map_name.clone()) {
            app.ensure_ninja_fire_brushes(&map);
        }
    }
    Ok(())
}),
CvarDef::new("r_lampShadowDebug", "0", "0|1|2", "Session-only: 0 off, 1 single diagnostic snapshot after next local-shadow selection, 2 repeat on player move/PVS reselection. Reports flare candidates, grouping, slots, plus post-encode moving-caster face masks and shader gate states in console/latest.log. No GPU readback or render changes.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::renderer::lamp_shadow_debug_mode() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::renderer::set_lamp_shadow_debug_mode(mode as u8);
    app.render_command(crate::renderer::RenderCommand::SetLampShadowDebug(mode as u8));
    app.push_console_line(format!("^3[LAMP DEBUG]^7 mode {mode}: {}. See console or DinurdoJK/logs/latest.log.", match mode {
        0 => "OFF", 1 => "one snapshot at current player location", _ => "detailed snapshots on each 32-unit/PVS reselection",
    }));
    Ok(())
}),
CvarDef::new("cg_remoteSmoothing", "0", "0..5", "Live remote visuals: 0 original; 1 buffered linear/hold; 2 buffered linear + bounded extrapolation; 3 Hermite + extrapolation; 4 jaMME-inspired commandTime-aware Hermite; 5 TaystJK cg_smoothClients 1 (native trajectory logic, no added buffer). Modes 1-4 also resample spectator follow; mode 5 keeps the legacy spectator-follow clock. Session-only.", CvarType::Int { min: 0, max: 5 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::mode() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_mode(mode as u8);
    if let Some(session) = app.game_session.as_mut() { session.remote_smoothing.sync_from_cvar(); }
    app.push_console_line(format!("^3cg_remoteSmoothing:^7 mode {mode} (0=original, 1=buffer+hold, 2=buffer+extrap, 3=buffer+Hermite+extrap, 4=jaMME-inspired, 5=TaystJK cg_smoothClients 1: native trajectory rules, no visual buffer). Modes 1-4 resample spectator follow; demos unchanged."));
    Ok(())
}),
CvarDef::new("cg_remoteBufferMs", "50", "0..100", "Extra visual-only snapshot interpolation delay (ms) for modes 1-4; 0 for low latency; used when cg_remoteBufferAuto=0. Ignored in mode 5. Session only.", CvarType::Int { min: 0, max: 100 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::buffer_ms() as i32), |_app, value| {
    let CvarValue::Int(ms) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_buffer_ms(ms as u32);
    Ok(())
}),
CvarDef::new("cg_remoteBufferAuto", "0", "0|1", "0 fixed cg_remoteBufferMs; 1 adaptive visual buffer for modes 1-4 from server snapshot cadence and arrival jitter (up to 100 ms). Ignored in mode 5. Session only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::cgame::remote_smoothing::buffer_auto()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_buffer_auto(enabled);
    Ok(())
}),
CvarDef::new("cg_remoteExtrapMaxMs", "35", "0..100", "Maximum visual-only extrapolation beyond the latest remote snapshot for smoothing modes 2/3/4 (not mode 5). 0 disables extrapolation. Session only.", CvarType::Int { min: 0, max: 100 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::extrap_max_ms() as i32), |_app, value| {
    let CvarValue::Int(ms) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_extrap_max_ms(ms as u32);
    Ok(())
}),
CvarDef::new("cg_remoteSmoothDebug", "0", "0..2", "0 off; 1 one-second smoothing/underrun/correction summary; 2 also logs per remote player (modes 1-4 only). Mode 5 reports native path, without history counters. No render overhead/readbacks. Session only.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::remote_smoothing::debug() as i32), |_app, value| {
    let CvarValue::Int(level) = value else { unreachable!() };
    crate::cgame::remote_smoothing::set_debug(level as u8);
    Ok(())
}),
CvarDef::new("cg_specCamDebug", "0", "0..20000", "Live spectator-camera jitter log: N logs the next N rendered follow frames to latest.log (SPECCAM lines: frame/presentation/damping clocks, snapshot times and commandTimes, raw vs presented origin/angles, ideal vs damped target/location, final camera). 0 stops. One-shot, session only, no render cost when 0.", CvarType::Int { min: 0, max: 20000 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(0), |app, value| {
    let CvarValue::Int(frames) = value else { unreachable!() };
    app.arm_spectator_camera_debug(frames as u32);
    app.push_console_line(format!("^3cg_specCamDebug:^7 {}", if frames > 0 {
        format!("logging the next {frames} spectator-follow frames to latest.log (grep SPECCAM)")
    } else { "OFF".to_owned() }));
    Ok(())
}),
CvarDef::new("cg_footstepDebug", "0", "0..3", "JK2 footstep pipeline diagnostics, session only: 0 off; 1 a two-second counter summary (legs-frame advances, due footfalls, ground/cg_footsteps gates, probe misses and SURF_NOSTEPS, queued, submitted to audio, voice pool, replacements); 2 also one rate-limited line per candidate (entity, animation, frames, groundEntityNum, bolt, trace fraction/entity/flags); 3 as 2 with the JK2 ground probe restricted to the world BSP (A/B against the entity-aware CG_Trace). Output goes to the log.", CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::cgame::footsteps::debug_level() as i32), |_app, value| {
    let CvarValue::Int(level) = value else { unreachable!() };
    crate::cgame::footsteps::set_debug_level(level as u8);
    Ok(())
}),
CvarDef::new("cg_remoteSmoothStats", "0", "0|1", "Write 1 to print an immediate remote visual history/jitter/underrun report (modes 1-4), or the native mode-5 status, to the in-game console. Read-back remains 0 (one-shot command cvar).", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(false), |app, value| {
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
CvarDef::new("r_riverPromotion", "0", "0|1|2|3", "Experimental authored river promotion on map reload. 0 off, 1 log candidates and hydraulic plans, 2 preview with authored water visible, 3 replace eligible water after convergence.", CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(crate::scene::river_promotion::mode())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::scene::river_promotion::set_mode(mode as u8);
    app.push_console_line(format!("r_riverPromotion {mode}: reload map to apply"));
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
CvarDef::new("r_ninjaWakePush", "1", "0..4", "Fluid Ninja actor wake push strength (Ninja ocean, Coast and Source-lake wake; not the river): scales the composite VeloFromBrushMotion of the shared wake. 1 = the Fluid Ninja Creek demo (water pushed up to ~8 m/s, can run ahead of a slower body); .25 = the Fluid Ninja Lake demo's own value. Session only, for A/B.", CvarType::Float { min: 0.0, max: 4.0 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Float(jka_ninja::wake_push()), |app, value| {
    let CvarValue::Float(scale) = value else { unreachable!() };
    jka_ninja::set_wake_push(scale);
    app.push_console_line(format!("^3r_ninjaWakePush:^7 {scale}"));
    Ok(())
}),
CvarDef::new("r_riverNinjaDebug", "0", "0|1", "Fluid Ninja river wake diagnostics. Logs actor capture, GPU contact, hash and painter samples once per second while the Fluid Ninja river is built in.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_ninja::wake_debug_enabled()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_ninja::set_wake_debug(enabled);
    app.push_console_line(format!("^3r_riverNinjaDebug:^7 {}", if enabled { "ON" } else { "OFF" }));
    Ok(())
}),
CvarDef::new("r_riverNinjaMaterialDebug", "0", "0..5", "Above-water Fluid Ninja material A/B: 0 full material, 1 omit wetmap foam color, 2 bypass the geometry-distance coverage mask, 3 omit scene refraction, 4 omit in-scattered light while retaining extinction, 5 display raw wetmap. Live and session only.", CvarType::Int { min: 0, max: 5 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(jka_river::river_ninja::material_debug() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    jka_river::river_ninja::set_material_debug(mode as u32);
    app.push_console_line(format!("^3r_riverNinjaMaterialDebug:^7 {}", match mode {
        1 => "Wetmap foam color omitted",
        2 => "Geometry-distance coverage mask bypassed",
        3 => "Scene refraction omitted",
        4 => "Volume in-scattering omitted",
        5 => "Raw wetmap",
        _ => "Full material",
    }));
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
CvarDef::new("r_riverAdaptiveMesh", "1", "0|1", "River mesh A/B (renderer only; the solver and riverbed do not change): 0 one vertex per solver cell everywhere; 1 fewer triangles for calm water: per 16x16-cell block, from the water's turbulence (surface curvature and flow speed), no distance term. Turbulent water keeps the tile resolution (the maximum); calmer water drops to 2/4/8-cell triangles; banks stay at least 4-cell. Each block holds its score (fast rise, ~0.5 s fall) and coarsens only past a margin, so it does not flicker. Block borders keep one vertex per cell (no cracks). Tune with r_riverMeshTurbulence; inspect with r_showtris 128.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_blocks::adaptive()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_blocks::set_adaptive(enabled);
    app.push_console_line(format!("^3r_riverAdaptiveMesh:^7 {} (renderer only, solver unchanged)", if enabled { "B by turbulence" } else { "A fixed" }));
    Ok(())
}),
CvarDef::new("r_riverBlockFrustum", "0", "0|1", "River near-window draw A/B: reject wet blocks outside a conservative horizontal view wedge before vertex work. Does not assume a water height, affect foam/physics, or cull when the view includes the vertical axis. Requires r_riverBlockCull 1.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(jka_river::river_blocks::frustum_enabled()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    jka_river::river_blocks::set_frustum_enabled(enabled);
    Ok(())
}),
CvarDef::new("r_riverMeshTurbulence", "1", "0..8", "r_riverAdaptiveMesh 1 sensitivity: multiplies the measured turbulence. Higher keeps more blocks at the tile resolution; lower lets more water use the coarse meshes. 0 makes all wet water the coarsest mesh (banks 4-cell).", CvarType::Float { min: 0.0, max: 8.0 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Float(jka_river::river_blocks::sensitivity()), |app, value| {
    let CvarValue::Float(sensitivity) = value else { unreachable!() };
    jka_river::river_blocks::set_sensitivity(sensitivity);
    app.push_console_line(format!("^3r_riverMeshTurbulence:^7 {sensitivity}"));
    Ok(())
}),
CvarDef::new("r_riverMaterialLOD", "1", "0|1|2", "River fragment shading A/B (session only): 0 native 12-sample advection and bicubic foam; 1 screen-space filtered 3-sample advection at coarse pixel footprints; 2 also switches subpixel foam-field filtering to bilinear. Applies to window and distant water, base and reflection composite. Simulation, WPO, and SSR unchanged.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(crate::river_runtime::material_lod())), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::river_runtime::set_material_lod(mode as u8);
    app.push_console_line(format!("^3r_riverMaterialLOD:^7 {}", match mode { 0 => "0 native (A)", 1 => "1 adaptive advected wave sampling", _ => "2 adaptive waves and foam filtering" }));
    Ok(())
}),
CvarDef::new("r_riverFusedMaterial", "1", "0|1", "River render A/B: 1 evaluates base water and its additive SSR/environment reflection in a single draw for a single live river; 0 retains the two-pass reference. Multiple live rivers always use the reference ordering. No solver or SSR quality change. May differ at coplanar edges, MSAA silhouettes and intermediate color rounding.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::fused_material()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::river_runtime::set_fused_material(enabled);
    Ok(())
}),
CvarDef::new("r_riverAdaptiveSSR", "0", "0|1", "River reflection A/B: reduce the native 16 SSR steps to 4..16 for short projected rays. Long rays retain native quality; short-ray hit positions may differ. Render only.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::adaptive_ssr()), |_app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::river_runtime::set_adaptive_ssr(enabled);
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
CvarDef::new("r_underwaterReflections", "1", "0|1", "Underwater surface reflections (live), Hybrid/Blue Water ocean and rivers: 1 the surface seen from below mirrors the submerged scene (screen traced) where it reflects, fully past Snell's window; 0 shows the water's own color there (cheaper: no trace).", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ocean::underwater_reflections()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ocean::set_underwater_reflections(enabled);
    app.push_console_line(format!("^3r_underwaterReflections:^7 {}", if enabled { "1 mirror the submerged scene" } else { "0 water color" }));
    Ok(())
}),
CvarDef::new("r_oceanUpdateSpread", "1", "0|1", "Blue Water ocean FFT pacing (live): 1 spreads each throttled update (r_oceanUpdates > 0) over the rendered frames of its period, computing it for the time it publishes; 0 encodes the whole update on its tick. The original/Hybrid ocean FFT already spreads its phases.", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::ocean::update_spread()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::ocean::set_update_spread(enabled);
    app.push_console_line(format!("^3r_oceanUpdateSpread:^7 {}", if enabled { "1 spread over rendered frames" } else { "0 whole update on its tick" }));
    Ok(())
}),
CvarDef::new("r_riverSolverSpread", "1", "0|1", "River solver pacing (live, same results): 1 encodes each 1/60 s solver frame's substeps as their time comes due, spreading its GPU cost evenly over the rendered frames; 0 encodes whole frames, a burst every 1/60 s (the native reference).", CvarType::Bool, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Bool(crate::river_runtime::solver_spread()), |app, value| {
    let CvarValue::Bool(enabled) = value else { unreachable!() };
    crate::river_runtime::set_solver_spread(enabled);
    app.push_console_line(format!("^3r_riverSolverSpread:^7 {}", if enabled { "1 spread over rendered frames" } else { "0 whole frames (native burst)" }));
    Ok(())
}),
CvarDef::new("r_riverSimRate", "60", "12..60", "River hydraulic rate in Hz (live, not Unreal). 60 native: 10 substeps of 1/600 s per 1/60 s solver frame. Lower rates keep the 1/60 s frame (surface updates, sources, drains, inlet) and run fewer, longer substeps: 30 = 5 substeps of 1/300 s, half the solver cost. Rounded to whole substeps (6 Hz each). Changes the flow slightly; the CFL velocity clamp scales with the rate.", CvarType::Int { min: 12, max: 60 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(i32::from(crate::river_runtime::sim_rate())), |app, value| {
    let CvarValue::Int(rate) = value else { unreachable!() };
    crate::river_runtime::set_sim_rate(rate.clamp(12, 60) as u8);
    let substeps = crate::river_runtime::sim_rate_substeps(10);
    app.push_console_line(format!("^3r_riverSimRate:^7 {} Hz -> {} substeps per solver frame (effective {} Hz)", rate, substeps, substeps * 6));
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
