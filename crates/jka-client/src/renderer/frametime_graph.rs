//! Render-thread frame-cadence diagnostic.
//!
//! Sampling is per frame; native UI geometry refreshes at 30 Hz. Measured CPU
//! intervals are taken from the preceding successfully rendered frame. GPU
//! profiling is optional and *asynchronous*; resolved GPU subsystem values are
//! only exposed as a separate readout, never added to the CPU/cadence bars.
use std::collections::VecDeque;

use crate::renderer::{Duration, FrameInfo, GpuPass, Instant, UiVertex, ui};
use crate::renderer::diagnostics::profiling::GpuTiming;

const HISTORY: usize = 8192;
const GRAPH_REFRESH: Duration = Duration::from_millis(33);
const RANK_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Default)]
struct FrameSample {
    elapsed_ms: f32,
    // Prepare, Acquire, Encode, Submit, Present, Other/pacing.
    phases: [f32; 6],
}

impl FrameSample {
    fn new(elapsed_ms: f32, previous_info: Option<&FrameInfo>) -> Self {
        let elapsed_ms = elapsed_ms.max(0.0);
        let mut phases = [0.0; 6];
        if let Some(info) = previous_info {
            let safe = |n: f64| if n.is_finite() && n > 0.0 { n as f32 } else { 0.0 };
            phases[0] = safe(info.cpu_prepare_ms);
            phases[1] = safe(info.cpu_acquire_ms);
            phases[2] = safe(info.cpu_encode_ms);
            // In early-submit mode surface acquisition happens during the
            // submit interval, rather than being an independent CPU interval.
            phases[3] = (safe(info.cpu_submit_ms)
                - if info.early_submit { phases[1] } else { 0.0 }).max(0.0);
            phases[4] = safe(info.cpu_present_ms);
            let accounted: f32 = phases[..5].iter().copied().sum();
            // Timing boundaries differ slightly from frame-start-to-start
            // cadence. Preserve the true bar height without falsely extending
            // it when two measured subintervals touch or overlap.
            if accounted > elapsed_ms && accounted > 0.0 {
                let scale = elapsed_ms / accounted;
                for phase in &mut phases[..5] { *phase *= scale; }
            } else {
                phases[5] = (elapsed_ms - accounted).max(0.0);
            }
        } else {
            phases[5] = elapsed_ms;
        }
        Self { elapsed_ms, phases }
    }
}

pub(in crate::renderer) struct FrameTimeGraph {
    enabled: bool,
    previous_frame: Option<Instant>,
    last_upload: Option<Instant>,
    samples: VecDeque<(Instant, FrameSample)>,
    sorted_window: Vec<f32>,
    vertices: Vec<UiVertex>,
    buffer: Option<wgpu::Buffer>,
    buffer_vertices: usize,
    vertex_count: u32,
    size: (u32, u32),
}

impl Default for FrameTimeGraph {
    fn default() -> Self {
        Self {
            enabled: false,
            previous_frame: None,
            last_upload: None,
            samples: VecDeque::new(),
            sorted_window: Vec::new(),
            vertices: Vec::new(),
            buffer: None,
            buffer_vertices: 0,
            vertex_count: 0,
            size: (0, 0),
        }
    }
}

