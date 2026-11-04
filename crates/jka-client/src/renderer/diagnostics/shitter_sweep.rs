//! One command, one log: isolate present/copy and scene costs on the current
//! game surface without changing resolution, simulation, or persistent cvars.
//! The bench is opt-in and has zero per-frame overhead outside a run.
use crate::renderer::{FrameInfo, Renderer};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const INITIAL_GRACE: Duration = Duration::from_secs(2);
const WARMUP: Duration = Duration::from_millis(600);
const SAMPLE: Duration = Duration::from_millis(1600);

struct Case {
    name: &'static str,
    description: &'static str,
    mode: Option<u32>,
    early: Option<bool>,
    copy: Option<bool>,
}

// Baseline repeats bracket potential drift. Copy and early-submit experiments
// intentionally use the same camera/world settings as the current preset.
const CASES: [Case; 9] = [
    Case { name: "baseline", description: "Exact original settings", mode: None, early: None, copy: None },
    Case { name: "direct", description: "Original world; early-submit OFF; present-copy OFF", mode: None, early: Some(false), copy: Some(false) },
    Case { name: "copy", description: "Original world; request offscreen render + swapchain copy", mode: None, early: Some(false), copy: Some(true) },
    Case { name: "early", description: "Original world; request early offscreen submit + copy", mode: None, early: Some(true), copy: Some(false) },
    Case { name: "shitter5", description: "Shitter BSP one-stage/lightmap; direct surface", mode: Some(5), early: Some(false), copy: Some(false) },
    Case { name: "flat12", description: "Flat-color BSP; direct surface", mode: Some(12), early: Some(false), copy: Some(false) },
    Case { name: "no_world10", description: "No world preparation/draw; other presentation remains", mode: Some(10), early: Some(false), copy: Some(false) },
    Case { name: "clear15", description: "One swapchain color clear + submit + present", mode: Some(15), early: Some(false), copy: Some(false) },
    Case { name: "baseline_repeat", description: "Restore original settings and measure drift", mode: None, early: None, copy: None },
];

#[derive(Default)]
struct Sample {
    frames: u64,
    wall_seconds: f64,
    cpu_frame: f64,
    prepare: f64,
    world_prep: f64,
    dyn_prepare: f64,
    acquire: f64,
    encode: f64,
    submit: f64,
    present: f64,
    gpu_ms_total: f64,
    gpu_frames: u64,
    world_draws: u64,
    models: u64,
    post_frames: u64,
    forced_scene_copy_frames: u64,
    legacy_copy_frames: u64,
    actual_early_frames: u64,
    actual_present_copy_frames: u64,
    errors: u64,
}

impl Sample {
    fn add(&mut self, info: &FrameInfo) {
        self.frames += 1;
        self.cpu_frame += info.frame_ms;
        self.prepare += info.cpu_prepare_ms;
        self.world_prep += info.prep_world_ms;
        self.dyn_prepare += info.dynamic_model_prepare_ms;
        self.acquire += info.cpu_acquire_ms;
        self.encode += info.cpu_encode_ms;
        self.submit += info.cpu_submit_ms;
        self.present += info.cpu_present_ms;
        self.world_draws += u64::from(info.world_encoded_batches);
        self.models += u64::from(info.dynamic_model_surfaces);
        self.post_frames += u64::from(info.effective_post);
        self.forced_scene_copy_frames += u64::from(info.scene_copy_forced);
        self.legacy_copy_frames += u64::from(info.legacy_blend_scene_copy);
        self.actual_early_frames += u64::from(info.early_submit);
        self.actual_present_copy_frames += u64::from(info.present_copy);
        if let Some(ms) = info.gpu_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) {
            self.gpu_ms_total += ms;
            self.gpu_frames += 1;
        }
    }
    fn mean(&self, v: f64) -> f64 {
        if self.frames == 0 { 0.0 } else { v / self.frames as f64 }
    }
    fn fps(&self) -> f64 {
        if self.wall_seconds > 0.0 { self.frames as f64 / self.wall_seconds } else { 0.0 }
    }
}

pub(in crate::renderer) struct ShitterSweep {
    path: PathBuf,
    original_mode: u32,
    original_early: bool,
    original_copy: bool,
    original_fps_cap: u32,
    surface_format: String,
    resolution: (u32, u32),
    present_mode: String,
    max_frame_latency: u32,
    adapter: String,
    driver: String,
    backend: String,
    case: usize,
    armed_at: Instant,
    configured_at: Instant,
    sampled_at: Option<Instant>,
    samples: [Sample; CASES.len()],
}

