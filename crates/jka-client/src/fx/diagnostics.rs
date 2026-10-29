//! Opt-in profiling of FX line promotion, worker tracing and legacy dlight work.
//! Toggle with `r_fxLightDiag 0|1` at runtime. No rendering path is modified.
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const TRIPMINE_PLAYS: usize = 0;
pub const FX_TRACES: usize = 1;
pub const FX_TRACE_US: usize = 2;
pub const WORKER_US: usize = 3;
pub const WORKER_CALLS: usize = 4;
pub const WORKER_WAIT_US: usize = 5;
pub const FX_END_FRAMES: usize = 6;
pub const CANDIDATE_LINES: usize = 7;
pub const CANDIDATES: usize = 8;
pub const PROMOTED: usize = 9;
pub const SPOTS: usize = 10;
pub const PROMOTION_US: usize = 11;
pub const PROMOTION_CALLS: usize = 12;
pub const MAX_SPOT_RADIUS_10: usize = 13;
pub const MAX_BEAM_LENGTH_10: usize = 14;
pub const MAX_CANDIDATES_FRAME: usize = 15;
pub const LEGACY_MASK_US: usize = 16;
pub const LEGACY_MASK_REBUILDS: usize = 17;
pub const LEGACY_MASK_HITS: usize = 18;
pub const LEGACY_BVH_VISITS: usize = 19;
pub const LEGACY_LIT_SURFACES: usize = 20;
pub const LEGACY_UPLOAD_BYTES: usize = 21;
pub const LEGACY_DRAW_US: usize = 22;
pub const LEGACY_INDICES: usize = 23;
pub const LEGACY_DRAW_GROUPS: usize = 24;
pub const MAX_PROMOTED_FRAME: usize = 25;
pub const FX_DRAW_LINES: usize = 26;
pub const FX_DRAW_TOTAL: usize = 27;
pub const ACTIVE_FX_MAX: usize = 28;
pub const SCHEDULED_FX_MAX: usize = 29;
pub const PROMOTION_LEVEL: usize = 30;
pub const FX_PLAY_US: usize = 31;
pub const FX_PLAY_CALLS: usize = 32;
const METRICS: usize = 33;
static VALUES: [AtomicU64; METRICS] = [const { AtomicU64::new(0) }; METRICS];
static ENABLED: AtomicBool = AtomicBool::new(false);
static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();

/// A session-only console cvar controls diagnostics across all FX and render threads.
/// Clear accumulated samples when changing state so each measurement starts fresh.
pub fn set_enabled(value: bool) {
    if enabled() == value { return; }
    // Block new counter updates while clearing the old reporting window.
    ENABLED.store(false, Ordering::Relaxed);
    for counter in &VALUES {
        counter.store(0, Ordering::Relaxed);
    }
    let clock = LAST.get_or_init(|| Mutex::new(Instant::now()));
    if let Ok(mut last) = clock.lock() {
        *last = Instant::now();
    }
    ENABLED.store(value, Ordering::Relaxed);
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
#[inline]
pub fn add(index: usize, amount: u64) {
    if enabled() { VALUES[index].fetch_add(amount, Ordering::Relaxed); }
}
#[inline]
pub fn peak(index: usize, value: u64) {
    if enabled() { VALUES[index].fetch_max(value, Ordering::Relaxed); }
}
#[inline]
pub fn set(index: usize, value: u64) {
    if enabled() { VALUES[index].store(value, Ordering::Relaxed); }
}
#[inline]
pub fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}
/// Called by the session once per frame; aggregates all threads once per second.
pub fn report_if_due() {
    if !enabled() { return; }
    let last = LAST.get_or_init(|| Mutex::new(Instant::now()));
    let Ok(mut last) = last.try_lock() else { return; };
    if last.elapsed() < Duration::from_secs(1) { return; }
    let elapsed = last.elapsed().as_secs_f64();
    *last = Instant::now();
    let mut n = [0u64; METRICS];
    for (i, counter) in VALUES.iter().enumerate() {
        n[i] = counter.swap(0, Ordering::Relaxed);
    }
    let avg = |sum: usize, calls: usize| n[sum] as f64 / (n[calls].max(1) as f64 * 1000.0);
    println!("[FX DIAG] {:.1}s level={} tripmine_plays={} ({:.0}/s) fx_worker={} wait_ms/frm={:.3} worker_ms/frm={:.3} traces={} trace_ms_total={:.3} plays={} play_ms/req={:.3}",
        elapsed, n[PROMOTION_LEVEL], n[TRIPMINE_PLAYS], n[TRIPMINE_PLAYS] as f64/elapsed,
        n[WORKER_CALLS], avg(WORKER_WAIT_US, FX_END_FRAMES), avg(WORKER_US, WORKER_CALLS),
        n[FX_TRACES], n[FX_TRACE_US] as f64 / 1000.0,
        n[FX_PLAY_CALLS], avg(FX_PLAY_US, FX_PLAY_CALLS));
    println!("[FX DIAG] fx_frames={} draws={} lines={} candidates={} line_candidates={} max_candidates/frame={} promoted={} spots={} max_promoted/frame={} promotion_ms/frm={:.3} max_spot_radius={:.1} max_beam_len={:.1}",
        n[FX_END_FRAMES], n[FX_DRAW_TOTAL], n[FX_DRAW_LINES], n[CANDIDATES], n[CANDIDATE_LINES], n[MAX_CANDIDATES_FRAME],
        n[PROMOTED], n[SPOTS], n[MAX_PROMOTED_FRAME], avg(PROMOTION_US,PROMOTION_CALLS),
        n[MAX_SPOT_RADIUS_10] as f64 / 10.0, n[MAX_BEAM_LENGTH_10] as f64 / 10.0);
    println!("[FX DIAG] legacy_mask rebuilds={} hits={} mask_ms_total={:.3} bvh_tests={} accepted={} uploads_kib={:.1} dlight_draw_ms_total={:.3} indices={} groups={} fx_active_max={} scheduled_max={}",
        n[LEGACY_MASK_REBUILDS], n[LEGACY_MASK_HITS], n[LEGACY_MASK_US] as f64/1000.0,
        n[LEGACY_BVH_VISITS], n[LEGACY_LIT_SURFACES], n[LEGACY_UPLOAD_BYTES] as f64/1024.0,
        n[LEGACY_DRAW_US] as f64 / 1000.0, n[LEGACY_INDICES], n[LEGACY_DRAW_GROUPS],
        n[ACTIVE_FX_MAX], n[SCHEDULED_FX_MAX]);
}
