//! Render-thread frame-cadence diagnostic.
//!
//! Sampling is per frame; native UI geometry refreshes at 30 Hz. Measured CPU
//! intervals are taken from the preceding successfully rendered frame. GPU
//! profiling is optional and *asynchronous*; resolved GPU subsystem values are
//! only exposed as a separate readout, never added to the CPU/cadence bars.
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::renderer::{Duration, FrameInfo, GpuPass, Instant, UiVertex, WorldGpu, WorldShaderVariantKey, ui};
use crate::renderer::diagnostics::profiling::GpuTiming;

const HISTORY: usize = 8192;
const STATS_SLOTS: usize = 3;
// A few expensive pipeline-statistics queries per second are enough for a
// diagnostic. Avoid measuring every frame on the high-FPS iGPU/RTX A/B path.
const STATS_REFRESH: Duration = Duration::from_millis(100);
const STATS_BYTES: u64 = 3 * 8;
/// Query 1 (ocean surface) resolves at the next legal resolve-buffer offset.
const OCEAN_STATS_OFFSET: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
const RESOLVED_BYTES: u64 = OCEAN_STATS_OFFSET + STATS_BYTES;

/// Only the first/main BSP render pass is measured; this is NOT a count for
/// shadows, dynamic models, post, or the entire frame. These are real hardware
/// invocation/primitive counts, not shader instructions or RAM traffic.
/// Hardware counts for the `OceanSurface` interval (every ocean surface draw:
/// clipmaps, analytic tail, Blue Water original surface, near detail).
/// `fragment_invocations / ocean visibility samples_passed` is the overshade
/// ratio: overdraw, depth-rejected-late work and helper lanes per visible pixel.
#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct OceanSurfaceStats {
    pub vertex_invocations: u64,
    pub clipped_primitives: u64,
    pub fragment_invocations: u64,
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct PrimaryBspStats {
    pub shader_mode: u8,
    pub vertex_invocations: u64,
    pub clipped_primitives: u64,
    pub fragment_invocations: u64,
}

struct StatsSlot {
    query: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readback: wgpu::Buffer,
    pending: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
    written: bool,
    resolved_this_frame: bool,
    ocean_written: bool,
    ocean_resolved: bool,
    shader_mode: u8,
}