impl ShitterSweep {
    pub(in crate::renderer) fn new(path: PathBuf, renderer: &mut Renderer, fps_cap: u32) -> Self {
        let adapter = renderer.adapter.get_info();
        let mut sweep = Self {
            path,
            original_mode: renderer.competitive_ab,
            original_early: renderer.early_submit_enabled,
            original_copy: renderer.present_copy_enabled,
            original_fps_cap: fps_cap,
            surface_format: format!("{:?}", renderer.config.format),
            resolution: (renderer.config.width, renderer.config.height),
            present_mode: format!("{:?}", renderer.config.present_mode),
            max_frame_latency: renderer.config.desired_maximum_frame_latency,
            adapter: adapter.name,
            driver: format!("{} {}", adapter.driver, adapter.driver_info),
            backend: format!("{:?}", adapter.backend),
            case: 0,
            armed_at: Instant::now(),
            configured_at: Instant::now(),
            sampled_at: None,
            samples: std::array::from_fn(|_| Sample::default()),
        };
        sweep.configure(renderer);
        eprintln!("[SHITTER SWEEP] armed 9 cases (2s close-console grace; 0.6s warmup + 1.6s measurement each). Output: {}", sweep.path.display());
        sweep
    }

    fn configure(&mut self, renderer: &mut Renderer) {
        let case = &CASES[self.case];
        renderer.set_competitive_ab(case.mode.unwrap_or(self.original_mode));
        renderer.early_submit_enabled = case.early.unwrap_or(self.original_early);
        renderer.present_copy_enabled = case.copy.unwrap_or(self.original_copy);
        self.configured_at = Instant::now();
        self.sampled_at = None;
        eprintln!("[SHITTER SWEEP] case {}/{} {} mode={} early={} copy={} (warmup)",
            self.case + 1, CASES.len(), case.name, renderer.competitive_ab,
            renderer.early_submit_enabled, renderer.present_copy_enabled);
    }

    pub(in crate::renderer) fn fps_cap(&self) -> u32 { self.original_fps_cap }

    pub(in crate::renderer) fn restore(&mut self, renderer: &mut Renderer) -> u32 {
        renderer.set_competitive_ab(self.original_mode);
        renderer.early_submit_enabled = self.original_early;
        renderer.present_copy_enabled = self.original_copy;
        eprintln!("[SHITTER SWEEP] restored competitiveAB={}, earlySubmit={}, presentCopy={}",
            self.original_mode, self.original_early, self.original_copy);
        self.original_fps_cap
    }

    /// True when the sweep is complete and can be moved to the IO writer.
    pub(in crate::renderer) fn record(&mut self, info: &FrameInfo, renderer: &mut Renderer) -> bool {
        let now = Instant::now();
        if now.duration_since(self.armed_at) < INITIAL_GRACE { return false; }
        if now.duration_since(self.configured_at) < WARMUP { return false; }
        let since = *self.sampled_at.get_or_insert(now);
        self.samples[self.case].add(info);
        if now.duration_since(since) < SAMPLE { return false; }
        self.samples[self.case].wall_seconds = now.duration_since(since).as_secs_f64();
        let sample = &self.samples[self.case];
        eprintln!("[SHITTER SWEEP] {:<16} fps={:.1} cpu={:.4}ms acquire={:.4}ms encode={:.4}ms submit={:.4}ms post={}/{} forced_copy={}/{}",
            CASES[self.case].name, sample.fps(), sample.mean(sample.cpu_frame),
            sample.mean(sample.acquire), sample.mean(sample.encode), sample.mean(sample.submit),
            sample.post_frames, sample.frames, sample.forced_scene_copy_frames, sample.frames);
        if self.case + 1 == CASES.len() {
            self.restore(renderer);
            return true;
        }
        self.case += 1;
        self.configure(renderer);
        false
    }

    pub(in crate::renderer) fn finish(self) {
        // Only the final small report is written on the background IO thread;
        // recording does not block the render loop on disk access.
        let path = self.path.clone();
        match std::thread::Builder::new().name("jka-shitter-sweep-write".into()).spawn(move || {
            let report = self.report();
            let result = (|| -> std::io::Result<()> {
                if let Some(parent) = self.path.parent() { std::fs::create_dir_all(parent)?; }
                std::fs::write(&self.path, report)
            })();
            match result {
                Ok(()) => eprintln!("[SHITTER SWEEP] COMPLETE: {} (send this TXT file)", self.path.display()),
                Err(error) => eprintln!("[SHITTER SWEEP] FAILED writing {}: {error}", self.path.display()),
            }
        }) {
            Ok(_) => {},
            Err(error) => eprintln!("[SHITTER SWEEP] failed to start log writer for {}: {error}", path.display()),
        }
    }

