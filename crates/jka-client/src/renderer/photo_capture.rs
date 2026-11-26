//! On-demand independent full-world capture. Reuses the Studio scene renderer,
//! not reflection probes or repeated screenshots of the swapchain.
use super::{block_on, Renderer, RenderSnapshot, ScreenshotFormat, ViewLatchMode};
use super::capture::ScreenshotDestination;
#[path = "photo_plan.rs"]
pub(crate) mod plan;
#[path = "photo_stream.rs"]
mod stream;
#[path = "photo_readback.rs"]
mod readback;
#[path = "photo_strips.rs"]
mod strips;
#[path = "photo_tiles.rs"]
mod tiles;
use plan::{CapturePlan, OutputPlan, PlanInputs, WorldPlan};
use crate::runtime::{ScreenshotOutput, UserEvent};
use std::{path::PathBuf, sync::Arc, thread::JoinHandle, time::{Duration, Instant}};
use winit::{dpi::PhysicalSize, event_loop::EventLoopProxy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PhotoProjection { Equirectangular, Cross, Faces, Planet, Tunnel, Cylindrical, Fisheye }
impl PhotoProjection {
    pub const ALL: [Self; 7] = [Self::Equirectangular, Self::Cross, Self::Faces, Self::Planet, Self::Tunnel, Self::Cylindrical, Self::Fisheye];
    pub fn name(self) -> &'static str { match self {
        Self::Equirectangular => "equirectangular", Self::Cross => "cross", Self::Faces => "cubemap", Self::Planet => "planet", Self::Tunnel => "tunnel", Self::Cylindrical => "cylindrical", Self::Fisheye => "fisheye",
    }}
    pub fn label(self) -> &'static str { match self {
        Self::Equirectangular => "Equirectangular", Self::Cross => "Cubemap cross", Self::Faces => "Cubemap faces", Self::Planet => "Little planet", Self::Tunnel => "Inverted planet", Self::Cylindrical => "Cylindrical", Self::Fisheye => "Dual fisheye",
    }}
    pub fn parse(value: &str) -> Option<Self> { Self::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(value)) }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhotoOptions {
    pub panorama: bool,
    pub projection: PhotoProjection,
    pub width: u32,
    pub panorama_width: u32,
    pub format: ScreenshotFormat,
    pub supersample: u32,
    pub max_msaa: bool,
    pub hide_ui: bool,
    pub hide_weapon: bool,
    pub hide_player: bool,
    /// Write server, player names and scoreboard into screenshot XMP
    /// (all screenshot paths, including `/screenshot`).
    pub embed_game_metadata: bool,
}
impl Default for PhotoOptions {
    fn default() -> Self { Self { panorama: false, projection: PhotoProjection::Equirectangular, width: 0, panorama_width: 8192, format: ScreenshotFormat::Png, supersample: 1, max_msaa: true, hide_ui: true, hide_weapon: true, hide_player: false, embed_game_metadata: true } }
}
impl PhotoOptions {
    pub const WIDTHS: [u32; 5] = [2048,4096,8192,12288,16384];
    /// libjpeg's JPEG_MAX_DIMENSION; every JPEG path uses this limit so the
    /// files open in libjpeg-based viewers and tools.
    pub const JPEG_MAX_DIMENSION: u32 = 65500;
    pub const MAX_JPEG_PANORAMA_WIDTH: u32 = Self::JPEG_MAX_DIMENSION;
    pub const EQUIRECT_WIDTHS: [u32; 10] = [2048,4096,8192,12288,16384,32768,65500,65536,100000,131072];
    pub fn valid_panorama_width(projection: PhotoProjection, width: u32) -> bool {
        if projection == PhotoProjection::Equirectangular { (2048..=131072).contains(&width) && width % 2 == 0 }
        else { Self::WIDTHS.contains(&width) }
    }
    /// Menu estimate of the automatic capture plan (nominal GPU limits).
    /// The worker re-plans from the device's limits and free VRAM.
    pub(crate) fn estimated_plan(self) -> Option<Result<CapturePlan, String>> {
        self.panorama.then(|| plan::plan(self, PlanInputs::nominal(self)))
    }
    pub fn parse_format(value: &str) -> Option<ScreenshotFormat> { match value.to_ascii_lowercase().as_str() { "png" => Some(ScreenshotFormat::Png), "jpg" | "jpeg" => Some(ScreenshotFormat::Jpeg), _ => None } }
    pub fn archive(self) -> String { format!("{},{},{},{},{},{},{},{},{},{},{}", self.panorama as u8, self.projection.name(), self.width, self.panorama_width, if self.format == ScreenshotFormat::Png {"png"} else {"jpg"}, self.supersample, self.hide_ui as u8, self.hide_weapon as u8, self.hide_player as u8, self.max_msaa as u8, self.embed_game_metadata as u8) }
    pub fn parse(value: &str) -> Option<Self> {
        let fields = value.split(',').collect::<Vec<_>>();
        // Shorter archives predate trailing fields; missing ones default on.
        let (legacy, max_msaa, embed) = match fields.as_slice() {
            [_, _, _, _, _, _, _, _, _] => (fields.as_slice(), "1", "1"),
            [_, _, _, _, _, _, _, _, _, msaa] => (&fields[..9], *msaa, "1"),
            [_, _, _, _, _, _, _, _, _, msaa, embed] => (&fields[..9], *msaa, *embed),
            _ => return None,
        };
        let [mode, projection, width, pano, format, sample, ui, weapon, player] = legacy else { return None; };
        let boolean = |v: &str| match v { "0" => Some(false), "1" => Some(true), _ => None };
        let width = width.parse::<u32>().ok()?;
        let panorama_width = pano.parse::<u32>().ok()?;
        let supersample = sample.parse::<u32>().ok()?;
        if (width != 0 && !Self::WIDTHS.contains(&width)) || !Self::valid_panorama_width(PhotoProjection::parse(projection)?, panorama_width) || ![1,2,4].contains(&supersample) { return None; }
        Some(Self { panorama: boolean(mode)?, projection: PhotoProjection::parse(projection)?, width, panorama_width, format: Self::parse_format(format)?, supersample, max_msaa: boolean(max_msaa)?, hide_ui: boolean(ui)?, hide_weapon: boolean(weapon)?, hide_player: boolean(player)?, embed_game_metadata: boolean(embed)? })
    }
    pub fn render_size(self, display: [u32;2]) -> Option<[u32;2]> {
        let base = if self.panorama {
            [if self.projection == PhotoProjection::Equirectangular {
                (f64::from(self.panorama_width) / std::f64::consts::PI).ceil() as u32
            } else { self.panorama_width / 4 }; 2]
        } else {
            self.output_size(display)
        };
        Some([base[0].checked_mul(self.supersample)?, base[1].checked_mul(self.supersample)?])
    }
    pub fn output_size(self, display: [u32;2]) -> [u32;2] {
        if !self.panorama {
            if self.width == 0 { return display; }
            return [self.width, ((u64::from(self.width)*u64::from(display[1])+u64::from(display[0])/2)/u64::from(display[0].max(1))).clamp(1,u64::from(u32::MAX)) as u32];
        }
        let w = self.panorama_width;
        match self.projection {
            PhotoProjection::Cross => [w, w*3/4], PhotoProjection::Faces => [w/4,w/4],
            PhotoProjection::Planet | PhotoProjection::Tunnel => [w,w],
            PhotoProjection::Cylindrical => [w, (w as f32 / std::f32::consts::PI).round() as u32],
            _ => [w,w/2],
        }
    }
}

