//! Render Studio export profiler. Each output frame is timestamped across its
//! round trip (app build -> render thread -> GPU readback -> app -> FFmpeg).
//! With offscreen captures two frames are in flight, so the app and render
//! "lanes" overlap: the slower lane sets the export rate. Written as a
//! per-frame CSV next to the export plus a console summary.
use crate::runtime::VideoFrameTiming;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

const STAGES: usize = 11;
const STAGE_NAMES: [&str; STAGES] = ["app_wait", "audio", "to_render", "render_wait", "render_cpu",
    "gpu_readback", "worker", "delivery", "crop", "encode_pipe", "depth_pipe"];
const STAGE_HINTS: [&str; STAGES] = [
    "main thread building the frame: demo seek/decode, scene, UI, snapshot (split below)",
    "native demo audio mix for this frame",
    "request reaches the render thread (its command drain)",
    "queued behind the capture in flight / until the render loop takes it (waiting, not work)",
    "render thread recording the export sources + composite (CPU)",
    "GPU latency: submit -> mapped (overlaps recording the next frame)",
    "unpack worker: queue + repack out of the mapped readback (off the render thread)",
    "render thread -> app event loop hand-off",
    "crop of a window readback to the viewer (0 for offscreen resolutions)",
    "hand-off to the FFmpeg writer thread: blocks only when FFmpeg falls behind",
    "hand-off to the depth EXR writer thread",
];
/// Stages done by the main thread / render-thread CPU work for one frame.
/// The repack runs on the unpack worker; GPU time overlaps all of them.
const APP_LANE: [usize; 5] = [0, 1, 8, 9, 10];
const RENDER_LANE: [usize; 1] = [4];

/// Main-thread work measured between frames, attributed to the next request.
#[derive(Clone, Copy)]
pub(in crate::app) enum StudioAppCost { Scene, Ui, Snapshot }
const APP_COSTS: usize = 3;
const APP_COST_NAMES: [&str; APP_COSTS] = ["scene_build", "ui", "snapshot"];

#[derive(Clone, Copy)]
struct Row {
    frame: u64,
    stages: [f32; STAGES],
    app_costs: [f32; APP_COSTS],
    gpu_wait: f32,
    readback_copy: f32,
    not_ready_ticks: u32,
    total: f32,
}

struct Pending {
    frame: u64,
    requested: Instant,
    app_wait: f64,
    audio_ms: f64,
    app_costs: [f64; APP_COSTS],
    not_ready_ticks: u32,
}

/// Milliseconds the app spent on one delivered frame (measured by the caller).
#[derive(Clone, Copy, Default)]
pub(in crate::app) struct StudioFrameAppCost {
    pub crop_ms: f64,
    pub encode_pipe_ms: f64,
    pub depth_pipe_ms: f64,
}

pub(in crate::app) struct StudioExportProfile {
    started: Instant,
    max_in_flight: u64,
    last_done: Instant,
    last_requested: Option<Instant>,
    /// (frame, delivered at) of recent frames: frame N-max_in_flight's
    /// delivery freed the pipeline slot that frame N is built in.
    done: VecDeque<(u64, Instant)>,
    pending: VecDeque<Pending>,
    app_costs: [f64; APP_COSTS],
    not_ready_ticks: u32,
    rows: Vec<Row>,
    size: (u32, u32),
}

fn ms(later: Instant, earlier: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64() * 1000.0
}

