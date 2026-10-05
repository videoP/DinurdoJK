//! Thread.
use crate::renderer::{
    block_on, mpsc, sample_view_rotation, scene, thread, Arc, CompanionId, CompanionRenderThread,
    Duration, DynamicLightsMode, EventLoopProxy, FrameInfo, GpuPass, HashMap, InputLatencySample,
    Instant, JoinHandle, LatestViewState, Mutex, PathBuf, PostEffects, PreparedMap, Receiver,
    RenderBatchTiming, RenderCommand, RenderError, RenderSnapshot, RenderStats, Renderer,
    RendererBackend, Sender, TryRecvError, UiSnapshot, UserEvent, Window, WorldRenderPath,
    WorldUploadTimings, PBR_PROFILE_MATERIALS, PBR_PROFILE_PARALLAX_OCCLUSION,
};

pub struct RenderThread {
    pub(in crate::renderer) command_tx: Sender<RenderCommand>,
    /// Kept on the Winit/app side so auxiliary surfaces are created on the
    /// native window's origin thread before being handed to render workers.
    pub(in crate::renderer) instance: wgpu::Instance,
    pub(in crate::renderer) latest_snapshot: Arc<Mutex<RenderSnapshot>>,
    pub(in crate::renderer) latest_view: Arc<LatestViewState>,
    pub(in crate::renderer) join: Option<JoinHandle<()>>,
}