pub struct PhotoCaptureRequest {
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub options: PhotoOptions,
    pub snapshot: RenderSnapshot,
    pub hud: crate::ui::UiSnapshot,
    pub map: Arc<crate::scene::PreparedMap>,
    pub game: Option<PathBuf>,
    pub directory: PathBuf,
    pub metadata: crate::screenshot::ScreenshotMetadata,
    pub display_size: [u32;2],
    pub viewed_client: Option<u16>,
}

/// Effective environment at the render-command boundary, not the UI cvars.
/// Recreate independent GPU resources from these immutable inputs. Do not share
/// mutable simulation targets or bind groups with the live render thread.
struct CaptureSceneState {
    post: Option<super::PostEffects>,
    exposure: wgpu::Buffer,
    ocean_settings: crate::ocean::OceanSettings,
    ocean_enabled: bool,
    ocean_sky_mode: u8,
    ocean_overrides: Vec<(usize, crate::ocean::OceanSettings)>,
    coastal_enabled: bool,
    coastal_pool: u8,
    coastal_atlas: Option<Arc<jka_assets::bsp::bathymetry::Atlas>>,
    upstream_shore_source: Option<Arc<crate::ocean::upstream_coast::Source>>,
    authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    sailing_sea: Option<Arc<jka_movement::SailingSea>>,
    ocean_time: Option<f32>,
    authored_times: Vec<(usize, f32)>,
    sun_selection: Option<super::lighting::sun_bake::SunBakeSelection>,
    fire: super::ninja_fire::FireCaptureState,
    overbright_bits: u8,
    jk2_legacy_color: bool,
    // If a feature was ready on the live renderer, a capture must not silently
    // downgrade it. Only compare view-independent ownership/readiness here.
    require_ocean: bool,
    require_ninja_pools: u8,
    require_atmosphere: bool,
    require_baked_light: bool,
    require_static_sun: bool,
}
impl CaptureSceneState {
    fn from_live(primary: &Renderer) -> Self {
        // Snapshot GPU adaptation at the same command boundary. Never share
        // the writable live state buffer, and don't map/stall the live queue.
        let bytes = std::mem::size_of::<super::AutoExposureState>() as u64;
        let exposure = primary.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Photo frozen exposure"), size: bytes,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = primary.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&primary.auto_exposure_state_buffer, 0, &exposure, 0, bytes);
        primary.queue.submit([encoder.finish()]);
        Self {
            post: primary.scene_post_effects,
            exposure,
            ocean_settings: primary.ocean_settings,
            ocean_enabled: primary.ocean_enabled,
            ocean_sky_mode: if primary.ocean_sky_extend { if primary.ocean_sky_infinite { 2 } else { 1 } } else { 0 },
            ocean_overrides: primary.promoted_ocean_overrides.clone(),
            coastal_enabled: primary.coastal_enabled,
            coastal_pool: primary.coastal_pool,
            coastal_atlas: primary.coastal_atlas.clone(),
            upstream_shore_source: primary.upstream_shore_source.clone(),
            authored_oceans: primary.authored_ocean_definitions.clone(),
            sailing_sea: primary.sailing_sea.clone(),
            ocean_time: primary.ocean.as_ref().map(|o| o.capture_time()),
            authored_times: primary.authored_oceans.iter().map(|(a, o)| (a.index, o.capture_time())).collect(),
            sun_selection: primary.world.as_ref().map(|w| w.sun_bake.selection()),
            fire: primary.ninja_fire_capture_state(),
            overbright_bits: primary.overbright_bits,
            jk2_legacy_color: primary.jk2_legacy_color,
            require_ocean: primary.ocean_render_ready(),
            require_ninja_pools: primary.ocean.as_ref().map_or(0, |o| o.capture_ninja_ready_mask()),
            require_atmosphere: primary.blue_water_sky.is_some(),
            require_baked_light: primary.baked_sun_cache_active(),
            require_static_sun: primary.static_sun_lightmap_active(),
        }
    }

