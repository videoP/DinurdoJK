use crate::{
    app::{event_workers, App},
    console::{
        cvars::{CvarDef, CvarFlags, CvarType, CvarValue},
        LatchScope,
    },
};
pub(super) const CVARS: &[CvarDef<App>] = &[
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
