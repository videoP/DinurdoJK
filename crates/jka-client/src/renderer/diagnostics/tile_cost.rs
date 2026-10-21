//! Measured per-tile GPU cost (`gputilecost`).
//!
//! The existing `r_shaderComplexity` heatmap is an estimate. This one measures:
//! each probe frame scissors every screen-space render pass to one screen tile
//! and keeps the ordinary per-pass GPU timestamps, tagged with the tile they
//! measured. A 1x1 scissor frame ("baseline") measures what does not scale with
//! covered pixels: vertex work, compute passes, pass and clear overhead. A tile's
//! cost for a pass is median(tile) - median(baseline). Unscissored frames ("full")
//! are interleaved so drift and coverage can be checked:
//! `baseline + sum(tiles) ~= full` for a pass whose fragment work is scissored.
//!
//! Probe frames show only one tile, and temporal histories (clouds, SSR, AO)
//! are disturbed while it runs. Hold the camera still; a frozen clock helps.
//! Normal frames pay one relaxed atomic load per scissor site.
use crate::renderer::diagnostics::profiling::{GpuPass, GPU_PASS_COUNT};
use crate::renderer::{FrameInfo, Renderer, UiVertex};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Probe rectangle in surface pixels: x | y << 16 | w << 32 | h << 48; 0 = off.
static PROBE_RECT: AtomicU64 = AtomicU64::new(0);
/// Surface size the rectangle is expressed in: width | height << 32.
static PROBE_SURFACE: AtomicU64 = AtomicU64::new(0);

const WARMUP_FRAMES: u32 = 60;
const ARM_DELAY: Duration = Duration::from_secs(2);
const TILES_PER_BASELINE: usize = 8;
const TILES_PER_FULL: usize = 16;

fn set_probe(rect: Option<[u32; 4]>, surface: [u32; 2]) {
    match rect {
        Some([x, y, w, h]) => {
            PROBE_SURFACE.store(u64::from(surface[0]) | (u64::from(surface[1]) << 32), Ordering::Relaxed);
            let packed = u64::from(x & 0xffff) | (u64::from(y & 0xffff) << 16)
                | (u64::from(w.max(1) & 0xffff) << 32) | (u64::from(h.max(1) & 0xffff) << 48);
            PROBE_RECT.store(packed, Ordering::Relaxed);
        }
        None => PROBE_RECT.store(0, Ordering::Relaxed),
    }
}

/// The probe tile scaled to a `width` x `height` target. None outside a probe.
#[inline]
pub(crate) fn probe_rect(width: u32, height: u32) -> Option<[u32; 4]> {
    let packed = PROBE_RECT.load(Ordering::Relaxed);
    if packed == 0 {
        return None;
    }
    let surface = PROBE_SURFACE.load(Ordering::Relaxed);
    let rect = [
        (packed & 0xffff) as u32,
        ((packed >> 16) & 0xffff) as u32,
        ((packed >> 32) & 0xffff) as u32,
        (packed >> 48) as u32,
    ];
    scale_rect(rect, [(surface & 0xffff_ffff) as u32, (surface >> 32) as u32], width, height)
}

/// `rect` in `surface` pixels mapped onto a `width` x `height` target covering
/// the same screen: never empty, always inside the target.
fn scale_rect(rect: [u32; 4], surface: [u32; 2], width: u32, height: u32) -> Option<[u32; 4]> {
    let [x, y, w, h] = rect.map(u64::from);
    let [sw, sh] = surface.map(u64::from);
    if sw == 0 || sh == 0 || width == 0 || height == 0 {
        return None;
    }
    let (tw, th) = (u64::from(width), u64::from(height));
    let x0 = (x * tw / sw).min(tw - 1);
    let y0 = (y * th / sh).min(th - 1);
    let x1 = ((x + w) * tw).div_ceil(sw).clamp(x0 + 1, tw);
    let y1 = ((y + h) * th).div_ceil(sh).clamp(y0 + 1, th);
    Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
}

/// Restrict a screen-space render pass whose attachments are `width` x `height`
/// to the probe tile. No-op outside a `gputilecost` run.
#[inline]
pub(crate) fn scissor(pass: &mut wgpu::RenderPass<'_>, width: u32, height: u32) {
    if let Some([x, y, w, h]) = probe_rect(width, height) {
        pass.set_scissor_rect(x, y, w, h);
    }
}

/// `scissor` for a pass whose attachments are exactly the probed surface size
/// (the scene color/depth and same-sized copies). The probe pauses itself
/// while the window size differs from the size it was armed at.
#[inline]
pub(crate) fn scissor_surface(pass: &mut wgpu::RenderPass<'_>) {
    let surface = PROBE_SURFACE.load(Ordering::Relaxed);
    scissor(pass, (surface & 0xffff_ffff) as u32, (surface >> 32) as u32);
}

