//! Diagnostics profiling.
use crate::renderer::{mpsc, Receiver, TryRecvError};

pub(in crate::renderer) const GPU_PASS_COUNT: usize = 53;

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
    // Sub-timing inside World: promoted/authored GodotOcean surface shading.
    // This excludes the scene-capture/underwater OceanOptics interval above.
    OceanSurface = 21,
    // Weather sub-timings. RainSim is an independent pre-depth compute interval
    // and belongs in the fallback frame sum. RainDraw is inside World and
    // RainHaze/RainLens are inside Post, so those three stay diagnostic-only.
    RainSim = 22,
    RainDraw = 23,
    RainHaze = 24,
    RainLens = 25,
    // Shared volumetric weather/fog froxel build. Independent pre-world compute.
    WeatherFog = 26,
    // GodotOcean FFT cascade generation plus the wind-foam streak transport.
    // Independent pre-world compute, encoded before the depth prepass, so it
    // belongs in the fallback frame sum. It was previously inside the Frame
    // interval but attributed to no pass, which made the ocean look cheaper
    // than it is: OceanSurface only covers the surface draw and OceanOptics
    // only the scene capture/composite.
    OceanSim = 27,
    // Sub-timings inside Post: these never contribute again to frame_ms.
    SsrVisibility = 28,
    SsrTrace = 29,
    CoastalSim = 30,
    // Sub-timing inside World after transparency; bounded original effects.
    BlueWaterUnderwater = 31,
    BlueWaterSkyPrepare = 32,
    BlueWaterSkyDraw = 33,
    BlueWaterClouds = 34,
    BlueWaterAerial = 35,
    // Diagnostics nested in world/pre-world. Not independent frame costs.
    BlueWaterCausticsCompute = 36,
    BlueWaterCausticsReceiver = 37,
    BlueWaterSceneCapture = 38,
    // Sub-timing inside World: native surfaceSprites (CPU or resident path).
    SurfaceSprites = 39,
    // Sub-timings inside Post; never independent frame costs.
    PostDof = 40,
    PostBloom = 41,
    // TAA/post resolve plus the brightness/gamma (or compact) presentation pass.
    PostFinal = 42,
    // Independent pre-world compute, before every mover geometry consumer.
    InlineTransform = 43,
    // Independent static-cache preparation; dynamic shadows are encoded pre-world.
    LocalShadowStatic = 44,
    LocalShadowMoving = 45,
    /// Core primary BSP render-pass interval, excluding most model/post tails.
    /// Nested within World; never add WorldMainPass to World or Frame.
    WorldMainPass = 46,
    // Nested within BlueWaterSkyPrepare. Do not sum with the parent interval.
    BlueWaterAtmoTables = 47,
    BlueWaterEnvironment = 48,
    BlueWaterAerialBuild = 49,
    // Live river (riversource) diagnostics. RiverSim is the whole per-frame
    // river compute (FFT detail, solver frames, window gather, material,
    // foam); it is encoded inside the OceanSim interval when an ocean exists,
    // so it stays out of the fallback frame sum. RiverSolve is nested in
    // RiverSim (solver frames + player collisions only). RiverDraw is nested
    // in World (scene copy, Single Layer Water body, composite, submerged).
    // With several rivers the first written endpoint wins (first river).
    RiverSim = 50,
    RiverSolve = 51,
    RiverDraw = 52,
}

impl GpuPass {
    pub(in crate::renderer) fn index(self) -> usize {
        self as usize
    }

