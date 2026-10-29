//! Lifecycle.
use crate::app::{
    keybinds, playerstate_vec3, prepared_map_has_ocean_surface, random_unit, scene,
    split_console_script, ui, ActiveEventLoop, App, ApplicationHandler, Arc, BindKey, Camera,
    ClientFramePerf, ConsoleSize, ControlFlow, DemoTimelineHit, DemoViewMode, DeviceEvent,
    Duration, ElementState, FrontendCinematic, FrontendPage, FullscreenMode, InputLatencySample,
    Instant, KeyCode, LiveJoinTiming, LocalServer, MapEditor, ModifiersState, MouseButton,
    MouseScrollDelta, OverlayMode, PerfStats, PhysicalKey, PhysicalPosition, PhysicalSize,
    PreparedMapCache, StartCause, RenderCommand, RenderThread, RendererBackend, ScreenshotOutput,
    ThreadPerfStats, UserEvent, Window, WindowEvent, WindowId, FRONTEND_CAMERA_EYE_HEIGHT,
    MAX_SPECTATOR_ORBIT_RANGE, MIN_SPECTATOR_ORBIT_RANGE,
};

/// `DJK_HEADLESS=1`: offline tools (emote_gallery) render without ever
/// showing or focusing the window, at below-normal process priority.
pub(crate) fn headless() -> bool {
    static HEADLESS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HEADLESS.get_or_init(|| {
        let on = std::env::var("DJK_HEADLESS").is_ok_and(|v| v == "1");
        #[cfg(windows)]
        if on {
            use windows_sys::Win32::System::Threading::{
                BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
            };
            // SAFETY: both calls take no pointers; the pseudo-handle is always valid.
            unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
        }
        on
    })
}