    fn apply_controls(&self, source: &mut Renderer, options: PhotoOptions) {
        if let Some(mut post) = self.post {
            // Capture-only exclusions, applied to effective controls rather
            // than rebuilding relighting/weather from the saved video menu.
            post.taa = false;
            post.dlss = false;
            post.rt_taa = 0;
            post.motion_blur_strength = 0.0;
            post.cloud_temporal = false;
            if options.panorama {
                post.ssao = false;
                post.contact_shadows = false;
                // Keep planar promotion, not view-local screen-space SSR.
                if post.reflection_quality.ssr_enabled() {
                    post.reflection_quality = crate::ui::ReflectionQuality::Low;
                }
                post.depth_of_field_strength = 0.0;
                post.dof_mode = crate::ui::DofMode::Off;
                post.crt_scope = crate::ui::CrtScope::Off;
                post.chromatic_aberration = 0.0;
                post.vignette = false;
                post.film_grain_strength = 0.0;
                post.rain_glass = false;
            }
            if options.panorama || options.supersample > 1 {
                post.fxaa = false;
                post.smaa = false;
            }
            source.set_post_effects(post);
        }
        source.overbright_bits = self.overbright_bits;
        source.jk2_legacy_color = self.jk2_legacy_color;
        source.refresh_output_gamma();
        source.set_ocean_settings(self.ocean_settings);
        source.set_ocean_enabled(self.ocean_enabled);
        source.photo_capture_active = true;
        let mut encoder = source.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.exposure, 0, &source.auto_exposure_state_buffer, 0,
            std::mem::size_of::<super::AutoExposureState>() as u64);
        source.queue.submit([encoder.finish()]);
    }

    fn install_map_state(&self, source: &mut Renderer) {
        // Map load explicitly clears promoted_ocean_overrides and resets
        // interaction; install the live descriptors only AFTER the load.
        source.promoted_ocean_overrides = self.ocean_overrides.clone();
        // SetCoastalWater is a runtime map command, not a video preference.
        // Share only its CPU atlas/terrain cache, never mutable GPU fields.
        source.coastal_enabled = self.coastal_enabled;
        source.coastal_pool = self.coastal_pool;
        source.coastal_atlas = self.coastal_atlas.clone();
        source.upstream_shore_source = self.upstream_shore_source.clone();
        source.set_ocean_sky_extend(self.ocean_sky_mode);
        source.set_sailing_sea(self.sailing_sea.clone());
        source.set_authored_oceans(self.authored_oceans.clone());
        if let (Some(ocean), Some(time)) = (source.ocean.as_mut(), self.ocean_time) {
            ocean.set_time(time);
        }
        for (descriptor, ocean) in &mut source.authored_oceans {
            if let Some((_, time)) = self.authored_times.iter().find(|(index, _)| *index == descriptor.index) {
                ocean.set_time(*time);
            }
        }
        if let Some(selection) = self.sun_selection {
            source.apply_sun_bake_selection(selection);
        }
    }

    fn ninja_pools_ready(&self, source: &Renderer) -> bool {
        let ready = source.ocean.as_ref().map_or(0, |o| o.capture_ninja_ready_mask());
        self.require_ninja_pools & !ready == 0
    }

    fn verify_ready(&self, source: &mut Renderer) -> Result<(), String> {
        if source.pipeline_jobs.stats().failed != 0 {
            return Err("Screenshot pipeline compilation failed; no image saved (see renderer console)".into());
        }
        let ready_pools = source.ocean.as_ref().map_or(0, |o| o.capture_ninja_ready_mask());
        let missing = if self.require_ocean && !source.ocean_render_ready() { "promoted ocean" }
            else if self.require_ninja_pools & !ready_pools != 0 { "Fluid Ninja ocean pools" }
            else if self.require_atmosphere && source.blue_water_sky.is_none() { "native atmosphere" }
            else if self.require_baked_light && !source.baked_sun_cache_active() { "baked relighting" }
            else if self.require_static_sun && !source.static_sun_lightmap_active() { "static sun relighting" }
            else { return Ok(()); };
        Err(format!("Screenshot capture could not reproduce live {missing}; no image saved"))
    }
}

/// Whole-capture preparation stamp. A lazy pipeline requested by ANY face or
/// a bake/ownership change invalidates all earlier faces. This is intentionally
/// independent of the projection and has no cost in the gameplay frame loop.
#[derive(Debug, PartialEq, Eq)]
struct CapturePreparation {
    jobs: (u64, u64, u64, u64, usize, usize),
    world: Option<super::WorldShaderVariantKey>,
    lighting: Option<(u64, bool, bool, bool)>,
    atmosphere: bool,
    ocean: bool,
    ninja_pools: u8,
    river_owners: u8,
    pending: bool,
}
impl CapturePreparation {
    fn read(source: &mut Renderer) -> Self {
        let jobs = source.pipeline_jobs.stats();
        let world = source.world.as_ref();
        Self {
            jobs: (jobs.queued, jobs.completed, jobs.stale, jobs.failed, jobs.compiling, jobs.ready),
            world: world.map(|w| w.active_pipeline_variant),
            lighting: world.map(|w| (w.static_sun.generation, w.sun_bake.ready, w.sun_bake.has_masks(), w.static_ao_lightmap_active)),
            atmosphere: source.blue_water_sky.is_some(),
            ocean: source.ocean_render_ready(),
            ninja_pools: source.ocean.as_ref().map_or(0, |o| o.capture_ninja_ready_mask()),
            river_owners: source.river_promotion_active,
            pending: jobs.compiling != 0 || source.scene_rebuild_pending || source.world_variant_pending.is_some()
                || source.static_ao_pending.is_some() || source.baked_brightness.busy
                || world.is_some_and(|w| w.sun_bake.capture_pending() || w.static_sun.capture_pending()),
        }
    }
}

fn poll_capture_preparation(source: &mut Renderer) {
    source.poll_static_ao_updates();
    source.poll_static_ao_worker();
    source.sync_sun_bake();
    source.sync_static_sun_lightmaps();
    source.sync_baked_sun_lightmap_cache();
    source.apply_pending_scene_rebuild();
}

pub(super) fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) { Err("Screenshot cancelled".into()) } else { Ok(()) }
}

pub(super) fn progress(proxy: &EventLoopProxy<UserEvent>, status: String, completed: u64, total: u64, started: Instant) {
    let eta_seconds = if completed > 0 && completed <= total {
        Some((started.elapsed().as_secs_f64() * (total-completed) as f64 / completed as f64).ceil() as u64)
    } else { None };
    let _ = proxy.send_event(UserEvent::PhotoCaptureProgress(crate::runtime::PhotoProgress { status, completed, total, eta_seconds }));
}