    /// Every pass, in query-index order. Used by reporters that walk a resolved
    /// `GpuTiming` without knowing which passes the frame happened to run.
    pub(in crate::renderer) const ALL: [Self; GPU_PASS_COUNT] = [
        Self::Frame,
        Self::Depth,
        Self::HiZ,
        Self::Cull,
        Self::Cluster,
        Self::World,
        Self::Post,
        Self::Ui,
        Self::Grass,
        Self::GrassPrepare,
        Self::DynamicModels,
        Self::OceanOptics,
        Self::DynamicModelsTranslucent,
        Self::RtBuild,
        Self::RtSkin,
        Self::RtAcceleration,
        Self::RtVisibility,
        Self::FxSpritesOpaque,
        Self::FxSpritesTranslucent,
        Self::LegacyDlights,
        Self::Clouds,
        Self::OceanSurface,
        Self::RainSim,
        Self::RainDraw,
        Self::RainHaze,
        Self::RainLens,
        Self::WeatherFog,
        Self::OceanSim,
        Self::SsrVisibility,
        Self::SsrTrace,
        Self::CoastalSim,
        Self::BlueWaterUnderwater,
        Self::BlueWaterSkyPrepare,
        Self::BlueWaterSkyDraw,
        Self::BlueWaterClouds,
        Self::BlueWaterAerial,
        Self::BlueWaterCausticsCompute,
        Self::BlueWaterCausticsReceiver,
        Self::BlueWaterSceneCapture,
        Self::SurfaceSprites,
        Self::PostDof,
        Self::PostBloom,
        Self::PostFinal,
        Self::InlineTransform,
        Self::LocalShadowStatic,
        Self::LocalShadowMoving,
        Self::WorldMainPass,
        Self::BlueWaterAtmoTables,
        Self::BlueWaterEnvironment,
        Self::BlueWaterAerialBuild,
        Self::RiverSim,
        Self::RiverSolve,
        Self::RiverDraw,
    ];