impl FrameTimeGraph {
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled { return; }
        self.enabled = enabled;
        self.previous_frame = None;
        self.last_upload = None;
        self.samples.clear();
        self.vertex_count = 0;
        if enabled {
            self.samples.reserve(HISTORY);
            self.sorted_window.reserve(HISTORY);
        }
    }

    pub fn draw_ready(&self) -> bool {
        self.enabled && self.vertex_count > 0 && self.buffer.is_some()
    }

    /// Called once per render-loop frame. The optional info belongs to the
    /// previously completed frame (same interval whose cadence ends at `now`).
    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        &mut self, now: Instant, previous_info: Option<&FrameInfo>,
        gpu_timing: Option<GpuTiming>, device: &wgpu::Device, queue: &wgpu::Queue,
        width: u32, height: u32,
    ) {
        if !self.enabled { return; }
        if let Some(previous) = self.previous_frame {
            let ms = now.saturating_duration_since(previous).as_secs_f32() * 1000.0;
            if ms.is_finite() {
                if self.samples.len() == HISTORY { self.samples.pop_front(); }
                self.samples.push_back((now, FrameSample::new(ms, previous_info)));
            }
        }
        self.previous_frame = Some(now);
        let dimensions_changed = self.size != (width, height);
        if !dimensions_changed
            && self.last_upload.is_some_and(|previous| now.saturating_duration_since(previous) < GRAPH_REFRESH)
        { return; }
        self.size = (width, height);
        self.last_upload = Some(now);
        if width == 0 || height == 0 || self.samples.is_empty() {
            self.vertex_count = 0;
            return;
        }

        self.sorted_window.clear();
        self.sorted_window.extend(self.samples.iter()
            .filter(|(when, _)| now.saturating_duration_since(*when) <= RANK_WINDOW)
            .map(|(_, sample)| sample.elapsed_ms));
        self.sorted_window.sort_unstable_by(f32::total_cmp);
        let percentile = |p: f32| -> f32 {
            let sorted = &self.sorted_window;
            if sorted.is_empty() { return 0.0; }
            let position = p * (sorted.len() - 1) as f32;
            let lo = position.floor() as usize;
            let hi = (lo + 1).min(sorted.len() - 1);
            sorted[lo] + (sorted[hi] - sorted[lo]) * position.fract()
        };
        let cutoffs = [percentile(0.80), percentile(0.95), percentile(0.99)];
        let gpu = gpu_timing.map(|timing| {
            let get = |pass: GpuPass| -> Option<f32> {
                timing.milliseconds[pass.index()]
                    .filter(|ms| ms.is_finite() && *ms >= 0.0)
                    .map(|ms| ms as f32)
            };
            // GPU query results can lag the CPU frame by several frames.
            // Keep these *independent* from the stacked CPU intervals.
            [get(GpuPass::OceanSim), get(GpuPass::OceanSurface),
             get(GpuPass::Clouds), get(GpuPass::BlueWaterClouds),
             get(GpuPass::World)]
        });
        ui::build_frame_time_graph_vertices(
            &mut self.vertices,
            self.samples.iter().rev().take(width as usize)
                .map(|(_, sample)| (sample.elapsed_ms, sample.phases)),
            width, height, cutoffs, gpu,
        );
        self.vertex_count = self.vertices.len() as u32;
        if self.vertices.len() > self.buffer_vertices {
            self.buffer_vertices = (self.vertices.len() + 1024).next_power_of_two();
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA frametime graph cached vertices"),
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

    pub fn draw<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline, bind_group: &'a wgpu::BindGroup,
    ) {
        if !self.draw_ready() { return; }
        let Some(buffer) = &self.buffer else { return; };
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
    fn resets_when_toggled() {
        let mut graph = FrameTimeGraph::default();
        assert!(!graph.draw_ready());
        graph.set_enabled(true);
        graph.previous_frame = Some(Instant::now());
        graph.set_enabled(false);
        assert!(graph.previous_frame.is_none());
        assert!(graph.samples.is_empty());
    }

    #[test]
    fn sample_parts_never_exceed_total() {
        let info = FrameInfo {
            cpu_prepare_ms: 2.0, cpu_acquire_ms: 1.0,
            cpu_encode_ms: 2.0, cpu_submit_ms: 1.0,
            cpu_present_ms: 1.0, ..FrameInfo::default()
        };
        let s = FrameSample::new(0.4, Some(&info));
        let total: f32 = s.phases.iter().sum();
        assert!((total - 0.4).abs() < 0.0001);
        let s = FrameSample::new(10.0, Some(&info));
        assert!((s.phases[5] - 3.0).abs() < 0.0001);
    }
}
