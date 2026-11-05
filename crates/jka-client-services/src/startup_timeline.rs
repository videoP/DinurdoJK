//! Startup milestones are recorded with atomic timestamps, not synchronous log
//! output on the startup/render critical path. The origin is entry into `main`,
//! NOT Windows process creation (which requires an external OS trace).
use std::sync::{atomic::{AtomicU64, Ordering}, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy)]
pub enum Milestone {
    AppReady, WindowCreated, RendererReady, MenuBuilt, FirstPresent,
    FirstMenuPresent, MapRequested, MapPrepared, FirstMapPresent,
    LoggingReady, GammaReady, OptionsReady, EventLoopReady, AppBegin,
    LutScanReady, ConfigReady, WorkersReady, CredentialsReady, WindowShown,
}
static ORIGIN: OnceLock<Instant> = OnceLock::new();
static TIMES: [AtomicU64; 19] = [const { AtomicU64::new(0) }; 19];

pub fn start() { let _ = ORIGIN.set(Instant::now()); }
pub fn mark(phase: Milestone) {
    let Some(start) = ORIGIN.get() else { return; };
    let index = phase as usize;
    let us = start.elapsed().as_micros().min(u64::MAX as u128) as u64;
    // 0 denotes an unobserved event. Keep the first value across restarts.
    let _ = TIMES[index].compare_exchange(0, us.max(1), Ordering::Relaxed, Ordering::Relaxed);
}
fn ms(phase: Milestone) -> String {
    match TIMES[phase as usize].load(Ordering::Relaxed) {
        0 => "-".into(),
        us => format!("{:.1}", us as f64 / 1000.0),
    }
}

pub fn report(label: &str) {
    use Milestone::*;
    println!("[STARTUP TIMELINE] {label} main_entry=0ms logging={} gamma={} options={} event_loop={} app_begin={} lut={} config={} workers={} credentials={} app={} window={} window_visible={} renderer_ready={} menu_built={} first_present={} first_menu_present={} map_requested={} map_prepared={} first_map_present={} (ms from main entry; - = not yet observed)",
        ms(LoggingReady), ms(GammaReady), ms(OptionsReady), ms(EventLoopReady), ms(AppBegin),
        ms(LutScanReady), ms(ConfigReady), ms(WorkersReady), ms(CredentialsReady),
        ms(AppReady), ms(WindowCreated), ms(WindowShown), ms(RendererReady), ms(MenuBuilt),
        ms(FirstPresent), ms(FirstMenuPresent), ms(MapRequested),
        ms(MapPrepared), ms(FirstMapPresent));
}