    pub(in crate::renderer) fn label(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Depth => "depth",
            Self::HiZ => "hiz",
            Self::Cull => "cull",
            Self::Cluster => "cluster",
            Self::World => "world",
            Self::Post => "post",
            Self::Ui => "ui",
            Self::Grass => "grass",
            Self::GrassPrepare => "grass_prepare",
            Self::DynamicModels => "dynamic_models",
            Self::OceanOptics => "ocean_optics",
            Self::DynamicModelsTranslucent => "dynamic_models_translucent",
            Self::RtBuild => "rt_build",
            Self::RtSkin => "rt_skin",
            Self::RtAcceleration => "rt_acceleration",
            Self::RtVisibility => "rt_visibility",
            Self::FxSpritesOpaque => "fx_sprites_opaque",
            Self::FxSpritesTranslucent => "fx_sprites_translucent",
            Self::LegacyDlights => "legacy_dlights",
            Self::Clouds => "clouds",
            Self::OceanSurface => "ocean_surface",
            Self::RainSim => "rain_sim",
            Self::RainDraw => "rain_draw",
            Self::RainHaze => "rain_haze",
            Self::RainLens => "rain_lens",
            Self::WeatherFog => "weather_fog",
            Self::OceanSim => "ocean_sim",
            Self::SsrVisibility => "ssr_visibility",
            Self::SsrTrace => "ssr_trace",
            Self::CoastalSim => "coastal_sim",
            Self::BlueWaterUnderwater => "blue_water_underwater",
            Self::BlueWaterSkyPrepare => "blue_water_sky_prepare",
            Self::BlueWaterSkyDraw => "blue_water_sky_draw",
            Self::BlueWaterClouds => "blue_water_clouds",
            Self::BlueWaterAerial => "blue_water_aerial",
            Self::BlueWaterAtmoTables => "blue_water_atmo_tables",
            Self::BlueWaterEnvironment => "blue_water_environment",
            Self::BlueWaterAerialBuild => "blue_water_aerial_build",
            Self::BlueWaterCausticsCompute => "blue_water_caustics_compute",
            Self::BlueWaterCausticsReceiver => "blue_water_caustics_receiver",
            Self::BlueWaterSceneCapture => "blue_water_scene_capture",
            Self::SurfaceSprites => "surface_sprites",
            Self::PostDof => "post_dof",
            Self::PostBloom => "post_bloom",
            Self::PostFinal => "post_final",
            Self::InlineTransform => "inline_transform",
            Self::LocalShadowStatic => "local_shadow_static",
            Self::LocalShadowMoving => "local_shadow_moving",
            Self::WorldMainPass => "world_main_pass",
            Self::RiverSim => "river_sim",
            Self::RiverSolve => "river_solve",
            Self::RiverDraw => "river_draw",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::renderer) struct GpuTiming {
    pub(in crate::renderer) milliseconds: [Option<f64>; GPU_PASS_COUNT],
}

impl Default for GpuTiming {
    fn default() -> Self { Self { milliseconds: [None; GPU_PASS_COUNT] } }
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
            GpuPass::InlineTransform,
            GpuPass::HiZ,
            GpuPass::Cull,
            GpuPass::Cluster,
            GpuPass::GrassPrepare,
            GpuPass::RainSim,
            GpuPass::WeatherFog,
            GpuPass::OceanSim,
            GpuPass::CoastalSim,
            GpuPass::RtBuild,
            GpuPass::RtVisibility,
            // Moving cubemap depth passes run before the world, independently
            // of the depth/world intervals. Static cache builds submit in an
            // earlier command encoder; they are sporadic rather than per-frame.
            GpuPass::LocalShadowMoving,
            GpuPass::LocalShadowStatic,
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
    // `written` and `query_indices` retain the stable pass/endpoint identity.
    // Actual GPU queries are allocated contiguously as timestamps are written,
    // so resolving 0..query_count needs no dummy writes or unaligned offsets.
    pub(in crate::renderer) written: [bool; GPU_PASS_COUNT * 2],
    pub(in crate::renderer) query_indices: [u32; GPU_PASS_COUNT * 2],
    pub(in crate::renderer) query_count: u32,
    pub(in crate::renderer) active: [bool; GPU_PASS_COUNT],
    /// `GpuProfiler::frame_tag` of the frame this slot measured.
    pub(in crate::renderer) tag: u32,
}

pub(in crate::renderer) struct GpuProfiler {
    pub(in crate::renderer) slots: Vec<GpuProfilerSlot>,
    pub(in crate::renderer) current: Option<usize>,
    pub(in crate::renderer) active: bool,
    pub(in crate::renderer) timestamp_period: f32,
    pub(in crate::renderer) timestamp_inside_encoders: bool,
    pub(in crate::renderer) timestamp_inside_passes: bool,
    pub(in crate::renderer) latest: Option<GpuTiming>,
    /// Bumped every time `latest` is replaced by a freshly resolved readback.
    /// Readback lands a few frames after the frame it measured, so consumers
    /// that record per render frame need this to tell a new measurement from
    /// the previous one repeated.
    pub(in crate::renderer) latest_generation: u64,
    /// Copied into the slot claimed by the next `begin_frame`, so a consumer can
    /// tell which frame configuration an asynchronous readback measured.
    pub(in crate::renderer) frame_tag: u32,
    /// When `Some`, every resolved readback is appended with its tag, not only
    /// the newest one kept in `latest` (several slots can land in one frame).
    pub(in crate::renderer) completed: Option<Vec<(u32, GpuTiming)>>,
}

// Returns the dense hardware query index for a pass endpoint. The canonical
// pass index remains stable in `written`/`query_indices` for readback, while
// hardware query indices are contiguous for a single aligned resolve.
fn allocate_query_index(
    written: &mut [bool; GPU_PASS_COUNT * 2],
    indices: &mut [u32; GPU_PASS_COUNT * 2],
    count: &mut u32,
    canonical_index: usize,
) -> Option<u32> {
    if written[canonical_index] {
        return None;
    }
    debug_assert!(*count < GPU_QUERY_COUNT);
    let index = *count;
    *count += 1;
    written[canonical_index] = true;
    indices[canonical_index] = index;
    Some(index)
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
                latest_generation: 0,
                frame_tag: 0,
                completed: None,
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
                    query_indices: [0; GPU_PASS_COUNT * 2],
                    query_count: 0,
                    active: [false; GPU_PASS_COUNT],
                    tag: 0,
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
            latest_generation: 0,
            frame_tag: 0,
            completed: None,
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
        // Mapping callbacks are driven by poll, but polling the device on
        // every rendered frame forces WGPU/Vulkan maintenance onto the CPU
        // critical path even while unused query slots remain available.
        // Only poll when all slots are occupied. The three buffered query
        // slots tolerate the resulting (at most two-frame) diagnostic delay.
        if self.slots.iter().all(|slot| slot.pending.is_some()) {
            let _ = device.poll(wgpu::PollType::Poll);
        }

        for slot in &mut self.slots {
            let completed = {
                let Some(pending) = slot.pending.as_ref() else {
                    continue;
                };
                match pending.try_recv() {
                    Ok(Ok(())) => {
                        let Ok(bytes) = slot.readback_buffer.slice(..).get_mapped_range() else {
                            slot.readback_buffer.unmap();
                            slot.pending = None;
                            continue;
                        };
                        let values: &[u64] = bytemuck::cast_slice(&bytes);
                        // Resolve data is dense, but timing consumers expect
                        // stable pass*2 / pass*2+1 indices. Restore those
                        // positions only for timestamps genuinely written.
                        let mut pass_values = [0u64; GPU_PASS_COUNT * 2];
                        for (canonical, &written) in slot.written.iter().enumerate() {
                            if written {
                                pass_values[canonical] =
                                    values[slot.query_indices[canonical] as usize];
                            }
                        }
                        let timing = GpuTiming::from_query_data(
                            &pass_values,
                            slot.active,
                            self.timestamp_period,
                        );
                        drop(bytes);
                        slot.readback_buffer.unmap();
                        Some(timing)
                    }
                    Ok(Err(_)) | Err(TryRecvError::Disconnected) => None,
                    Err(TryRecvError::Empty) => continue,
                }
            };
            slot.pending = None;
            if let (Some(timing), Some(log)) = (completed, self.completed.as_mut()) {
                log.push((slot.tag, timing));
            }
            if completed.is_some() {
                self.latest = completed;
                self.latest_generation = self.latest_generation.wrapping_add(1);
            }
        }

        self.current = self.slots.iter().position(|slot| slot.pending.is_none());
        if let Some(current) = self.current {
            self.slots[current].written = [false; GPU_PASS_COUNT * 2];
            self.slots[current].query_count = 0;
            self.slots[current].active = [false; GPU_PASS_COUNT];
            self.slots[current].tag = self.frame_tag;
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
        let Some(dense_index) = allocate_query_index(
            &mut slot.written,
            &mut slot.query_indices,
            &mut slot.query_count,
            query_index,
        ) else {
            return;
        };
        encoder.write_timestamp(&slot.query_set, dense_index);
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
        let Some(dense_index) = allocate_query_index(
            &mut slot.written,
            &mut slot.query_indices,
            &mut slot.query_count,
            query_index,
        ) else {
            return;
        };
        pass.write_timestamp(&slot.query_set, dense_index);
        slot.active[pass_index] = slot.written[pass_index * 2] && slot.written[pass_index * 2 + 1];
    }

    pub(in crate::renderer) fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(current) = self.current else {
            return;
        };
        let slot = &self.slots[current];
        if slot.query_count == 0 {
            return;
        }

        // A single resolve at offset 0 is always aligned to WGPU's
        // QUERY_RESOLVE_BUFFER_ALIGNMENT (256 bytes). Sparse query indices
        // used to resolve at 8 * start, which crashed when start was not
        // a multiple of 32. Dense allocation also avoids dummy timestamps.
        encoder.resolve_query_set(
            &slot.query_set,
            0..slot.query_count,
            &slot.resolve_buffer,
            0,
        );
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

    pub(in crate::renderer) fn latest_timing(&self) -> Option<GpuTiming> {
        self.latest
    }

    /// Identity of the current `latest` measurement. Equal values across two
    /// render frames mean both reported the same resolved GPU frame.
    pub(in crate::renderer) fn latest_generation(&self) -> u64 {
        self.latest_generation
    }
}

#[cfg(test)]
mod sparse_gpu_query_tests {
    use super::{allocate_query_index, GpuPass, GpuTiming, GPU_PASS_COUNT};