    fn report(&self) -> String {
        let mut s = String::with_capacity(6000);
        let _ = writeln!(s, "DINURDOJK SHITTER FULL-FRAME / PRESENTATION SWEEP v1\n");
        let _ = writeln!(s, "adapter: {}  backend: {}  driver: {}", self.adapter, self.backend, self.driver);
        let _ = writeln!(s, "surface: {}x{} {} present={}; max_frame_latency={}", self.resolution.0, self.resolution.1,
            self.surface_format, self.present_mode, self.max_frame_latency);
        let _ = writeln!(s, "original settings: r_competitiveAB={} r_earlySubmit={} r_presentCopy={} com_maxfps={}",
            self.original_mode, self.original_early as u8, self.original_copy as u8, self.original_fps_cap);
        let _ = writeln!(s, "warmup per phase: {:.2}s, measurement per phase: ~{:.2}s, initial grace: {:.1}s",
            WARMUP.as_secs_f64(), SAMPLE.as_secs_f64(), INITIAL_GRACE.as_secs_f64());
        let _ = writeln!(s, "All modes use the same resolution, surface format, adapter, present mode and world. Renderer-only FPS cap forced to 0 (uncapped), then restored.");
        let _ = writeln!(s, "Reported FPS uses successful frames over wall-clock measurement time. CPU stage timings are not GPU bandwidth or GPU execution times.\n");
        for (index, case) in CASES.iter().enumerate() {
            let row = &self.samples[index];
            let mode = case.mode.unwrap_or(self.original_mode);
            let early = case.early.unwrap_or(self.original_early);
            let copy = case.copy.unwrap_or(self.original_copy);
            let _ = writeln!(s, "[{}] {} | {}", index+1, case.name, case.description);
            let _ = writeln!(s, "  request: mode={} earlySubmit={} presentCopy={}; frames={} wall={:.4}s fps={:.1} errors={}",
                mode, early as u8, copy as u8, row.frames, row.wall_seconds, row.fps(), row.errors);
            let _ = writeln!(s, "  CPU mean ms: frame={:.4} prepare={:.4} world_prepare={:.4} models_prepare={:.4} acquire={:.4} encode={:.4} submit={:.4} present={:.4}",
                row.mean(row.cpu_frame), row.mean(row.prepare), row.mean(row.world_prep), row.mean(row.dyn_prepare),
                row.mean(row.acquire), row.mean(row.encode), row.mean(row.submit), row.mean(row.present));
            if row.gpu_frames != 0 {
                let _ = writeln!(s, "  GPU estimate ms: {:.4} ({} asynchronously profiled frames; may be delayed)",
                    row.gpu_ms_total / row.gpu_frames as f64, row.gpu_frames);
            } else {
                let _ = writeln!(s, "  GPU estimate: unavailable (GPU timestamps not forced, to avoid changing the workload)");
            }
            let _ = writeln!(s, "  actual passes, frames: post={} forced_scene_copy={} legacy_blend_copy={} early_submit={} present_copy={}",
                row.post_frames, row.forced_scene_copy_frames, row.legacy_copy_frames,
                row.actual_early_frames, row.actual_present_copy_frames);
            let _ = writeln!(s, "  mean world_encoded_batches={:.1} dynamic_model_surfaces={:.1}\n",
                row.mean(row.world_draws as f64), row.mean(row.models as f64));
        }
        if self.samples[0].fps() > 0.0 {
            let base = self.samples[0].fps();
            let _ = writeln!(s, "RATIOS relative to baseline (higher is faster):");
            for (i, c) in CASES.iter().enumerate() {
                let _ = writeln!(s, "  {:<17} {:.2}x", c.name, self.samples[i].fps() / base);
            }
        }
        let _ = writeln!(s, "\nINTERPRETATION NOTES:");
        let _ = writeln!(s, "- Copy and early-submit cases are requests. Actual early/copy frame counts reveal whether surface/CRT/UI eligibility allowed them.");
        let _ = writeln!(s, "- r_competitiveAB 15 bypasses rendering; clear/present has no depth attachment. It is not a full-game performance prediction.");
        let _ = writeln!(s, "- r_competitiveAB 10 omits world prep/draw but retains other eligible rendering work. Flat12 keeps real world geometry.");
        let _ = writeln!(s, "- A full-screen transfer occurs only when the respective path is active, not merely because two render passes exist.");
        let _ = writeln!(s, "- CPU wall-time FPS includes renderer loop, app activity and scheduling; CPU stage timings do not include all frame scheduling.");
        let _ = writeln!(s, "- This test never changes output resolution, native format, depth format, graphics preset or player model fidelity; temporary renderer FPS cap override is restored.");
        let _ = writeln!(s, "- All original benchmarked settings were restored before the report was written.");
        s
    }
}