/// Game time the capture steps Ninja fire fields before rendering.
const FIRE_WARM_SECONDS: f32 = 3.0;

pub(super) const OOM_PREFIX: &str = "Screenshot GPU memory exhausted: ";

/// Per-attempt OOM + validation error scopes on the capture thread (scopes
/// are thread-local, so live-renderer errors are never attributed here).
/// Checked after every world render: an allocation failure is reported at
/// the first tile instead of rendering with invalid resources, and the
/// resulting validation errors no longer reach wgpu's fatal default handler.
pub(super) struct GpuWatch {
    device: wgpu::Device,
    scopes: Option<(wgpu::ErrorScopeGuard, wgpu::ErrorScopeGuard)>,
}
impl GpuWatch {
    fn new(device: &wgpu::Device) -> Self {
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        Self { device: device.clone(), scopes: Some((oom, validation)) }
    }
    fn pop(&mut self) -> Result<(), String> {
        let Some((oom, validation)) = self.scopes.take() else { return Ok(()); };
        let validation = block_on(validation.pop());
        let oom = block_on(oom.pop());
        match (oom, validation) {
            (Some(error), _) => Err(format!("{OOM_PREFIX}{error}")),
            (None, Some(error)) => Err(format!("Screenshot GPU validation error: {error}")),
            (None, None) => Ok(()),
        }
    }
    pub(super) fn check(&mut self) -> Result<(), String> {
        let result = self.pop();
        if result.is_ok() { *self = Self::new(&self.device); }
        result
    }
}
impl Drop for GpuWatch {
    fn drop(&mut self) { let _ = self.pop(); }
}

/// One `[PHOTO PERF]` line per job (success, error or cancel). Stages that
/// overlap (GPU, unpack, projection, encode) are reported separately and are
/// not an additive decomposition of `total`. `*_wait` values are wall-clock
/// blocking, not GPU execution time (no timestamp queries are used).
#[derive(Default)]
pub(super) struct PhotoPerf {
    plan: Option<CapturePlan>,
    fallback: Option<String>,
    msaa: u32,
    init: Duration,
    setup: Duration,
    pub(super) preparation_wait: Duration,
    pub(super) sweep: Duration,
    pub(super) render_cpu: Duration,
    pub(super) gpu_wait: Duration,
    pub(super) readback_wait: Duration,
    pub(super) store_write: Duration,
    pub(super) unpack: Duration,
    pub(super) projection: Duration,
    encode: Duration,
    encode_input_wait: Duration,
    encode_backpressure: Duration,
    commit: Duration,
    pub(super) world_renders: u64,
    pub(super) discarded_renders: u64,
    pub(super) restarts: u32,
    pub(super) restart_reason: &'static str,
    pub(super) strips: u64,
    pub(super) store_written: u64,
    pub(super) store_read: u64,
    pub(super) cache: (u64, u64, u64),
    pub(super) peak_gpu_buffers: u64,
    output_bytes: u64,
    /// Process dedicated-VRAM growth (DXGI) after the first world render.
    vram_baseline: Option<u64>,
    pub(super) vram_delta: Option<i64>,
}
impl PhotoPerf {
    /// Once per job, after the first world render: measured footprint of the
    /// capture renderer (+cube) to calibrate the planner estimate.
    pub(super) fn sample_vram(&mut self, source: &Renderer) {
        if self.vram_delta.is_some() { return; }
        let (Some(base), Some(now)) = (self.vram_baseline, super::diagnostics::vram::query(&source.adapter.get_info())) else { return; };
        self.vram_delta = Some(now.usage as i64 - base as i64);
    }
    fn add_encode(&mut self, stats: stream::EncodeStats, backpressure: Duration) {
        self.encode += stats.wall;
        self.encode_input_wait += stats.input_wait;
        self.encode_backpressure += backpressure;
        self.output_bytes += stats.bytes;
    }
    fn report(&self, options: PhotoOptions, output: [u32;2], total: Duration, outcome: &str) {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mb = |b: u64| b as f64 / f64::from(1u32 << 20);
        let jpeg = options.format == ScreenshotFormat::Jpeg;
        let streamed = self.plan.is_some_and(|p| p.output != OutputPlan::Direct);
        let codec = match (jpeg, streamed) {
            (true, true) => "jpg q95 4:4:4 jpeg-encoder", (true, false) => "jpg q95 4:2:0 libjpeg-turbo-rs",
            (false, true) => "png rgb8 stream", (false, false) => "png rgba8",
        };
        let plan = self.plan.map_or_else(|| "standard".to_owned(), |p| format!(
            "\"{}\" face={} render={} core={} guard={} tiles={}x{}x6 est_gpu_mb={:.0} est_ram_mb={:.0} est_disk_mb={:.0} budget={}",
            p.label(), p.face, p.render, p.core, p.guard, p.tiles_per_side, p.tiles_per_side,
            mb(p.gpu_bytes), mb(p.ram_bytes), mb(p.disk_bytes), if p.measured { "dxgi" } else { "default" }));
        eprintln!(
            "[PHOTO PERF] outcome={outcome} plan={plan} fallback={} output={}x{} projection={} codec={codec} msaa={} ssaa={} \
             world_renders={} discarded={} restarts={}{} init_ms={:.1} setup_ms={:.1} prep_wait_ms={:.1} sweep_ms={:.1} render_cpu_ms={:.1} \
             gpu_wait_wall_ms={:.1} readback_wait_wall_ms={:.1} store_write_ms={:.1} unpack_ms={:.1} projection_ms={:.1} encode_ms={:.1} \
             encode_input_wait_ms={:.1} encode_backpressure_ms={:.1} commit_ms={:.1} total_ms={:.1} strips={} store_written_mb={:.1} \
             store_read_mb={:.1} cache_hits={} cache_misses={} cache_evictions={} peak_readback_mb={:.1} output_mb={:.1} vram_delta_mb={}",
            self.fallback.as_deref().unwrap_or("-"), output[0], output[1], options.projection.name(), self.msaa, options.supersample,
            self.world_renders, self.discarded_renders, self.restarts,
            if self.restarts > 0 { format!("({})", self.restart_reason) } else { String::new() },
            ms(self.init), ms(self.setup), ms(self.preparation_wait), ms(self.sweep), ms(self.render_cpu),
            ms(self.gpu_wait), ms(self.readback_wait), ms(self.store_write), ms(self.unpack), ms(self.projection), ms(self.encode),
            ms(self.encode_input_wait), ms(self.encode_backpressure), ms(self.commit), ms(total), self.strips, mb(self.store_written),
            mb(self.store_read), self.cache.0, self.cache.1, self.cache.2, mb(self.peak_gpu_buffers), mb(self.output_bytes),
            self.vram_delta.map_or_else(|| "-".to_owned(), |d| format!("{:.0}", d as f64 / f64::from(1u32 << 20))));
    }
}