/// For passes that set their own scissor: the intersection with the probe tile
/// (a 1x1 rect at the tile corner when they do not overlap).
pub(crate) fn constrain(own: [u32; 4], width: u32, height: u32) -> [u32; 4] {
    let Some([px, py, pw, ph]) = probe_rect(width, height) else { return own };
    let x0 = own[0].max(px);
    let y0 = own[1].max(py);
    let x1 = (own[0] + own[2]).min(px + pw);
    let y1 = (own[1] + own[3]).min(py + ph);
    if x1 <= x0 || y1 <= y0 {
        return [px, py, 1, 1];
    }
    [x0, y0, x1 - x0, y1 - y0]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Job {
    Full,
    Baseline,
    Tile(u32),
}

impl Job {
    /// Index into `samples`; the profiler tag is this plus one (0 = untagged).
    fn index(self) -> usize {
        match self {
            Job::Full => 0,
            Job::Baseline => 1,
            Job::Tile(tile) => 2 + tile as usize,
        }
    }
}

type PassSample = [Option<f64>; GPU_PASS_COUNT];

pub(in crate::renderer) struct TileCostProbe {
    stem: PathBuf,
    cols: u32,
    rows: u32,
    surface: [u32; 2],
    samples_per_job: usize,
    armed_at: Instant,
    warmup: u32,
    order: Vec<Job>,
    cursor: usize,
    measured_frames: u32,
    max_frames: u32,
    samples: Vec<Vec<PassSample>>,
    adapter: String,
    backend: String,
}

impl TileCostProbe {
    pub(in crate::renderer) fn new(stem: PathBuf, cols: u32, rows: u32, samples_per_job: usize, renderer: &mut Renderer) -> Self {
        let cols = cols.clamp(1, 64);
        let rows = rows.clamp(1, 64);
        let tiles = cols * rows;
        let mut order = Vec::with_capacity(tiles as usize * 2);
        for tile in 0..tiles as usize {
            if tile % TILES_PER_FULL == 0 {
                order.push(Job::Full);
            }
            if tile % TILES_PER_BASELINE == 0 {
                order.push(Job::Baseline);
            }
            order.push(Job::Tile(tile as u32));
        }
        let samples_per_job = samples_per_job.clamp(3, 64);
        let adapter = renderer.adapter.get_info();
        renderer.gpu_profiler.completed = Some(Vec::with_capacity(8));
        renderer.gpu_profiler.frame_tag = 0;
        Self {
            stem,
            cols,
            rows,
            surface: [renderer.config.width, renderer.config.height],
            samples_per_job,
            armed_at: Instant::now(),
            warmup: WARMUP_FRAMES,
            max_frames: (order.len() * samples_per_job * 4) as u32 + 2_000,
            order,
            cursor: 0,
            measured_frames: 0,
            samples: vec![Vec::with_capacity(samples_per_job); 2 + tiles as usize],
            adapter: adapter.name,
            backend: format!("{:?}", adapter.backend),
        }
    }

    fn tile_rect(&self, tile: u32) -> [u32; 4] {
        let [w, h] = self.surface;
        let (col, row) = (tile % self.cols, tile / self.cols);
        let x0 = col * w / self.cols;
        let y0 = row * h / self.rows;
        let x1 = (col + 1) * w / self.cols;
        let y1 = (row + 1) * h / self.rows;
        [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
    }

    /// Choose this frame's scissor and profiler tag. Call before `render()`.
    pub(in crate::renderer) fn begin_frame(&mut self, renderer: &mut Renderer) {
        renderer.gpu_profiler.frame_tag = 0;
        if self.armed_at.elapsed() < ARM_DELAY || [renderer.config.width, renderer.config.height] != self.surface {
            set_probe(None, self.surface);
            return;
        }
        // Skip tiles that already have enough samples so the run converges.
        // Full and baseline keep sampling for the whole run: a baseline taken
        // only in the first few frames drifts away from late tiles and, on a
        // fine grid, zeroes every tile cost.
        let mut job = self.order[self.cursor];
        for _ in 0..self.order.len() {
            if !matches!(job, Job::Tile(_)) || self.samples[job.index()].len() < self.samples_per_job {
                break;
            }
            self.cursor = (self.cursor + 1) % self.order.len();
            job = self.order[self.cursor];
        }
        self.cursor = (self.cursor + 1) % self.order.len();
        let rect = match job {
            Job::Full => None,
            Job::Baseline => Some([0, 0, 1, 1]),
            Job::Tile(tile) => Some(self.tile_rect(tile)),
        };
        set_probe(rect, self.surface);
        if self.warmup > 0 {
            self.warmup -= 1;
            return;
        }
        renderer.gpu_profiler.frame_tag = job.index() as u32 + 1;
    }

    /// File resolved timings. Returns Err when the run must stop early, and
    /// Ok(true) when every job has enough samples.
    pub(in crate::renderer) fn record(&mut self, info: &FrameInfo, renderer: &mut Renderer) -> Result<bool, String> {
        if renderer.gpu_profiler.frame_tag != 0 {
            self.measured_frames += 1;
            if info.world_path != "unified" {
                return Err(format!("needs the unified world path (frame used '{}')", info.world_path));
            }
        }
        if [renderer.config.width, renderer.config.height] != self.surface {
            return Err("window size changed".to_owned());
        }
        if let Some(log) = renderer.gpu_profiler.completed.as_mut() {
            for (tag, timing) in log.drain(..) {
                let Some(job) = (tag as usize).checked_sub(1) else { continue };
                if let Some(rows) = self.samples.get_mut(job) {
                    rows.push(timing.milliseconds);
                }
            }
        }
        if self.measured_frames > self.max_frames {
            return Err("too few GPU timings resolved (is r_gpuTimings available?)".to_owned());
        }
        Ok(self.samples.iter().all(|rows| rows.len() >= self.samples_per_job))
    }

    /// Stop scissoring and release the profiler hooks. Safe to call twice.
    pub(in crate::renderer) fn restore(&self, renderer: &mut Renderer) {
        set_probe(None, self.surface);
        renderer.gpu_profiler.frame_tag = 0;
        renderer.gpu_profiler.completed = None;
    }

    pub(in crate::renderer) fn progress(&self) -> (usize, usize) {
        let done = self.samples.iter().map(|rows| rows.len().min(self.samples_per_job)).sum();
        (done, self.samples.len() * self.samples_per_job)
    }

    /// Reduce to per-pass tile costs, write CSV/JSON on a worker thread and
    /// return the report for the overlay.
    pub(in crate::renderer) fn finish(self) -> TileCostReport {
        let tiles = (self.cols * self.rows) as usize;
        let median = |rows: &[PassSample], pass: usize| -> Option<f64> {
            let mut values: Vec<f64> = rows.iter().filter_map(|row| row[pass]).filter(|v| v.is_finite()).collect();
            if values.is_empty() {
                return None;
            }
            values.sort_unstable_by(f64::total_cmp);
            Some(values[values.len() / 2])
        };
        let mut passes = Vec::new();
        for pass in GpuPass::ALL {
            let index = pass.index();
            let Some(full) = median(&self.samples[0], index) else { continue };
            let baseline = median(&self.samples[1], index).unwrap_or(f64::NAN);
            let tile_medians: Vec<f64> = (0..tiles)
                .map(|tile| median(&self.samples[2 + tile], index).unwrap_or(f64::NAN))
                .collect();
            // NaN (no data) must stay NaN: f64::max would turn it into 0.
            let costs: Vec<f64> = tile_medians.iter()
                .map(|m| if (m - baseline).is_finite() { (m - baseline).max(0.0) } else { f64::NAN })
                .collect();
            passes.push(PassTileCost { pass, full_ms: full, baseline_ms: baseline, tile_ms: tile_medians, cost_ms: costs });
        }
        let report = TileCostReport {
            cols: self.cols,
            rows: self.rows,
            surface: self.surface,
            samples_per_job: self.samples_per_job,
            passes,
        };
        let csv = report.csv(&self);
        let json = report.json(&self);
        let stem = self.stem.clone();
        let _ = std::thread::Builder::new().name("jka-tile-cost-write".to_owned()).spawn(move || {
            let csv_file = stem.with_extension("csv");
            let json_file = stem.with_extension("json");
            let result = (|| -> std::io::Result<()> {
                if let Some(parent) = csv_file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&csv_file, csv)?;
                std::fs::write(&json_file, serde_json::to_vec_pretty(&json).unwrap_or_default())?;
                Ok(())
            })();
            match result {
                Ok(()) => eprintln!("[TILE COST] COMPLETE: {} and {}", csv_file.display(), json_file.display()),
                Err(error) => eprintln!("[TILE COST] write failed: {error}"),
            }
        });
        report
    }
}

impl Drop for TileCostProbe {
    /// A probe dropped on any path (renderer error, re-arm) must not leave
    /// every later frame scissored.
    fn drop(&mut self) {
        set_probe(None, self.surface);
    }
}

pub(in crate::renderer) struct PassTileCost {
    pub(in crate::renderer) pass: GpuPass,
    pub(in crate::renderer) full_ms: f64,
    pub(in crate::renderer) baseline_ms: f64,
    pub(in crate::renderer) tile_ms: Vec<f64>,
    pub(in crate::renderer) cost_ms: Vec<f64>,
}

impl PassTileCost {
    fn sum_ms(&self) -> f64 {
        self.cost_ms.iter().copied().filter(|v| v.is_finite()).sum()
    }

    /// (baseline + summed tile cost) / full: ~100% when the pass's pixel work is
    /// scissored and additive; far below when the pass is not scissored.
    fn coverage(&self) -> f64 {
        (self.baseline_ms + self.sum_ms()) / self.full_ms.max(1e-9)
    }
}

pub(in crate::renderer) struct TileCostReport {
    pub(in crate::renderer) cols: u32,
    pub(in crate::renderer) rows: u32,
    pub(in crate::renderer) surface: [u32; 2],
    pub(in crate::renderer) samples_per_job: usize,
    pub(in crate::renderer) passes: Vec<PassTileCost>,
}

impl TileCostReport {
    fn csv(&self, probe: &TileCostProbe) -> String {
        use std::fmt::Write;
        let mut csv = String::from("pass,tile,col,row,x,y,w,h,median_ms,baseline_ms,cost_ms,full_ms\n");
        for pass in &self.passes {
            for (tile, (median, cost)) in pass.tile_ms.iter().zip(&pass.cost_ms).enumerate() {
                let [x, y, w, h] = probe.tile_rect(tile as u32);
                let _ = writeln!(&mut csv, "{},{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6}",
                    pass.pass.label(), tile, tile as u32 % self.cols, tile as u32 / self.cols, x, y, w, h,
                    median, pass.baseline_ms, cost, pass.full_ms);
            }
        }
        csv
    }

    fn json(&self, probe: &TileCostProbe) -> serde_json::Value {
        let passes: Vec<_> = self.passes.iter().map(|pass| {
            let mut ranked: Vec<(usize, f64)> = pass.cost_ms.iter().copied().enumerate()
                .filter(|(_, v)| v.is_finite()).collect();
            ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
            let top: Vec<_> = ranked.iter().take(8).map(|(tile, cost)| serde_json::json!({
                "tile": tile, "col": *tile as u32 % self.cols, "row": *tile as u32 / self.cols, "cost_ms": cost,
            })).collect();
            serde_json::json!({
                "pass": pass.pass.label(),
                "full_ms": pass.full_ms,
                "baseline_ms": pass.baseline_ms,
                "sum_tile_cost_ms": pass.sum_ms(),
                "coverage": pass.coverage(),
                "top_tiles": top,
            })
        }).collect();
        serde_json::json!({
            "experiment": "gputilecost: measured per-tile GPU pass cost (scissor sweep)",
            "adapter": probe.adapter,
            "backend": probe.backend,
            "resolution": self.surface,
            "grid": [self.cols, self.rows],
            "samples_per_job": self.samples_per_job,
            "frames_measured": probe.measured_frames,
            "passes": passes,
            "notes": [
                "cost = median(pass ms with every screen-space render pass scissored to the tile) - median(pass ms with a 1x1 scissor).",
                "baseline_ms = vertex, compute and fixed pass cost that does not scale with covered pixels.",
                "coverage = (baseline + sum of tile costs) / full; far below 1 means that pass is not scissored (compute, offscreen) or is not additive across tiles.",
                "Probe frames disturb temporal histories (clouds, SSR, AO); hold the camera still.",
            ],
        })
    }

    pub(in crate::renderer) fn find(&self, pass: GpuPass) -> Option<&PassTileCost> {
        self.passes.iter().find(|entry| entry.pass.index() == pass.index())
    }

    /// The pass shown by default: the whole GPU frame when timed, else World.
    pub(in crate::renderer) fn default_pass(&self) -> Option<GpuPass> {
        self.find(GpuPass::Frame).or_else(|| self.find(GpuPass::World)).map(|entry| entry.pass)
    }
}

/// Parse a pass by its report label (`ocean_surface`) or without underscores.
pub(in crate::renderer) fn parse_pass(name: &str) -> Option<GpuPass> {
    let wanted: String = name.chars().filter(|c| *c != '_').flat_map(char::to_lowercase).collect();
    GpuPass::ALL.into_iter().find(|pass| pass.label().replace('_', "") == wanted)
}

/// Cached overlay geometry for the finished report.
pub(in crate::renderer) struct TileCostOverlay {
    pub(in crate::renderer) report: TileCostReport,
    pub(in crate::renderer) shown: GpuPass,
    pub(in crate::renderer) visible: bool,
    vertices: Vec<UiVertex>,
    buffer: Option<wgpu::Buffer>,
    buffer_vertices: usize,
    vertex_count: u32,
    built_for: Option<(usize, u32, u32)>,
}

impl TileCostOverlay {
    pub(in crate::renderer) fn new(report: TileCostReport) -> Option<Self> {
        let shown = report.default_pass()?;
        Some(Self {
            report,
            shown,
            visible: true,
            vertices: Vec::new(),
            buffer: None,
            buffer_vertices: 0,
            vertex_count: 0,
            built_for: None,
        })
    }

    pub(in crate::renderer) fn show(&mut self, pass: GpuPass) -> bool {
        if self.report.find(pass).is_none() {
            return false;
        }
        self.shown = pass;
        self.visible = true;
        self.built_for = None;
        true
    }

    pub(in crate::renderer) fn available_passes(&self) -> String {
        self.report.passes.iter().map(|entry| entry.pass.label()).collect::<Vec<_>>().join(" ")
    }

    /// Rebuild the vertex buffer when the shown pass or the window size changed.
    pub(in crate::renderer) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        if !self.visible {
            return;
        }
        let key = (self.shown.index(), width, height);
        if self.built_for == Some(key) {
            return;
        }
        self.built_for = Some(key);
        let Some(entry) = self.report.find(self.shown) else {
            self.vertex_count = 0;
            return;
        };
        let header = [
            format!("GPU TILE COST (MEASURED)   PASS {}   FULL {:.3} MS   BASELINE {:.3} MS   SUM OF TILES {:.3} MS   COVERAGE {:.0}%",
                entry.pass.label().to_uppercase(), entry.full_ms, entry.baseline_ms, entry.sum_ms(), entry.coverage() * 100.0),
            format!("GRID {}X{} AT {}X{}   {} SAMPLES/TILE   TILE = PASS MS SCISSORED TO TILE - 1X1 BASELINE",
                self.report.cols, self.report.rows, self.report.surface[0], self.report.surface[1], self.report.samples_per_job),
            "GPUTILECOST SHOW <PASS>   GPUTILECOST OFF   GPUTILECOST PASSES".to_owned(),
        ];
        crate::ui::build_tile_cost_vertices(&mut self.vertices, &crate::ui::TileCostView {
            cols: self.report.cols,
            rows: self.report.rows,
            costs: &entry.cost_ms,
            header: &header,
        }, width, height);
        self.vertex_count = self.vertices.len() as u32;
        if self.vertices.len() > self.buffer_vertices {
            self.buffer_vertices = (self.vertices.len() + 1024).next_power_of_two();
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA GPU tile cost overlay vertices"),
                size: (self.buffer_vertices * std::mem::size_of::<UiVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let Some(buffer) = &self.buffer {
            if !self.vertices.is_empty() {
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.vertices));
            }
        }
    }

    pub(in crate::renderer) fn draw_ready(&self) -> bool {
        self.visible && self.vertex_count > 0 && self.buffer.is_some()
    }

    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        bind_group: &'a wgpu::BindGroup,
    ) {
        if !self.draw_ready() {
            return;
        }
        let Some(buffer) = &self.buffer else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rect_maps_screen_area_and_stays_inside_target() {
        let surface = [2560, 1440];
        assert_eq!(scale_rect([160, 90, 160, 90], surface, 2560, 1440), Some([160, 90, 160, 90]));
        // Half-resolution target: the same screen area.
        assert_eq!(scale_rect([160, 90, 160, 90], surface, 1280, 720), Some([80, 45, 80, 45]));
        // A 1x1 probe never becomes empty or leaves a smaller target.
        let [x, y, w, h] = scale_rect([2559, 1439, 1, 1], surface, 640, 360).unwrap();
        assert!(x + w <= 640 && y + h <= 360 && w >= 1 && h >= 1);
        assert_eq!(scale_rect([0, 0, 1, 1], [0, 0], 640, 360), None);
    }

    #[test]
    fn parse_pass_accepts_labels_and_compact_names() {
        assert_eq!(parse_pass("ocean_surface").map(GpuPass::index), Some(GpuPass::OceanSurface.index()));
        assert_eq!(parse_pass("OceanOptics").map(GpuPass::index), Some(GpuPass::OceanOptics.index()));
        assert!(parse_pass("nope").is_none());
    }
}