impl StudioExportProfile {
    pub fn new(max_in_flight: u64) -> Self {
        let now = Instant::now();
        Self { started: now, max_in_flight: max_in_flight.max(1), last_done: now,
            last_requested: None, done: VecDeque::new(), pending: VecDeque::new(),
            app_costs: [0.0; APP_COSTS], not_ready_ticks: 0, rows: Vec::new(), size: (0, 0) }
    }
    /// An export tick that could not request a frame yet (scene/decoder pending).
    pub fn not_ready(&mut self) { self.not_ready_ticks += 1; }
    pub fn app_cost(&mut self, kind: StudioAppCost, ms: f64) { self.app_costs[kind as usize] += ms; }
    pub fn requested(&mut self, frame: u64, audio_ms: f64) {
        let now = Instant::now();
        let slot_free = frame.checked_sub(self.max_in_flight)
            .and_then(|freed_by| self.done.iter().find(|(f, _)| *f == freed_by).map(|(_, at)| *at))
            .unwrap_or(self.started);
        let build_start = self.last_requested.map_or(slot_free, |last| last.max(slot_free));
        self.pending.push_back(Pending {
            frame, requested: now, app_wait: ms(now, build_start), audio_ms,
            app_costs: std::mem::take(&mut self.app_costs),
            not_ready_ticks: std::mem::take(&mut self.not_ready_ticks),
        });
        self.last_requested = Some(now);
    }
    /// `received` = the moment the app got the frame event; call when it is written.
    pub fn frame(&mut self, frame: u64, size: (u32, u32), timing: &VideoFrameTiming,
        received: Instant, cost: StudioFrameAppCost) {
        let done = Instant::now();
        let Some(pending) = self.pending.iter().position(|p| p.frame == frame)
            .and_then(|i| self.pending.remove(i)) else { return; };
        let stages = [
            pending.app_wait,
            pending.audio_ms,
            ms(timing.received, pending.requested),
            ms(timing.frame_started, timing.received),
            ms(timing.presented, timing.frame_started),
            ms(timing.queued, timing.presented),
            ms(timing.worker_done, timing.queued),
            ms(received, timing.worker_done),
            cost.crop_ms, cost.encode_pipe_ms, cost.depth_pipe_ms,
        ];
        self.rows.push(Row {
            frame, stages: stages.map(|v| v as f32), app_costs: pending.app_costs.map(|v| v as f32),
            gpu_wait: timing.gpu_wait_ms as f32, readback_copy: timing.readback_copy_ms as f32,
            not_ready_ticks: pending.not_ready_ticks, total: ms(done, self.last_done) as f32,
        });
        self.size = size;
        self.last_done = done;
        self.done.push_back((frame, done));
        if self.done.len() > 8 { self.done.pop_front(); }
    }