/// Planner inputs from the live device: real limits, the capture MSAA worst
/// case, and DXGI free dedicated VRAM when available (covers Vulkan and DX12;
/// integrated GPUs and other platforms use the documented default budget).
fn plan_inputs(primary: &Renderer, options: PhotoOptions, gameplay_msaa: u32) -> PlanInputs {
    let limits = primary.device.limits();
    let info = primary.adapter.get_info();
    let headroom = (info.device_type != wgpu::DeviceType::IntegratedGpu)
        .then(|| super::diagnostics::vram::query(&info)).flatten()
        .map(|v| v.budget.saturating_sub(v.usage));
    let msaa = if options.max_msaa {
        primary.supported_msaa.iter().chain(&primary.hdr_supported_msaa).copied().max().unwrap_or(1)
    } else { gameplay_msaa.max(1) };
    let env = |name: &str| std::env::var(name).ok();
    PlanInputs {
        max_texture: limits.max_texture_dimension_2d,
        max_buffer: limits.max_buffer_size,
        msaa,
        // Leave a quarter of the OS-reported headroom to the live renderer.
        gpu_budget: headroom.map_or(plan::DEFAULT_GPU_BUDGET, |h| h / 4 * 3),
        measured: headroom.is_some(),
        force: env("JKA_PHOTO_PLAN").as_deref().and_then(plan::ForcePlan::parse),
        tile_render: env("JKA_PHOTO_TILE").and_then(|v| v.parse().ok()),
    }
}

struct PhotoScratch(PathBuf);
impl Drop for PhotoScratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn commit_photo_outputs(outputs: Vec<ScreenshotOutput>, directory: &std::path::Path, cancel: &std::sync::atomic::AtomicBool) -> Result<Vec<ScreenshotOutput>, String> {
    check_cancel(cancel)?;
    let mut paths = Vec::new();
    for output in outputs {
        let ScreenshotOutput::Saved(path) = output else { return Err("Unexpected photo capture output".into()); };
        let name = path.file_name().ok_or("Missing photo file name")?;
        let dest = directory.join(name);
        if dest.exists() { return Err("Screenshot destination already exists".into()); }
        paths.push((path, dest));
    }
    let mut committed = Vec::new();
    for (from, to) in paths {
        if let Err(error) = std::fs::rename(from, &to) {
            for path in &committed { let _ = std::fs::remove_file(path); }
            return Err(format!("Could not publish screenshot: {error}"));
        }
        committed.push(to);
    }
    Ok(committed.into_iter().map(ScreenshotOutput::Saved).collect())
}