    #[test]
    fn sparse_writes_resolve_as_one_aligned_dense_range() {
        let mut written = [false; GPU_PASS_COUNT * 2];
        let mut indices = [0; GPU_PASS_COUNT * 2];
        let mut count = 0;
        // A high-numbered pass may start before an earlier-numbered pass.
        for (canonical, expected) in [(92, 0), (10, 1), (11, 2), (93, 3)] {
            assert_eq!(
                allocate_query_index(&mut written, &mut indices, &mut count, canonical),
                Some(expected),
            );
        }
        assert_eq!(
            allocate_query_index(&mut written, &mut indices, &mut count, 10),
            None,
        );
        assert_eq!(count, 4);
        // Reconstruct canonical pass timing from resolved dense timestamps.
        let values = [100u64, 120, 170, 200];
        let mut canonical_values = [0u64; GPU_PASS_COUNT * 2];
        for (canonical, &is_written) in written.iter().enumerate() {
            if is_written {
                canonical_values[canonical] = values[indices[canonical] as usize];
            }
        }
        let mut active = [false; GPU_PASS_COUNT];
        active[GpuPass::WorldMainPass.index()] = true;
        active[GpuPass::World.index()] = true;
        let timing = GpuTiming::from_query_data(&canonical_values, active, 1.0);
        assert_eq!(timing.milliseconds[GpuPass::WorldMainPass.index()], Some(0.0001));
        assert_eq!(timing.milliseconds[GpuPass::World.index()], Some(0.00005));
    }
}
