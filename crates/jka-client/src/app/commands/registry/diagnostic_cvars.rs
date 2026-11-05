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
CvarDef::new("r_riverDebug", "0", "0|1|2", "Live riversource diagnostics: 0 off, 1 one-second solver/draw status, 2 detailed spawn geometry and CPU encoding timings. Session-only; no GPU readbacks or stalls.", CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None, |_app| CvarValue::Int(crate::river_runtime::debug_mode() as i32), |app, value| {
    let CvarValue::Int(mode) = value else { unreachable!() };
    crate::river_runtime::set_debug_mode(mode as u8);
    app.push_console_line(format!("^3r_riverDebug:^7 {}", match mode {
        0 => "OFF",
        1 => "ON (river progress and draw eligibility every second)",
        _ => "VERBOSE (plus source bed geometry and CPU encode timings; no GPU stalls)",
    }));
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