/// One isolated worker, using the same shared-device initialization as Studio.
/// It never mutates the primary camera, render targets, or UI composition.
pub(super) fn start(primary: &Renderer, request: PhotoCaptureRequest, proxy: EventLoopProxy<UserEvent>) -> Result<JoinHandle<()>, String> {
    let options = request.options;
    let output = options.output_size(request.display_size);
    let render_size = options.render_size(request.display_size)
        .ok_or("Screenshot render dimensions overflow; reduce supersampling")?;
    let limits = primary.device.limits();
    if options.format == ScreenshotFormat::Jpeg && output.into_iter().any(|v| v > PhotoOptions::JPEG_MAX_DIMENSION) {
        return Err("JPEG dimensions exceed libjpeg's 65,500-pixel limit; select PNG or reduce resolution".into());
    }
    // Panoramas: world render, cube retention and output are planned once
    // here from the device's limits and memory budget.
    let inputs = plan_inputs(primary, options, request.hud.video.msaa_samples);
    let plan = if options.panorama { Some(plan::plan(options, inputs)?) } else { None };
    let target_size = plan.map_or(render_size, |p| [p.render; 2]);
    if target_size.into_iter().any(|v| v == 0 || v > limits.max_texture_dimension_2d)
        || (plan.is_none() && output.into_iter().any(|v| v == 0 || v > limits.max_texture_dimension_2d)) {
        return Err("Screenshot exceeds GPU texture limits; reduce resolution or supersampling".into());
    }
    let bytes = u64::from((output[0]*4).div_ceil(256)*256)*u64::from(output[1]);
    if plan.is_none() && bytes > limits.max_buffer_size { return Err("Screenshot exceeds GPU readback buffer limit; reduce resolution".into()); }
    if let Some(plan) = plan { eprintln!("[PHOTO] plan: {}", plan.describe()); }
    check_cancel(&request.cancel)?;
    let init = Renderer::new_scene(primary, request.game.as_deref(), PhysicalSize::new(1,1));
    // Lazy future: allocates nothing unless an OOM retry needs a clean renderer.
    let retry_init = Renderer::new_scene(primary, request.game.as_deref(), PhysicalSize::new(1,1));
    let vram_baseline = super::diagnostics::vram::query(&primary.adapter.get_info()).map(|v| v.usage);
    let device = primary.device.clone();
    let time = primary.frame_time();
    let scene = CaptureSceneState::from_live(primary);
    std::thread::Builder::new().name("photo-capture".into()).stack_size(8*1024*1024).spawn(move || {
        let started = Instant::now();
        let mut perf = PhotoPerf { plan, vram_baseline, ..Default::default() };
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let cancel = Arc::clone(&request.cancel);
        let mut request = request;
        let directory = request.directory.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_cancel(&cancel)?;
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            let scratch_path = directory.join(format!(".photo-{}-{}", std::process::id(), request.metadata.captured_unix_ms));
            std::fs::create_dir(&scratch_path).map_err(|e| e.to_string())?;
            let scratch = PhotoScratch(scratch_path);
            request.directory = scratch.0.clone();
            let mut attempt_plan = plan;
            let mut renderers = [Some(init), Some(retry_init)].into_iter().flatten();
            let outputs = loop {
                if let Some(plan) = attempt_plan {
                    progress(&proxy, format!("PHOTO: preparing ({})", plan.describe()), 0, 0, Instant::now());
                }
                let init = renderers.next().ok_or("Screenshot renderer unavailable")?;
                let init_started = Instant::now();
                let mut source = block_on(init)?;
                perf.init += init_started.elapsed();
                check_cancel(&cancel)?;
                let target = attempt_plan.map_or(target_size, |p| [p.render; 2]);
                let mut watch = GpuWatch::new(&device);
                let result = capture(&mut source, &mut request, attempt_plan, target, time, &scene, &proxy, &mut perf, &mut watch)
                    .and_then(|outputs| watch.check().map(|()| outputs));
                drop(watch);
                // A failed attempt's renderer and resources are dropped before
                // any retry: the next attempt is a clean capture generation.
                drop(source);
                match result {
                    Err(error) if error.starts_with(OOM_PREFIX) => {
                        let next = attempt_plan.and_then(|p| p.oom_fallback(options, inputs));
                        let Some(next) = next else { break Err(error); };
                        eprintln!("[PHOTO] {} ran out of GPU memory; retrying with a fresh renderer: {}",
                            attempt_plan.map_or("capture", |p| p.label()), next.describe());
                        perf.fallback = Some(format!("\"{}\"@{}px(OOM)", next.label(), next.render));
                        perf.plan = Some(next);
                        attempt_plan = Some(next);
                    }
                    result => break result,
                }
            }?;
            check_cancel(&cancel)?;
            Ok((outputs, scratch))
        })).unwrap_or_else(|_| Err("Screenshot renderer panicked".into()));
        let result = match block_on(oom.pop()) { Some(error) => Err(format!("{OOM_PREFIX}{error}")), None => result };
        let commit_started = Instant::now();
        let result = result.and_then(|(outputs, _scratch)| commit_photo_outputs(outputs, &directory, &cancel));
        perf.commit = commit_started.elapsed();
        let outcome = match &result { Ok(_) => "ok", Err(e) if e == "Screenshot cancelled" => "cancelled", Err(_) => "error" };
        perf.report(options, output, started.elapsed(), outcome);
        let _ = proxy.send_event(UserEvent::PhotoCaptureFinished(result));
    }).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn capture(source: &mut Renderer, request: &mut PhotoCaptureRequest, plan: Option<CapturePlan>, render_size: [u32;2], time: f32, scene: &CaptureSceneState, proxy: &EventLoopProxy<UserEvent>, perf: &mut PhotoPerf, watch: &mut GpuWatch) -> Result<Vec<ScreenshotOutput>,String> {
    let setup_started = Instant::now();
    let options = request.options;
    let output = options.output_size(request.display_size);
    let cancel = Arc::clone(&request.cancel);
    check_cancel(&cancel)?;
    let mut video = request.hud.video;
    // No per-view temporal history, exposure adaptation or lens-space effects.
    // Standard capture also has no previous camera/history in this new scene.
    video.taa = false;
    video.dlss = false;
    video.rt_taa = 0;
    video.motion_blur_strength = 0.0;
    video.cloud_temporal = false;
    if options.panorama {
        video.ssao = false;
        video.contact_shadows = false;
        if video.reflection_quality.ssr_enabled() {
            video.reflection_quality = crate::ui::ReflectionQuality::Low;
        }
        video.depth_of_field_strength = 0.0;
        video.dof_mode = crate::ui::DofMode::Off;
        video.crt_scope = crate::ui::CrtScope::Off;
        video.chromatic_aberration = 0.0;
        video.vignette = false;
        video.film_grain_strength = 0.0;
        video.rain_glass = false;
        video.fxaa = false;
        video.smaa = false;
    }
    // Spatial supersampling resolves actual subpixels; avoid adding an extra
    // morphological edge blur on top of it. Gameplay preferences stay intact.
    if options.supersample > 1 {
        video.fxaa = false;
        video.smaa = false;
    }
    video.skip_ui = options.panorama || options.hide_ui;
    if options.max_msaa { video.msaa_samples = 1; }
    super::render_thread::apply_scene_video_settings(source, &video);
    scene.apply_controls(source, options);
    if options.max_msaa {
        // The scene format can require FP16 even with r_hdr off (e.g. fog,
        // bloom or volumetric clouds). Select from both capabilities only
        // AFTER applying the full capture video settings.
        let float_scene = source.gameplay_scene_format() == wgpu::TextureFormat::Rgba16Float;
        video.msaa_samples = source.supported_msaa.iter().copied()
            .filter(|n| !float_scene || source.hdr_supported_msaa.contains(n))
            .max().unwrap_or(1);
        source.set_msaa(video.msaa_samples);
    }
    source.apply_pending_scene_rebuild();
    request.hud.video = video;
    source.set_ui(request.hud.clone());
    // Size-dependent pipelines should be built for the final target, not 1x1.
    source.resize(PhysicalSize::new(render_size[0],render_size[1]));
    source.apply_pending_scene_rebuild();
    source.scene_time = Some(time);
    source.previous_camera_position = request.snapshot.camera.position;
    source.load_shared_studio_map((*request.map).clone(), None);
    scene.install_map_state(source);
    source.apply_pending_scene_rebuild();
    let snap = &request.snapshot;
    source.weather_server_time_ms = snap.weather_server_time_ms;
    source.update_inline_models(snap.inline_models.as_deref().map(Vec::as_slice));
    source.set_screen_fx(if video.skip_ui { &[] } else { snap.screen_fx.as_slice() });
    source.set_cloud_foreground(&snap.cloud_foreground);
    if !video.skip_ui { source.set_player_names(snap.player_names.clone()); }
    source.set_jump_shade(snap.jump_shade);
    source.set_ninja_fire_scene(snap.ninja_fire.clone(), snap.weather_server_time_ms);
    source.install_ninja_fire_capture_state(scene.fire);
    source.set_ninja_blood_scene(snap.ninja_blood.clone());
    source.ninja_fog.set_scene(snap.ninja_fog.as_ref());
    source.river_actors = Arc::clone(&snap.ocean_actors);
    source.river_ninja_entities = snap.ninja_entities.clone();
    source.dynamic_model_renderer.entity_water = Arc::clone(&snap.player_water);
    source.ocean_interaction.set_sources(&snap.ocean_actors);
    if let Some(ocean) = source.ocean.as_mut() { ocean.set_lake_actors(&snap.ocean_actors); }
    let models = snap.dynamic_models.iter().filter(|model| {
        if (options.panorama || options.hide_weapon) && model.vertices.first().is_some_and(|v| v.depth_hack > 0.0) { return false; }
        if options.hide_player && request.viewed_client == Some(model.entity_num) { return false; }
        true
    }).cloned().map(|mut model| { if options.panorama { model.raster_visible = true; } model }).collect::<Vec<_>>();
    let render = |source: &mut Renderer, camera: &crate::camera::Camera| -> Result<(),String> {
        check_cancel(&cancel)?;
        source.history_valid = false;
        source.camera_history_valid = false;
        source.cloud_history_valid = false;
        source.ssao_history_valid = false;
        source.ssr_history_valid = false;
        // A visibility query from a different cube face cannot suppress this
        // face's water or its refraction capture.
        source.ocean_visibility.primed = false;
        source.ocean_hidden_time = 0.0;
        source.render(camera, snap.player_position, snap.area_mask.as_ref(), &models, &[], snap.transient_lights.as_slice(), snap.studio_environment, None, snap.dof_focus_target, None, camera, 0.0, 0.0, ViewLatchMode::Disabled, None)
            .map(|_| ()).map_err(|error| format!("Screenshot scene render failed: {error:?}"))
    };
    let format = source.offscreen_output.as_ref().ok_or("Missing offscreen output")?.format();
    if !matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb) { return Err(format!("Unsupported screenshot color format: {format:?}")); }
    let mut camera = snap.camera;
    camera.set_studio_viewport(None);
    // Ninja fire is a time-stepped GPU simulation that this new renderer has
    // never run. One render creates its targets at the capture size; then
    // only the fire is stepped to a settled state before the frozen sweep.
    if source.ninja_fire.wants_frame() {
        render(source, &camera)?;
        perf.world_renders += 1;
        perf.discarded_renders += 1;
        watch.check()?;
        let warm = Instant::now();
        let steps = source.warm_ninja_fire_for_capture(&camera, FIRE_WARM_SECONDS, &cancel)?;
        watch.check()?;
        eprintln!("[PHOTO] Ninja fire warm-up: {steps} simulation ticks in {:.0} ms", warm.elapsed().as_secs_f64() * 1000.0);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut retries = 0u32;
    perf.msaa = video.msaa_samples;
    perf.setup = setup_started.elapsed();
    if !options.panorama {
        loop {
            poll_capture_preparation(source);
            let before = CapturePreparation::read(source);
            render(source, &camera)?;
            perf.world_renders += 1;
            wait_capture_submission(source)?;
            let after = CapturePreparation::read(source);
            if !before.pending && !after.pending && before == after && scene.ninja_pools_ready(source) {
                scene.verify_ready(source)?;
                break;
            }
            retries += 1;
            perf.restarts = retries;
            if retries == 1 { eprintln!("[PHOTO] preparing live scene resources before capture"); }
            wait_capture_preparation(source, deadline, &cancel)?;
        }
        let texture = source.offscreen_output.as_ref().unwrap().clone();
        let texture = if options.supersample > 1 {
            downsample(source, &texture, output, options.supersample)?
        } else { texture };
        return Ok(vec![save(source, &texture, &request, "standard")?]);
    }
    let plan = plan.ok_or("Missing panorama capture plan")?;
    camera.clear_first_person_bob();
    camera.shake = Default::default();
    // cg_fov is a legacy 4:3 horizontal angle, not a literal square FOV.
    camera.set_presentation_fov((4.0_f32/3.0).atan().to_degrees()*2.0);
    // WGPU cube order: +X, -X, +Y, -Y, +Z, -Z; local +Z is the
    // capture heading, +X is camera right, +Y is world up.
    let half_pi = std::f32::consts::FRAC_PI_2;
    let faces = [(-half_pi,0.0,"right"),(half_pi,0.0,"left"),(0.0,half_pi,"up"),(0.0,-half_pi,"down"),(0.0,0.0,"front"),(std::f32::consts::PI,0.0,"back")];
    if plan.world == WorldPlan::DiskTiles {
        return tiles::capture_disk(source, request, camera, faces, &plan, scene, &render, proxy, perf, watch).map(|p| vec![p]);
    }
    // GPU-resident cube (whole faces or tiles). OOM is detected by `watch`
    // and retried by the caller with a fresh renderer and a smaller plan.
    let cube = texture(&source.device, format, [plan.face; 2], 6);
    watch.check()?;
    tiles::sweep(source, request, camera, faces, &plan, scene, &render, tiles::Target::Cube(&cube), proxy, perf, watch)?;
    check_cancel(&cancel)?;
    let output_started = Instant::now();
    let mut results = Vec::new();
    if plan.output == OutputPlan::Strips {
        results.push(strips::encode_strips(source, &cube, &plan, &request, options.projection as u32, options.projection.name(), proxy, perf)?);
        return Ok(results);
    }
    progress(proxy, "PHOTO: projecting and encoding".into(), 0, 0, Instant::now());
    let projection = strips::Projection::new(&source.device, &cube, options.supersample, 1);
    if options.projection == PhotoProjection::Faces {
        // All layers are filled first: filtering along cube edges may read a neighbor.
        for (index, (_,_,name)) in faces.into_iter().enumerate() {
            let projected = strips::project_full(source, &projection, format, output, 7 + index as u32);
            results.push(save(source, &projected, &request, name)?);
        }
    } else {
        let projected = strips::project_full(source, &projection, format, output, options.projection as u32);
        results.push(save(source, &projected, &request, options.projection.name())?);
    }
    perf.projection += output_started.elapsed();
    Ok(results)
}