    /// Writes the CSV and returns (console lines, one-line status suffix).
    pub fn finish(&self, csv: &Path, encoder_flush_ms: f64, mux_ms: f64, settings: &str) -> (Vec<String>, String) {
        let mut text = String::from("frame,total_ms");
        for name in STAGE_NAMES { let _ = write!(text, ",{name}_ms"); }
        for name in APP_COST_NAMES { let _ = write!(text, ",{name}_ms"); }
        text.push_str(",gpu_wait_ms,readback_copy_ms,not_ready_ticks\n");
        for row in &self.rows {
            let _ = write!(text, "{},{:.3}", row.frame, row.total);
            for v in row.stages { let _ = write!(text, ",{v:.3}"); }
            for v in row.app_costs { let _ = write!(text, ",{v:.3}"); }
            let _ = writeln!(text, ",{:.3},{:.3},{}", row.gpu_wait, row.readback_copy, row.not_ready_ticks);
        }
        let csv_note = match std::fs::write(csv, text) {
            Ok(()) => csv.display().to_string(),
            Err(error) => format!("not written ({error})"),
        };
        let frames = self.rows.len().max(1) as f64;
        let frame_wall: f64 = self.rows.iter().map(|r| f64::from(r.total)).sum();
        let fps = self.rows.len() as f64 / (frame_wall / 1000.0).max(1e-6);
        let per_frame = frame_wall / frames;
        let wall = ms(Instant::now(), self.started);
        let avg = |f: &dyn Fn(&Row) -> f32| self.rows.iter().map(|r| f64::from(f(r))).sum::<f64>() / frames;
        let mut lines = vec![format!(
            "^2RENDER STUDIO PERF:^7 {} frames {}x{} in {:.1}s = {fps:.1} fps (frames {:.1}s + finish {:.1}s) · {} in flight · {settings}",
            self.rows.len(), self.size.0, self.size.1, wall / 1000.0, frame_wall / 1000.0,
            (encoder_flush_ms + mux_ms) / 1000.0, self.max_in_flight)];
        let means: Vec<f64> = (0..STAGES).map(|s| avg(&|r| r.stages[s])).collect();
        let mut order: Vec<usize> = (0..STAGES).collect();
        order.sort_by(|a, b| means[*b].total_cmp(&means[*a]));
        let p95 = |s: usize| {
            let mut v: Vec<f32> = self.rows.iter().map(|r| r.stages[s]).collect();
            v.sort_by(f32::total_cmp);
            v.get(((v.len() as f64 * 0.95) as usize).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0)
        };
        for &s in &order {
            if means[s] < 0.05 { continue; }
            lines.push(format!("  {:<12} {:>7.2} ms avg {:>7.2} p95 {:>4.0}% of frame  {}",
                STAGE_NAMES[s], means[s], p95(s), means[s] / per_frame.max(1e-9) * 100.0, STAGE_HINTS[s]));
        }
        let costs: Vec<f64> = (0..APP_COSTS).map(|c| avg(&|r| r.app_costs[c])).collect();
        let tick_other = (means[0] - costs.iter().sum::<f64>()).max(0.0);
        lines.push(format!("  app_wait = scene_build {:.2} + ui {:.2} + snapshot {:.2} + other tick {:.2} ms · not_ready ticks/frame {:.2}",
            costs[0], costs[1], costs[2], tick_other, avg(&|r| r.not_ready_ticks as f32)));
        lines.push(format!("  gpu_readback until mapped {:.2} ms · repack {:.2} ms (unpack worker) · encoder flush {:.0} ms · mux {:.0} ms",
            avg(&|r| r.gpu_wait), avg(&|r| r.readback_copy), encoder_flush_ms, mux_ms));
        let app_lane: f64 = APP_LANE.iter().map(|&s| means[s]).sum();
        let render_lane: f64 = RENDER_LANE.iter().map(|&s| means[s]).sum();
        let unpack_lane = avg(&|r| r.readback_copy);
        let verdict = if self.max_in_flight > 1 {
            let busiest = app_lane.max(render_lane).max(unpack_lane);
            if means[9] > per_frame * 0.15 {
                // The writer queue is full: the main thread waits on FFmpeg.
                format!("FFmpeg encoder (queue full: main thread waits {:.2} ms/frame, encoder flush {encoder_flush_ms:.0} ms; \r
                    try a faster preset or hardware encoder)", means[9])
            } else if per_frame > busiest * 1.15 {
                // Neither CPU lane fills the frame interval: the GPU is the limit.
                format!("GPU (frame {per_frame:.2} ms vs main {app_lane:.2}, render {render_lane:.2}, unpack {unpack_lane:.2}; GPU latency {:.2} ms)",
                    means[5])
            } else {
                let name = if busiest == app_lane { "main thread" }
                    else if busiest == render_lane { "render thread" } else { "unpack worker" };
                format!("{name} {busiest:.2} ms/frame (main {app_lane:.2}, render {render_lane:.2}, unpack {unpack_lane:.2}; frame {per_frame:.2} ms)")
            }
        } else {
            let top = order[0];
            format!("{} {:.0}% (serial: app {app_lane:.2} + render {render_lane:.2} ms)",
                STAGE_NAMES[top], means[top] / per_frame.max(1e-9) * 100.0)
        };
        lines.push(format!("  Bottleneck: {verdict} · CSV: {csv_note}"));
        (lines, format!(" · {fps:.1} fps export, bottleneck {verdict}"))
    }
}