impl RenderThread {
    pub fn spawn(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        backend: RendererBackend,
        initial: RenderSnapshot,
        initial_ui: UiSnapshot,
        base: PathBuf,
        game: Option<PathBuf>,
        detail_texture_path: String,
    ) -> Result<Self, String> {
        let (command_tx, command_rx) = mpsc::channel();
        // Render snapshots are mailbox state, not a work queue: the renderer only
        // needs the newest camera/input state available when a frame begins.
        // A shared latest snapshot avoids retaining stale mouse samples when input
        // arrives faster than rendering (for example 1000 Hz input at 240-500 FPS).
        let latest_snapshot = Arc::new(Mutex::new(initial.clone()));
        let mut initial_view_camera = initial.camera;
        if let Some([yaw, pitch]) = initial.input_view_rotation {
            initial_view_camera.yaw = yaw;
            initial_view_camera.pitch = pitch;
        }
        let latest_view = Arc::new(LatestViewState::new(initial_view_camera));
        let render_snapshot = Arc::clone(&latest_snapshot);
        let render_latest_view = Arc::clone(&latest_view);
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = match backend {
            RendererBackend::Vulkan => wgpu::Backends::VULKAN,
            RendererBackend::Dx12 => wgpu::Backends::DX12,
        };
        if backend == RendererBackend::Dx12 {
            // Use the shipped dynamic DXC runtime explicitly. This avoids linking
            // static DXC (and its ATL build dependency) while also avoiding an
            // automatic fallback to legacy FXC when DXC is expected to be present.
            // Ship dxcompiler.dll (and its matching dxil.dll) beside DinurdoJK.exe.
            instance_desc.backend_options.dx12.shader_compiler =
                wgpu::Dx12Compiler::default_dynamic_dxc();
        }
        let instance = wgpu::Instance::new(instance_desc);
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| format!("Could not create WGPU surface: {error}"))?;
        // Winit's Win32 window handles are thread-affine. Keep one cheap WGPU
        // Instance handle here so future native surfaces can be created by App
        // on the Winit thread, then moved to their dedicated render workers.
        let surface_instance = instance.clone();
        let panic_proxy = proxy.clone();
        let join = thread::Builder::new()
            .name("jka-render".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_thread_main(
                        window,
                        instance,
                        surface,
                        proxy,
                        command_rx,
                        render_snapshot,
                        render_latest_view,
                        initial,
                        initial_ui,
                        base,
                        game,
                        detail_texture_path,
                    )
                }));
                if let Err(payload) = result {
                    let detail = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    let _ = panic_proxy.send_event(UserEvent::RendererError(format!(
                        "render thread panicked: {detail}"
                    )));
                }
            })
            .map_err(|e| format!("Could not start render thread: {e}"))?;
        Ok(Self {
            command_tx,
            instance: surface_instance,
            latest_snapshot,
            latest_view,
            join: Some(join),
        })
    }

    /// Create a companion surface on the Winit/window thread, then transfer
    /// the already-created WGPU surface to the renderer. This mirrors the
    /// primary window path and avoids Win32 raw-handle thread affinity.
    pub fn create_companion(&self, id: CompanionId, window: Arc<Window>) -> Result<(), String> {
        let size = window.inner_size();
        let surface = self
            .instance
            .create_surface(window.clone())
            .map_err(|error| format!("Could not create companion WGPU surface: {error}"))?;
        self.command_tx
            .send(RenderCommand::CreateCompanion {
                id,
                window,
                size,
                surface,
            })
            .map_err(|_| "Companion renderer is unavailable".to_owned())?;
        Ok(())
    }

    pub fn command(&self, command: RenderCommand) {
        let _ = self.command_tx.send(command);
    }

    pub fn publish(&self, snapshot: RenderSnapshot) {
        // Subframe input is permanent. Publish the tiny *input-view* tuple first.
        // In third person the snapshot camera is derived output and must never
        // overwrite newer raw player viewangles in the lock-free mailbox.
        if let Some([yaw, pitch]) = snapshot.input_view_rotation {
            self.latest_view.publish_rotation(
                yaw,
                pitch,
                snapshot.dynamic_crosshair_world,
                snapshot.input_latency,
            );
        }
        match self.latest_snapshot.lock() {
            Ok(mut latest) => *latest = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    pub fn publish_subframe_view_rotation(
        &self,
        yaw: f32,
        pitch: f32,
        crosshair_world: Option<[f32; 3]>,
        input: InputLatencySample,
    ) {
        self.latest_view
            .publish_rotation(yaw, pitch, crosshair_world, Some(input));
    }

    pub fn shutdown(&mut self) {
        let _ = self.command_tx.send(RenderCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Ask the render thread to exit without waiting for it. Swapchain and
    /// surface teardown can synchronously message the window's owning thread
    /// (DXGI and some Vulkan drivers do), so the UI thread must keep pumping
    /// its event loop while the renderer drops instead of blocking in `join`.
    pub fn request_shutdown(&self) {
        let _ = self.command_tx.send(RenderCommand::Shutdown);
    }

    /// True once the render thread has returned (or was already detached).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Detach a render thread that never exited so dropping this handle cannot
    /// block on it. The thread keeps its window/device alive until it dies.
    pub fn abandon(&mut self) {
        self.join.take();
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn render_thread_main(
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    proxy: EventLoopProxy<UserEvent>,
    command_rx: Receiver<RenderCommand>,
    latest_snapshot: Arc<Mutex<RenderSnapshot>>,
    latest_view: Arc<LatestViewState>,
    mut snapshot: RenderSnapshot,
    initial_ui: UiSnapshot,
    base: PathBuf,
    game: Option<PathBuf>,
    detail_texture_path: String,
) {
    let renderer_init_started = Instant::now();
    let mut renderer = match block_on(Renderer::new(
        window,
        instance,
        surface,
        &base,
        game.as_deref(),
        initial_ui.video.max_frame_latency,
        &detail_texture_path,
    )) {
        Ok(renderer) => renderer,
        Err(error) => {
            let _ = proxy.send_event(UserEvent::RendererError(error));
            return;
        }
    };
    rverbose!(
        1,
        "[RENDER INIT] base device/resources/pipelines {:.1} ms",
        renderer_init_started.elapsed().as_secs_f64() * 1000.0
    );
    renderer.set_vsync(initial_ui.video.vsync);
    renderer
        .dynamic_model_renderer
        .set_fx_zero_alpha_discard(initial_ui.video.fx_zero_alpha_discard);
    renderer.set_msaa(initial_ui.video.msaa_samples);
    renderer.set_texture_filter(initial_ui.video.texture_filter);
    renderer.set_picmip(initial_ui.video.picmip);
    renderer.set_detail_textures(initial_ui.video.detail_textures);
    renderer.set_detail_texture_fade(
        initial_ui.video.detail_texture_fade,
        initial_ui.video.detail_texture_fade_distance,
    );
    renderer.set_wireframe_mask(initial_ui.video.wireframe_mask);
    renderer.debug_volumes.triggers_enabled = initial_ui.video.draw_triggers;
    renderer.debug_volumes.clips_enabled = initial_ui.video.draw_clip_brushes;
    renderer.set_pvs_mode(initial_ui.video.pvs_mode);
    let mut fps_cap = initial_ui.video.fps_cap;
    let mut input_latelatch = initial_ui.video.input_latelatch;
    renderer.set_gamma_method(
        if initial_ui.video.gamma_method == crate::gamma::GammaMethod::Hardware {
            crate::gamma::GammaMethod::Shader
        } else {
            initial_ui.video.gamma_method
        },
    );
    renderer.set_gamma(initial_ui.video.gamma);
    renderer.set_model_brightness(initial_ui.video.effective_model_brightness());
    renderer.set_dynamic_light_brightness(initial_ui.video.effective_dynamic_light_brightness());
    renderer
        .surface_deformation
        .set_mode(&renderer.queue, initial_ui.video.footprints);
    renderer.set_post_effects(PostEffects {
        hdr: initial_ui.video.hdr,
        auto_exposure: initial_ui.video.auto_exposure,
        tone_mapping: initial_ui.video.tone_mapping,
        bloom: initial_ui.video.bloom,
        halation: initial_ui.video.halation,
        ssao: initial_ui.video.ssao,
        static_bsp_ao: initial_ui.video.static_bsp_ao,
        static_bsp_ao_lightmap: initial_ui.video.static_bsp_ao_lightmap,
        static_bsp_ao_samples: initial_ui.video.static_bsp_ao_samples,
        static_bsp_ao_resolution: initial_ui.video.static_bsp_ao_resolution,
        static_bsp_ao_strength: initial_ui.video.static_bsp_ao_strength,
        static_bsp_ao_range: initial_ui.video.static_bsp_ao_range,
        static_bsp_ao_current_cell: initial_ui.video.static_bsp_ao_current_cell,
        fxaa: initial_ui.video.fxaa,
        smaa: initial_ui.video.smaa,
        taa: initial_ui.video.taa,
        contact_shadows: initial_ui.video.contact_shadows,
        fog_mode: initial_ui.video.fog_mode,
        fog_strength: initial_ui.video.fog_strength,
        sun_override: initial_ui.video.sun_override,
        sun_yaw: initial_ui.video.sun_yaw,
        sun_pitch: initial_ui.video.sun_pitch,
        sun_intensity: initial_ui.video.sun_intensity,
        sun_color: initial_ui.video.sun_color,
        sun_visibility: initial_ui.video.sun_visibility,
        entity_sun_lighting: initial_ui.video.entity_sun_lighting,
        clouds: initial_ui.video.clouds,
        cloud_type: initial_ui.video.cloud_type,
        cloud_quality: initial_ui.video.cloud_quality,
        cloud_coverage: initial_ui.video.cloud_coverage,
        cloud_height: initial_ui.video.cloud_height,
        cloud_thickness: initial_ui.video.cloud_thickness,
        weather_wind: initial_ui.video.weather_wind,
        cloud_shadows: initial_ui.video.cloud_shadows,
        cloud_render_resolution: initial_ui.video.cloud_render_resolution,
        cloud_temporal: initial_ui.video.cloud_temporal,
        cloud_temporal_depth_fix: initial_ui.video.cloud_temporal_depth_fix,
        cloud_shear: initial_ui.video.cloud_shear,
        cloud_base_variation: initial_ui.video.cloud_base_variation,
        cloud_shape_evolution: initial_ui.video.cloud_shape_evolution,
        cloud_terrain_interaction: initial_ui.video.cloud_terrain_interaction,
        cloud_empty_skip: initial_ui.video.cloud_empty_skip,
        cloud_aerial: initial_ui.video.cloud_aerial,
        cloud_sky_ambient: initial_ui.video.cloud_sky_ambient,
        cloud_history_blend: initial_ui.video.cloud_history_blend,
        cloud_motion_reject: initial_ui.video.cloud_motion_reject,
        cloud_history_depth_reject: initial_ui.video.cloud_history_depth_reject,
        cloud_thickness_variation: initial_ui.video.cloud_thickness_variation,
        cloud_size: initial_ui.video.cloud_size,
        rain: initial_ui.video.rain,
        rain_intensity: initial_ui.video.rain_intensity,
        puddle_quality: initial_ui.video.puddle_quality,
        puddle_scatter: initial_ui.video.puddle_scatter,
        rain_grade: initial_ui.video.rain_grade,
        reflection_quality: initial_ui.video.reflection_quality,
        reflection_debug: initial_ui.video.reflection_debug,
        chromatic_aberration: initial_ui.video.chromatic_aberration,
        vignette: initial_ui.video.vignette,
        film_grain_strength: initial_ui.video.film_grain_strength,
        motion_blur_strength: initial_ui.video.motion_blur_strength,
        depth_of_field_strength: initial_ui.video.depth_of_field_strength,
        dof_quality: initial_ui.video.dof_quality,
        color_lut: initial_ui.video.color_lut,
        color_lut_strength: initial_ui.video.color_lut_strength,
        split_toning: initial_ui.video.split_toning,
    });
    renderer.set_grass_enabled(initial_ui.video.grass);
    renderer.set_grass_precompute(initial_ui.video.grass_precompute);
    renderer.set_grass_mid_lod(initial_ui.video.grass_mid_lod);
    renderer.set_grass_front_to_back(initial_ui.video.grass_front_to_back);
    let mut initial_ocean_settings = initial_ui.video.ocean_settings;
    initial_ocean_settings.wind = initial_ui.video.weather_wind;
    renderer.set_ocean_settings(initial_ocean_settings);
    renderer.set_ocean_enabled(initial_ui.video.ocean);
    renderer.set_gpu_timings(initial_ui.video.gpu_timings);
    renderer.set_ghoul2_batch_draws(initial_ui.video.ghoul2_batch_draws);
    renderer.set_gpu_visibility(initial_ui.video.gpu_driven, initial_ui.video.hiz_occlusion);
    renderer.set_dynamic_lighting(initial_ui.video.dynamic_lights);
    renderer.set_rt_samples(initial_ui.video.rt_samples);
    renderer.set_dynamic_light_falloff(initial_ui.video.dynamic_light_falloff);
    renderer.set_rt_half_resolution(initial_ui.video.rt_half_resolution);
    renderer.set_map_light_simulation(initial_ui.video.map_light_simulation);
    renderer.set_emissive_area_lights(initial_ui.video.emissive_area_lights);
    renderer.set_entity_ambient_lighting(initial_ui.video.entity_ambient_lighting);
    renderer.set_voxel_probe_gi(initial_ui.video.voxel_probe_gi);
    renderer.set_local_light_shadows(initial_ui.video.local_light_shadows);
    renderer.set_entity_shadow_light(initial_ui.video.entity_shadow_light);
    renderer.set_pbr_settings(
        initial_ui.video.pbr,
        initial_ui.video.deluxe_mapping,
        initial_ui.video.deluxe_specular,
    );
    renderer.set_cascaded_shadows(initial_ui.video.dynamic_shadows);
    renderer.set_cull_debug(initial_ui.video.cull_debug);
    renderer.set_force_unified_world(initial_ui.video.force_unified_world);
    renderer.set_pom_enabled(initial_ui.video.pom);
    renderer.set_planar_reflection_debug(initial_ui.video.planar_reflection_debug);
    let mut perf_trace = initial_ui.video.perf_trace;
    renderer.set_ui(initial_ui);
    renderer.set_dynamic_hud(snapshot.hud, snapshot.movement_hud);
    renderer.set_player_names(snapshot.player_names.clone());
    renderer.set_jump_shade(snapshot.jump_shade);
    renderer.set_screen_fx(&snapshot.screen_fx);
    renderer.set_cloud_foreground(&snapshot.cloud_foreground);
    rverbose!(
        1,
        "[RENDER INIT] ready after applying video state {:.1} ms total",
        renderer_init_started.elapsed().as_secs_f64() * 1000.0
    );
    let _ = proxy.send_event(UserEvent::RendererReady {
        supported_msaa: renderer.supported_msaa.clone(),
        wireframe_supported: renderer.wireframe_supported,
    });
    let mut companion_renderers: HashMap<CompanionId, CompanionRenderThread> = HashMap::new();

    let mut stats_started = Instant::now();
    // Start of the current cap interval. Pacing from a fixed cadence rather than
    // from the top of each frame keeps the loop's own overhead (command drain,
    // snapshot swap, scene rebuild) out of the interval, so a cap of N yields N.
    let mut frame_pace_anchor = Instant::now();
    let mut stats_frames = 0_u64;
    let mut last_info = FrameInfo::default();
    let mut last_measured_input_sequence = 0_u64;
    let mut input_latency_samples = 0_u64;
    let mut input_latch_samples = 0_u64;
    let mut input_event_to_sim_sum_ms = 0.0_f64;
    let mut input_sim_to_render_sum_ms = 0.0_f64;
    let mut input_event_to_latch_sum_ms = 0.0_f64;
    let mut input_latch_to_submit_sum_ms = 0.0_f64;
    let mut input_latch_to_present_call_sum_ms = 0.0_f64;
    let mut input_event_to_present_call_sum_ms = 0.0_f64;
    let mut input_event_to_present_call_max_ms = 0.0_f64;
    let mut pending_world_uploaded: Option<(
        u64,
        Instant,
        String,
        usize,
        usize,
        f64,
        WorldUploadTimings,
        scene::MapLoadTimings,
        Option<scene::BspMapStats>,
    )> = None;

    // The render thread can run several times faster than camera/input updates.
    // Track the previous *camera snapshot* rather than the previous rendered frame
    // so camera motion remains visible instead of collapsing to zero at 2000+ FPS.
    let mut motion_reference_camera = snapshot.camera;
    let mut shake_rng = crate::camera::ShakeRng::default();
    let mut motion_camera_sample_dt = 1.0 / 120.0;
    let mut motion_camera_changed_at = Instant::now();

    // Opt-in console diagnostics. Keep the normal maximum-FPS path free of
    // per-frame formatting/printing, and only aggregate when explicitly asked.
    let mut trace_started = Instant::now();
    let mut trace_frames = 0_u64;
    let mut trace_frame_ms = 0.0_f64;
    let mut trace_prepare_ms = 0.0_f64;
    let mut trace_acquire_ms = 0.0_f64;
    let mut trace_encode_ms = 0.0_f64;
    let mut trace_submit_ms = 0.0_f64;
    let mut trace_present_ms = 0.0_f64;
    let mut trace_dynamic_model_ms = 0.0_f64;
    let mut trace_grass_cpu_ms = 0.0_f64;
    // Advanced-path prepare breakdown: camera, shadow, world, reflection.
    let mut trace_prep_stage_ms = [0.0_f64; 4];
    if perf_trace {
        println!("JKA perf trace enabled (enable GPU TIMESTAMPS too for per-pass GPU timings)");
    }

    let mut gamma_preparation = None;
    let mut first_frame_notified = false;
    let mut batch_timing: Option<RenderBatchTiming> = None;

    let mut deferred_load_map = None;

    loop {
        {
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Render,
                crate::thread_activity::Task::RenderCommands,
            );
            if let Some((request_id, started, name, map)) = deferred_load_map.take() {
                let map: Box<PreparedMap> = map;
                let triangles = map.triangles;
                let batches = map.batches.len();
                let timings = map.load_timings;
                let bsp_stats = map.bsp_stats.clone();
                let upload_started = Instant::now();
                let upload_timings = renderer.load_map(*map);
                let upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
                pending_world_uploaded = Some((
                    request_id,
                    started,
                    name,
                    triangles,
                    batches,
                    upload_ms,
                    upload_timings,
                    timings,
                    bsp_stats,
                ));
            }
            loop {
                let received = command_rx.try_recv();
                let command_started = Instant::now();
                match received {
                    Ok(RenderCommand::BatchBegin(title)) => {
                        renderer.begin_settings_batch();
                        batch_timing = Some(RenderBatchTiming::new(title));
                    }
                    Ok(RenderCommand::BatchLabel(label)) => {
                        if let Some(batch) = batch_timing.as_mut() {
                            batch.current = label;
                        }
                    }
                    Ok(RenderCommand::BatchEnd) => {
                        let finish_started = Instant::now();
                        renderer.end_settings_batch();
                        if let Some(mut batch) = batch_timing.take() {
                            batch.current = "(deferred world pipeline variant)".to_owned();
                            batch.record(finish_started.elapsed());
                            batch.print();
                        }
                    }
                    Ok(RenderCommand::Resize(size)) => renderer.resize(size),
                    Ok(RenderCommand::SetVsync(vsync)) => renderer.set_vsync(vsync),
                    Ok(RenderCommand::SetMaxFrameLatency(latency)) => {
                        renderer.set_max_frame_latency(latency)
                    }
                    Ok(RenderCommand::SetInputLateLatch(enabled)) => {
                        input_latelatch = enabled;
                    }
                    Ok(RenderCommand::SetMsaa(samples)) => renderer.set_msaa(samples),
                    Ok(RenderCommand::SetTextureFilter(filter)) => {
                        renderer.set_texture_filter(filter)
                    }
                    Ok(RenderCommand::SetPicmip(picmip)) => renderer.set_picmip(picmip),
                    Ok(RenderCommand::SetDetailTextures(mode)) => {
                        renderer.set_detail_textures(mode)
                    }
                    Ok(RenderCommand::SetDetailTexture { path, game }) => {
                        renderer.set_detail_texture(&path, game.as_deref())
                    }
                    Ok(RenderCommand::SetDetailTextureFade { enabled, distance }) => {
                        renderer.set_detail_texture_fade(enabled, distance)
                    }
                    Ok(RenderCommand::SetWireframeMask(mask)) => renderer.set_wireframe_mask(mask),
                    Ok(RenderCommand::SetDebugVolumes { triggers, clips }) => {
                        renderer.debug_volumes.triggers_enabled = triggers;
                        renderer.debug_volumes.clips_enabled = clips;
                    }
                    Ok(RenderCommand::SetEntityMarkers(mesh)) => {
                        renderer.set_entity_markers(mesh);
                    }
                    Ok(RenderCommand::SetSunRayPreview(enabled)) => {
                        renderer.debug_volumes.sun_ray_enabled = enabled;
                    }
                    Ok(RenderCommand::SetPvsMode(mode)) => renderer.set_pvs_mode(mode),
                    Ok(RenderCommand::SetFpsCap(cap)) => fps_cap = cap,
                    Ok(RenderCommand::SetGamma(gamma)) => renderer.set_gamma(gamma),
                    Ok(RenderCommand::SetGammaMethod(method)) => {
                        gamma_preparation = None;
                        renderer.set_gamma_method(method);
                    }
                    Ok(RenderCommand::PrepareGammaMethod(generation)) => {
                        renderer.set_gamma_method(crate::gamma::GammaMethod::Hardware);
                        gamma_preparation = Some(generation);
                    }
                    Ok(RenderCommand::SetModelBrightness(scale)) => {
                        renderer.set_model_brightness(scale)
                    }
                    Ok(RenderCommand::SetDynamicLightBrightness(scale)) => {
                        renderer.set_dynamic_light_brightness(scale)
                    }
                    Ok(RenderCommand::SetPostEffects(settings)) => {
                        renderer.set_post_effects(settings)
                    }
                    Ok(RenderCommand::SetFootprintMode(mode)) => {
                        renderer.surface_deformation.set_mode(&renderer.queue, mode)
                    }
                    Ok(RenderCommand::SetGrassEnabled(enabled)) => {
                        renderer.set_grass_enabled(enabled)
                    }
                    Ok(RenderCommand::SetGrassPrecompute(enabled)) => {
                        renderer.set_grass_precompute(enabled)
                    }
                    Ok(RenderCommand::SetGrassMidLod(enabled)) => {
                        renderer.set_grass_mid_lod(enabled)
                    }
                    Ok(RenderCommand::SetGrassFrontToBack(enabled)) => {
                        renderer.set_grass_front_to_back(enabled)
                    }
                    Ok(RenderCommand::SetContactShadowDebug(enabled)) => {
                        renderer.contact_shadow_debug = enabled
                    }
                    Ok(RenderCommand::SetOceanEnabled(enabled)) => {
                        renderer.set_ocean_enabled(enabled)
                    }
                    Ok(RenderCommand::SetOceanSettings(settings)) => {
                        renderer.set_ocean_settings(settings)
                    }
                    Ok(RenderCommand::SetAuthoredOceans(oceans)) => {
                        renderer.set_authored_oceans(oceans)
                    }
                    Ok(RenderCommand::SetOceanTime(time)) => {
                        if let Some(ocean) = &mut renderer.ocean {
                            ocean.set_time(time);
                        }
                        for (_, ocean) in &mut renderer.authored_oceans {
                            ocean.set_time(time);
                        }
                    }
                    Ok(RenderCommand::SetPerfTrace(enabled)) => {
                        perf_trace = enabled;
                        trace_started = Instant::now();
                        trace_frames = 0;
                        trace_frame_ms = 0.0;
                        trace_prepare_ms = 0.0;
                        trace_acquire_ms = 0.0;
                        trace_encode_ms = 0.0;
                        trace_submit_ms = 0.0;
                        trace_present_ms = 0.0;
                        trace_grass_cpu_ms = 0.0;
                        trace_prep_stage_ms = [0.0; 4];
                        println!(
                            "JKA perf trace {}",
                            if enabled { "enabled" } else { "disabled" }
                        );
                    }
                    Ok(RenderCommand::SetGpuTimings(enabled)) => renderer.set_gpu_timings(enabled),
                    Ok(RenderCommand::SetFxZeroAlphaDiscard(enabled)) => {
                        renderer
                            .dynamic_model_renderer
                            .set_fx_zero_alpha_discard(enabled);
                    }
                    Ok(RenderCommand::SetGhoul2BatchDraws(mode)) => {
                        renderer.set_ghoul2_batch_draws(mode)
                    }
                    Ok(RenderCommand::SetGpuVisibility {
                        gpu_driven,
                        hiz_occlusion,
                    }) => {
                        renderer.set_gpu_visibility(gpu_driven, hiz_occlusion);
                    }
                    Ok(RenderCommand::SetDynamicLighting(mode)) => {
                        renderer.set_dynamic_lighting(mode);
                    }
                    Ok(RenderCommand::SetRtSamples(samples)) => renderer.set_rt_samples(samples),
                    Ok(RenderCommand::SetDynamicLightFalloff(mode)) => {
                        renderer.set_dynamic_light_falloff(mode)
                    }
                    Ok(RenderCommand::SetRtHalfResolution(enabled)) => {
                        renderer.set_rt_half_resolution(enabled)
                    }
                    Ok(RenderCommand::SetMapLightSimulation(enabled)) => {
                        renderer.set_map_light_simulation(enabled);
                    }
                    Ok(RenderCommand::SetClassicWorldLighting {
                        fullbright,
                        vertex_light,
                        lightmap_only,
                    }) => {
                        renderer.set_classic_world_lighting(
                            fullbright,
                            vertex_light,
                            lightmap_only,
                        );
                    }
                    Ok(RenderCommand::SetEmissiveAreaLights(enabled)) => {
                        renderer.set_emissive_area_lights(enabled);
                    }
                    Ok(RenderCommand::SetEntityAmbientLighting(mode)) => {
                        renderer.set_entity_ambient_lighting(mode);
                    }
                    Ok(RenderCommand::SetVoxelProbeGi(enabled)) => {
                        renderer.set_voxel_probe_gi(enabled);
                    }
                    Ok(RenderCommand::SetLocalLightShadows(enabled)) => {
                        renderer.set_local_light_shadows(enabled);
                    }
                    Ok(RenderCommand::SetEntityShadowLight(source)) => {
                        renderer.set_entity_shadow_light(source);
                    }
                    Ok(RenderCommand::SetPbrSettings {
                        enabled,
                        deluxe_mapping,
                        deluxe_specular,
                    }) => {
                        renderer.set_pbr_settings(enabled, deluxe_mapping, deluxe_specular);
                    }
                    Ok(RenderCommand::SetCascadedShadows(mode)) => {
                        renderer.set_cascaded_shadows(mode);
                    }
                    Ok(RenderCommand::SetCullDebug(mode)) => {
                        renderer.set_cull_debug(mode);
                    }
                    Ok(RenderCommand::SetForceUnifiedWorld(force)) => {
                        renderer.set_force_unified_world(force);
                    }
                    Ok(RenderCommand::SetPom(enabled)) => {
                        renderer.set_pom_enabled(enabled);
                    }
                    Ok(RenderCommand::SetPlanarReflectionDebug(mode)) => {
                        renderer.set_planar_reflection_debug(mode);
                    }
                    Ok(RenderCommand::SetPuddleDebug(enabled)) => {
                        renderer.set_puddle_debug(enabled);
                    }
                    Ok(RenderCommand::DumpMaterials { filter, all }) => {
                        renderer.dump_materials(filter.as_deref(), all);
                    }
                    Ok(RenderCommand::InspectSurface {
                        x,
                        y,
                        width,
                        height,
                        entity_hint,
                    }) => {
                        let info = renderer.inspect_surface(
                            &snapshot.camera,
                            snapshot.dynamic_models.as_slice(),
                            x,
                            y,
                            width,
                            height,
                            entity_hint,
                        );
                        let _ = proxy.send_event(UserEvent::SurfaceInspected(info));
                    }
                    Ok(RenderCommand::ClearSurfaceInspection) => {
                        renderer.inspector_vertex_range = None;
                        renderer.inspector_entity_num = None;
                    }
                    Ok(RenderCommand::AddSurfaceDeformation(stamp)) => {
                        renderer.surface_deformation.push(&renderer.queue, stamp);
                    }
                    Ok(RenderCommand::Screenshot {
                        directory,
                        mut metadata,
                    }) => {
                        // Use the renderer-owned snapshot as authority for the camera fields
                        // instead of the older main-thread copy bundled with the command.
                        metadata.camera_origin = Some(crate::scene::jka_position(
                            snapshot.camera.position.to_array(),
                        ));
                        metadata.view_angles = Some([
                            -snapshot.camera.pitch.to_degrees(),
                            snapshot.camera.yaw.to_degrees().rem_euclid(360.0),
                            0.0,
                        ]);
                        metadata.fov = snapshot.camera.cg_fov();
                        // Capture what the center ray is actually rendering on the same
                        // renderer snapshot as the screenshot. Preserve `/trace`'s
                        // selection/highlight state: screenshot metadata is observational.
                        let old_range = renderer.inspector_vertex_range.clone();
                        let old_entity = renderer.inspector_entity_num;
                        renderer.inspector_vertex_range = None;
                        renderer.inspector_entity_num = None;
                        let width = renderer.config.width.max(1);
                        let height = renderer.config.height.max(1);
                        let hit = renderer.inspect_surface(
                            &snapshot.camera,
                            snapshot.dynamic_models.as_slice(),
                            width as f32 * 0.5,
                            height as f32 * 0.5,
                            width,
                            height,
                            None,
                        );
                        renderer.inspector_vertex_range = old_range;
                        renderer.inspector_entity_num = old_entity;
                        metadata.crosshair = hit.map(|info| {
                            let material = info
                                .summary
                                .iter()
                                .find(|(label, _)| label.eq_ignore_ascii_case("MATERIAL"))
                                .map(|(_, value)| value.clone());
                            let distance = info
                                .summary
                                .iter()
                                .find(|(label, _)| label.eq_ignore_ascii_case("DISTANCE"))
                                .map(|(_, value)| value.clone());
                            crate::screenshot::ScreenshotCrosshair {
                                kind: info.kind,
                                title: info.title,
                                material,
                                distance,
                                entity_num: info.hit_entity_num,
                            }
                        });
                        renderer.request_screenshot(directory, metadata)
                    }
                    Ok(RenderCommand::CopyFrameToClipboard) => renderer.request_clipboard_capture(),
                    Ok(RenderCommand::SetUi(ui)) => renderer.set_ui(ui),
                    Ok(RenderCommand::SetTransientUi {
                        chat_lines,
                        center_print,
                        demo_timeline,
                        prediction_debug,
                        crosshair_target,
                        force_select,
                        follow_name,
                        game_timer,
                        mini_scores,
                        race_timer,
                        vote_line,
                        scoreboard,
                        scoreboard_focus_client,
                        speedometer,
                        lagometer,
                        team_overlay,
                    }) => {
                        renderer.set_transient_ui(
                            chat_lines,
                            center_print,
                            demo_timeline,
                            prediction_debug,
                            crosshair_target,
                            force_select,
                            follow_name,
                            game_timer,
                            mini_scores,
                            race_timer,
                            vote_line,
                            scoreboard,
                            scoreboard_focus_client,
                            speedometer,
                            lagometer,
                            team_overlay,
                        );
                    }
                    Ok(RenderCommand::SetUiTelemetry { perf, threads }) => {
                        renderer.set_ui_telemetry(perf, threads);
                    }
                    Ok(RenderCommand::SetEgui(frame)) => renderer.set_egui(frame),
                    Ok(RenderCommand::CreateCompanion {
                        id,
                        window,
                        size,
                        surface,
                    }) => {
                        if let Some(mut old) = companion_renderers.remove(&id) {
                            old.shutdown();
                        }
                        renderer.remove_companion_scene_target(id);
                        match CompanionRenderThread::spawn(
                            id,
                            window,
                            size,
                            surface,
                            renderer.adapter.clone(),
                            renderer.device.clone(),
                            renderer.queue.clone(),
                            proxy.clone(),
                        ) {
                            Ok(worker) => {
                                companion_renderers.insert(id, worker);
                            }
                            Err(error) => {
                                let _ =
                                    proxy.send_event(UserEvent::CompanionError { id: id.0, error });
                            }
                        }
                    }
                    Ok(RenderCommand::DestroyCompanion { id }) => {
                        renderer.remove_companion_scene_target(id);
                        if let Some(mut worker) = companion_renderers.remove(&id) {
                            worker.shutdown();
                        }
                    }
                    Ok(RenderCommand::ResizeCompanion { id, size }) => {
                        if let Some(worker) = companion_renderers.get(&id) {
                            worker.resize(size);
                        }
                    }
                    Ok(RenderCommand::SetCompanionEgui { id, frame }) => {
                        if let Some(worker) = companion_renderers.get(&id) {
                            worker.publish(frame);
                        }
                    }
                    Ok(RenderCommand::SetAssetPreviewMode(enabled)) => {
                        renderer.set_asset_preview_mode(enabled);
                    }
                    Ok(RenderCommand::SetAssetPreviewViewport(viewport)) => {
                        renderer.asset_preview_viewport = viewport;
                    }
                    Ok(RenderCommand::UnloadMap) => renderer.unload_map(),
                    Ok(RenderCommand::LoadMap {
                        request_id,
                        started,
                        name,
                        map,
                    }) => {
                        // The upload blocks this thread for seconds, and the loading
                        // screen is drawn by this same thread. Hold the map back,
                        // stop draining so command order is kept, and let one frame
                        // present the latest loading state ("uploading") first; the
                        // upload itself runs at the top of the next iteration.
                        deferred_load_map = Some((request_id, started, name, map));
                        break;
                    }
                    Ok(RenderCommand::Shutdown) => {
                        let teardown_started = Instant::now();
                        rverbose!(1, "[RENDER SHUTDOWN] command received; dropping renderer");
                        // Swapchain/device teardown can wedge or fault inside the
                        // GPU driver. If it is still running after a few seconds,
                        // log it and dump every thread's stack while it is stuck.
                        let (teardown_done, teardown_wait) = mpsc::channel::<()>();
                        let _ = thread::Builder::new()
                            .name("render-teardown-watchdog".into())
                            .spawn(move || {
                                if teardown_wait.recv_timeout(Duration::from_secs(4))
                                    == Err(mpsc::RecvTimeoutError::Timeout)
                                {
                                    eprintln!(
                                        "[RENDER SHUTDOWN] renderer drop still running after 4 s (GPU driver hang?)"
                                    );
                                    crate::crash::write_hang_dump("hang-render-shutdown");
                                }
                            });
                        for (_, mut companion) in companion_renderers.drain() {
                            companion.shutdown();
                        }
                        drop(renderer);
                        let _ = teardown_done.send(());
                        rverbose!(
                            1,
                            "[RENDER SHUTDOWN] renderer dropped {:.1} ms",
                            teardown_started.elapsed().as_secs_f64() * 1000.0
                        );
                        return;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        rverbose!(
                            1,
                            "[RENDER SHUTDOWN] command channel closed; render thread exiting"
                        );
                        for (_, mut companion) in companion_renderers.drain() {
                            companion.shutdown();
                        }
                        return;
                    }
                }
                if let Some(batch) = batch_timing.as_mut() {
                    batch.record(command_started.elapsed());
                }
            }
            if let Some((completed, total)) = renderer.poll_static_ao_progress() {
                let _ = proxy.send_event(UserEvent::StaticAoProgress { completed, total });
            }
            renderer.poll_static_ao_updates();
            renderer.poll_static_ao_worker();
            let next = match latest_snapshot.lock() {
                Ok(latest) => latest.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            let position_delta = next.camera.position - snapshot.camera.position;
            let yaw_delta = (next.camera.yaw - snapshot.camera.yaw).abs();
            let pitch_delta = (next.camera.pitch - snapshot.camera.pitch).abs();
            let camera_changed = position_delta.length_squared() > 1.0e-10
                || yaw_delta > 1.0e-7
                || pitch_delta > 1.0e-7;
            if camera_changed {
                let now = Instant::now();
                let elapsed = now.duration_since(motion_camera_changed_at).as_secs_f32();
                // Large position discontinuities are camera cuts/teleports, not shutter
                // motion. Reset the reference instead of smearing the whole image.
                let camera_cut = position_delta.length_squared() > 256.0 * 256.0;
                if camera_cut {
                    motion_reference_camera = next.camera;
                    motion_camera_sample_dt = 1.0 / 120.0;
                } else {
                    // After an idle period, don't interpret the idle time as the camera's
                    // sample interval. The first new input sample should still blur.
                    motion_camera_sample_dt = if elapsed > 0.0 && elapsed <= 0.05 {
                        elapsed.max(1.0 / 10_000.0)
                    } else {
                        1.0 / 120.0
                    };
                    motion_reference_camera = snapshot.camera;
                }
                motion_camera_changed_at = now;
            }
            // HUD input/velocity/health are mailbox state just like the camera:
            // update only the tiny hot vertex buffer when they actually change.
            renderer.set_dynamic_hud(next.hud, next.movement_hud);
            renderer.set_player_names(next.player_names.clone());
            renderer.set_jump_shade(next.jump_shade);
            renderer.set_screen_fx(&next.screen_fx);
            renderer.set_cloud_foreground(&next.cloud_foreground);
            snapshot = next;
        }

        if renderer.size.width == 0 || renderer.size.height == 0 {
            thread::sleep(Duration::from_millis(8));
            continue;
        }

        // Every settings command for this frame has been drained, so a burst that
        // asked for the scene rebuild several times over pays for it once.
        renderer.apply_pending_scene_rebuild();

        renderer.update_inline_models(snapshot.inline_models.as_deref().map(Vec::as_slice));

        if let Some(scene) = snapshot.companion_scene.as_ref() {
            if let Some(worker) = companion_renderers.get(&scene.id) {
                if let Some(sources) = renderer.ensure_companion_scene_target(
                    scene.id,
                    worker.size(),
                    worker.scene_consumed(),
                ) {
                    worker.set_scene_sources(sources);
                }
            }
        }

        let frame_loop_started = Instant::now();
        // Permanent subframe input samples the same lock-free latest-view mailbox
        // at frame start. `cl_input_latelatch 1` defers that exact sample until
        // the renderer's last coherent camera point, making the A/B comparison
        // about latch timing rather than about two different input pipelines.
        let mut frame_camera = snapshot.camera;
        if let Some(shake) = snapshot.camera_shake {
            let now = Instant::now();
            if let Some(offset) = shake.sample(now, &mut shake_rng) {
                frame_camera.shake = offset;
                frame_camera.apply_shake();
            }
        }
        let frame_start_view_sample = (!input_latelatch)
            .then(|| {
                sample_view_rotation(&mut frame_camera, latest_view.as_ref(), snapshot.view_latch)
            })
            .flatten();
        let frame_crosshair_world = frame_start_view_sample
            .map(|measurement| measurement.crosshair_world)
            .unwrap_or(snapshot.dynamic_crosshair_world);
        let input_latency_sample = frame_start_view_sample
            .map(|measurement| measurement.input)
            .or(snapshot.input_latency)
            .filter(|sample| sample.sequence != last_measured_input_sequence);
        let render_result = {
            let mut model_source = snapshot.model_frame_source;
            if let Some(source) = model_source.as_mut() {
                source.raster_surfaces = snapshot
                    .dynamic_models
                    .iter()
                    .filter(|surface| surface.entity_num == source.entity && surface.raster_visible)
                    .count();
            }
            renderer.model_frame_log.set_source(model_source);
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Render,
                crate::thread_activity::Task::RenderFrame,
            );
            renderer.render(
                &frame_camera,
                snapshot.player_position,
                snapshot.area_mask.as_ref(),
                snapshot.dynamic_models.as_slice(),
                snapshot.transient_lights.as_slice(),
                snapshot.companion_scene.as_ref(),
                snapshot.dof_focus_target,
                frame_crosshair_world,
                &motion_reference_camera,
                motion_camera_sample_dt,
                motion_camera_changed_at.elapsed().as_secs_f32(),
                snapshot.view_latch,
                input_latelatch.then_some(latest_view.as_ref()),
            )
        };
        if gamma_preparation.is_some() && render_result.is_ok() && !renderer.baked_brightness.busy {
            let generation = gamma_preparation.take().unwrap();
            let _ = proxy.send_event(UserEvent::GammaPrepared(generation));
        }
        match render_result {
            Ok(info) => {
                for scene_frame in renderer.take_companion_scene_ready() {
                    if let Some(worker) = companion_renderers.get(&scene_frame.id) {
                        worker.publish_scene(scene_frame);
                    }
                }
                if !first_frame_notified {
                    first_frame_notified = true;
                    let _ = proxy.send_event(UserEvent::RendererFirstFrame);
                }
                let splashes = renderer.weather.rain.take_water_splashes();
                if !splashes.is_empty() {
                    let _ = proxy.send_event(UserEvent::WaterSplashes(splashes));
                }
                if perf_trace {
                    trace_frames += 1;
                    trace_frame_ms += info.frame_ms;
                    trace_prepare_ms += info.cpu_prepare_ms;
                    trace_acquire_ms += info.cpu_acquire_ms;
                    trace_encode_ms += info.cpu_encode_ms;
                    trace_submit_ms += info.cpu_submit_ms;
                    trace_present_ms += info.cpu_present_ms;
                    trace_dynamic_model_ms += info.dynamic_model_prepare_ms;
                    trace_grass_cpu_ms += info.grass.cpu_ms;
                    trace_prep_stage_ms[0] += info.prep_camera_ms;
                    trace_prep_stage_ms[1] += info.prep_shadow_ms;
                    trace_prep_stage_ms[2] += info.prep_world_ms;
                    trace_prep_stage_ms[3] += info.prep_reflection_ms;
                }
                last_info = info;
                let late_measurement = input_latelatch
                    .then_some(last_info.late_latch)
                    .flatten()
                    .filter(|measurement| {
                        measurement.input.sequence != last_measured_input_sequence
                    });
                let measured_sample = late_measurement
                    .map(|measurement| measurement.input)
                    .or(input_latency_sample);
                if let Some(sample) = measured_sample {
                    let render_sample_at = late_measurement
                        .map(|measurement| measurement.sampled_at)
                        .or_else(|| {
                            frame_start_view_sample
                                .filter(|measurement| measurement.input.sequence == sample.sequence)
                                .map(|measurement| measurement.sampled_at)
                        })
                        .unwrap_or(frame_loop_started);
                    let input_event_to_sim_ms = sample
                        .simulation_at
                        .saturating_duration_since(sample.event_at)
                        .as_secs_f64()
                        * 1000.0;
                    let input_sim_to_render_ms = render_sample_at
                        .saturating_duration_since(sample.simulation_at)
                        .as_secs_f64()
                        * 1000.0;
                    let present_completed_at = last_info
                        .present_call_completed_at
                        .unwrap_or_else(Instant::now);
                    let input_event_to_present_call_ms = present_completed_at
                        .saturating_duration_since(sample.event_at)
                        .as_secs_f64()
                        * 1000.0;
                    input_latency_samples += 1;
                    input_event_to_sim_sum_ms += input_event_to_sim_ms;
                    input_sim_to_render_sum_ms += input_sim_to_render_ms;
                    if let Some(measurement) = late_measurement {
                        input_latch_samples += 1;
                        input_event_to_latch_sum_ms += measurement
                            .sampled_at
                            .saturating_duration_since(sample.event_at)
                            .as_secs_f64()
                            * 1000.0;
                        if let Some(submit_at) = last_info.submit_call_at {
                            input_latch_to_submit_sum_ms += submit_at
                                .saturating_duration_since(measurement.sampled_at)
                                .as_secs_f64()
                                * 1000.0;
                        }
                        input_latch_to_present_call_sum_ms += present_completed_at
                            .saturating_duration_since(measurement.sampled_at)
                            .as_secs_f64()
                            * 1000.0;
                    }
                    input_event_to_present_call_sum_ms += input_event_to_present_call_ms;
                    input_event_to_present_call_max_ms =
                        input_event_to_present_call_max_ms.max(input_event_to_present_call_ms);
                    last_measured_input_sequence = sample.sequence;
                }
                if let Some((
                    request_id,
                    started,
                    name,
                    triangles,
                    batches,
                    upload_ms,
                    upload_timings,
                    timings,
                    bsp_stats,
                )) = pending_world_uploaded.take()
                {
                    let _ = proxy.send_event(UserEvent::WorldUploaded {
                        request_id,
                        name,
                        triangles,
                        batches,
                        upload_ms,
                        upload_timings,
                        first_frame_ms: started.elapsed().as_secs_f64() * 1000.0,
                        timings,
                        bsp_stats,
                    });
                }

                // Slow levelshot VFS/decode work happens only after a visible
                // unknown-map frame has completed present().
                renderer.resolve_pending_ui_levelshot();
            }
            Err(RenderError::Lost | RenderError::Outdated) => renderer.reconfigure(),
            Err(RenderError::Timeout) => {}
            Err(RenderError::Occluded) => {
                // An occluded/minimized/hidden surface cannot make forward progress
                // by spinning at uncapped render speed. In particular, startup used
                // to hold the HWND hidden until the first successful present, making
                // this path a self-sustaining busy loop. RendererReady now reveals
                // deliberately hidden startup/restart windows; this small backoff
                // also keeps genuinely occluded windows from burning a CPU core.
                thread::sleep(Duration::from_millis(8));
            }
            Err(RenderError::Validation) => {
                let _ = proxy.send_event(UserEvent::RendererError(
                    "GPU surface validation error".into(),
                ));
            }
        }

        if let Some(result) = renderer.take_screenshot_result() {
            let _ = proxy.send_event(UserEvent::ScreenshotFinished(result));
        }

        stats_frames += 1;
        let elapsed = stats_started.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let fps = stats_frames as f64 / elapsed.as_secs_f64();
            let threads = {
                // Sample outside the main-thread event callback so observing the
                // overlay does not make MAIN appear busy by definition. Mark this
                // tiny stats section as render work so RENDER's instantaneous state
                // remains meaningful at the sample point.
                let _activity = crate::thread_activity::activity(
                    crate::thread_activity::ThreadSlot::Render,
                    crate::thread_activity::Task::RenderFrame,
                );
                crate::thread_activity::sample()
            };
            let gpu_timing = renderer.gpu_profiler.latest;
            let gpu_pass =
                |pass: GpuPass| gpu_timing.and_then(|timing| timing.milliseconds[pass.index()]);
            if proxy
                .send_event(UserEvent::RenderStats(RenderStats {
                    fps,
                    frame_ms: last_info.frame_ms,
                    cpu_prepare_ms: last_info.cpu_prepare_ms,
                    cpu_acquire_ms: last_info.cpu_acquire_ms,
                    cpu_encode_ms: last_info.cpu_encode_ms,
                    cpu_submit_ms: last_info.cpu_submit_ms,
                    cpu_present_ms: last_info.cpu_present_ms,
                    dynamic_model_prepare_ms: last_info.dynamic_model_prepare_ms,
                    dynamic_model_surfaces: last_info.dynamic_model_surfaces,
                    dynamic_model_vertices: last_info.dynamic_model_vertices,
                    dynamic_model_indices: last_info.dynamic_model_indices,
                    gpu_ms: last_info.gpu_ms,
                    gpu_depth_ms: gpu_pass(GpuPass::Depth),
                    gpu_hiz_ms: gpu_pass(GpuPass::HiZ),
                    gpu_cull_ms: gpu_pass(GpuPass::Cull),
                    gpu_cluster_ms: gpu_pass(GpuPass::Cluster),
                    gpu_world_ms: gpu_pass(GpuPass::World),
                    gpu_fx_sprites_ms: match (
                        gpu_pass(GpuPass::FxSpritesOpaque),
                        gpu_pass(GpuPass::FxSpritesTranslucent),
                    ) {
                        (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                        (Some(opaque), None) => Some(opaque),
                        (None, Some(translucent)) => Some(translucent),
                        (None, None) => None,
                    },
                    gpu_post_ms: gpu_pass(GpuPass::Post),
                    gpu_ui_ms: gpu_pass(GpuPass::Ui),
                    cull_visible: last_info.cull_visible,
                    cull_frustum_rejected: last_info.cull_frustum_rejected,
                    cull_hiz_rejected: last_info.cull_hiz_rejected,
                    cull_pvs_rejected: last_info.cull_pvs_rejected,
                    cull_area_rejected: last_info.cull_area_rejected,
                    input_event_to_sim_ms: (input_latency_samples != 0)
                        .then(|| input_event_to_sim_sum_ms / input_latency_samples as f64),
                    input_sim_to_render_ms: (input_latency_samples != 0)
                        .then(|| input_sim_to_render_sum_ms / input_latency_samples as f64),
                    input_event_to_latch_ms: (input_latch_samples != 0)
                        .then(|| input_event_to_latch_sum_ms / input_latch_samples as f64),
                    input_latch_to_submit_ms: (input_latch_samples != 0)
                        .then(|| input_latch_to_submit_sum_ms / input_latch_samples as f64),
                    input_latch_to_present_call_ms: (input_latch_samples != 0)
                        .then(|| input_latch_to_present_call_sum_ms / input_latch_samples as f64),
                    input_event_to_present_call_ms: (input_latency_samples != 0)
                        .then(|| input_event_to_present_call_sum_ms / input_latency_samples as f64),
                    input_event_to_present_call_max_ms: (input_latency_samples != 0)
                        .then_some(input_event_to_present_call_max_ms),
                    input_latency_samples: u32::try_from(input_latency_samples).unwrap_or(u32::MAX),
                    msaa_samples: renderer.msaa_samples,
                    vsync: renderer.vsync,
                    threads,
                }))
                .is_err()
            {
                return;
            }
            stats_started = Instant::now();
            stats_frames = 0;
            input_latency_samples = 0;
            input_latch_samples = 0;
            input_event_to_sim_sum_ms = 0.0;
            input_sim_to_render_sum_ms = 0.0;
            input_event_to_latch_sum_ms = 0.0;
            input_latch_to_submit_sum_ms = 0.0;
            input_latch_to_present_call_sum_ms = 0.0;
            input_event_to_present_call_sum_ms = 0.0;
            input_event_to_present_call_max_ms = 0.0;
        }

        if perf_trace && trace_started.elapsed() >= Duration::from_secs(1) {
            let elapsed = trace_started.elapsed().as_secs_f64();
            let frames = trace_frames.max(1) as f64;
            let gpu = renderer.gpu_profiler.latest;
            let gpu_value =
                |pass: GpuPass| gpu.and_then(|timing| timing.milliseconds[pass.index()]);
            let gpu_ms = |pass: GpuPass| {
                gpu_value(pass).map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"))
            };
            let dynamic_model_gpu = match (
                gpu_value(GpuPass::DynamicModels),
                gpu_value(GpuPass::DynamicModelsTranslucent),
            ) {
                (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                (Some(opaque), None) => Some(opaque),
                (None, Some(translucent)) => Some(translucent),
                (None, None) => None,
            };
            let dynamic_model_ms =
                dynamic_model_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            let fx_sprite_gpu = match (
                gpu_value(GpuPass::FxSpritesOpaque),
                gpu_value(GpuPass::FxSpritesTranslucent),
            ) {
                (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                (Some(opaque), None) => Some(opaque),
                (None, Some(translucent)) => Some(translucent),
                (None, None) => None,
            };
            let fx_sprite_ms =
                fx_sprite_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            let grass_prepare_gpu = gpu_value(GpuPass::GrassPrepare);
            let grass_draw_gpu = gpu_value(GpuPass::Grass);
            let grass_total_gpu = match (grass_prepare_gpu, grass_draw_gpu) {
                (Some(prepare), Some(draw)) => Some(prepare + draw),
                (None, Some(draw)) => Some(draw),
                _ => None,
            };
            let grass_total_ms =
                grass_total_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            println!(
                "[JKA PERF] fps={:.1} cpu_frame={:.4}ms prep={:.4}ms acquire={:.4}ms encode={:.4}ms submit={:.4}ms present={:.4}ms dyn_prepare={:.4}ms g2_draws={}/{} batch={} world_batches[pvs={} frustum_reject={} encoded={} multidraw_groups={} multidraw_batches={}] hotpath=1",
                trace_frames as f64 / elapsed,
                trace_frame_ms / frames,
                trace_prepare_ms / frames,
                trace_acquire_ms / frames,
                trace_encode_ms / frames,
                trace_submit_ms / frames,
                trace_present_ms / frames,
                trace_dynamic_model_ms / frames,
                last_info.ghoul2_gpu_draw_calls,
                last_info.ghoul2_gpu_instances,
                renderer.dynamic_model_renderer.ghoul2_batch_draws.config_value(),
                last_info.world_pvs_batches,
                last_info.world_frustum_rejected,
                last_info.world_encoded_batches,
                last_info.world_multidraw_groups,
                last_info.world_multidraw_batches,
            );
            // A/B line: which world renderer ran, plus the unified prepare split.
            // FastBaseline does not split its prepare stage, so those read 0.
            println!(
                "[JKA PERF PATH] path={} gpu_timings={} cpu_frame={:.4}ms prep={:.4}ms[camera={:.4} shadow={:.4} world={:.4} reflection={:.4} dynamic={:.4}] encode={:.4}ms submit={:.4}ms present={:.4}ms",
                last_info.world_path,
                renderer.gpu_profiler.enabled(),
                trace_frame_ms / frames,
                trace_prepare_ms / frames,
                trace_prep_stage_ms[0] / frames,
                trace_prep_stage_ms[1] / frames,
                trace_prep_stage_ms[2] / frames,
                trace_prep_stage_ms[3] / frames,
                trace_dynamic_model_ms / frames,
                trace_encode_ms / frames,
                trace_submit_ms / frames,
                trace_present_ms / frames,
            );
            let client = snapshot.client_perf;
            println!(
                "[JKA CLIENT PERF] total={:.4}ms snapshot={:.4}ms audio={:.4}ms events={:.4}ms event_prep={:.4}ms jobs={} pool_threads={} parallel={} event_decode={:.4}ms decode_jobs={} decode_parallel={} entities={:.4}ms players={:.4}ms followed={:.4}ms fx={:.4}ms fx_tess={:.4}ms g2_mode={} g2_pose={:.4}ms/{} motion={:.4}ms/{} g2_skin={:.4}ms g2_bolt={:.4}ms/{} surfaces={} verts={} cull={}/{} lod=[{},{},{},{}] dyn[surfaces={} verts={} indices={}]",
                client.total_ms,
                client.snapshot_ms,
                client.audio_ms,
                client.events_ms,
                client.event_prepare_ms,
                client.event_worker_jobs,
                client.event_worker_threads,
                client.event_worker_parallel,
                client.event_sound_decode_ms,
                client.event_sound_decode_jobs,
                client.event_sound_decode_parallel,
                client.entity_present_ms,
                client.player_present_ms,
                client.followed_player_ms,
                client.fx_ms,
                client.fx_tessellate_ms,
                client.ghoul2_skinning_mode.config_value(),
                client.ghoul2_pose_ms,
                client.ghoul2_pose_evals,
                client.ghoul2_motion_pose_ms,
                client.ghoul2_motion_pose_evals,
                client.ghoul2_skin_ms,
                client.ghoul2_bolt_ms,
                client.ghoul2_bolt_queries,
                client.ghoul2_surfaces,
                client.ghoul2_vertices,
                client.ghoul2_frustum_culled,
                client.ghoul2_frustum_tests,
                client.ghoul2_lod_counts[0],
                client.ghoul2_lod_counts[1],
                client.ghoul2_lod_counts[2],
                client.ghoul2_lod_counts[3],
                client.dynamic_surfaces,
                client.dynamic_vertices,
                client.dynamic_indices,
            );
            println!(
                "[JKA FX PERF] draws={} sprite={} oriented={} line={} quad={} mesh={} cylinder={} output[surfaces={} cpu_surfaces={} cpu_verts={} cpu_indices={} gpu_sprite_batches={} gpu_sprite_instances={}] zero_alpha_discard={} gpu_sprites={}ms",
                client.fx_draws,
                client.fx_sprites,
                client.fx_oriented_quads,
                client.fx_lines,
                client.fx_quads,
                client.fx_meshes,
                client.fx_cylinders,
                client.fx_render_surfaces,
                client.fx_cpu_geom_surfaces,
                client.fx_cpu_vertices,
                client.fx_cpu_indices,
                client.fx_gpu_sprite_batches,
                client.fx_gpu_sprite_instances,
                u8::from(renderer.dynamic_model_renderer.fx_zero_alpha_discard),
                fx_sprite_ms,
            );
            let dlight = renderer.legacy_dlight_perf_stats();
            if renderer.dynamic_lights_mode == DynamicLightsMode::Legacy {
                let avg_radius = if dlight.transient_count == 0 {
                    0.0
                } else {
                    dlight.radius_sum / dlight.transient_count as f64
                };
                let avg_candidates = if dlight.surfaces_touched == 0 {
                    0.0
                } else {
                    dlight.candidate_pairs as f64 / dlight.surfaces_touched as f64
                };
                let mask_density = if dlight.surfaces_eligible == 0 || dlight.transient_count == 0 {
                    0.0
                } else {
                    100.0 * dlight.candidate_pairs as f64
                        / (dlight.surfaces_eligible * dlight.transient_count) as f64
                };
                let avg_surfaces_per_light = if dlight.transient_count == 0 {
                    0.0
                } else {
                    dlight.total_surfaces_per_light as f64 / dlight.transient_count as f64
                };
                println!(
                    "[JKA DLIGHT PERF] transient={} sources[saber={} fx={} saber_mark={}] radius[avg={:.2} max={:.2}] surfaces[total={} eligible={} touched={}] pairs={} mask_density={:.2}% candidates_per_touched[avg={:.2} max={}] buckets[0={} 1={} 2-4={} 5-8={} 9-16={} 17+={}] pairs_by_source[saber={} fx={} saber_mark={}] surfaces_per_light[avg={:.2} max={}]",
                    dlight.transient_count,
                    dlight.saber_sources,
                    dlight.authored_fx_sources,
                    dlight.saber_mark_sources,
                    avg_radius,
                    dlight.radius_max,
                    dlight.surfaces_total,
                    dlight.surfaces_eligible,
                    dlight.surfaces_touched,
                    dlight.candidate_pairs,
                    mask_density,
                    avg_candidates,
                    dlight.max_candidates_per_surface,
                    dlight.surface_buckets[0],
                    dlight.surface_buckets[1],
                    dlight.surface_buckets[2],
                    dlight.surface_buckets[3],
                    dlight.surface_buckets[4],
                    dlight.surface_buckets[5],
                    dlight.saber_pairs,
                    dlight.authored_fx_pairs,
                    dlight.saber_mark_pairs,
                    avg_surfaces_per_light,
                    dlight.max_surfaces_per_light,
                );
            }
            println!(
                "[JKA PERF GPU] frame={}ms rt_build={}ms world={}ms dlights={}ms dyn_models={}ms fx_sprites={}ms ocean_optics={}ms grass={}ms grass_prepare={}ms grass_draw={}ms post={}ms depth={}ms hiz={}ms cull={}ms cluster={}ms ui={}ms clouds={}ms",
                gpu_ms(GpuPass::Frame),
                gpu_ms(GpuPass::RtBuild),
                gpu_ms(GpuPass::World),
                gpu_ms(GpuPass::LegacyDlights),
                dynamic_model_ms,
                fx_sprite_ms,
                gpu_ms(GpuPass::OceanOptics),
                grass_total_ms,
                gpu_ms(GpuPass::GrassPrepare),
                gpu_ms(GpuPass::Grass),
                gpu_ms(GpuPass::Post),
                gpu_ms(GpuPass::Depth),
                gpu_ms(GpuPass::HiZ),
                gpu_ms(GpuPass::Cull),
                gpu_ms(GpuPass::Cluster),
                gpu_ms(GpuPass::Ui),
                gpu_ms(GpuPass::Clouds),
            );
            if renderer.hardware_rt_active() {
                if let Some(rt) = renderer.ray_traced_shadows.as_ref() {
                    println!(
                        "[JKA RT PERF] build={}ms skin={}ms accel={}ms visibility={}ms sun_shadow_rate={} static_opaque[geoms={} tris={}] masked[geoms={} tris={}] dynamic[instances={} rigid_tris={} skinned_tris={} masked_rigid={} masked_skinned={}] caches[rigid={} skinned={}] alpha[textures={} packed_words={} generation={}] sun_rays={} local_rays={} (budget_per_light_evaluation; not_measured)",
                    gpu_ms(GpuPass::RtBuild),
                    gpu_ms(GpuPass::RtSkin),
                    gpu_ms(GpuPass::RtAcceleration),
                    gpu_ms(GpuPass::RtVisibility),
                    if renderer.rt_reduced_shadows { "quarter" } else { "full" },
                    rt.geometry_count,
                    rt.triangle_count,
                    rt.masked_geometry_count,
                    rt.masked_triangle_count,
                    rt.last_dynamic_instance_count,
                    rt.active_rigid_triangles,
                    rt.active_skinned_triangles,
                    rt.active_masked_rigid,
                    rt.active_masked_skinned,
                    rt.rigid_blas_cache.len(),
                    rt.skinned_blas_cache.len(),
                    rt.alpha_textures.len(),
                    rt.alpha_texels.len(),
                        rt.alpha_buffers_generation,
                        if renderer.world.as_ref().is_some_and(|world| world.active_pipeline_variant.ray_traced_sun) { renderer.rt_samples } else { 0 },
                        if renderer.ray_traced_local_shadows_active() { format!("up_to_{}_per_segment_1_per_point", renderer.rt_samples) } else { "0".into() },
                    );
                }
            }
            let grass = last_info.grass;
            println!(
                "[JKA GRASS PERF] cpu_select={:.4}ms gpu={}ms gpu_prepare={}ms gpu_draw={}ms patches={}/{} reject[pvs={} frustum={} distance={} lod_empty={}] draw_records={} submit_calls={} blades[high={} mid={} low={}] triangles={} ab[precompute={} mid_lod={} front_to_back={}]",
                trace_grass_cpu_ms / frames,
                grass_total_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}")),
                gpu_ms(GpuPass::GrassPrepare),
                gpu_ms(GpuPass::Grass),
                grass.patches_visible,
                grass.patches_total,
                grass.pvs_rejected,
                grass.frustum_rejected,
                grass.distance_rejected,
                grass.lod_empty,
                grass.draw_calls,
                grass.submit_calls,
                grass.high_blades,
                grass.mid_blades,
                grass.low_blades,
                grass.triangles,
                u8::from(renderer.grass_precompute_enabled),
                u8::from(renderer.grass_mid_lod_enabled),
                u8::from(renderer.grass_front_to_back_enabled),
            );
            println!(
                "[JKA PERF PLAN] post={} gamma_fast={} linear_depth={} hiz={} gpu_cull={} compact={} clustered={} gamma={:.3} grain={:.3} lut={} lut_strength={:.2} fog={:?} hdr={} tonemap={} bloom={} ssao={} static_ao={} fxaa={} smaa={} taa={} contact={} ssr={} motion={:.2} motion_scale={:.2} dof={:.2} dof_quality={} dof_focus={:.0} ca={:.2} vignette={} local_shadows={} pbr={} pom={} cascaded={}",
                u8::from(renderer.frame_plan.use_post),
                u8::from(renderer.frame_plan.gamma_only_post),
                u8::from(renderer.frame_plan.needs_linear_depth),
                u8::from(renderer.frame_plan.use_hiz),
                u8::from(renderer.frame_plan.use_gpu_culling),
                u8::from(
                    renderer.frame_plan.use_gpu_culling
                        && renderer.gpu_compaction_supported
                        && renderer
                            .world
                            .as_ref()
                            .is_some_and(|world| !world.compact_groups.is_empty()),
                ),
                u8::from(renderer.frame_plan.use_clustered_lighting),
                renderer.gamma,
                renderer.film_grain_strength,
                renderer.color_lut_preset.config_value(),
                renderer.color_lut_strength,
                renderer.weather.fog.mode,
                u8::from(renderer.hdr_enabled),
                u8::from(renderer.tone_mapping_enabled),
                u8::from(renderer.bloom_enabled),
                u8::from(renderer.ssao_enabled),
                u8::from(renderer.static_bsp_ao_enabled),
                u8::from(renderer.fxaa_enabled),
                u8::from(renderer.smaa_enabled),
                u8::from(renderer.taa_enabled),
                u8::from(renderer.contact_shadows_enabled),
                u8::from(renderer.ssr_enabled),
                renderer.motion_blur_strength,
                renderer.motion_blur_runtime_scale,
                renderer.depth_of_field_strength,
                renderer.dof_quality.config_value(),
                renderer.dof_focus_distance,
                renderer.chromatic_aberration_strength,
                u8::from(renderer.vignette_enabled),
                u8::from(renderer.local_light_shadows_enabled),
                u8::from(renderer.pbr_enabled && PBR_PROFILE_MATERIALS),
                u8::from(renderer.pbr_enabled && PBR_PROFILE_PARALLAX_OCCLUSION),
                u8::from(renderer.cascaded_shadows_enabled),
            );
            let (point_sources, area_sources, total_sources) =
                renderer
                    .world
                    .as_ref()
                    .map_or((0_usize, 0_usize, 0_usize), |world| {
                        let area_sources = world
                            .dynamic_lights
                            .iter()
                            .filter(|light| {
                                light
                                    .emitter_normal
                                    .iter()
                                    .any(|component| component.abs() > 1.0e-6)
                            })
                            .count();
                        (
                            world.dynamic_lights.len().saturating_sub(area_sources),
                            area_sources,
                            world.dynamic_lights.len(),
                        )
                    });
            let (pipeline_variant_label, cached_pipeline_variants) =
                renderer.world.as_ref().map_or_else(
                    || (renderer.world_shader_variant_key().short_label(), 0_usize),
                    |world| {
                        (
                            world.active_pipeline_variant.short_label(),
                            world.pipeline_variants.len(),
                        )
                    },
                );
            println!(
                "[JKA PERF STATE] camera=[{:.2},{:.2},{:.2}] yaw={:.5} pitch={:.5} dlights={} point={} area={} gi={} world_path={} shader={} pipeline_variant={} cached_variants={} planar={} planar_active={} lights_total={} point_sources={} area_sources={} transient_sources={} active_lights={}",
                snapshot.camera.position.x,
                snapshot.camera.position.y,
                snapshot.camera.position.z,
                snapshot.camera.yaw,
                snapshot.camera.pitch,
                renderer.dynamic_lights_mode.config_value(),
                u8::from(renderer.point_lighting_enabled()),
                u8::from(renderer.emissive_area_lights_enabled),
                u8::from(renderer.voxel_probe_gi_enabled),
                match renderer.frame_plan.world_path {
                    WorldRenderPath::FastBaseline => "fast-baseline",
                    WorldRenderPath::Advanced => "advanced",
                },
                if renderer.enhanced_world_shader_needed() {
                    "enhanced"
                } else {
                    "lean"
                },
                pipeline_variant_label,
                cached_pipeline_variants,
                renderer.planar_reflection_mode.label(),
                u8::from(renderer.planar_reflection.active),
                total_sources,
                point_sources,
                area_sources,
                renderer.transient_lights.len(),
                renderer.active_light_count(),
            );
            trace_started = Instant::now();
            trace_frames = 0;
            trace_frame_ms = 0.0;
            trace_prepare_ms = 0.0;
            trace_acquire_ms = 0.0;
            trace_encode_ms = 0.0;
            trace_submit_ms = 0.0;
            trace_present_ms = 0.0;
            trace_dynamic_model_ms = 0.0;
            trace_grass_cpu_ms = 0.0;
            trace_prep_stage_ms = [0.0; 4];
        }

        if fps_cap > 0 {
            let target = Duration::from_secs_f64(1.0 / fps_cap as f64);
            let deadline = frame_pace_anchor + target;
            precise_frame_wait(deadline);
            // On time: keep the exact cadence. Late (frame slower than the cap,
            // cap changed, menu/pause transition): restart it rather than
            // bursting to catch up.
            let now = Instant::now();
            frame_pace_anchor = if now.saturating_duration_since(deadline) > target {
                now
            } else {
                deadline
            };
        } else {
            frame_pace_anchor = Instant::now();
        }
    }
}

pub(in crate::renderer) fn precise_frame_wait(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        if remaining > Duration::from_millis(2) {
            thread::sleep(remaining - Duration::from_millis(1));
        } else if remaining > Duration::from_micros(150) {
            thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}