fn show_startup_window(window: &Window) {
    if headless() {
        return;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Graphics::Gdi::{GetStockObject, BLACK_BRUSH},
            UI::WindowsAndMessaging::{SetClassLongPtrW, GCLP_HBRBACKGROUND},
        };
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                // Winit has no class background brush. Supply a stock black
                // brush so Windows paints the client area before WGPU is ready.
                // Stock brushes live for the process lifetime; do not delete it.
                unsafe {
                    SetClassLongPtrW(
                        handle.hwnd.get() as *mut std::ffi::c_void,
                        GCLP_HBRBACKGROUND,
                        GetStockObject(BLACK_BRUSH) as isize,
                    );
                }
            }
        }
    }
    window.set_visible(true);
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        let cause = match cause {
            StartCause::Init => "init",
            StartCause::Poll => "poll",
            StartCause::ResumeTimeReached { .. } => "resume_time_reached",
            StartCause::WaitCancelled { .. } => "wait_cancelled",
        };
        if let Some(render) = &self.render {
            render.record_input_diagnostic_event_loop_turn(
                Instant::now(),
                cause,
                self.window_focused,
                self.captured,
                self.mouse_grab_mode,
            );
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if headless() {
            self.window_focused = false;
            self.video.fullscreen = FullscreenMode::Windowed;
            self.applied_fullscreen = FullscreenMode::Windowed;
        }
        // Set up placement/fullscreen while hidden, then show the native window
        // before graphics initialization so startup is not console-only.
        let mut attributes = Window::default_attributes()
            .with_title("DinurdoJK")
            .with_visible(false)
            .with_active(!headless())
            .with_inner_size(PhysicalSize::new(
                self.video.resolution[0],
                self.video.resolution[1],
            ));
        if let Some([x, y]) = self.video.window_position {
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("Window creation failed: {error}");
                event_loop.exit();
                return;
            }
        };
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::WindowCreated);
        if !self.video.fullscreen.is_windowed() && !headless() {
            if let Err(error) = Self::apply_fullscreen_mode(
                &window,
                self.video.fullscreen,
                self.video.resolution,
                self.video.renderer_backend,
            ) {
                eprintln!("Fullscreen setup failed: {error}; falling back to windowed mode");
                self.video.fullscreen = FullscreenMode::Windowed;
            }
        }
        if self.video.fullscreen.is_windowed() && self.video.window_maximized {
            window.set_maximized(true);
        }
        show_startup_window(&window);
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::WindowShown);
        self.applied_fullscreen = self.video.fullscreen;
        self.applied_renderer_backend = self.video.renderer_backend;
        self.applied_gpu_adapter = self.video.gpu_adapter;
        self.applied_resolution = self.video.resolution;
        self.applied_grass = self.video.grass;
        self.applied_ocean = self.video.ocean;
        self.reset_egui_for_window(&window);
        let initial = self.render_snapshot();
        let initial_ui = self.ui_snapshot();
        let render = match RenderThread::spawn(
            window.clone(),
            self.proxy.clone(),
            self.video.renderer_backend,
            initial,
            initial_ui,
            self.base.clone(),
            self.game.clone(),
            "auto".to_owned(),
        ) {
            Ok(render) => render,
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
                return;
            }
        };
        self.attach_companion_to_renderer(&render);
        self.window = Some(window);
        self.render = Some(render);
        self.profile_background_requested=None;
        self.sync_classic_world_lighting();
        self.previous_tick = Instant::now();
        // A config written before this rule, or by hand, can still say 0. Resolve
        // it now that the window exists and the monitor refresh rate is readable.
        self.set_fps_cap(self.video.fps_cap);
        // Start *CPU-only* map preparation while WGPU initializes. A worker
        // result arriving before the first egui-presented frame is held below;
        // it cannot preempt the first clickable menu with GPU uploads.
        if self.front_end && !self.startup_first_frame_seen {
            self.request_frontend_background();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::Event,
        );
        match event {
            UserEvent::Update(event) => {
                self.handle_update_event(event);
            }
            UserEvent::SavedAccountsReady(result) => {
                // An account edit (possibly a save) may have happened while
                // the worker was enumerating. Never overwrite that newer UI
                // state with a stale snapshot.
                if self.account_editor.is_none() {
                    match result {
                        Ok(accounts) => {
                            self.saved_accounts = accounts;
                            self.account_credential_error = None;
                        }
                        Err(error) => self.account_credential_error = Some(error),
                    }
                    self.egui_repaint_requested = true;
                }
            }
            UserEvent::FrontendSceneCacheReady { path, generation, image } => {
                if self.frontend_scene_cache_generation == generation
                    && self.frontend_scene_cache_path.as_ref() == Some(&path)
                {
                    self.frontend_stage.cache_pending = false;
                    if let Some(image) = image {
                        // One decoded bitmap feeds the normal egui fallback AND
                        // the WGPU compositor. The copy/upload is on the render
                        // thread, never the UI frame loop.
                        self.render_command(crate::renderer::RenderCommand::SetNeonMenuCache(Some(image.clone())));
                        self.frontend_scene_texture = Some(self.egui_ctx.load_texture(
                            "frontend-scene-snapshot", image, egui::TextureOptions::LINEAR,
                        ));
                        self.egui_repaint_requested = true;
                    }
                }
            }
            UserEvent::FrontendStagePresented(token) => self.on_frontend_stage_presented(token),
            UserEvent::NeonMenuCompositorReady => {
                self.neon_menu.renderer_ready = true;
                self.egui_repaint_requested = true;
            },
            UserEvent::GammaPrepared(generation) => self.on_gamma_prepared(generation),
            UserEvent::HardwareGamma(status) => self.on_hardware_gamma(status),
            UserEvent::ConsoleCommand(command) => {
                // Route terminal input through the exact in-game console parser while
                // preserving any partially typed line in the graphical console.
                let pending_input = std::mem::replace(&mut self.console_input, command);
                let pending_cursor = std::mem::replace(&mut self.console_cursor, 0);
                self.console_cursor = self.console_input.len();
                let pending_history_index = self.console_history_index;
                self.execute_console();
                self.console_input = pending_input;
                self.console_cursor = pending_cursor.min(self.console_input.len());
                self.console_history_index = pending_history_index;
                self.publish_ui();
            }
            UserEvent::StudioSourceReady {id,error,vram,scenes} => {
                if self.render_studio.bsp_world.as_ref().is_some_and(|world|world.id==id) {
                    self.studio_bsp_ready(id,error);
                } else {self.on_studio_source_ready(id,error,vram,scenes);}
            },
            #[cfg(test)]
            UserEvent::StudioTestScenePresented {..} => {},
            UserEvent::ScreenshotFinished(result) => {
                match result {
                    Ok(ScreenshotOutput::FrontendSceneSaved(path)) => {
                        if self.frontend_scene_cache_path.as_ref() == Some(&path) {
                            // Do not pop a just-created cache into the middle of
                            // the cold-start black->live crossfade. It is ready
                            // for the next time this scene is selected.
                            self.push_console_line(format!(
                                "^2MENU SCENE CACHE:^7 {}", path.display()
                            ));
                        }
                    }
                    Ok(ScreenshotOutput::Saved(path)) => {
                        self.screenshot_catalog_loaded = false;
                        self.screenshot_preview_pending = None;
                        self.console_status = format!("WROTE {}", path.display());
                        self.push_console_path_line(format!("^2{}", self.console_status), path);
                    }
                    Ok(ScreenshotOutput::Clipboard) => {
                        self.console_status = "COPIED SCREENSHOT TO CLIPBOARD".into();
                        self.push_console_line(format!("^2{}", self.console_status));
                    }
                    Ok(ScreenshotOutput::VideoFrame {
                        frame_index,
                        width,
                        height,
                        bgra,
                        depth_f32le,
                        timing,
                    }) => {
                        if self.render_studio.still.is_some() {
                            self.on_render_studio_still_frame(frame_index, width, height, bgra, depth_f32le);
                        } else if self.render_studio.export.is_some() {
                            self.on_render_studio_frame(frame_index, width, height, bgra, depth_f32le, timing);
                        } else if self.demo_video_export.is_some() {
                            self.on_demo_video_frame(frame_index, width, height, bgra, depth_f32le);
                        }
                    }
                    Err(error) => {
                        if self.render_studio.export.is_some() || self.render_studio.still.is_some() {
                            self.fail_render_studio_export(format!("frame capture failed: {error}"));
                        } else if self.demo_video_export.is_some() {
                            self.fail_demo_video_export(format!("frame capture failed: {error}"));
                        } else {
                            self.console_status = format!("SCREENSHOT CAPTURE FAILED: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                }
                self.publish_ui();
            }
            UserEvent::HdrOutputChanged { status, active, available_modes } => {
                let was_native_hdr = self.hdr_output_active;
                self.hdr_output_status = status;
                self.hdr_output_active = active;
                self.hdr_available_modes = available_modes;
                self.hdr_modes_known = true;
                self.demote_unsupported_hdr_output();
                if self.hdr_output_active && self.video.gamma_method.is_hardware() {
                    self.set_gamma_method(crate::gamma::GammaMethod::Shader);
                }
                // A live Windows HDR->SDR transition changes the format of an
                // already allocated FP16 compositor. Rebuild once in SDR so
                // no hidden output submission survives the capability loss.
                if was_native_hdr && !active && !self.video_restart_in_flight() {
                    self.restart_renderer_internal(false);
                    return;
                }
                self.publish_ui();
            }
            UserEvent::RendererReady {
                supported_msaa,
                wireframe_supported,
                hdr_output_status,
                hdr_output_active,
                hdr_available_modes,
                adapter_name,
                adapter_type,
            } => {
                crate::startup_timeline::mark(crate::startup_timeline::Milestone::RendererReady);
                self.reload_studio_gpu_sources();
                // A restarted renderer has no Studio viewer rectangle: clear the
                // send latches so the next UI frame re-sends them. Without this,
                // a vid_restart/mode change left Studio frames rendering (and
                // exporting) as full-window draws with the editor UI in them.
                self.render_studio_world_compositor = None;
                self.asset_preview_viewport_key = None;
                if let Some(size) = self.render_studio.capture_size {
                    self.render_command(RenderCommand::SetStudioExportSize(Some(size)));
                }
                self.active_gpu_name = adapter_name;
                self.active_gpu_type = adapter_type;
                self.hdr_output_status = hdr_output_status;
                self.hdr_output_active = hdr_output_active;
                self.hdr_available_modes = hdr_available_modes;
                self.hdr_modes_known = true;
                self.demote_unsupported_hdr_output();
                // A startup SDR fallback does not allocate the HDR compositor;
                // the applied output must match the renderer, not the request.
                if !self.hdr_output_active
                    && self.applied_hdr_output != 0
                    && self.hdr_available_modes & (1 << self.applied_hdr_output.min(3)) == 0
                {
                    self.applied_hdr_output = 0;
                }
                if self.hdr_output_active && self.video.gamma_method.is_hardware() {
                    self.set_gamma_method(crate::gamma::GammaMethod::Shader);
                }
                self.supported_msaa = supported_msaa;
                self.wireframe_supported = wireframe_supported;
                if !self.supported_msaa.contains(&self.video.msaa_samples) {
                    self.video.msaa_samples = 1;
                    self.mark_config_dirty();
                }
                if !self.wireframe_supported && self.video.wireframe_mask != 0 {
                    self.video.wireframe_mask = 0;
                    self.mark_config_dirty();
                }
                self.console_status = format!(
                    "RENDER THREAD READY | MSAA {:?} | WIREFRAME {}",
                    self.supported_msaa,
                    if self.wireframe_supported {
                        "SUPPORTED"
                    } else {
                        "UNAVAILABLE"
                    }
                );
                self.push_console_line(format!("^2{}", self.console_status));
                self.publish_ui();

                // The native window is already visible during graphics startup.
                // Request its first rendered frame once the renderer is ready.
                if !self.startup_first_frame_seen {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                self.show_window_after_restart();
            }
            UserEvent::CompanionReady { id } => {
                if self
                    .companion
                    .as_ref()
                    .is_some_and(|companion| companion.id.0 == id)
                {
                    self.console_status = "COMPANION WINDOW: READY".into();
                    self.mark_companion_dirty();
                    self.egui_repaint_requested = true;
                }
            }
            UserEvent::CompanionError { id, error } => {
                if self
                    .companion
                    .as_ref()
                    .is_some_and(|companion| companion.id.0 == id)
                {
                    self.console_status = format!("COMPANION WINDOW ERROR: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                    self.companion_target_enabled = false;
                }
            }
            UserEvent::BlueWaterThunder(volume) => {
                if self.renderer_restart_suspended {
                    return;
                }
                if self.video.blue_water_environment.weather_backend==1 {
                    if let Some(sound)=self.game_session.as_mut().and_then(|s|s.sound_presenter.as_mut()) {sound.play_weather_thunder(volume);}
                }
            }
            UserEvent::RendererFirstFrame => {
                crate::startup_timeline::mark(crate::startup_timeline::Milestone::FirstPresent);
                // The main HWND has now presented. Move the already-visible
                // startup console behind it and release queued terminal output.
                crate::startup_console::reveal();
                crate::logging::activate_console_output();
                self.sync_gamma_focus();
                if self.startup_first_frame_seen {
                    if self.front_end || self.live_without_world || self.demo_without_world {
                        if let Some(restart_started) = self.renderer_restart_started.take() {
                            let total_ms = restart_started.elapsed().as_secs_f64() * 1000.0;
                            println!(
                                "[VID_RESTART] backend/display restart to first {} frame {:.1} ms",
                                if self.front_end {
                                    "menu"
                                } else {
                                    "worldless live"
                                },
                                total_ms
                            );
                        }
                        self.resume_client_after_video_restart();
                        self.begin_pending_video_confirmation();
                        self.publish_ui();
                    }
                    return;
                }
                self.startup_first_frame_seen = true;
                if self.updater.auto_check && !headless() && !self.video.skip_ui {
                    self.check_for_updates(false);
                }
                if self.front_end {
                    self.console_status = "MAIN MENU".into();
                    self.publish_ui();
                    // Frontend prep was already requested in resumed(). Any
                    // subsequent +map/+connect cancels the speculative request
                    // through the existing monotonically increasing request ID.
                    if self.video.skip_ui {
                        // A developer who disabled the UI still needs a map;
                        // in that case no egui-containing frame can arrive.
                        self.startup_first_menu_frame_seen = true;
                        if let Some(prepared) = self.deferred_frontend_map.take() {
                            let _ = self.proxy.send_event(prepared);
                        }
                    }
                } else {
                    let initial_source = self.initial_source.clone();
                    self.request_map(initial_source);
                    self.console_map_barrier = !self.startup_commands.is_empty();
                }
                for command in std::mem::take(&mut self.startup_commands) {
                    println!("STARTUP COMMAND: {command}");
                    self.push_console_command_echo(format!("^7] {command}"));
                    self.append_console_commands(split_console_script(&command));
                }
                if let Some(path) = self.startup_demo_file.take() {
                    self.play_demo_file(&path);
                }
            }
            UserEvent::RendererFirstMenuFrame => {
                if !self.startup_first_menu_frame_seen {
                    self.startup_first_menu_frame_seen = true;
                    crate::startup_timeline::mark(crate::startup_timeline::Milestone::FirstMenuPresent);
                    crate::startup_timeline::report("first_menu");
                }
                // Sending this back through the normal event path preserves all
                // existing MapPrepared semantics and its request-id rejection.
                if let Some(prepared) = self.deferred_frontend_map.take() {
                    let _ = self.proxy.send_event(prepared);
                }
            }
            UserEvent::WaterSplashes(splashes) => self.play_water_splashes(&splashes),
            UserEvent::RenderStats(stats) => {
                self.accumulate_perf_sample(&stats);
                self.video.msaa_samples = stats.msaa_samples;
                // Renderer statistics are asynchronous telemetry, not a settings
                // acknowledgement. A queued sample can be older than a menu/cvar
                // change, so never let it overwrite the requested VSync state.
                let client = self
                    .game_session
                    .as_ref()
                    .map_or_else(ClientFramePerf::default, |playback| playback.client_perf);
                self.perf = PerfStats {
                    fps: stats.fps,
                    frame_ms: stats.frame_ms,
                    cpu_prepare_ms: stats.cpu_prepare_ms,
                    cpu_acquire_ms: stats.cpu_acquire_ms,
                    cpu_encode_ms: stats.cpu_encode_ms,
                    cpu_submit_ms: stats.cpu_submit_ms,
                    cpu_present_ms: stats.cpu_present_ms,
                    dynamic_model_prepare_ms: stats.dynamic_model_prepare_ms,
                    dynamic_model_surfaces: stats.dynamic_model_surfaces,
                    dynamic_model_vertices: stats.dynamic_model_vertices,
                    dynamic_model_indices: stats.dynamic_model_indices,
                    client_total_ms: client.total_ms,
                    client_snapshot_ms: client.snapshot_ms,
                    client_audio_ms: client.audio_ms,
                    client_events_ms: client.events_ms,
                    client_event_prepare_ms: client.event_prepare_ms,
                    client_event_worker_jobs: client.event_worker_jobs,
                    client_event_worker_threads: client.event_worker_threads,
                    client_event_worker_parallel: client.event_worker_parallel,
                    client_event_sound_decode_ms: client.event_sound_decode_ms,
                    client_event_sound_decode_jobs: client.event_sound_decode_jobs,
                    client_event_sound_decode_parallel: client.event_sound_decode_parallel,
                    client_entity_present_ms: client.entity_present_ms,
                    client_player_present_ms: client.player_present_ms,
                    client_followed_player_ms: client.followed_player_ms,
                    client_fx_ms: client.fx_ms,
                    client_fx_tessellate_ms: client.fx_tessellate_ms,
                    client_fx_draws: client.fx_draws,
                    client_fx_sprites: client.fx_sprites,
                    client_fx_oriented_quads: client.fx_oriented_quads,
                    client_fx_lines: client.fx_lines,
                    client_fx_quads: client.fx_quads,
                    client_fx_meshes: client.fx_meshes,
                    client_fx_cylinders: client.fx_cylinders,
                    client_fx_render_surfaces: client.fx_render_surfaces,
                    client_fx_cpu_geom_surfaces: client.fx_cpu_geom_surfaces,
                    client_fx_cpu_vertices: client.fx_cpu_vertices,
                    client_fx_cpu_indices: client.fx_cpu_indices,
                    client_fx_gpu_sprite_batches: client.fx_gpu_sprite_batches,
                    client_fx_gpu_sprite_instances: client.fx_gpu_sprite_instances,
                    ghoul2_pose_ms: client.ghoul2_pose_ms,
                    ghoul2_motion_pose_ms: client.ghoul2_motion_pose_ms,
                    ghoul2_skin_ms: client.ghoul2_skin_ms,
                    ghoul2_bolt_ms: client.ghoul2_bolt_ms,
                    ghoul2_pose_evals: client.ghoul2_pose_evals,
                    ghoul2_motion_pose_evals: client.ghoul2_motion_pose_evals,
                    ghoul2_bolt_queries: client.ghoul2_bolt_queries,
                    ghoul2_surfaces: client.ghoul2_surfaces,
                    ghoul2_vertices: client.ghoul2_vertices,
                    ghoul2_frustum_tests: client.ghoul2_frustum_tests,
                    ghoul2_frustum_culled: client.ghoul2_frustum_culled,
                    ghoul2_lod_counts: client.ghoul2_lod_counts,
                    gpu_ms: stats.gpu_ms,
                    gpu_depth_ms: stats.gpu_depth_ms,
                    gpu_hiz_ms: stats.gpu_hiz_ms,
                    gpu_cull_ms: stats.gpu_cull_ms,
                    gpu_cluster_ms: stats.gpu_cluster_ms,
                    gpu_world_ms: stats.gpu_world_ms,
                    gpu_fx_sprites_ms: stats.gpu_fx_sprites_ms,
                    gpu_post_ms: stats.gpu_post_ms,
                    gpu_ui_ms: stats.gpu_ui_ms,
                    cull_visible: stats.cull_visible,
                    cull_frustum_rejected: stats.cull_frustum_rejected,
                    cull_hiz_rejected: stats.cull_hiz_rejected,
                    cull_pvs_rejected: stats.cull_pvs_rejected,
                    cull_area_rejected: stats.cull_area_rejected,
                    input_event_to_sim_ms: stats.input_event_to_sim_ms,
                    input_sim_to_render_ms: stats.input_sim_to_render_ms,
                    input_event_to_latch_ms: stats.input_event_to_latch_ms,
                    input_latch_to_submit_ms: stats.input_latch_to_submit_ms,
                    input_latch_to_present_call_ms: stats.input_latch_to_present_call_ms,
                    input_event_to_present_call_ms: stats.input_event_to_present_call_ms,
                    input_event_to_present_call_max_ms: stats.input_event_to_present_call_max_ms,
                    input_latency_samples: stats.input_latency_samples,
                };
                self.threads = stats.threads.iter().map(|thread| ThreadPerfStats {
                    id: thread.id,
                    name: thread.name.clone(),
                    task: thread.task,
                    active: thread.active,
                    busy_percent: thread.busy_percent,
                }).collect();
                // Perf/thread counters refresh at 2 Hz. Share the thread array
                // instead of cloning the entire retained UI/console snapshot, and
                // do not send anything when the renderer HUD is not displaying it.
                if self.video.draw_fps != 0 {
                    self.publish_telemetry_ui();
                }
            }
            UserEvent::RendererError(error) => {
                // A failed renderer must reveal diagnostics even without a frame.
                crate::startup_console::reveal();
                crate::logging::activate_console_output();
                self.restore_gamma_before_restart();
                self.renderer_restart_started = None;
                self.show_window_after_restart();
                eprintln!("Renderer: {error}");
                self.console_status = format!("RENDERER ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));

                // Without a fallback, unblock input so the user can quit or
                // recover from console rather than being trapped in restart.
                if self.pending_renderer_restart.is_none()
                    && self.pending_video_confirmation.is_none()
                    && self.video_confirmation.is_none()
                {
                    self.resume_client_after_video_restart();
                }
                // A renderer restart can fail asynchronously after RenderThread::spawn
                // has returned (for example a DXGI surface/configuration failure).
                // Do not leave the user staring at a dead renderer until the normal
                // 15-second confirmation timeout expires; restore the last known-good
                // backend/display mode immediately.
                if self.video_confirmation.is_some() || self.pending_video_confirmation.is_some() {
                    self.revert_video_settings("FAILED");
                    return;
                }
                self.publish_ui();
            }
            UserEvent::SurfaceInspected(mut info) => {
                if let Some(info) = &mut info {
                    self.enrich_surface_inspector(info);
                }
                match info {
                    Some(info) => self.push_trace_entry(info),
                    None => {
                        self.surface_inspector = None;
                        self.publish_ui();
                    }
                }
            }
            UserEvent::MapLoadProgress {
                request_id,
                task,
                completed,
                total,
            } => {
                if request_id & super::render_studio_demo::STUDIO_REQUEST_BIT != 0 {
                    self.studio_map_progress(request_id, task, completed, total);
                    return;
                }
                if request_id != self.latest_request_id {
                    return;
                }
                let mut changed = false;
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.update_worker(task, completed, total);
                        changed = true;
                    }
                }
                if changed {
                    self.publish_ui();
                }
            }
            UserEvent::StaticAoProgress { completed, total } => {
                self.static_ao_progress = if total == 0 || completed >= total {
                    None
                } else {
                    Some((completed, total))
                };
                self.publish_ui();
            }
            UserEvent::SteamAudioScenePrepared {
                request_id,
                map_name,
                result,
            } => {
                if request_id != self.latest_request_id || !self.audio.steam_audio {
                    return;
                }
                match result {
                    Ok(prepared) => {
                        let has_background_bake = prepared.bake_request.is_some();
                        self.steam_audio_acoustic_mesh =
                            Some(Arc::clone(&prepared.acoustic_mesh));
                        self.steam_audio_bake = prepared.bake.clone();
                        self.steam_audio_bake_error = None;
                        if !has_background_bake {
                            self.steam_audio_bake_progress = None;
                        }
                        self.latest_prepare_options.steam_audio = true;
                        if let Some(cache) = &mut self.prepared_map_cache {
                            cache.prepare_options.steam_audio = true;
                            let cached = Arc::make_mut(&mut cache.map);
                            cached.steam_audio_acoustic_mesh =
                                Some(Arc::clone(&prepared.acoustic_mesh));
                            cached.steam_audio_bake = prepared.bake.clone();
                            // The targeted worker owns any live bake request.
                            cached.steam_audio_bake_request = None;
                        }
                        if let Some(sound) = self
                            .game_session
                            .as_mut()
                            .and_then(|session| session.sound_presenter.as_mut())
                        {
                            sound.set_steam_audio_map(
                                Some(Arc::clone(&prepared.acoustic_mesh)),
                                prepared.bake.clone(),
                            );
                        }
                        println!(
                            "{map_name}: Steam Audio current-map acoustic scene attached{}",
                            if prepared.bake.is_some() {
                                " (cached bake loaded)"
                            } else if has_background_bake {
                                " (background bake queued)"
                            } else {
                                ""
                            }
                        );
                    }
                    Err(error) => {
                        self.steam_audio_bake_progress = None;
                        self.steam_audio_bake_error = Some(error.clone());
                        eprintln!(
                            "{map_name}: Steam Audio current-map acoustic preparation failed: {error}"
                        );
                        self.push_console_error(format!(
                            "^1Steam Audio:^7 current-map acoustic preparation failed: {error}"
                        ));
                    }
                }
                self.publish_ui();
            }
            UserEvent::SteamAudioBakeProgress {
                request_id,
                map_name,
                progress,
            } => {
                if request_id != self.latest_request_id {
                    // Its result would be discarded; stop burning cores on it.
                    crate::steam_audio::cancel_background_bake();
                    return;
                }
                if !self.audio.steam_audio {
                    return;
                }
                let progress = progress.clamp(0.0, 1.0);
                self.steam_audio_bake_progress = if progress >= 1.0 {
                    None
                } else {
                    Some(progress)
                };
                self.steam_audio_bake_error = None;
                if progress <= f32::EPSILON {
                    println!(
                        "{map_name}: Steam Audio background bake started; direct current-map acoustics remain active while cached probe data is prepared"
                    );
                }
                self.publish_ui();
            }
            UserEvent::SteamAudioBakeFinished {
                request_id,
                map_name,
                result,
                elapsed_ms,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                self.steam_audio_bake_progress = None;
                match result {
                    Ok(data) => {
                        println!(
                            "{map_name}: Steam Audio background bake finished in {:.1} ms: {} probes / {:.2} MiB / {}",
                            elapsed_ms,
                            data.probe_count,
                            data.serialized_bytes() as f64 / (1024.0 * 1024.0),
                            if data.runtime_validated { "validated" } else { "NOT validated" },
                        );
                        if self.audio.steam_audio {
                            let data = Arc::new(data);
                            self.steam_audio_bake = Some(Arc::clone(&data));
                            self.steam_audio_bake_error = None;
                            if let Some(cache) = &mut self.prepared_map_cache {
                                let cached = Arc::make_mut(&mut cache.map);
                                cached.steam_audio_bake = Some(Arc::clone(&data));
                                cached.steam_audio_bake_request = None;
                            }
                            if let Some(sound) = self
                                .game_session
                                .as_mut()
                                .and_then(|session| session.sound_presenter.as_mut())
                            {
                                sound.set_steam_audio_map(
                                    self.steam_audio_acoustic_mesh.clone(),
                                    Some(data),
                                );
                            }
                        } else {
                            println!(
                                "{map_name}: Steam Audio bake was cached, but s_steamAudio is now off; runtime attachment skipped"
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("{map_name}: Steam Audio background bake failed: {error}");
                        if self.audio.steam_audio {
                            self.steam_audio_bake = None;
                            self.steam_audio_bake_error = Some(error);
                        }
                    }
                }
                self.publish_ui();
            }
            UserEvent::ShaderRemapPrepared {
                request_id, started, name, base_texture_count, result,
            } => {
                if request_id != self.latest_request_id || !self.shader_remap_upload {
                    return; // superseded by a real map load or renderer restart
                }
                // A second command while the worker was resolving targets
                // supersedes its result. Never install stale stage bindings.
                if self.shader_remap_dirty {
                    self.shader_remap_upload = false;
                    if let Err(error) = self.rebuild_shader_remapped_world() {
                        self.push_console_line(format!("^1shader remap: {error}"));
                    }
                    return;
                }
                match result {
                    Ok(plan) => self.render_command(RenderCommand::RemapMap {
                        request_id, started, name, plan, base_texture_count,
                        studio_world: None,
                    }),
                    Err(error) => {
                        self.shader_remap_upload = false;
                        self.push_console_line(format!("^1shader remap: {error}"));
                        self.publish_ui();
                    }
                }
            }
            UserEvent::MapPrepared {
                request_id,
                started,
                name,
                mut map,
                fx_light_promotion,
            } => {
                if self.render_studio.bsp_world.as_ref().is_some_and(|world|world.id==request_id) {
                    self.studio_bsp_prepared(request_id,map);
                    return;
                }
                if request_id & super::render_studio_demo::STUDIO_REQUEST_BIT != 0 {
                    self.on_studio_source_prepared(request_id,map,fx_light_promotion);
                    return;
                }
                if request_id != self.latest_request_id {
                    return;
                }
                let frontend_background =
                    self.front_end && self.frontend_background_request_id == Some(request_id);
                if frontend_background {
                    crate::startup_timeline::mark(crate::startup_timeline::Milestone::MapPrepared);
                    if !self.startup_first_menu_frame_seen {
                        // No GPU resource allocation/uploads before the first
                        // usable menu frame. A later gameplay/map request
                        // cancels this using latest_request_id as usual.
                        self.deferred_frontend_map = Some(UserEvent::MapPrepared {
                            request_id, started, name, map, fx_light_promotion,
                        });
                        return;
                    }
                }
                let map_edit_preview = self.map_edit_preview_request_id == Some(request_id);
                if frontend_background {
                    self.fx_light_promotion_catalog = Arc::clone(&fx_light_promotion);
                    self.menu_props = None;
                }
                if !frontend_background && !map_edit_preview {
                    self.fx_light_promotion_catalog = Arc::clone(&fx_light_promotion);
                    // Live-map preparation can overlap the previous CGame. Only
                    // patch a session that already belongs to this prepared map;
                    // a later GameSession construction reads the app catalog.
                    let session_matches_map = self.game_session.as_ref().is_some_and(|session| {
                        session.map_name.as_deref().is_some_and(|map_name| {
                            Self::normalized_bsp_name(map_name)
                                == Self::normalized_bsp_name(&name)
                        })
                    });
                    if session_matches_map {
                        if let Some(session) = self.game_session.as_mut() {
                            session.set_fx_light_promotion_catalog(Arc::clone(&fx_light_promotion));
                        }
                    }
                }
                if let Some(net) = &self.net {
                    use crate::ocean::authoring::{AuthoredOcean, CS_OCEANS, CS_WEATHER};
                    let strings = &net.session().decoder().configstrings;
                    let mut oceans = Vec::new();
                    for index in 0..8 {
                        if let Some(mut ocean) = strings
                            .get(&(CS_OCEANS + index as u16))
                            .and_then(|s| AuthoredOcean::parse(index, s))
                        {
                            if let Some(weather) = strings.get(&(CS_WEATHER + ocean.weather as u16))
                            {
                                ocean.apply_weather(weather);
                            }
                            oceans.push(ocean);
                        }
                    }
                    if !oceans.is_empty() {
                        let map = map.as_mut();
                        scene::append_authored_ocean_planes(
                            &oceans,
                            &mut map.vertices,
                            &mut map.batches,
                            &mut map.pvs_batches,
                        );
                        map.authored_oceans = oceans;
                    }
                }
                if map_edit_preview {
                    // Source-map preparation rebuilt movement collision from the
                    // same text revision as this render world. Hold it until the
                    // renderer reports WorldUploaded so the player never collides
                    // against geometry that is not the geometry on screen yet.
                    self.map_edit_preview_pending_collision = map.collision.clone();
                    if self.map_edit_preview_pending_collision.is_none() {
                        eprintln!(
                            "Map editor preview {name}: rebuilt source map has no gameplay collision; keeping previous collision world"
                        );
                    }

                    // Do not clone this prepared world into the restart cache on
                    // every mouse gesture. The edit preview is deliberately
                    // transient; cloning the full CPU map here can cost more than
                    // the brush rebuild itself on large source maps.
                    self.triangles = map.triangles;
                    self.render_command(RenderCommand::LoadMap {
                        request_id,
                        started,
                        name,
                        map,
                        studio_world: None,
                    });
                    self.publish_snapshot();
                    return;
                }

                let has_grass = !map.grass_patches.is_empty();
                let has_ocean = self.latest_prepare_options.ocean
                    && prepared_map_has_ocean_surface(map.as_ref());
                let has_acoustics = map.steam_audio_acoustic_mesh.is_some();
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapGrass,
                            has_grass,
                            true,
                        );
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapOcean,
                            has_ocean,
                            false,
                        );
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapAcoustics,
                            has_acoustics,
                            true,
                        );
                    }
                }
                // Baselines include globally sent misc_bsp holders even before
                // the first active snapshot. Reserve their shared-handle copies
                // during the normal upload, avoiding a second upload on join.
                if !frontend_background {
                    let decoder = self
                        .net
                        .as_ref()
                        .map(|net| net.session().decoder())
                        .or_else(|| self.game_session.as_ref().map(|session| &session.decoder));
                    if let Some(decoder) = decoder {
                        let mut counts = std::collections::BTreeMap::new();
                        for entity in decoder.baselines.values() {
                            if entity.field_i32("eType") == Some(crate::cgame::ET_MOVER)
                                && entity.field_i32("solid") == Some(0x00ff_ffff)
                            {
                                if let Some(model) = entity
                                    .field_i32("modelindex")
                                    .and_then(|index| u32::try_from(index).ok())
                                {
                                    *counts.entry(model).or_insert(0) += 1;
                                }
                            }
                        }
                        map.reserve_inline_instances(&counts);
                    }
                }
                // CG_Init applies CS_SHADERSTATE before the first rendered frame.
                // Consume those actions only when this worker result belongs to
                // the cgame that owns them. A speculative live-map prefetch can
                // finish while the previous cgame is still alive; its remap
                // state must never be baked into the prefetched/new world.
                let shader_remap_map_matches_session = !frontend_background
                    && self.game_session.as_ref().is_some_and(|session| {
                        session.map_name.as_deref().is_some_and(|map_name| {
                            Self::normalized_bsp_name(map_name)
                                == Self::normalized_bsp_name(&name)
                        })
                    });
                if shader_remap_map_matches_session {
                    self.consume_server_shader_remaps();
                }

                let cache_started = Instant::now();
                    self.prepared_map_cache = Some(PreparedMapCache {
                        label: name.clone(),
                        prepare_options: self.latest_prepare_options,
                        // Keep the authored/unremapped world as the restart source.
                        // Runtime R_RemapShader changes are presentation state, not
                        // a destructive rewrite of this cache.
                        map: Arc::new(map.as_ref().clone()),
                    });
                rverbose!(
                    2,
                    "Map restart cache: retained prepared CPU map in {:.1} ms",
                    cache_started.elapsed().as_secs_f64() * 1000.0
                );

                if shader_remap_map_matches_session && !self.shader_remaps.is_empty() {
                    if let Err(error) = scene::apply_shader_remaps_in_place(
                        map.as_mut(),
                        &self.base,
                        self.game.as_deref(),
                        self.latest_prepare_options,
                        &self.shader_remaps,
                    ) {
                        self.push_console_line(format!("^1shader remap: {error}"));
                    }
                }
                // The unmodified cache was captured above. Only transform the
                // derived render copy; toggling the cvar later recovers the
                // authored two-pass recipe without BSP/asset re-preparation.
                if self.video.stage_collapse_extra {
                    scene::apply_reverse_lightmap_collapse_in_place(map.as_mut());
                }
                if self.video.additive_stage_fusion {
                    scene::fuse_additive_stage_fusion_in_place(map.as_mut());
                } else {
                    scene::expand_additive_stage_fusion_in_place(map.as_mut());
                }
                if shader_remap_map_matches_session {
                    // This upload now represents the complete remap/reset state
                    // observed at MapPrepared. Later server commands can set the
                    // bit again while this upload is in flight.
                    self.shader_remap_dirty = false;
                }
                // What is resident is what this map was prepared with, whichever
                // renderer instance loaded it. Enabling grass/ocean beyond that
                // needs the map prepared again.
                self.applied_grass = self.latest_prepare_options.grass;
                self.applied_ocean = self.latest_prepare_options.ocean;
                self.forget_trace();
                self.entity_graph = map.entity_graph.clone();
                self.entity_marker_cache = None;
                if map.map_file_stats.is_some() {
                    let loose_map = map.source.extension().is_some_and(|extension| {
                        extension.to_string_lossy().eq_ignore_ascii_case("map")
                    });
                    if loose_map {
                        let should_refresh_editor = self.map_editor.as_ref().is_none_or(|editor| {
                            editor.source_path.as_path() != map.source.as_path()
                                || !editor.has_pending_changes()
                        });
                        if should_refresh_editor {
                            match MapEditor::open(&map.source) {
                                Ok(editor) => self.map_editor = Some(editor),
                                Err(error) => {
                                    eprintln!("Source map editor: {error}");
                                    self.map_editor = None;
                                    self.console_status =
                                        format!("MAP EDITOR UNAVAILABLE: {error}");
                                }
                            }
                        }
                    } else {
                        self.map_editor = None;
                        if self.source_map_edit_on_load {
                            self.console_status = format!(
                                "MAP EDITOR: {} is packaged in {}; save-back requires a loose maps/*.map file",
                                name,
                                map.source.display()
                            );
                            self.push_console_path_line(
                                format!("^3{}", self.console_status),
                                map.source.clone(),
                            );
                        }
                    }
                } else {
                    self.map_editor = None;
                    self.source_map_edit_on_load = false;
                }
                if crate::logging::renderer_verbose_enabled(1) {
                    crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!(
                            "{}: {} triangles, {} draw batches, {} textures, {} lightmap pages; source {}",
                            name,
                            map.triangles,
                            map.batches.len(),
                            map.textures.len(),
                            map.lightmap_pages,
                            map.source.display()
                        ),
                        map.source.clone(),
                    );
                }
                if let Some(distance_cull) = map.distance_cull {
                    rverbose!(
                        1,
                        "{}: worldspawn distanceCull {:.0} map units -> zFar cap {:.0}",
                        name,
                        distance_cull,
                        crate::camera::far_distance_for_cull(distance_cull),
                    );
                } else {
                    rverbose!(
                        1,
                        "{}: no worldspawn distanceCull; using JKA default {:.0} -> zFar cap {:.0}",
                        name,
                        crate::camera::DEFAULT_DISTANCE_CULL,
                        crate::camera::far_distance_for_cull(crate::camera::DEFAULT_DISTANCE_CULL),
                    );
                }
                if let Some(stats) = map.map_file_stats {
                    rverbose!(
                        2,
                        "Source .map: {} entities, {} brushes parsed ({} worldspawn + {} func_group static), {} rendered faces, {} runtime/non-static entity brushes skipped, {} patches skipped, {} degenerate faces, {} invalid brushes skipped",
                        stats.entities,
                        stats.brushes,
                        stats.world_brushes,
                        stats.grouped_world_brushes,
                        stats.rendered_faces,
                        stats.skipped_entity_brushes,
                        stats.patches_skipped,
                        stats.degenerate_faces,
                        stats.skipped_brushes,
                    );
                    rverbose!(
                        2,
                        "Source .map runtime prep: {} utility faces dropped, {} spatial chunks, {} geometry groups -> {} WGPU draw batches, spatial batching={}; {} gameplay collision brushes; brush reconstruction {:.2} ms on {} map worker(s)",
                        stats.utility_faces_skipped,
                        stats.spatial_chunks,
                        stats.geometry_groups,
                        stats.draw_batches,
                        stats.spatial_batching,
                        stats.collision_brushes,
                        stats.reconstruction_ms,
                        stats.worker_count,
                    );
                }
                for warning in &map.warnings {
                    if map.map_file_stats.is_some() {
                        println!("Warning: {warning}");
                    } else {
                        rverbose!(1, "Material: {warning}");
                    }
                }
                self.live_without_world = false;
                self.demo_without_world = false;
                if self.map_name != name {
                    self.promoted_oceans.clear();
                    self.promoted_ocean_overrides.clear();
                    self.ocean_settings_target = -1;
                }
                self.map_name = name.clone();
                self.triangles = map.triangles;
                self.map_distance_cull = map
                    .distance_cull
                    .unwrap_or(crate::camera::DEFAULT_DISTANCE_CULL);
                self.map_authored_sun = map.sun;
                self.map_authored_oceans = map.authored_oceans.clone();
                self.authored_ocean_preview = false;
                self.refresh_authored_oceans();
                self.map_collision = map.collision.clone();
                self.map_mark_surfaces = map.mark_surfaces.clone();
                self.map_static_models = Arc::clone(&map.static_models);
                self.map_visibility = map.visibility.clone();
                self.map_physics_collision = map.physics_collision.clone();
                self.physics_mesh_label = self
                    .latest_prepare_options
                    .client_physics
                    .then(|| name.clone());
                self.map_movement = map.movement.clone();
                self.steam_audio_acoustic_mesh = map.steam_audio_acoustic_mesh.clone();
                self.steam_audio_bake = map.steam_audio_bake.clone();
                self.steam_audio_bake_progress = None;
                self.steam_audio_bake_error = None;
                let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
                let bake = self.steam_audio_bake.clone();
                if let Some(session) = self.game_session.as_mut() {
                    if let Err(error) = session
                        .player_presenter
                        .set_physics_map_mesh(&self.map_physics_collision)
                    {
                        eprintln!("RAPIER MAP COLLISION ERROR: {error}");
                    }
                    session
                        .weapon_fx
                        .set_collision_world(self.map_collision.clone());
                    session
                        .weapon_fx
                        .set_mark_surfaces(self.map_mark_surfaces.clone());
                    session
                        .entity_presenter
                        .set_static_models(Arc::clone(&self.map_static_models));
                    session
                        .player_presenter
                        .set_collision_world(self.map_collision.clone());
                    session
                        .weapon_fx
                        .set_saber_impact_fx(self.video.saber_impact_fx);
                    session.weapon_fx.set_saber_marks(self.video.saber_marks);
                    if let Some(sound) = session.sound_presenter.as_mut() {
                        sound.set_steam_audio_map(acoustic_mesh, bake);
                        sound.set_eal_map(&name);
                    }
                }
                self.third_person_camera.reset();
                self.solo_dynamic_models = Arc::new(Vec::new());
                if frontend_background {
                    self.preserve_game_state_on_next_map_upload = false;
                    self.local_server = None;
                    self.spawns = map.spawns.clone();
                    self.spawn_index = 0;
                    let stage = scene::menu_stage(&name);
                    if let Some(spawn) = self.spawns.first().copied() {
                        let mut position = spawn.position;
                        position[1] += FRONTEND_CAMERA_EYE_HEIGHT;
                        let mut yaw = spawn.yaw;
                        let mut pitch = (-2.0_f32).to_radians();
                        if let Some(stage) = stage {
                            // Authored staging replaces the spawn-point camera.
                            position = scene::render_position(stage.camera);
                            yaw = stage.camera_yaw.to_radians();
                            pitch = (-stage.camera_pitch).to_radians();
                        }
                        // Preserve the configured gameplay FOV in App::camera;
                        // render_snapshot gives this scene its own fixed FOV.
                        // Replacing the stored value here would overwrite the
                        // console/UI cg_fov getter and the archived setting.
                        self.camera =
                            Camera::new_with_fov(position, yaw, self.camera.cg_fov());
                        self.frontend_cinematic = Some(FrontendCinematic {
                            position,
                            yaw,
                            pitch,
                            anim_started: Instant::now(),
                            world_ready_at: None,
                            model_visible_since: None,
                            pose_hold_ms: None,
                            pose_locked_at: None,
                            fade_started: None,
                        });
                    } else {
                        self.frontend_cinematic = None;
                    }
                    self.keys.clear();
                    self.movement_keys.clear();
                    self.live_buttons.clear();
                    self.live_button_sources.clear();
                    self.japro_zoom_transition_at = None;
                    self.japro_flipkick_frames = 0;
                    self.japro_flipkick_jumps = 0;
                    self.japro_flipkick_moveup = false;
                    self.mouse_buttons_down.clear();
                    self.noclip_primary_down = false;
                    self.noclip_alt_down = false;
                    self.last_mouse_motion_at = None;
                    self.previous_tick = Instant::now();
                    self.console_status = format!("MAIN MENU BACKGROUND: {name}");
                    println!("Frontend cinematic: loaded {name} with no local player/session");
                } else if self.preserve_game_state_on_next_map_upload {
                    self.preserve_game_state_on_next_map_upload = false;
                } else if self.game_session.is_some() {
                    // Demo/network sessions already own their authoritative state.
                    // Never create a second local authority while their map uploads.
                    self.local_server = None;
                    self.spawns = map.spawns.clone();
                    self.spawn_index = 0;
                } else {
                    self.spawns = map.spawns.clone();
                    // Same pick JKA makes for a local client's first spawn; the
                    // spectator view and the first Join both use it.
                    self.spawn_index = crate::scene::select_spawn_index(
                        &self.spawns,
                        [0.0; 3],
                        true,
                        random_unit(),
                    )
                    .unwrap_or(0);
                    self.solo_initial_spawn_pending = true;
                    // A local server is a new client connection from the input
                    // layer's point of view. Do not inherit selection or
                    // one-shot button state from a previous game.
                    self.live_input = crate::net::LiveInput::default();
                    self.live_provisional = None;
                    if let Some(spawn) = self.spawns.get(self.spawn_index).copied() {
                        self.camera =
                            Camera::new_with_fov(spawn.position, spawn.yaw, self.camera.cg_fov());
                        let saber_movement =
                            self.load_local_saber_movement().unwrap_or_else(|error| {
                                eprintln!("{error}; using OpenJK default local saber metadata");
                                [
                                    jka_movement::SaberMovementInfo::equipped_default(),
                                    jka_movement::SaberMovementInfo::default(),
                                ]
                            });
                        match LocalServer::new(
                            &name,
                            map.movement.clone(),
                            map.collision.clone(),
                            spawn,
                            &self.solo_client_info,
                            saber_movement,
                            &map.fx_runners,
                            &map.world_effect_registrations,
                            &map.brush_entities,
                            map.worldspawn_music.as_deref(),
                            map.visibility
                                .as_ref()
                                .map(jka_assets::bsp::Visibility::area_locator),
                        ) {
                            Ok(mut server) => {
                                server.set_char_color(self.network.char_color);
                                server.set_mouse_input_settings(self.mouse_input);
                                if let Err(error) =
                                    server.set_command_rate(self.network.command_rate)
                                {
                                    self.console_status = format!("COMMAND RATE ERROR: {error}");
                                }
                                server.camera(&mut self.camera);
                                match self.build_local_game_session(&server) {
                                    Ok(mut session) => {
                                        if let Some(snapshot) = server.take_snapshot(true) {
                                            session.snapshots = 1;
                                            session.live_snapshots.push_back(snapshot);
                                        }
                                        devprintln!(
                                            1,
                                            "LOCAL SERVER: shim ready for {} (decoded snapshot/CGame boundary)",
                                            server.map_name()
                                        );
                                        self.local_server = Some(server);
                                        self.game_session = Some(session);
                                    }
                                    Err(error) => {
                                        self.local_server = None;
                                        self.game_session = None;
                                        eprintln!("Local CGame setup: {error}");
                                        self.console_status = format!("LOCAL CGAME ERROR: {error}");
                                    }
                                }
                            }
                            Err(error) => {
                                self.local_server = None;
                                eprintln!("Local server setup: {error}");
                            }
                        }
                        self.apply_pending_screenshot_jump(&name);
                        self.keys.clear();
                        self.movement_keys.clear();
                        self.live_buttons.clear();
                        self.live_button_sources.clear();
                        self.japro_zoom_transition_at = None;
                        self.japro_flipkick_frames = 0;
                        self.japro_flipkick_jumps = 0;
                        self.japro_flipkick_moveup = false;
                        self.mouse_buttons_down.clear();
                        self.noclip_primary_down = false;
                        self.noclip_alt_down = false;
                        self.last_mouse_motion_at = None;
                        self.previous_tick = Instant::now();
                    }
                }
                if self.pending_frontend_map_launch {
                    self.pending_frontend_map_launch = false;
                    self.frontend_background_request_id = None;
                    self.frontend_cinematic = None;
                    self.front_end = false;
                    self.frontend_page = FrontendPage::Main;
                    self.menu_selected = 0;
                    self.set_overlay(OverlayMode::None);
                    self.sync_render_fps_cap();
                }
                if self.source_map_edit_on_load {
                    self.source_map_edit_on_load = false;
                    if self.map_editor.is_some() {
                        self.set_overlay(OverlayMode::MapEdit);
                    }
                }
                self.apply_distance_cull();
                self.console_status = format!("UPLOADING {name} TO RENDER THREAD...");
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.begin_upload();
                    }
                }
                self.publish_ui();
                // A deferred enable (grass/ocean staged while this map lacked the
                // data) was never sent to the renderer; deliver the current
                // choice before the upload so the new world is built with it.
                self.sync_world_flares();
                self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                // Apply cg_waterwake gate on every map load / renderer restart,
                // including archived settings loaded before session creation.
                self.normalize_ocean_settings();
                self.render_command(RenderCommand::SetOceanSettings(self.effective_ocean_settings()));
                self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                // Apply the configured effects before upload so the world is
                // compiled for their scene format on its first upload.
                self.sync_post_effects();
                let studio_world = None;
                self.render_command(RenderCommand::LoadMap {
                    request_id,
                    started,
                    name,
                    map,
                    studio_world,
                });
                self.publish_snapshot();
            }
            UserEvent::PhysicsMeshReady { label, result } => {
                self.finish_physics_mesh(label, result);
            }
            UserEvent::MapFailed {
                request_id,
                started,
                name,
                error,
            } => {
                if self.render_studio.bsp_world.as_ref().is_some_and(|world|world.id==request_id) {
                    self.studio_bsp_ready(request_id,Some(error));
                    return;
                }
                if request_id & super::render_studio_demo::STUDIO_REQUEST_BIT != 0 {
                    self.on_studio_source_ready(request_id,Some(error),None,usize::MAX);
                    return;
                }
                if request_id != self.latest_request_id {
                    return;
                }
                if self.map_edit_preview_request_id == Some(request_id) {
                    self.map_edit_preview_request_id = None;
                    self.map_edit_preview_pending_collision = None;
                    let needs_newest = self.map_edit_preview_dirty;
                    self.map_edit_preview_dirty = false;
                    let message = format!("Textured brush preview failed: {error}");
                    eprintln!("Map editor preview {name}: {error}");
                    if let Some(editor) = self.map_editor.as_mut() {
                        editor.status = message.clone();
                    }
                    self.console_status = message;
                    // If another drag/external save landed while this failed
                    // document was being built, do not lose it. The existing
                    // preview coalescer should immediately chase the newest
                    // working source just as it does after a successful upload.
                    if needs_newest {
                        self.queue_map_edit_preview();
                    }
                    self.egui_repaint_requested = true;
                    return;
                }
                self.pending_frontend_map_launch = false;
                self.preserve_game_state_on_next_map_upload = false;
                self.renderer_restart_started = None;
                if self.pending_video_confirmation.is_none() && self.video_confirmation.is_none() {
                    self.resume_client_after_video_restart();
                }
                eprintln!(
                    "Map load failed for {name} after {:.1} ms: {error}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.loading = None;

                if self.front_end && self.frontend_background_request_id == Some(request_id) {
                    self.frontend_background_request_id = None;
                    self.frontend_cinematic = None;
                    self.console_status = format!("MAIN MENU - 3D BACKGROUND UNAVAILABLE: {error}");
                    self.push_console_line(format!(
                        "^3Frontend background {name} could not be loaded; keeping the menu usable without it."
                    ));
                    self.publish_ui();
                    return;
                }

                let live_map = self
                    .game_session
                    .as_ref()
                    .filter(|session| session.live && !session.local)
                    .and_then(|session| session.map_name.clone());
                if let Some(map_name) = live_map {
                    if self.network.allow_missing_map {
                        self.prime_live_session_without_world(&map_name, &error);
                    } else {
                        self.console_status = format!("MAP LOAD FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                        self.push_console_line(
                            "^3Live session closed cleanly; set cl_allowMissingMap 1 to continue without the BSP."
                                .to_owned(),
                        );
                        self.disconnect_to_main_menu();
                    }
                    return;
                }

                self.game_session = None;
                self.console_status = format!("MAP LOAD FAILED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                if self.shader_remap_upload {
                    // Runtime R_RemapShader rebuilds are presentation-only
                    // world swaps. Do not replay real map-load lifecycle work
                    // (demo clocks, join timing, video confirmation, physics,
                    // or map-loaded console noise) after their first frame.
                    self.loading = None;
                    self.shader_remap_upload = false;
                    self.publish_ui();
                    if self.shader_remap_dirty {
                        if let Err(error) = self.rebuild_shader_remapped_world() {
                            self.push_console_line(format!("^1shader remap: {error}"));
                        }
                    }
                    return;
                }
                if self.video_confirmation.is_some() || self.pending_video_confirmation.is_some() {
                    self.revert_video_settings("FAILED");
                    return;
                }
                self.publish_ui();
            }
            UserEvent::WorldUploaded {
                request_id,
                name,
                triangles,
                batches,
                upload_ms,
                upload_timings,
                first_frame_ms,
                timings,
                bsp_stats,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                if self.shader_remap_upload {
                    // A resident material-stage update is not a new world. In
                    // particular do not print [MAP LOAD]/LOADED, restart demo
                    // clocks, re-enter map physics, reconfigure UI, or publish
                    // zero-filled BSP upload counters. The remap render command
                    // remains synchronized to the next successful frame.
                    if upload_timings.remap_full_fallback {
                        // Rare material changes that introduce world geometry
                        // still rebuild. The flag must be visible rather than
                        // falsely claiming the resident path succeeded.
                        self.coastal_debug_sent = None;
                        self.coastal_live_sent = None;
                        self.promoted_oceans = upload_timings.promoted_oceans.clone();
                        self.promoted_ocean_overrides.clear();
                        self.render_command(RenderCommand::SetPromotedOceanSettings(
                            self.promoted_ocean_overrides.clone(),
                        ));
                        rverbose!(1,
                            "[SHADER REMAP] full compatibility reconstruction {:.2} ms; first visible frame {:.2} ms",
                            upload_ms, first_frame_ms,
                        );
                    } else {
                        rverbose!(1,
                            "[SHADER REMAP] resident update {:.2} ms; first visible frame {:.2} ms; no BSP upload",
                            upload_ms, first_frame_ms,
                        );
                    }
                    self.shader_remap_upload = false;
                    self.publish_ui();
                    if self.shader_remap_dirty {
                        if let Err(error) = self.rebuild_shader_remapped_world() {
                            self.push_console_line(format!("^1shader remap: {error}"));
                        }
                    }
                    return;
                }
                // The renderer clears map-relative debug geometry on every
                // upload, including reloads whose promoted descriptors match.
                self.coastal_debug_sent = None;
                self.coastal_live_sent = None;
                if self.promoted_oceans != upload_timings.promoted_oceans {
                    self.promoted_oceans = upload_timings.promoted_oceans.clone();
                    self.promoted_ocean_overrides.clear();
                    let eye = self.camera.position.to_array();
                    self.ocean_settings_target = self.promoted_oceans.iter().min_by(|a,b| {
                        let distance = |p: &crate::ocean::appearance::PromotedOcean| {
                            let s = p.surface;
                            (eye[0]-eye[0].clamp(s.minimum[0],s.maximum[0])).powi(2)
                                +(eye[2]-eye[2].clamp(s.minimum[1],s.maximum[1])).powi(2)
                                +(eye[1]-s.plane_height).powi(2)
                        };
                        distance(a).total_cmp(&distance(b))
                    }).map_or(-1, |p| p.index as i32);
                }
                self.render_command(RenderCommand::SetPromotedOceanSettings(self.promoted_ocean_overrides.clone()));
                if self.map_edit_preview_request_id == Some(request_id) {
                    if let Some(collision) = self.map_edit_preview_pending_collision.take() {
                        self.map_collision = Some(collision.clone());
                        if let Some(server) = self.local_server.as_mut() {
                            // This swaps only the immutable movement world. Player
                            // position, command time, saber state and server clock
                            // stay exactly where they were.
                            server.replace_collision_world(collision);
                        }
                        if let Some(session) = self.game_session.as_mut() {
                            // Keep presentation traces (saber marks/FX and player
                            // ground traces) on the same collision revision as both
                            // the renderer and the local movement server.
                            session
                                .weapon_fx
                                .set_collision_world(self.map_collision.clone());
                            session
                                .player_presenter
                                .set_collision_world(self.map_collision.clone());
                        }
                    }
                    self.map_edit_preview_request_id = None;
                    let needs_newest = self.map_edit_preview_dirty;
                    self.map_edit_preview_dirty = false;
                    if needs_newest {
                        // The cursor advanced while this world was building. It
                        // is safe to show this intermediate textured state, then
                        // immediately chase the newest snapped brush position.
                        self.queue_map_edit_preview();
                    } else if let Some(editor) = self.map_editor.as_mut() {
                        editor.mark_preview_synced();
                    }
                    self.egui_repaint_requested = true;
                    self.publish_snapshot();
                    self.publish_ui();
                    return;
                }
                let frontend_background_first_frame =
                    self.frontend_background_request_id == Some(request_id);
                if frontend_background_first_frame {
                    crate::startup_timeline::mark(crate::startup_timeline::Milestone::FirstMapPresent);
                    crate::startup_timeline::report("first_map");
                    self.frontend_background_request_id = None;
                    if let Some(cinematic) = self.frontend_cinematic.as_mut() {
                        // The BSP has presented, but the requested Ghoul2 model
                        // can still be an async fallback. Readiness is checked
                        // after the real player presenter submits its geometry.
                        cinematic.world_ready_at = Some(Instant::now());
                    }
                    self.egui_repaint_requested = true;
                }
                let join_first_world = self.live_join_timing.is_some()
                    && (self
                        .live_join_timing
                        .as_ref()
                        .and_then(|timing| timing.prefetched_map.as_deref())
                        .is_some_and(|map| {
                            Self::normalized_bsp_name(map) == Self::normalized_bsp_name(&name)
                        })
                        || self.game_session.as_ref().is_some_and(|session| {
                            session.live
                                && session.map_name.as_deref().is_some_and(|map| {
                                    Self::normalized_bsp_name(map)
                                        == Self::normalized_bsp_name(&name)
                                })
                        }));
                if join_first_world {
                    // A speculative first render does not prove server admission.
                    // The handshake overlay remains until the active snapshot.
                    self.egui_repaint_requested = true;
                    let elapsed = self
                        .live_join_timing
                        .as_ref()
                        .map(LiveJoinTiming::elapsed_ms)
                        .unwrap_or(0.0);
                    if let Some(timing) = self.live_join_timing.as_mut() {
                        timing.first_world_frame_ms = Some(elapsed);
                    }
                    rverbose!(
                        1,
                        "[JOIN] connect -> first rendered world frame {elapsed:.1} ms ({name})"
                    );
                    if crate::logging::renderer_verbose_enabled(1) {
                        self.push_console_line(format!(
                            "^5[JOIN]^7 connect -> first rendered world frame {elapsed:.1} ms"
                        ));
                    }
                }
                if let Some(restart_started) = self.renderer_restart_started.take() {
                    let total_ms = restart_started.elapsed().as_secs_f64() * 1000.0;
                    rverbose!(
                        1,
                        "[VID_RESTART] backend/display restart to first world frame {:.1} ms",
                        total_ms
                    );
                    if crate::logging::renderer_verbose_enabled(1) {
                        self.push_console_line(format!(
                            "^5[VID_RESTART]^7 total {:.1} ms to first world frame",
                            total_ms
                        ));
                    }
                }
                if crate::logging::renderer_verbose_enabled(1) {
                    self.push_console_line(format!(
                        "^5[MAP LOAD]^7 {name} {:.1} ms first-frame | prep {:.1} | bsp {:.1} | collision {:.1} | shaders {:.1} | textures decode {:.1} | geometry {:.1} | renderer setup {:.1} | workers {}",
                        first_frame_ms,
                        timings.prepare_wall_ms,
                        timings.bsp_parse_ms,
                        timings.collision_ms,
                        timings.shader_parse_wall_ms,
                        timings.texture_decode_ms,
                        timings.geometry_ms,
                        upload_ms,
                        timings.worker_count,
                    ));
                }
                // These spans come from different threads and can overlap.
                // This residual includes handshake/event-loop/snapshot scheduling,
                // not just GPU queue or swapchain presentation waits.
                let first_frame_unattributed_ms =
                    (first_frame_ms - timings.prepare_wall_ms - upload_ms).max(0.0);
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP FIRST FRAME]^7 prep wall {:.1} ms | renderer setup {:.1} ms | unaccounted/overlap {:.1} ms (includes connection/UI, NOT GPU wait)",
                        timings.prepare_wall_ms,
                        upload_ms,
                        first_frame_unattributed_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP PREP DETAIL]^7 grass wall {:.1} ms | grass worker CPU {:.1} ms | surface sprites wall {:.1} ms (worker CPU {:.1}) | gi {:.1} ms | ocean mesh {:.1} ms | acoustics {:.1} ms | PVS plans {:.1} ms",
                        timings.grass_ms,
                        timings.grass_cpu_ms,
                        timings.surface_sprites_ms,
                        timings.surface_sprites_cpu_ms,
                        timings.gi_ms,
                        timings.ocean_ms,
                        timings.steam_audio_ms,
                        timings.portal_plans_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP PREP STAGES]^7 asset index {:.1} | bsp read {:.1} | movement {:.1} | shader read {:.1} | shader parse wall {:.1} (cpu {:.1}) | lightmaps {:.1} | materials {:.1} | geometry {:.1} | tex preload wall {:.1} (read {:.1}, decode {:.1}, mip {:.1}, {} image(s)) | portal plans {:.1} | generated normals {:.1} ms ({} map(s))",
                        timings.asset_index_ms,
                        timings.bsp_read_ms,
                        timings.movement_ms,
                        timings.shader_read_ms,
                        timings.shader_parse_wall_ms,
                        timings.shader_parse_cpu_ms,
                        timings.lightmap_ms,
                        timings.material_ms,
                        timings.geometry_ms,
                        timings.texture_preload_wall_ms,
                        timings.texture_read_ms,
                        timings.texture_decode_ms,
                        timings.texture_mip_ms,
                        timings.texture_images,
                        timings.portal_plans_ms,
                        timings.generated_normal_ms,
                        timings.generated_normals,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(3) {
                    self.push_console_line(format!(
                        "^6[MAP PREP TIMELINE]^7 loader thread, in order: {}",
                        scene::PREP_PHASES
                            .iter()
                            .zip(timings.phase_ms)
                            .map(|(label, ms)| format!("{label} {ms:.1}"))
                            .collect::<Vec<_>>()
                            .join(" | "),
                    ));
                }
                let detail = timings.geometry_detail_ms;
                if crate::logging::renderer_verbose_enabled(3) {
                    self.push_console_line(format!(
                        "^6[MAP PREP GEOMETRY]^7 dlight surfaces {:.1} | surface walk {:.1} wall (PVS signatures {:.1} CPU summed over threads) | piece pack {:.1} (ordering {:.1} summed over groups) | archive handles opened {} in {:.1} ms (summed over threads)",
                        detail[0],
                        detail[1],
                        detail[2],
                        detail[3],
                        detail[4],
                        timings.archive_opens,
                        timings.archive_open_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP TEXTURE DECODE]^7 tga {} image(s) {:.1} ms | jpg {} image(s) {:.1} ms | png {} image(s) {:.1} ms (CPU summed over workers)",
                        timings.texture_format_images[0],
                        timings.texture_format_decode_ms[0],
                        timings.texture_format_images[1],
                        timings.texture_format_decode_ms[1],
                        timings.texture_format_images[2],
                        timings.texture_format_decode_ms[2],
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[OCEAN MESH]^7 promoted water surfaces {} | clipmap {} verts / {} tris, inner cell {:.0}u, outer cell {:.0}u",
                        upload_timings.ocean_water_batches,
                        upload_timings.ocean_clipmap_vertices,
                        upload_timings.ocean_clipmap_tris,
                        upload_timings.ocean_clipmap_spacing,
                        upload_timings.ocean_coarsest_spacing,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP CPU/GPU SPLIT]^7 vertex CPU {:.1} / wgpu calls {:.1} ms | material classify {:.1} | BC colour worker {:.1} (wait {:.1}) | BC lightmap worker {:.1} (wait {:.1}) | texture wgpu calls {:.1} | lightmap wgpu calls {:.1} ms; calls include driver CPU time, NOT GPU execution",
                        upload_timings.vertex_cpu_ms,
                        upload_timings.vertex_gpu_calls_ms,
                        upload_timings.texture_classify_ms,
                        upload_timings.texture_encode_cpu_ms,
                        upload_timings.texture_encode_wait_ms,
                        upload_timings.lightmap_encode_cpu_ms,
                        upload_timings.lightmap_encode_wait_ms,
                        upload_timings.texture_gpu_calls_ms,
                        upload_timings.lightmap_gpu_calls_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    let texture_cpu_other_ms = (upload_timings.texture_upload_ms
                        - upload_timings.skybox_average_ms
                        - upload_timings.texture_encode_wait_ms
                        - upload_timings.texture_gpu_calls_ms).max(0.0);
                    self.push_console_line(format!(
                        "^6[MAP SKYBOX AVERAGE]^7 CPU {:.1} ms | {} primary-sky face slot(s), {} unique image(s) | texture other CPU {:.1} ms",
                        upload_timings.skybox_average_ms,
                        upload_timings.skybox_face_references,
                        upload_timings.skybox_unique_images,
                        texture_cpu_other_ms,
                    ));
                }
                let upload_accounted_ms = upload_timings.vertex_buffer_ms
                    + upload_timings.texture_classify_ms
                    + upload_timings.texture_upload_ms
                    + upload_timings.lightmap_upload_ms
                    + upload_timings.reflection_probe_upload_ms
                    + upload_timings.batch_bind_groups_ms
                    + upload_timings.cull_resources_ms
                    + upload_timings.lighting_resources_ms
                    + upload_timings.pipeline_create_ms
                    + upload_timings.visibility_tables_ms
                    + upload_timings.grass_upload_ms
                    + upload_timings.finalize_ms
                    + upload_timings.pre_build_ms
                    + upload_timings.snow_shell_ms
                    + upload_timings.prelude_ms
                    + upload_timings.post_bind_groups_ms
                    + upload_timings.post_misc_ms
                    + upload_timings.frame_plan_ms
                    + upload_timings.variant_ms;
                let upload_other_ms = (upload_ms - upload_accounted_ms).max(0.0);
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP RENDER-UPLOAD 2]^7 load_map pre-build {:.1} | snow shell {:.1} | CPU copies (inspector/AO) {:.1} | cull+froxel bind groups {:.1} | videos/lights/AO request/weather/fog {:.1} | planar+frame plan {:.1} | pipeline variant {:.1}",
                        upload_timings.pre_build_ms,
                        upload_timings.snow_shell_ms,
                        upload_timings.prelude_ms,
                        upload_timings.post_bind_groups_ms,
                        upload_timings.post_misc_ms,
                        upload_timings.frame_plan_ms,
                        upload_timings.variant_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP RENDER-UPLOAD]^7 vertex {:.1} | textures {:.1} | lightmaps {:.1} | probes {:.1} | batches {:.1} | cull {:.1} | lighting {:.1} | pipelines {:.1} | visibility {:.1} | grass {:.1} | finalize {:.1} | other {:.1}",
                        upload_timings.vertex_buffer_ms,
                        upload_timings.texture_upload_ms,
                        upload_timings.lightmap_upload_ms,
                        upload_timings.reflection_probe_upload_ms,
                        upload_timings.batch_bind_groups_ms,
                        upload_timings.cull_resources_ms,
                        upload_timings.lighting_resources_ms,
                        upload_timings.pipeline_create_ms,
                        upload_timings.visibility_tables_ms,
                        upload_timings.grass_upload_ms,
                        upload_timings.finalize_ms,
                        upload_other_ms,
                    ));
                }
                if let (Some(stats), Some(sound)) = (
                    bsp_stats.as_ref(),
                    self.game_session
                        .as_mut()
                        .and_then(|playback| playback.sound_presenter.as_mut()),
                ) {
                    sound.set_inline_model_midpoints(Arc::clone(&stats.inline_model_midpoints));
                }
                if let (Some(stats), Some(playback)) =
                    (bsp_stats.as_ref(), self.game_session.as_mut())
                {
                    playback
                        .weapon_fx
                        .set_inline_model_bounds(Arc::clone(&stats.inline_model_bounds));
                }
                if let Some(stats) = bsp_stats {
                    if crate::logging::renderer_verbose_enabled(2) {
                        self.push_console_line(format!(
                            "^5[MAP STATS]^7 brushes {} | brushsides {} | clipping planes {} | surfaces {} | bsp verts {} | bsp indices {} | render tris {} | batches {} | nodes {} | leaves {} | clusters {}",
                            stats.brushes, stats.brush_sides, stats.planes, stats.surfaces,
                            stats.vertices, stats.indices, triangles, batches, stats.collision_nodes,
                            stats.collision_leaves, stats.pvs_clusters,
                        ));
                    }
                }
                self.console_status = format!("{} {name}: {:.3} seconds",
                    if upload_timings.studio_world_reused { "RESTORED" } else { "LOADED" },
                    first_frame_ms / 1000.0);
                self.push_console_line(format!("^2{}", self.console_status));
                self.loading = None;
                // Runtime shader-remap uploads go straight to the render thread
                // and therefore finish here (there is no MapPrepared event).
                self.shader_remap_upload = false;
                // Physics may have been enabled while this map was loading.
                self.ensure_map_physics_mesh();

                if self.game_session.as_ref().is_some_and(|session| {
                    session.live
                        && session.map_name.as_deref().is_some_and(|map| {
                            Self::normalized_bsp_name(map) == Self::normalized_bsp_name(&name)
                        })
                }) && self.front_end
                {
                    self.pending_frontend_map_launch = false;
                    self.front_end = false;
                    self.frontend_page = FrontendPage::Main;
                    self.menu_selected = 0;
                    self.sync_render_fps_cap();
                }

                // OpenJK deliberately avoids advancing a demo across level-load
                // latency. Our equivalent boundary is WorldUploaded: the render
                // thread has produced the first real world frame, so initialize
                // the demo clock from the first snapshot now.
                self.start_demo_after_map_load(&name);
                if self.studio_demo_scene_active() {
                    self.render_command(RenderCommand::SetAssetPreviewMode(false));
                }

                // The world is now actually visible. Restore gameplay/audio
                // before the separate keep/revert confirmation window begins.
                self.resume_client_after_video_restart();
                // Start the keep/revert clock only after a real world frame has
                // been rendered. This gives the user the full timeout to inspect
                // the new mode instead of spending it on adapter/device/pipeline
                // creation and map upload.
                self.begin_pending_video_confirmation();
                self.publish_ui();

                // A shader remap can arrive before the pristine world exists or
                // while a prior remap upload is in flight. Collapse all such
                // changes into one fresh cold-path rebuild now that the renderer
                // has completed the current upload.
                if self.shader_remap_dirty {
                    if let Err(error) = self.rebuild_shader_remapped_world() {
                        self.push_console_line(format!("^1shader remap: {error}"));
                    }
                }
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let companion_matches = self
            .companion
            .as_ref()
            .is_some_and(|companion| companion.window.id() == window_id);
        if companion_matches {
            self.handle_companion_window_event(window_id, event);
            return;
        }
        let window_matches = match self.window.as_ref() {
            Some(window) => window.id() == window_id,
            None => false,
        };
        if !window_matches {
            return;
        }
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::Input,
        );

        if self.renderer_restart_suspended {
            // Still service native window/display events so wgpu surface
            // replacement and CloseRequested continue working. Ignore ALL
            // gameplay/UI keyboard, mouse and wheel actions (including Alt+Enter).
            if let WindowEvent::Focused(focused) = &event {
                self.window_focused = *focused;
                self.sync_gamma_focus();
                self.set_capture(false);
                return;
            }
            if matches!(&event,
                WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::ModifiersChanged(..)
            ) {
                return;
            }
        }

        if self.video_confirmation.is_some()
            && matches!(&event,
                WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::CursorMoved { .. }
            )
        {
            return;
        }

        if self.studio_navigation_window_event(&event) { return; }

        // A focused Remap Editor text field owns *every* typed key before
        // global shortcuts and Dev-bind dismissal. Otherwise a shader path
        // containing a key bound to /dev (or the console toggle) would close
        // the editor instead of entering the character. Escape remains the
        // explicit menu-close key; without text focus Dev still means Back.
        if self.overlay == OverlayMode::ShaderRemap
            && self.egui_ctx.memory(|m| m.focused().is_some())
            && matches!(
                &event,
                WindowEvent::KeyboardInput { event: key, .. }
                    if key.physical_key != PhysicalKey::Code(KeyCode::Escape)
            )
        {
            let _ = self.route_egui_window_event(&event);
            return;
        }

        // Pinned search is a modal text-input owner. Process navigation here,
        // before global binds, and send every other keystroke to egui. While
        // it is pinned the original emote key can be released safely.
        if self.overlay == OverlayMode::EmoteRadial {
            if let WindowEvent::KeyboardInput { event: key, .. } = &event {
                if let PhysicalKey::Code(code) = key.physical_key {
                    if key.state == ElementState::Pressed && code == KeyCode::Tab {
                        if !key.repeat { self.toggle_emote_radial_search(); }
                        return;
                    }
                    if self.emote_radial_search.pinned {
                        if self.key_bound_to_emote(code) && key.state == ElementState::Released {
                            self.keys.remove(&code);
                            self.execute_binding_release_from(
                                keybinds::bind_key_for_code(code), BindKey::Keyboard(code));
                        }
                        if key.state == ElementState::Pressed {
                            match code {
                                KeyCode::Escape if !key.repeat => { self.close_emote_radial(); return; }
                                KeyCode::Enter | KeyCode::NumpadEnter if !key.repeat => {
                                    self.activate_emote_radial_search_selection(); return;
                                }
                                KeyCode::ArrowUp => { self.emote_radial_search_move(-1); return; }
                                KeyCode::ArrowDown => { self.emote_radial_search_move(1); return; }
                                _ => {}
                            }
                        }
                        let _ = self.route_egui_window_event(&event);
                        return;
                    }
                } else if self.emote_radial_search.pinned {
                    let _ = self.route_egui_window_event(&event);
                    return;
                }
            }
        }

        // Alt+Enter is an application-level display shortcut. Handle it before
        // egui, chat, console, or any game overlay gets a chance to consume the
        // key event. Trigger once per physical press, not on OS key repeat.
        if let WindowEvent::KeyboardInput { event, .. } = &event {
            if event.state == ElementState::Pressed
                && !event.repeat
                && self.modifiers.alt_key()
                && matches!(
                    event.physical_key,
                    PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter)
                )
            {
                self.toggle_borderless_exclusive();
                return;
            }
        }

        // Both inspection pages are dismissible with the Dev bind. Handle
        // keyboard and mouse bindings ahead of egui, which otherwise consumes
        // them on these fullscreen/popup pages. A press only dismisses; it
        // must not also open the wheel or invoke a tool.
        if matches!(self.overlay, OverlayMode::Trace | OverlayMode::EntityGraph | OverlayMode::ShaderRemap) {
            let dismiss = match &event {
                WindowEvent::KeyboardInput { event: key, .. } => {
                    key.state == ElementState::Pressed
                        && !key.repeat
                        && match key.physical_key {
                            PhysicalKey::Code(code) => self.key_bound_to_dev(code),
                            _ => false,
                        }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    *state == ElementState::Pressed && self.mouse_button_bound_to_dev(*button)
                }
                _ => false,
            };
            if dismiss {
                self.toggle_dev_radial(); // Context-sensitive Back; no wheel opens.
                return;
            }
        }

        // In pinned search the OS cursor and egui own clicks; only release
        // a held emote mouse bind silently (do NOT close the pinned browser).
        if self.overlay == OverlayMode::EmoteRadial && self.emote_radial_search.pinned {
            if let WindowEvent::MouseInput { state: ElementState::Released, button, .. } = &event {
                if self.mouse_button_bound_to_emote(*button) {
                    let number = match button {
                        MouseButton::Left => Some(1), MouseButton::Right => Some(2),
                        MouseButton::Middle => Some(3), MouseButton::Back => Some(4),
                        MouseButton::Forward => Some(5), _ => None,
                    };
                    if let Some(number) = number {
                        self.mouse_buttons_down.remove(&number);
                        self.execute_binding_release(BindKey::Mouse(number));
                    }
                }
            }
            if matches!(&event, WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }) {
                let _ = self.route_egui_window_event(&event);
                return;
            }
        }

        // Both wheels own their mouse input before egui/gameplay. A category
        // click stays in the same captured overlay; only a command click commits.
        if self.overlay == OverlayMode::EmoteRadial && !self.emote_radial_search.pinned {
            if let WindowEvent::MouseInput { state, button, .. } = &event {
                if *state == ElementState::Released && self.mouse_button_bound_to_emote(*button) {
                    let number = match button {
                        MouseButton::Left => Some(1),
                        MouseButton::Right => Some(2),
                        MouseButton::Middle => Some(3),
                        MouseButton::Back => Some(4),
                        MouseButton::Forward => Some(5),
                        _ => None,
                    };
                    if let Some(number) = number {
                        self.mouse_buttons_down.remove(&number);
                        self.execute_binding_release(BindKey::Mouse(number));
                    }
                    self.close_emote_radial();
                } else if *state == ElementState::Pressed && *button == MouseButton::Left
                    && !self.mouse_button_bound_to_emote(*button)
                {
                    self.activate_emote_radial_selection();
                } else if *state == ElementState::Pressed && *button == MouseButton::Right {
                    self.emote_radial_back();
                }
                return;
            }
        }

        // Developer wheel owns mouse clicks *before* egui/normal game binds.
        // Clicking confirms the current relative-motion selection; releasing
        // the bound mouse button simply dismisses, without performing a tool.
        if self.overlay == OverlayMode::DevRadial {
            if let WindowEvent::MouseInput { state, button, .. } = &event {
                if *state == ElementState::Released && self.mouse_button_bound_to_dev(*button) {
                    let number = match button {
                        MouseButton::Left => Some(1),
                        MouseButton::Right => Some(2),
                        MouseButton::Middle => Some(3),
                        MouseButton::Back => Some(4),
                        MouseButton::Forward => Some(5),
                        _ => None,
                    };
                    if let Some(number) = number {
                        self.mouse_buttons_down.remove(&number);
                        self.execute_binding_release(BindKey::Mouse(number));
                    }
                    self.close_dev_radial();
                } else if *state == ElementState::Pressed
                    && *button == MouseButton::Left
                    && !self.mouse_button_bound_to_dev(*button)
                {
                    self.activate_dev_radial_selection();
                }
                // Do not leak other buttons to egui or gameplay. In particular,
                // clicking a non-selected region closes without attacking.
                return;
            }
        }
        // Placement is a camera-captured gameplay mode, not an egui dialog.
        // Consume mouse actions before gameplay bindings (+attack / +altattack)
        // and before spectator demo controls.
        if self.overlay == OverlayMode::None && self.model_placement.active {
            if let WindowEvent::MouseInput { state, button, .. } = &event {
                if *state == ElementState::Released && self.placement_release_drag(*button) {
                    return; // Never leak an editor-owned release into a game bind.
                }
                if *state == ElementState::Pressed {
                    if self.model_placement.edit_mode {
                        // Ctrl+LMB on empty editor space opens the Add browser.
                        // Ctrl+LMB on a hit object still starts vertical drag.
                        // While previewing a new asset, LMB places it first.
                        if *button == MouseButton::Left
                            && self.modifiers.control_key()
                            && !self.model_placement.editor_placing
                            && self.model_placement.hover.is_none()
                            && self.model_placement.bmodel_hover.is_none()
                            && self.model_placement.fx_hover.is_none()
                        {
                            self.open_model_placement_picker();
                            return;
                        }
                        let owned = if self.model_placement.editor_placing {
                            match button {
                                MouseButton::Left => self.placement_click(false),
                                MouseButton::Right => {
                                    self.model_placement.rotating = true;
                                    self.model_placement.preview_rotation = None;
                                    // Own the release even if Esc/Backspace
                                    // interrupts a preview rotation.
                                    self.model_placement.editor_mouse_owned = Some(*button);
                                    true
                                }
                                _ => false,
                            }
                        } else {
                            match button {
                                MouseButton::Left => self.placement_click(false),
                                MouseButton::Middle | MouseButton::Right => self.placement_begin_rotation(*button),
                                _ => false,
                            }
                        };
                        if owned { return; }
                        // Empty-space clicks are gameplay/noclip input, not an
                        // editor operation; pass both press and release through.
                    } else {
                        match button {
                            MouseButton::Left => {
                                if self.placement_click(false) { return; }
                            }
                            MouseButton::Right => {
                                self.model_placement.rotating = true;
                                // Only Alt+RMB (local roll) uses a quaternion;
                                // normal yaw and Shift pitch retain v1 controls.
                                self.model_placement.preview_rotation = None;
                                return;
                            }
                            _ => {}
                        }
                    }
                } else if (!self.model_placement.edit_mode || self.model_placement.editor_placing)
                    && *button == MouseButton::Right {
                    self.model_placement.rotating = false;
                    self.model_placement.preview_rotation = None;
                    return;
                }
            }
        }
        if self.route_egui_window_event(&event) {
            return;
        }
        if let WindowEvent::MouseInput { state, button, .. } = &event {
            // Demo playback reuses spectator mouse bindings. +speed temporarily
            // releases the cursor for timeline interaction; non-left spectator
            // buttons may still be explicit controls while that modifier is held.
            let demo_spectator_button =
                self.spectator_binding_context() && !matches!(*button, MouseButton::Left);
            if self.overlay == OverlayMode::None && (self.captured || demo_spectator_button) {
                let down = *state == ElementState::Pressed;
                let button_number = match button {
                    MouseButton::Left => Some(1),
                    MouseButton::Right => Some(2),
                    MouseButton::Middle => Some(3),
                    MouseButton::Back => Some(4),
                    MouseButton::Forward => Some(5),
                    _ => None,
                };
                if let Some(button_number) = button_number {
                    if down {
                        self.mouse_buttons_down.insert(button_number);
                    } else {
                        self.mouse_buttons_down.remove(&button_number);
                    }
                    if down {
                        self.execute_binding_press(BindKey::Mouse(button_number));
                    } else {
                        self.execute_binding_release(BindKey::Mouse(button_number));
                    }
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => self.request_quit(),
            WindowEvent::Resized(size) => {
                if self.pending_renderer_restart.is_none()
                    && !self.display_transition_active()
                    && self.applied_fullscreen.is_windowed()
                    && size.width > 0
                    && size.height > 0
                {
                    self.video.window_maximized = self
                        .window
                        .as_ref()
                        .is_some_and(|window| window.is_maximized());
                    if !self.video.window_maximized {
                        self.video.resolution = [size.width, size.height];
                        self.applied_resolution = [size.width, size.height];
                    }
                    self.mark_config_dirty();
                }
                self.render_command(RenderCommand::Resize(size));
                self.publish_ui();
            }
            WindowEvent::Moved(position) => {
                self.sync_gamma_monitor();
                if self.pending_renderer_restart.is_none()
                    && !self.display_transition_active()
                    && self.applied_fullscreen.is_windowed()
                    && !self
                        .window
                        .as_ref()
                        .is_some_and(|window| window.is_maximized())
                {
                    self.video.window_position = Some([position.x, position.y]);
                    self.mark_config_dirty();
                }
                if self.front_end {
                    self.sync_render_fps_cap();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } if self.video_confirmation.is_some() => {
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match code {
                    KeyCode::Enter | KeyCode::KeyY => self.confirm_video_settings(),
                    KeyCode::Escape | KeyCode::KeyN => self.revert_video_settings("REJECTED"),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.controls_page_active() && self.controls_waiting_for_key =>
            {
                let key = match delta {
                    MouseScrollDelta::LineDelta(_, y) if y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::LineDelta(_, y) if y < 0.0 => Some(BindKey::WheelDown),
                    MouseScrollDelta::PixelDelta(pos) if pos.y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::PixelDelta(pos) if pos.y < 0.0 => Some(BindKey::WheelDown),
                    _ => None,
                };
                if let Some(key) = key {
                    self.bind_control_key(key);
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.overlay == OverlayMode::None && self.model_placement.active =>
            {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32 / 120.0).clamp(-8.0, 8.0),
                };
                if self.model_placement.edit_mode && !self.model_placement.editor_placing {
                    // Scale the selected MD3, not the placement ghost. All
                    // wheel ticks in a burst currently form separate undo units.
                    if amount.is_finite() && amount.abs() > 0.0 {
                        if let Some(crate::app::egui_model_placement::EditSelection::Md3(i)) = self.model_placement.editor_selected {
                            if i < self.model_placement.placed.len() {
                                self.placement_checkpoint();
                                let p = &mut self.model_placement.placed[i];
                                p.scale = (p.scale * (amount * 0.085).exp()).clamp(0.01, 100.0);
                                self.model_placement.placed_dirty = true;
                            }
                        }
                    }
                } else if amount.is_finite() && self.model_placement.kind == crate::app::egui_model_placement::PlacementKind::Md3 {
                    self.model_placement.scale = (self.model_placement.scale
                        * (amount * 0.085).exp()).clamp(0.01, 100.0);
                    self.egui_repaint_requested = true;
                } else if amount.is_finite() && self.model_placement.kind == crate::app::egui_model_placement::PlacementKind::Fx {
                    // An effect has no size: the wheel sets how often it replays.
                    self.model_placement.fx_delay = (self.model_placement.fx_delay
                        * (amount * 0.085).exp()).clamp(50.0, 5000.0);
                    self.egui_repaint_requested = true;
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.overlay == OverlayMode::None
                    && self.captured
                    && self.spectator_orbit_follow_active() =>
            {
                let (amount, key) = match delta {
                    MouseScrollDelta::LineDelta(_, y) if y > 0.0 => (y, Some(BindKey::WheelUp)),
                    MouseScrollDelta::LineDelta(_, y) if y < 0.0 => (y, Some(BindKey::WheelDown)),
                    MouseScrollDelta::PixelDelta(position) if position.y > 0.0 => (
                        (position.y as f32 / 120.0).clamp(-8.0, 8.0),
                        Some(BindKey::WheelUp),
                    ),
                    MouseScrollDelta::PixelDelta(position) if position.y < 0.0 => (
                        (position.y as f32 / 120.0).clamp(-8.0, 8.0),
                        Some(BindKey::WheelDown),
                    ),
                    _ => (0.0, None),
                };
                // Orbit owns the wheel by default, but an explicit spectator
                // override wins. Inherited gameplay wheel binds do not disable
                // orbit zoom.
                if let Some(key) = key {
                    if self.bindings.get_spectator(key).is_some() {
                        self.execute_binding_tap(key);
                        return;
                    }
                }
                if amount.abs() > f32::EPSILON {
                    self.spectator_camera.orbit_range = (self.spectator_camera.orbit_range
                        * (-amount * 0.12).exp())
                    .clamp(MIN_SPECTATOR_ORBIT_RANGE, MAX_SPECTATOR_ORBIT_RANGE);
                    self.mark_config_dirty();
                    self.publish_ui();
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.overlay == OverlayMode::None && self.captured =>
            {
                let key = match delta {
                    MouseScrollDelta::LineDelta(_, y) if y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::LineDelta(_, y) if y < 0.0 => Some(BindKey::WheelDown),
                    MouseScrollDelta::PixelDelta(pos) if pos.y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::PixelDelta(pos) if pos.y < 0.0 => Some(BindKey::WheelDown),
                    _ => None,
                };
                if let Some(key) = key {
                    self.execute_binding_tap(key);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.overlay == OverlayMode::Console => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 4.0).round() as i32,
                    MouseScrollDelta::PixelDelta(position) => (position.y / 24.0).round() as i32,
                };
                self.scroll_console(rows);
                self.publish_ui();
            }
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                // Focus loss is cancellation, never a tool commit. Otherwise
                // Alt+Tab could leave a hidden/locked wheel stranded.
                if !focused && self.overlay == OverlayMode::DevRadial {
                    self.close_dev_radial();
                } else if !focused && self.overlay == OverlayMode::EmoteRadial {
                    self.close_emote_radial();
                }
                self.sync_gamma_focus();
                self.apply_audio_mix();

                if focused && !headless() {
                    if let Some(window) = self.window.as_ref() {
                        // Cancel any outstanding taskbar/dock attention request
                        // as soon as the user returns to the game.
                        window.request_user_attention(None);
                    }
                    // A Windows Locked grab pins the cursor at its *current screen
                    // position*. In a windowed game that position may still be outside
                    // the client area after Alt+Tab; a successful grab then traps the
                    // cursor outside the game, and repeating the grab does not help.
                    // Move it inside *before* restoring capture, only when the normal
                    // gameplay/demo input rules actually want a captured cursor.
                    if self.overlay == OverlayMode::None {
                        let demo_active = self.demo_playback_active();
                        let wants_capture = !demo_active
                            || (!self.live_buttons.contains("+speed") && !self.demo_scrub_dragging);
                        if wants_capture
                            && !self.captured
                            && !self.connection_modal_active()
                            && self.applied_fullscreen.is_windowed()
                        {
                            if let Some(window) = self.window.as_ref() {
                                let size = window.inner_size();
                                if size.width > 0 && size.height > 0 {
                                    let center = PhysicalPosition::new(
                                        (size.width / 2) as i32,
                                        (size.height / 2) as i32,
                                    );
                                    if let Err(error) = window.set_cursor_position(center) {
                                        eprintln!("Windowed focus: cursor recenter failed: {error}");
                                    }
                                }
                            }
                        }
                        if demo_active {
                            self.sync_demo_camera_capture();
                        } else {
                            self.set_capture(true);
                        }
                    }
                    return;
                }

                // True exclusive fullscreen keeps owning the monitor after focus loss
                // unless the window gets out of the way. Match normal game behavior on
                // Alt+Tab; Windows restores it when the user switches back. Entering a
                // fullscreen mode produces a transient focus loss of its own, so skip
                // this while a video change is waiting to be kept or reverted.
                if self.applied_fullscreen == FullscreenMode::Exclusive
                    && self.applied_renderer_backend != RendererBackend::Dx12
                    && !self.video_restart_in_flight()
                {
                    if let Some(window) = self.window.as_ref() {
                        window.set_minimized(true);
                    }
                }

                self.release_all_pressed_bindings();
                self.keys.clear();
                self.movement_keys.clear();
                self.mouse_buttons_down.clear();
                self.noclip_primary_down = false;
                self.noclip_alt_down = false;
                self.modifiers = ModifiersState::empty();
                self.set_capture(false);
                if let Some(player) = &mut self.local_server {
                    player.pause();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
                if self.overlay == OverlayMode::Console {
                    let width = self
                        .window
                        .as_ref()
                        .map_or(0, |window| window.inner_size().width);
                    let hover = ui::console_help_hit(width, position.x, position.y);
                    if hover != self.console_help_hover {
                        self.console_help_hover = hover;
                        self.publish_ui();
                    }
                }
                if self.overlay == OverlayMode::MapEdit {
                    let size = self
                        .window
                        .as_ref()
                        .map(|window| window.inner_size())
                        .unwrap_or_default();
                    let mut changed = false;
                    if let Some(editor) = self.map_editor.as_mut() {
                        changed = editor.update_drag(
                            &self.camera,
                            size.width,
                            size.height,
                            self.cursor_position,
                        );
                    }
                    if changed {
                        self.egui_repaint_requested = true;
                        // Keep the real textured world following the brush while
                        // dragging. Only one source-map rebuild can be in flight;
                        // additional snapped mouse steps collapse into a dirty bit.
                        self.queue_map_edit_preview();
                        self.publish_snapshot();
                    }
                }
                if self.demo_scrub_dragging {
                    self.update_demo_scrub(position.x);
                }
                if self.overlay == OverlayMode::Console && self.console_selecting {
                    let size = self
                        .window
                        .as_ref()
                        .map(|window| window.inner_size())
                        .unwrap_or_default();
                    if let Some(point) =
                        self.console_point_at(size.width, size.height, position.x, position.y)
                    {
                        self.console_selection_focus = Some(point);
                        self.publish_ui();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if self.controls_page_active() && self.controls_waiting_for_key => {
                let key = match button {
                    MouseButton::Left => Some(BindKey::Mouse(1)),
                    MouseButton::Right => Some(BindKey::Mouse(2)),
                    MouseButton::Middle => Some(BindKey::Mouse(3)),
                    MouseButton::Back => Some(BindKey::Mouse(4)),
                    MouseButton::Forward => Some(BindKey::Mouse(5)),
                    _ => None,
                };
                if let Some(key) = key {
                    self.bind_control_key(key);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::MapEdit => {
                let size = self
                    .window
                    .as_ref()
                    .map(|window| window.inner_size())
                    .unwrap_or_default();
                if let Some(editor) = self.map_editor.as_mut() {
                    editor.begin_drag(&self.camera, size.width, size.height, self.cursor_position);
                    self.egui_repaint_requested = true;
                }
                self.publish_snapshot();
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::MapEdit => {
                let mut finished_drag = false;
                let preview = if let Some(editor) = self.map_editor.as_mut() {
                    if editor.finish_drag() {
                        finished_drag = true;
                        match editor.working_source_text() {
                            Ok(text) => Some((editor.source_path.clone(), text)),
                            Err(error) => {
                                editor.status =
                                    format!("Could not build textured preview: {error}");
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                if finished_drag {
                    self.egui_repaint_requested = true;
                }
                if preview.is_some() {
                    self.queue_map_edit_preview();
                }
                self.publish_snapshot();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::None && self.demo_playback_active() => {
                match self.demo_timeline_hit(self.cursor_position.0, self.cursor_position.1) {
                    Some(DemoTimelineHit::PlayPause) => self.toggle_demo_pause_ui(),
                    Some(DemoTimelineHit::Track(fraction)) => self.begin_demo_scrub(fraction),
                    Some(DemoTimelineHit::MarkIn) => self.mark_demo_export_in(),
                    Some(DemoTimelineHit::MarkOut) => self.mark_demo_export_out(),
                    Some(DemoTimelineHit::Export) => {
                        if self.demo_export_available() {
                            self.start_demo_video_export();
                        }
                    }
                    Some(DemoTimelineHit::ExportPreset) => self.cycle_demo_export_preset(),
                    Some(DemoTimelineHit::ExportFps) => self.cycle_demo_export_fps(),
                    Some(DemoTimelineHit::ExportDepth) => self.toggle_demo_export_depth(),
                    Some(DemoTimelineHit::Speed) => self.cycle_demo_speed_ui(),
                    None => {}
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::None && self.demo_scrub_dragging => {
                self.finish_demo_scrub();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::Console => {
                let size = self
                    .window
                    .as_ref()
                    .map(|window| window.inner_size())
                    .unwrap_or_default();
                if self.click_console_suggest(
                    size.width,
                    size.height,
                    self.cursor_position.0,
                    self.cursor_position.1,
                ) {
                    self.console_selecting = false;
                } else if let Some(point) = self.console_point_at(
                    size.width,
                    size.height,
                    self.cursor_position.0,
                    self.cursor_position.1,
                ) {
                    if let Some(target) = self.console_path_at(point) {
                        self.console_selecting = false;
                        self.console_selection_anchor = None;
                        self.console_selection_focus = None;
                        self.console_last_click = None;
                        self.console_click_count = 0;
                        let _ = self.reveal_console_path(&target);
                    } else {
                        self.begin_console_selection(point);
                    }
                } else {
                    self.console_selection_anchor = None;
                    self.console_selection_focus = None;
                    self.console_selecting = false;
                    self.console_last_click = None;
                    self.console_click_count = 0;
                }
                self.publish_ui();
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::Console => {
                self.console_selecting = false;
                if self.console_selection_anchor == self.console_selection_focus {
                    self.console_selection_anchor = None;
                    self.console_selection_focus = None;
                }
                self.publish_ui();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::None => {
                if self.demo_playback_active() {
                    self.sync_demo_camera_capture();
                } else {
                    self.set_capture(true);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // Windows' desktop PrintScreen capture can be stale in Vulkan
                // exclusive fullscreen. Capture the actual WGPU surface instead
                // and replace the clipboard image, but do not write a screenshot
                // file. /screenshot is the explicit file-writing path.
                // Key ownership is remembered at key-down. Even if Tab opens
                // an overlay, the subsequent release still belongs to editing.
                if self.model_placement.editor_keys_owned.contains(&code) {
                    if event.state == ElementState::Released {
                        self.model_placement.editor_keys_owned.remove(&code);
                    }
                    return;
                }
                if self.overlay == OverlayMode::None && self.model_placement.active {
                    if event.state == ElementState::Pressed && !event.repeat {
                        let controlled = match code {
                            KeyCode::Tab => {
                                if self.model_placement.kind == crate::app::egui_model_placement::PlacementKind::Bmodel {
                                    self.open_bmodel_placement_picker();
                                } else { self.open_model_placement_picker(); }
                                true
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() => {
                                self.undo_model_placement(); true
                            }
                            KeyCode::KeyY if self.modifiers.control_key() => {
                                self.redo_model_placement(); true
                            }
                            KeyCode::KeyD if self.modifiers.control_key() && self.model_placement.edit_mode => {
                                self.duplicate_selected_placement(); true
                            }
                            KeyCode::Backspace if self.model_placement.editor_placing => {
                                self.finish_editor_add(); true
                            }
                            KeyCode::Delete | KeyCode::KeyX if self.model_placement.edit_mode
                                && !self.model_placement.editor_placing => {
                                self.delete_hovered_placement(); true
                            }
                            _ => false,
                        };
                        if controlled {
                            self.model_placement.editor_keys_owned.insert(code);
                            return;
                        }
                    }
                }
                if code == KeyCode::PrintScreen {
                    // Trigger on key-up so Windows has already performed its own
                    // PrintScreen clipboard update. Our worker writes the fresh
                    // GPU frame afterwards and therefore wins the clipboard race.
                    if event.state == ElementState::Released {
                        self.request_clipboard_capture();
                    }
                    return;
                }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && code == KeyCode::Backquote
                {
                    let desired_size = if self.modifiers.control_key() {
                        ConsoleSize::Full
                    } else if self.modifiers.shift_key() {
                        ConsoleSize::Half
                    } else {
                        ConsoleSize::Normal
                    };
                    if self.overlay == OverlayMode::Console && self.console_size == desired_size {
                        self.set_overlay(self.overlay_after_console());
                    } else {
                        if self.overlay != OverlayMode::Console {
                            self.overlay_before_console = self.overlay;
                        }
                        self.console_size = desired_size;
                        self.set_overlay(OverlayMode::Console);
                    }
                    return;
                }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && self.modifiers.control_key()
                    && code == KeyCode::KeyC
                    && matches!(
                        self.overlay,
                        OverlayMode::None
                            | OverlayMode::Game
                            | OverlayMode::Video
                            | OverlayMode::Trace
                            | OverlayMode::StrafeTrails
                            | OverlayMode::ShaderRemap
                            | OverlayMode::RaceGhosts
                            | OverlayMode::Telemarks
                    )
                    && self.surface_inspector.is_some()
                {
                    self.copy_surface_inspector_text();
                    return;
                }
                match self.overlay {
                    OverlayMode::Game => {
                        self.handle_game_key(&event, code);
                        return;
                    }
                    OverlayMode::Chat => {
                        self.handle_chat_key(&event, code);
                        return;
                    }
                    OverlayMode::Console => {
                        self.handle_console_key(&event, code);
                        return;
                    }
                    OverlayMode::Video => {
                        self.handle_video_key(&event, code);
                        return;
                    }
                    OverlayMode::Vgs => {
                        self.handle_vgs_key(&event, code);
                        return;
                    }
                    OverlayMode::HudEdit => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.hud_edit_drag_origin = None;
                            self.hud_edit_drag_delta = [0.0, 0.0];
                            self.hud_edit_chat_resize_origin = None;
                            self.hud_edit_chat_resize_corner = None;
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::CameraEdit => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.finish_camera_edit(false);
                        }
                        return;
                    }
                    OverlayMode::EntityGraph => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::ModelPicker | OverlayMode::BmodelPicker | OverlayMode::MapMods => {
                        if event.state == ElementState::Pressed && !event.repeat
                            && code == KeyCode::Escape {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::EmoteRadial => {
                        if self.key_bound_to_emote(code) {
                            if event.state == ElementState::Released {
                                self.keys.remove(&code);
                                self.execute_binding_release_from(
                                    keybinds::bind_key_for_code(code),
                                    BindKey::Keyboard(code),
                                );
                                self.close_emote_radial();
                            }
                        } else if code == KeyCode::Escape && event.state == ElementState::Pressed && !event.repeat {
                            self.emote_radial_back();
                        } else {
                            self.handle_dev_wheel_gameplay_key(code, event.state, event.repeat);
                        }
                        return;
                    }
                    OverlayMode::DevRadial => {
                        if self.key_bound_to_dev(code) {
                            if event.state == ElementState::Released {
                                self.keys.remove(&code);
                                self.execute_binding_release_from(
                                    keybinds::bind_key_for_code(code),
                                    BindKey::Keyboard(code),
                                );
                                self.close_dev_radial();
                            }
                        } else {
                            self.handle_dev_wheel_gameplay_key(code, event.state, event.repeat);
                        }
                        return;
                    }
                    OverlayMode::Trace => {
                        if event.state == ElementState::Pressed && !event.repeat {
                            if code == KeyCode::Escape {
                                self.close_trace_overlay();
                            } else if self.key_bound_to_trace(code) {
                                // Toggle: `trace_surface_center` closes the
                                // popup when it's already open.
                                self.trace_surface_center();
                            }
                        }
                        return;
                    }
                    OverlayMode::StrafeTrails => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::ShaderRemap => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::Telemarks => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::RaceGhosts => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::MapEdit => {
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && code == KeyCode::Escape
                        {
                            if let Some(editor) = self.map_editor.as_mut() {
                                editor.cancel_drag();
                            }
                            self.set_overlay(OverlayMode::None);
                            self.publish_snapshot();
                        }
                        return;
                    }
                    OverlayMode::None => {}
                }

                if self.demo_video_export.is_some()
                    && event.state == ElementState::Pressed
                    && !event.repeat
                {
                    if code == KeyCode::Escape {
                        self.cancel_demo_video_export();
                    }
                    return;
                }

                if self.demo_playback_active()
                    && event.state == ElementState::Pressed
                    && !event.repeat
                {
                    match code {
                        KeyCode::BracketLeft => {
                            self.cycle_demo_follow(-1);
                            return;
                        }
                        KeyCode::BracketRight => {
                            self.cycle_demo_follow(1);
                            return;
                        }
                        KeyCode::Home => {
                            self.reset_demo_view_authoritative();
                            self.console_status = "DEMO CAMERA: RECORDED POV".into();
                            return;
                        }
                        KeyCode::ArrowLeft => {
                            self.seek_demo_relative(-10_000);
                            return;
                        }
                        KeyCode::ArrowRight => {
                            self.seek_demo_relative(10_000);
                            return;
                        }
                        KeyCode::ArrowUp => {
                            self.cycle_demo_speed_ui();
                            return;
                        }
                        KeyCode::ArrowDown => {
                            self.lower_demo_speed_ui();
                            return;
                        }
                        _ => {}
                    }
                }

                match event.state {
                    ElementState::Pressed => {
                        self.keys.insert(code);
                    }
                    ElementState::Released => {
                        self.keys.remove(&code);
                    }
                }
                let bind_key = keybinds::bind_key_for_code(code);
                if event.state == ElementState::Pressed && !event.repeat {
                    if code == KeyCode::Escape {
                        // Editing is persistent, but Escape must still open the
                        // ordinary JKA menu. Roll back only the current drag.
                        if self.model_placement.edit_mode && self.model_placement.editor_drag.is_some() {
                            self.cancel_placement();
                        }
                        let overlay = self.remembered_in_game_menu_overlay();
                        self.set_overlay(overlay);
                        return;
                    }
                    if code == KeyCode::KeyN && self.modifiers.shift_key() {
                        self.cycle_spawn();
                        return;
                    }
                    self.execute_binding_press_from(bind_key, BindKey::Keyboard(code));
                } else if event.state == ElementState::Released {
                    // TaystJK CL_KeyUpEvent always emits the `-` half of every
                    // +command in the bind, so held actions cannot stick.
                    self.execute_binding_release_from(bind_key, BindKey::Keyboard(code));
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if headless() || self.demo_video_export.is_some() || self.renderer_restart_suspended {
            return;
        }
        if self.render_studio.navigation.looking {
            if !self.studio_navigation_available() {
                self.set_studio_mouse_look(false);
            } else if let DeviceEvent::MouseMotion { delta } = event {
                self.render_studio.navigation.raw_delta[0] += delta.0 as f32;
                self.render_studio.navigation.raw_delta[1] += delta.1 as f32;
                self.egui_repaint_requested = true;
            }
            return;
        }
        if matches!(self.overlay, OverlayMode::DevRadial | OverlayMode::EmoteRadial) {
            if self.window_focused && (self.overlay != OverlayMode::EmoteRadial
                || !self.emote_radial_search.pinned) {
                if let DeviceEvent::MouseMotion { delta } = event {
                    // Raw relative mouse movement drives a virtual stick.
                    // Clamp its *length*, not X/Y independently: this avoids
                    // building up an off-screen offset that makes switching
                    // segments feel like hunting for the selection again.
                    let dpi = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32).max(0.25);
                    // Shared Dev/Emote sensitivity. A small hand
                    // movement can no longer fling the highlight across tools.
                    let scale = 0.004 / dpi;
                    let x = self.dev_radial_offset[0] + delta.0 as f32 * scale;
                    let y = self.dev_radial_offset[1] + delta.1 as f32 * scale;
                    let length = x.hypot(y);
                    let adjust = if length > 1.0 { 1.0 / length } else { 1.0 };
                    self.dev_radial_offset = [x * adjust, y * adjust];
                    self.egui_repaint_requested = true;
                }
            }
            return;
        }
        if self.model_placement.active && self.model_placement.editor_drag.is_some()
            && self.overlay == OverlayMode::None && self.window_focused {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.placement_drag_motion(delta);
            }
            return;
        }
        if self.model_placement.active && self.model_placement.rotating
            && self.overlay == OverlayMode::None && self.window_focused {
            if let DeviceEvent::MouseMotion { delta } = event {
                if self.modifiers.alt_key() {
                    // Keep v2: roll around the preview object's local forward
                    // axis, tracking a quaternion while Alt remains held.
                    let orientation = self.model_placement.preview_rotation.unwrap_or_else(||
                        super::egui_model_placement::placement_orientation(self.model_placement.angles));
                    let next = super::egui_model_placement::placement_rotate_local(
                        orientation, 0.0, 0.0, delta.0 as f32 * 0.20);
                    self.model_placement.preview_rotation = Some(next);
                    self.model_placement.angles =
                        super::egui_model_placement::placement_rotation_angles(next);
                } else {
                    // Keep v1: directly edit yaw or pitch, without local
                    // quaternion composition. Invalidate cached roll when
                    // changing modifiers mid-drag to prevent orientation jumps.
                    self.model_placement.preview_rotation = None;
                    let (pitch, yaw) = if self.modifiers.shift_key() {
                        (-delta.1 as f32 * 0.20, 0.0)
                    } else {
                        (0.0, delta.0 as f32 * 0.20)
                    };
                    self.model_placement.angles =
                        super::egui_model_placement::placement_rotate_yaw_pitch(
                            self.model_placement.angles, pitch, yaw);
                }
                self.egui_repaint_requested = true;
            }
            return;
        }
        if !self.captured || self.overlay != OverlayMode::None {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            let event_at = Instant::now();
            let device_id_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                device_id.hash(&mut hasher);
                hasher.finish()
            };
            let diag_window_focused = self.window_focused;
            let diag_captured = self.captured;
            let diag_grab_mode = self.mouse_grab_mode;
            if let Some(render) = &self.render {
                render.begin_input_diagnostic_mouse(event_at);
            }
            if self.spectator_orbit_follow_active() {
                let yaw_degrees =
                    delta.0 as f32 * self.mouse_input.sensitivity * self.mouse_input.yaw;
                let pitch_degrees =
                    delta.1 as f32 * self.mouse_input.sensitivity * self.mouse_input.pitch;
                let fallback = match self.demo_view_mode {
                    DemoViewMode::Follow(client) if self.demo_playback_active() => self
                        .game_session
                        .as_ref()
                        .and_then(|session| session.demo_fake_view(client))
                        .map(|view| view.view_angles),
                    _ => None,
                }
                .or_else(|| {
                    self.game_session
                        .as_ref()
                        .and_then(|session| session.current_snapshot.as_ref())
                        .and_then(|snapshot| playerstate_vec3(&snapshot.player_state, "viewangles"))
                })
                .unwrap_or([0.0; 3]);
                self.spectator_camera_state
                    .apply_orbit_mouse(yaw_degrees, pitch_degrees, fallback);
                if let Some(render) = &self.render {
                    render.finish_input_diagnostic_mouse(
                        event_at,
                        None,
                        Instant::now(),
                        delta.0,
                        delta.1,
                        None,
                        device_id_hash,
                        diag_window_focused,
                        diag_captured,
                        diag_grab_mode,
                    );
                }
                return;
            }
            if self.demo_playback_active() && self.demo_view_mode == DemoViewMode::Free {
                let yaw_degrees =
                    delta.0 as f32 * self.mouse_input.sensitivity * self.mouse_input.yaw;
                let pitch_degrees =
                    delta.1 as f32 * self.mouse_input.sensitivity * self.mouse_input.pitch;
                self.camera.yaw -= yaw_degrees.to_radians();
                self.camera.pitch = (self.camera.pitch - pitch_degrees.to_radians())
                    .clamp((-89.0_f32).to_radians(), 89.0_f32.to_radians());
                self.publish_snapshot();
                if let Some(render) = &self.render {
                    render.finish_input_diagnostic_mouse(
                        event_at,
                        None,
                        Instant::now(),
                        delta.0,
                        delta.1,
                        None,
                        device_id_hash,
                        diag_window_focused,
                        diag_captured,
                        diag_grab_mode,
                    );
                }
                return;
            }
            self.mouse_input_sequence = self.mouse_input_sequence.wrapping_add(1);
            if self.mouse_input_sequence == 0 {
                self.mouse_input_sequence = 1;
            }

            let elapsed = self
                .last_mouse_motion_at
                .replace(event_at)
                .map(|last| event_at.saturating_duration_since(last))
                .unwrap_or(Duration::from_millis(1));
            if self
                .net
                .as_ref()
                .is_some_and(|net| net.state() >= jka_protocol::session::ConnectionState::Primed)
            {
                // Event-rate CL_MouseMove. This only advances cl.viewangles;
                // Pmove/usercmd cadence remains unchanged. The next normal
                // client frame consumes the same viewangles for prediction,
                // while the renderer may expose this real mouse sample to
                // first person or the safe no-trace third-person latch path.
                let simulation_at = Instant::now();
                let (mx, my) = self.mouse_input.scale_mouse(delta, elapsed);
                self.live_input
                    .apply_mouse(self.mouse_input.yaw * mx, self.mouse_input.pitch * my);
                let sample = InputLatencySample {
                    sequence: self.mouse_input_sequence,
                    event_at,
                    simulation_at,
                };
                self.last_simulated_mouse_input = Some(sample);
                // Keep the crosshair endpoint coherent with this exact raw-input
                // view sample before publishing the pair to the render seqlock.
                if self.crosshair.dynamic != 0
                    || (self.video.depth_of_field_strength > 0.001 && self.video.dof_mode == ui::DofMode::On && self.video.dof_autofocus)
                {
                    self.update_crosshair_target(event_at);
                }
                self.publish_live_subframe_view(sample);
                if let Some(render) = &self.render {
                    render.finish_input_diagnostic_mouse(
                        event_at,
                        Some(sample.simulation_at),
                        Instant::now(),
                        delta.0,
                        delta.1,
                        Some(sample.sequence),
                        device_id_hash,
                        diag_window_focused,
                        diag_captured,
                        diag_grab_mode,
                    );
                }
            } else {
                let mut applied = false;
                let mut applied_sample = None;
                if let Some(player) = &mut self.local_server {
                    let simulation_at = Instant::now();
                    player.set_mouse_input_settings(self.mouse_input);
                    player.apply_mouse_look_timed(delta, elapsed);
                    let sample = InputLatencySample {
                        sequence: self.mouse_input_sequence,
                        event_at,
                        simulation_at,
                    };
                    self.last_simulated_mouse_input = Some(sample);
                    applied_sample = Some(sample);
                    applied = true;
                }
                if applied {
                    // Trace first, then publish rotation + endpoint together. This
                    // prevents the renderer from late-latching a new angle while
                    // still drawing the previous mouse sample's dynamic crosshair.
                    if self.crosshair.dynamic != 0
                        || (self.video.depth_of_field_strength > 0.001 && self.video.dof_mode == ui::DofMode::On && self.video.dof_autofocus)
                    {
                        self.update_crosshair_target(event_at);
                    }
                    // OpenJK-forced views still accumulate mouse into future usercmds,
                    // but they do no render-only subframe camera work.
                    let subframe_event_handled = self
                        .local_server
                        .as_ref()
                        .is_some_and(|server| server.view().view_forced != 0)
                        || applied_sample
                            .is_some_and(|sample| self.publish_local_subframe_view(sample));
                    if !subframe_event_handled {
                        // The no-trace third-person latch is unavailable
                        // (for example camera collision/damping). Preserve the
                        // old local fallback and rerun the authoritative CGame
                        // camera rather than approximating through geometry.
                        self.update_solo_player_view_and_presentation();
                        if self.crosshair.dynamic != 0
                            || (self.video.depth_of_field_strength > 0.001
                                && self.video.dof_mode == ui::DofMode::On
                                && self.video.dof_autofocus)
                        {
                            self.update_crosshair_target(event_at);
                        }
                        self.publish_snapshot();
                    }
                }
                if let Some(render) = &self.render {
                    render.finish_input_diagnostic_mouse(
                        event_at,
                        applied_sample.map(|sample| sample.simulation_at),
                        Instant::now(),
                        delta.0,
                        delta.1,
                        applied_sample.map(|sample| sample.sequence),
                        device_id_hash,
                        diag_window_focused,
                        diag_captured,
                        diag_grab_mode,
                    );
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Fullscreen/backend restarts are deliberately staged across event-loop
        // turns so Windows can finish changing the HWND/display mode before the
        // next WGPU surface is created.
        self.continue_pending_renderer_restart(event_loop);
        self.sync_companion_window(event_loop);
        self.tick_emote_gallery();
        self.tick();
        self.tick_companion_ui();
        let bench_turn = BENCH_TURN_COUNTS.load(std::sync::atomic::Ordering::Relaxed);
        if bench_turn != 0 {
            let period = BENCH_TURN_PERIOD.load(std::sync::atomic::Ordering::Relaxed) as u32;
            let step = BENCH_TURN_STEP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let bench_turn = if period != 0 && (step / period) % 2 == 1 { -bench_turn } else { bench_turn };
            let captured = std::mem::replace(&mut self.captured, true);
            // Dummy ids are only compared/hashed by the handler.
            let device = winit::event::DeviceId::dummy();
            self.device_event(event_loop, device, DeviceEvent::MouseMotion { delta: (bench_turn as f64, 0.0) });
            self.captured = captured;
        }
        if self.quit_requested {
            self.flush_config();
            if !self.background_jobs.flush_io(Duration::from_millis(500)) {
                eprintln!("Config/background I/O did not finish before quit");
            }
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(1),
        ));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.exit_process();
    }
}

/// `benchturn` synthetic mouse counts per event-loop turn (0 = off).
pub(in crate::app) static BENCH_TURN_COUNTS: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
/// `benchturn` reversal period in event-loop turns (0 = keep turning).
pub(in crate::app) static BENCH_TURN_PERIOD: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
static BENCH_TURN_STEP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl App {
    /// Flush what must survive and end the process without orderly teardown.
    pub(in crate::app) fn exit_process(&mut self) -> ! {
        self.restore_gamma_before_exit();
        self.flush_config();
        if !self.background_jobs.flush_io(Duration::from_millis(500)) {
            eprintln!("Background I/O did not finish before fast exit");
        }
        // Chat logging is the only buffered durable gameplay output. Give its
        // dedicated writer a short bounded window to append the footer/flush
        // before the Windows fast-exit path terminates all worker threads.
        self.chat_log.shutdown(Duration::from_millis(250));
        // CL_Disconnect on quit: tell the server instead of timing out.
        if let Some(mut net) = self.net.take() {
            net.disconnect();
        }

        // The window is already hidden and everything durable is flushed
        // (config, disconnect, demo; latest.log flushes every line). Joining the
        // render thread and dropping the renderer, workers and caches only
        // frees memory the OS reclaims anyway, and cost 200-500 ms of black
        // screen. Exit directly; this also skips the winit teardown race that
        // the orderly shutdown had to work around. TerminateProcess skips even
        // the DLL detach work of a normal exit (about 50 ms faster); Windows
        // restores an exclusive-fullscreen display mode when the process dies.
        #[cfg(windows)]
        {
            extern "system" {
                fn GetCurrentProcess() -> *mut std::ffi::c_void;
                fn TerminateProcess(process: *mut std::ffi::c_void, exit_code: u32) -> i32;
            }
            unsafe { TerminateProcess(GetCurrentProcess(), 0) };
        }
        std::process::exit(0);
    }
}