/// Wait only for the capture's already-submitted work, not for an idle shared
/// graphics queue (the primary renderer may continue to submit frames).
fn wait_capture_submission(source: &Renderer) -> Result<(), String> {
    let submission = source.queue.submit([]);
    source.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(std::time::Duration::from_secs(60)) })
        .map(|_| ()).map_err(|e| format!("Screenshot GPU wait failed: {e:?}"))
}

fn wait_capture_preparation(source: &mut Renderer, deadline: std::time::Instant, cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    loop {
        check_cancel(cancel)?;
        if std::time::Instant::now() >= deadline {
            return Err("Screenshot scene preparation did not settle; no image saved (see renderer console)".into());
        }
        poll_capture_preparation(source);
        let state = CapturePreparation::read(source);
        if state.jobs.3 != 0 {
            return Err("Screenshot pipeline compilation failed; no image saved (see renderer console)".into());
        }
        let baking = source.static_ao_pending.is_some() || source.world.as_ref().is_some_and(|w|
            w.sun_bake.capture_pending() || w.static_sun.capture_pending());
        // Ready PSOs are installed only by their owning frame preparation
        // code. Return to render when compilation and CPU bake work are idle.
        if state.jobs.4 == 0 && !baking { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn texture(device: &wgpu::Device, format: wgpu::TextureFormat, size: [u32;2], layers: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor { label: Some("Photo capture output"), size: wgpu::Extent3d {width:size[0],height:size[1],depth_or_array_layers:layers}, mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2, format, usage:wgpu::TextureUsages::COPY_SRC|wgpu::TextureUsages::COPY_DST|wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats:&[] })
}
/// Integer-ratio area resolve: integrate all rendered subpixels. For sRGB
/// targets textureLoad decodes and the render attachment encodes automatically.
fn downsample(source: &Renderer, input: &wgpu::Texture, size: [u32;2], factor: u32) -> Result<wgpu::Texture,String> {
    if input.width() != size[0] * factor || input.height() != size[1] * factor {
        return Err("Unexpected screenshot supersampling dimensions".into());
    }
    let device = &source.device;
    let output = texture(device, input.format(), size, 1);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Photo spatial AA resolve"),
        source: wgpu::ShaderSource::Wgsl(include_str!("photo_downsample.wgsl")
            .replace("FACTOR_VALUE", &format!("{factor}i")).into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Photo spatial AA resolve"), layout: None,
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"), targets: &[Some(wgpu::ColorTargetState { format: input.format(), blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let input_view = input.create_view(&Default::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Photo spatial AA source"), layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&input_view) }],
    });
    let view = output.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Photo spatial AA resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    source.queue.submit([encoder.finish()]);
    Ok(output)
}
fn save(source: &mut Renderer, texture: &wgpu::Texture, request: &PhotoCaptureRequest, name: &str) -> Result<ScreenshotOutput,String> {
    check_cancel(&request.cancel)?;
    let destination = ScreenshotDestination::File { directory:request.directory.clone(), metadata:request.metadata.clone(), format:request.options.format, suffix:Some(format!("_{}_{name}", request.metadata.captured_unix_ms)), spherical:request.options.panorama && request.options.projection == PhotoProjection::Equirectangular };
    let readback = source.prepare_readback_for(destination,texture.width(),texture.height(),texture.format()).ok_or_else(|| "Screenshot readback unavailable".to_owned())?;
    let mut readback = readback;
    source.submit_screenshot_copy(texture,&mut readback);
    if let Some(output) = source.queue_screenshot_encode(readback)? { return Ok(output); }
    source.screenshot_worker_rx.recv().map_err(|_| "Screenshot encoder stopped".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jpeg_limit_is_independent_of_capture_plan() {
        let mut options = PhotoOptions::default();
        options.panorama = true;
        options.panorama_width = 16384;
        options.format = ScreenshotFormat::Jpeg;
        assert!(options.estimated_plan().is_some_and(|p| p.is_ok_and(|p| p.output == OutputPlan::Strips)));
        assert!(options.output_size([1,1]).into_iter().all(|v| v <= PhotoOptions::JPEG_MAX_DIMENSION));
        options.panorama_width = PhotoOptions::MAX_JPEG_PANORAMA_WIDTH;
        assert!(options.output_size([1,1]).into_iter().all(|v| v <= PhotoOptions::JPEG_MAX_DIMENSION));
        options.panorama_width += 2;
        assert!(options.output_size([1,1]).into_iter().any(|v| v > PhotoOptions::JPEG_MAX_DIMENSION));
        options.panorama_width = 65536;
        assert!(options.output_size([1,1]).into_iter().any(|v| v > u16::MAX as u32));
    }
    #[test]
    fn old_photo_preferences_keep_values_and_enable_max_msaa() {
        let old = "1,cross,4096,8192,jpg,2,0,1,0";
        let options = PhotoOptions::parse(old).unwrap();
        assert!(options.max_msaa);
        assert_eq!(options.supersample, 2);
        assert_eq!(options.projection, PhotoProjection::Cross);
        assert_eq!(PhotoOptions::parse(&options.archive()), Some(options));
        let mut options = options;
        options.max_msaa = false;
        assert_eq!(PhotoOptions::parse(&options.archive()), Some(options));
    }
    #[test]
    fn spatial_aa_keeps_output_dimensions() {
        let mut options = PhotoOptions::default();
        options.supersample = 4;
        assert_eq!(options.output_size([1920,1080]), [1920,1080]);
        assert_eq!(options.render_size([1920,1080]), Some([7680,4320]));
        options.panorama = true;
        options.supersample = 2;
        assert_eq!(options.output_size([1920,1080]), [8192,4096]);
        assert_eq!(options.render_size([1920,1080]), Some([5216,5216]));
    }
}