const GRAPH_REFRESH: Duration = Duration::from_millis(33);
// All-map eligibility is descriptive and independent of frame-to-frame camera
// visibility; avoid distorting the FPS measurement by scanning every redraw.
const MAP_ELIGIBILITY_REFRESH: Duration = Duration::from_secs(1);
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
    stats_slots: Vec<StatsSlot>,
    stats_current: Option<usize>,
    stats_latest: Option<PrimaryBspStats>,
    ocean_stats_latest: Option<OceanSurfaceStats>,
    stats_last_query: Option<Instant>,
    // World address, coarse batch count, active variant, computed-at, counts.
    fallback_cache: Option<(usize, usize, WorldShaderVariantKey, Instant, Option<[u32; 11]>)>,
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
            stats_slots: Vec::new(),
            stats_current: None,
            stats_latest: None,
            ocean_stats_latest: None,
            stats_last_query: None,
            fallback_cache: None,
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
        self.stats_current = None;
        self.stats_last_query = None;
        if !enabled { self.stats_latest = None; self.ocean_stats_latest = None; self.fallback_cache = None; }
        if enabled {
            self.samples.reserve(HISTORY);
            self.sorted_window.reserve(HISTORY);
        }
    }

    /// Nonblocking polling plus a free query slot. Never stall waiting for an
    /// earlier GPU frame, and don't overwrite buffers still mapped asynchronously.
    pub fn begin_stats_frame(&mut self, device: &wgpu::Device) {
        self.stats_current = None;
        // A render_perf_capture also records these (same 10 Hz cadence).
        if !self.enabled && !crate::renderer::diagnostics::perf_capture::enabled() { return; }
        if !device.features().contains(wgpu::Features::PIPELINE_STATISTICS_QUERY) { return; }
        if self.stats_slots.is_empty() {
            let flags = wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
                | wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT
                | wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS;
            self.stats_slots = (0..STATS_SLOTS).map(|_| StatsSlot {
                query: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("FPS graph primary BSP pipeline statistics"),
                    ty: wgpu::QueryType::PipelineStatistics(flags), count: 3,
                }),
                resolved: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("BSP stats resolve"), size: RESOLVED_BYTES,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("BSP stats readback"), size: RESOLVED_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                pending: None, written: false, resolved_this_frame: false,
                ocean_written: false, ocean_resolved: false,
                shader_mode: 0,
            }).collect();
        }
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.stats_slots {
            let Some(pending) = slot.pending.as_ref() else { continue; };
            match pending.try_recv() {
                Ok(Ok(())) => {
                    if let Ok(data) = slot.readback.slice(..).get_mapped_range() {
                        let values: &[u64] = bytemuck::cast_slice(&data);
                        if slot.resolved_this_frame && values.len() >= 3 {
                            self.stats_latest = Some(PrimaryBspStats {
                                shader_mode: slot.shader_mode,
                                vertex_invocations: values[0],
                                clipped_primitives: values[1],
                                fragment_invocations: values[2],
                            });
                        }
                        let ocean = (OCEAN_STATS_OFFSET / 8) as usize;
                        if slot.ocean_resolved && values.len() >= ocean + 3 {
                            self.ocean_stats_latest = Some(OceanSurfaceStats {
                                vertex_invocations: values[ocean],
                                clipped_primitives: values[ocean + 1],
                                fragment_invocations: values[ocean + 2],
                            });
                        }
                        drop(data);
                    }
                    slot.readback.unmap();
                    slot.pending = None;
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    slot.pending = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let now = Instant::now();
        if self.stats_last_query.is_some_and(|last| now.saturating_duration_since(last) < STATS_REFRESH) {
            return;
        }
        self.stats_current = self.stats_slots.iter().position(|slot| slot.pending.is_none());
        if let Some(current) = self.stats_current {
            self.stats_last_query = Some(now);
            self.stats_slots[current].written = false;
            self.stats_slots[current].resolved_this_frame = false;
            self.stats_slots[current].ocean_written = false;
            self.stats_slots[current].ocean_resolved = false;
        }
    }

    pub fn latest_primary_bsp_stats(&self) -> Option<PrimaryBspStats> {
        self.stats_latest
    }

    pub fn latest_ocean_surface_stats(&self) -> Option<OceanSurfaceStats> {
        self.ocean_stats_latest
    }

    /// Query index 0 brackets the primary BSP pass; index 1 the ocean surface.
    pub fn stats_query_set(&self) -> Option<wgpu::QuerySet> {
        self.stats_slots.get(self.stats_current?).map(|slot| slot.query.clone())
    }

    pub fn stats_query_ended(&mut self, shader_mode: u8) {
        if let Some(slot) = self.stats_current.and_then(|i| self.stats_slots.get_mut(i)) {
            slot.written = true;
            slot.shader_mode = shader_mode;
        }
    }

    pub fn ocean_stats_query_ended(&mut self) {
        if let Some(slot) = self.stats_current.and_then(|i| self.stats_slots.get_mut(i)) {
            slot.ocean_written = true;
        }
    }

    /// Call after the ocean surface pass ended, in the encoder that recorded it.
    pub fn resolve_ocean_stats(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(slot) = self.stats_current.and_then(|i| self.stats_slots.get_mut(i)) else { return; };
        if slot.ocean_written && !slot.ocean_resolved {
            encoder.resolve_query_set(&slot.query, 1..2, &slot.resolved, OCEAN_STATS_OFFSET);
            slot.ocean_resolved = true;
        }
    }

    /// Call after the main BSP pass ended and before a possible threaded encoder split.
    pub fn resolve_stats(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(slot) = self.stats_current.and_then(|i| self.stats_slots.get_mut(i)) else { return; };
        if slot.written && !slot.resolved_this_frame {
            encoder.resolve_query_set(&slot.query, 0..1, &slot.resolved, 0);
            slot.resolved_this_frame = true;
        }
    }

    /// Copy on a separate command buffer, matching the existing GPU timestamp
    /// profiler's Vulkan-safe resolve/readback ordering.
    pub fn stats_copy(&self, device: &wgpu::Device) -> Option<wgpu::CommandBuffer> {
        let slot = self.stats_slots.get(self.stats_current?)?;
        if !slot.resolved_this_frame && !slot.ocean_resolved { return None; }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("BSP pipeline statistics readback"),
        });
        encoder.copy_buffer_to_buffer(&slot.resolved, 0, &slot.readback, 0, RESOLVED_BYTES);
        Some(encoder.finish())
    }

    pub fn submit_stats(&mut self) {
        let Some(slot) = self.stats_current.take().and_then(|i| self.stats_slots.get_mut(i)) else { return; };
        if !slot.resolved_this_frame && !slot.ocean_resolved { return; }
        let (tx, rx) = mpsc::channel();
        slot.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        slot.pending = Some(rx);
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
        width: u32, height: u32, world: Option<&WorldGpu>, ocean_samples_passed: u64,
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
            [get(GpuPass::World), get(GpuPass::Depth), get(GpuPass::HiZ),
             get(GpuPass::Cull), get(GpuPass::Cluster), get(GpuPass::Post),
             get(GpuPass::Ui), get(GpuPass::OceanSim), get(GpuPass::OceanSurface),
             get(GpuPass::Clouds), get(GpuPass::BlueWaterClouds)]
        });
        let fallback_map = world.and_then(|world| {
            let address = world as *const WorldGpu as usize;
            let count = world.coarse_batches.len();
            let variant = world.active_pipeline_variant;
            let stale = self.fallback_cache.as_ref().is_none_or(|(old_address, old_count, old_variant, at, _)| {
                *old_address != address || *old_count != count || *old_variant != variant
                    || now.saturating_duration_since(*at) >= MAP_ELIGIBILITY_REFRESH
            });
            if stale {
                self.fallback_cache = Some((address, count, variant, now,
                    crate::renderer::world::programs::map_eligibility_counts(world)));
            }
            self.fallback_cache.as_ref()
                .and_then(|(_, _, _, _, counts)| *counts)
                .map(|counts| (crate::renderer::world::programs::active_mode(variant), counts))
        });
        let summary = ui::FrameGraphSummary {
            average_fps: if self.sorted_window.is_empty() { 0.0 } else {
                1000.0 / (self.sorted_window.iter().copied().sum::<f32>()
                    / self.sorted_window.len() as f32).max(0.000_001)
            },
            fps_one_percent_low: if cutoffs[2] > 0.0 { 1000.0 / cutoffs[2] } else { 0.0 },
            cpu: previous_info.map(|info| [info.cpu_prepare_ms, info.cpu_acquire_ms,
                info.cpu_encode_ms, info.cpu_submit_ms, info.cpu_present_ms]),
            shader: previous_info.map(|info| info.world_shader_programs),
            world_draws: previous_info.map(|info| info.world_encoded_batches),
            pipeline_counts: previous_info.map(|info| info.world_pipeline_counts),
            cull: previous_info.map(|info| [info.cull_visible, info.cull_frustum_rejected,
                info.cull_hiz_rejected, info.cull_pvs_rejected, info.cull_area_rejected]),
            bsp: self.stats_latest.map(|v| (v.shader_mode, [v.vertex_invocations,
                v.clipped_primitives, v.fragment_invocations])),
            ocean: self.ocean_stats_latest.map(|v| ([v.vertex_invocations,
                v.clipped_primitives, v.fragment_invocations], ocean_samples_passed)),
            fallback_map,
        };
        ui::build_frame_time_graph_vertices(
            &mut self.vertices,
            self.samples.iter().rev().take(width as usize)
                .map(|(_, sample)| (sample.elapsed_ms, sample.phases)),
            width, height, cutoffs, gpu, summary,
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
