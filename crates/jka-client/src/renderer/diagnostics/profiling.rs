//! Diagnostics profiling.
use crate::renderer::{mpsc, Receiver, TryRecvError};

pub(in crate::renderer) const GPU_PASS_COUNT: usize = 21;

pub(in crate::renderer) const GPU_QUERY_COUNT: u32 = (GPU_PASS_COUNT * 2) as u32;

pub(in crate::renderer) const GPU_PROFILER_SLOTS: usize = 3;

pub(in crate::renderer) const CULL_DIAGNOSTIC_READBACK_SLOTS: usize = 3;

pub(in crate::renderer) const CULL_DIAGNOSTIC_COUNTER_BYTES: u64 = 16;

#[derive(Clone, Copy)]
pub(in crate::renderer) enum GpuPass {
    Frame = 0,
    Depth = 1,
    HiZ = 2,
    Cull = 3,
    Cluster = 4,
    World = 5,
    Post = 6,
    Ui = 7,
    // Sub-timing inside the World interval; do not add it to frame_ms fallback.
    Grass = 8,
    // Compute preparation runs immediately before World and therefore *is* an
    // independent frame interval in the no-Frame-timestamp fallback.
    GrassPrepare = 9,
    // Sub-timing inside World: dynamic MD3/GLM entities, including GPU Ghoul2 skinning.
    DynamicModels = 10,
    // Sub-timing inside World: scene capture, absorption/caustics, composite.
    OceanOptics = 11,
    // Sub-timing inside World: non-depth-writing dynamic FX deferred until after
    // ocean surface/spray composition so water cannot overpaint sabers/particles.
    DynamicModelsTranslucent = 12,
    // Independent pre-world interval: RT-only Ghoul2 compute skinning plus
    // dynamic BLAS/TLAS construction. Cold when RT Shadows is disabled.
    RtBuild = 13,
    // Sub-intervals of RtBuild; exclude from the frame-time fallback sum.
    RtSkin = 14,
    RtAcceleration = 15,
    RtVisibility = 16,
    // Sub-timings inside dynamic models for GPU-instanced EFX sprites. Keep
    // opaque/depth-writing and translucent phases separate because the ocean
    // path can split them into distinct render passes.
    FxSpritesOpaque = 17,
    FxSpritesTranslucent = 18,
    // Sub-timing inside World: OpenJK-style additive projected-light redraw of
    // only BSP triangle runs carrying a non-zero Legacy dlight surface mask.
    LegacyDlights = 19,
    // Independent interval between the world and post passes: volumetric cloud
    // march plus its temporal resolve.
    Clouds = 20,
}

impl GpuPass {
    pub(in crate::renderer) fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct GpuTiming {
    pub(in crate::renderer) milliseconds: [Option<f64>; GPU_PASS_COUNT],
}

impl GpuTiming {
    pub(in crate::renderer) fn from_query_data(
        values: &[u64],
        active: [bool; GPU_PASS_COUNT],
        period: f32,
    ) -> Self {
        let mut milliseconds = [None; GPU_PASS_COUNT];
        let nanoseconds_per_tick = f64::from(period);
        for pass in 0..GPU_PASS_COUNT {
            if active[pass] {
                let start = values[pass * 2];
                let end = values[pass * 2 + 1];
                milliseconds[pass] = end
                    .checked_sub(start)
                    .map(|ticks| ticks as f64 * nanoseconds_per_tick / 1_000_000.0);
            }
        }
        Self { milliseconds }
    }

    pub(in crate::renderer) fn frame_ms(self) -> Option<f64> {
        if let Some(frame) = self.milliseconds[GpuPass::Frame.index()] {
            return Some(frame);
        }

        let mut total = 0.0;
        let mut any = false;
        for pass in [
            GpuPass::Depth,
            GpuPass::HiZ,
            GpuPass::Cull,
            GpuPass::Cluster,
            GpuPass::GrassPrepare,
            GpuPass::RtBuild,
            GpuPass::RtVisibility,
            GpuPass::World,
            GpuPass::Clouds,
            GpuPass::Post,
            GpuPass::Ui,
        ] {
            if let Some(milliseconds) = self.milliseconds[pass.index()] {
                total += milliseconds;
                any = true;
            }
        }
        any.then_some(total)
    }
}

pub(in crate::renderer) struct GpuProfilerSlot {
    pub(in crate::renderer) query_set: wgpu::QuerySet,
    pub(in crate::renderer) resolve_buffer: wgpu::Buffer,
    pub(in crate::renderer) readback_buffer: wgpu::Buffer,
    pub(in crate::renderer) pending: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
    // Tracks timestamp slots that were genuinely written by the measured frame.
    // Before resolve we fill any holes with harmless dummy timestamps so Vulkan
    // never sees an unavailable query in the resolved range. `active` remains
    // true only for passes with both real endpoints, so dummy values are ignored.
    pub(in crate::renderer) written: [bool; GPU_PASS_COUNT * 2],
    pub(in crate::renderer) active: [bool; GPU_PASS_COUNT],
}

pub(in crate::renderer) struct GpuProfiler {
    pub(in crate::renderer) slots: Vec<GpuProfilerSlot>,
    pub(in crate::renderer) current: Option<usize>,
    pub(in crate::renderer) active: bool,
    pub(in crate::renderer) timestamp_period: f32,
    pub(in crate::renderer) timestamp_inside_encoders: bool,
    pub(in crate::renderer) timestamp_inside_passes: bool,
    pub(in crate::renderer) latest: Option<GpuTiming>,
}

impl GpuProfiler {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        timestamp_supported: bool,
        timestamp_inside_encoders: bool,
        timestamp_inside_passes: bool,
    ) -> Self {
        if !timestamp_supported {
            return Self {
                slots: Vec::new(),
                current: None,
                active: false,
                timestamp_period: 0.0,
                timestamp_inside_encoders: false,
                timestamp_inside_passes: false,
                latest: None,
            };
        }

        let slots = (0..GPU_PROFILER_SLOTS)
            .map(|_| {
                let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("JKA GPU frame profiler queries"),
                    ty: wgpu::QueryType::Timestamp,
                    count: GPU_QUERY_COUNT,
                });
                let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA GPU profiler resolve buffer"),
                    size: u64::from(GPU_QUERY_COUNT) * 8,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA GPU profiler readback buffer"),
                    size: u64::from(GPU_QUERY_COUNT) * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                GpuProfilerSlot {
                    query_set,
                    resolve_buffer,
                    readback_buffer,
                    pending: None,
                    written: [false; GPU_PASS_COUNT * 2],
                    active: [false; GPU_PASS_COUNT],
                }
            })
            .collect();

        Self {
            slots,
            current: None,
            active: false,
            timestamp_period: queue.get_timestamp_period(),
            timestamp_inside_encoders,
            timestamp_inside_passes,
            latest: None,
        }
    }

    pub(in crate::renderer) fn enabled(&self) -> bool {
        self.active && !self.slots.is_empty()
    }

    pub(in crate::renderer) fn available(&self) -> bool {
        !self.slots.is_empty()
    }

    pub(in crate::renderer) fn set_enabled(&mut self, enabled: bool) {
        self.active = enabled && self.available();
        if !self.active {
            self.current = None;
            self.latest = None;
        }
    }

    pub(in crate::renderer) fn begin_frame(&mut self, device: &wgpu::Device) {
        if !self.enabled() {
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);

        for slot in &mut self.slots {
            let completed = {
                let Some(pending) = slot.pending.as_ref() else {
                    continue;
                };
                match pending.try_recv() {
                    Ok(Ok(())) => {
                        let bytes = slot.readback_buffer.slice(..).get_mapped_range();
                        let values: &[u64] = bytemuck::cast_slice(&bytes);
                        let timing =
                            GpuTiming::from_query_data(values, slot.active, self.timestamp_period);
                        drop(bytes);
                        slot.readback_buffer.unmap();
                        Some(timing)
                    }
                    Ok(Err(_)) | Err(TryRecvError::Disconnected) => None,
                    Err(TryRecvError::Empty) => continue,
                }
            };
            slot.pending = None;
            if completed.is_some() {
                self.latest = completed;
            }
        }

        self.current = self.slots.iter().position(|slot| slot.pending.is_none());
        if let Some(current) = self.current {
            self.slots[current].written = [false; GPU_PASS_COUNT * 2];
            self.slots[current].active = [false; GPU_PASS_COUNT];
        }
    }

    pub(in crate::renderer) fn write_encoder_timestamp(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        pass: GpuPass,
        end: bool,
    ) {
        if !self.timestamp_inside_encoders {
            return;
        }
        let Some(current) = self.current else {
            return;
        };
        let pass_index = pass.index();
        let query_index = pass_index * 2 + usize::from(end);
        let slot = &mut self.slots[current];
        // A query index may only be written once per profiler frame. Guarding
        // this also makes optional/overlapping renderer branches harmless.
        if slot.written[query_index] {
            return;
        }
        encoder.write_timestamp(&slot.query_set, query_index as u32);
        slot.written[query_index] = true;
        slot.active[pass_index] = slot.written[pass_index * 2] && slot.written[pass_index * 2 + 1];
    }

    pub(in crate::renderer) fn write_render_pass_timestamp(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        timed_pass: GpuPass,
        end: bool,
    ) {
        if !self.timestamp_inside_passes {
            return;
        }
        let Some(current) = self.current else {
            return;
        };
        let pass_index = timed_pass.index();
        let query_index = pass_index * 2 + usize::from(end);
        let slot = &mut self.slots[current];
        if slot.written[query_index] {
            return;
        }
        pass.write_timestamp(&slot.query_set, query_index as u32);
        slot.written[query_index] = true;
        slot.active[pass_index] = slot.written[pass_index * 2] && slot.written[pass_index * 2 + 1];
    }

    pub(in crate::renderer) fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(current) = self.current else {
            return;
        };
        if !self.slots[current].written.iter().any(|written| *written) {
            return;
        }

        let slot = &mut self.slots[current];
        // WebGPU/Vulkan requires every query in a resolved range to have become
        // available. Optional renderer passes leave holes in our fixed query set,
        // so write harmless end-of-frame timestamps into those unused slots before
        // resolving the whole compact range. These dummy slots are not marked
        // active and therefore never appear in reported timings.
        for query_index in 0..GPU_QUERY_COUNT as usize {
            if !slot.written[query_index] {
                encoder.write_timestamp(&slot.query_set, query_index as u32);
            }
        }
        encoder.resolve_query_set(&slot.query_set, 0..GPU_QUERY_COUNT, &slot.resolve_buffer, 0);
    }

    pub(in crate::renderer) fn encode_readback_copy(
        &self,
        device: &wgpu::Device,
    ) -> Option<wgpu::CommandBuffer> {
        let current = self.current?;
        let slot = &self.slots[current];
        if !slot.written.iter().any(|written| *written) {
            return None;
        }

        // Keep the resolve and resolve-buffer copy in separate command buffers.
        // This avoids a long-standing Vulkan/wgpu hazard where resolving a
        // timestamp and consuming that resolve buffer in the same encoder can
        // yield zero/stale results. Queue order still guarantees the copy waits
        // for the main frame command buffer's resolve.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("JKA GPU profiler readback copy encoder"),
        });
        encoder.copy_buffer_to_buffer(
            &slot.resolve_buffer,
            0,
            &slot.readback_buffer,
            0,
            u64::from(GPU_QUERY_COUNT) * 8,
        );
        Some(encoder.finish())
    }

    pub(in crate::renderer) fn submit(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        if !self.slots[current].active.iter().any(|active| *active) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.slots[current].readback_buffer.slice(..).map_async(
            wgpu::MapMode::Read,
            move |result| {
                let _ = sender.send(result);
            },
        );
        self.slots[current].pending = Some(receiver);
    }

    pub(in crate::renderer) fn latest_frame_ms(&self) -> Option<f64> {
        self.latest.and_then(GpuTiming::frame_ms)
    }
}
