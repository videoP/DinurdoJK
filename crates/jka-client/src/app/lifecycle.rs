//! Lifecycle.
use crate::app::{
    keybinds, playerstate_vec3, prepared_map_has_ocean_surface, random_unit, scene,
    split_console_script, ui, ActiveEventLoop, App, ApplicationHandler, Arc, BindKey, Camera,
    ClientFramePerf, ConsoleSize, ControlFlow, DemoTimelineHit, DemoViewMode, DeviceEvent,
    Duration, ElementState, FrontendCinematic, FrontendPage, FullscreenMode, InputLatencySample,
    Instant, KeyCode, LiveJoinTiming, LocalServer, MapEditor, ModifiersState, MouseButton,
    MouseScrollDelta, OverlayMode, PerfStats, PhysicalKey, PhysicalPosition, PhysicalSize,
    PreparedMapCache, RenderCommand, RenderThread, RendererBackend, ScreenshotOutput,
    ThreadPerfStats, UserEvent, Window, WindowEvent, WindowId, FRONTEND_CAMERA_EYE_HEIGHT,
    MAX_SPECTATOR_ORBIT_RANGE, MIN_SPECTATOR_ORBIT_RANGE,
};

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Keep the native window hidden while WGPU creates the device/surface and
        // builds the initial renderer state. RendererReady reveals it before the
        // first present; waiting for RendererFirstFrame can deadlock when a hidden
        // Windows surface reports itself as occluded.
        let mut attributes = Window::default_attributes()
            .with_title("DinurdoJK")
            .with_visible(false)
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
        if !self.video.fullscreen.is_windowed() {
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
        self.applied_fullscreen = self.video.fullscreen;
        self.applied_renderer_backend = self.video.renderer_backend;
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
        self.sync_classic_world_lighting();
        self.previous_tick = Instant::now();
        // A config written before this rule, or by hand, can still say 0. Resolve
        // it now that the window exists and the monitor refresh rate is readable.
        self.set_fps_cap(self.video.fps_cap);
        // The initial map request is intentionally deferred until RendererFirstFrame.
        // That gives the startup splash a real presented frame and prevents the fast
        // map worker from finishing invisibly while the renderer is still initializing.
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::Event,
        );
        match event {
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
            UserEvent::ScreenshotFinished(result) => {
                match result {
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
                    Err(error) => {
                        self.console_status = format!("SCREENSHOT CAPTURE FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                    }
                }
                self.publish_ui();
            }
            UserEvent::RendererReady {
                supported_msaa,
                wireframe_supported,
            } => {
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

                // The native window is created hidden so Windows never exposes an
                // uninitialized swapchain while WGPU is still building. Do not wait
                // for the first successful present to reveal it, though: a hidden
                // HWND is allowed to make surface acquisition return Occluded, which
                // would otherwise create a startup deadlock (hidden -> occluded -> no
                // first frame -> still hidden). RendererReady means device/surface
                // setup and the initial UI state are complete, so it is the correct
                // point to make the window drawable.
                if !self.startup_first_frame_seen {
                    if let Some(window) = &self.window {
                        window.set_visible(true);
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
            UserEvent::RendererFirstFrame => {
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
                        self.begin_pending_video_confirmation();
                        self.publish_ui();
                    }
                    return;
                }
                self.startup_first_frame_seen = true;
                if self.front_end {
                    self.console_status = "MAIN MENU".into();
                    self.publish_ui();
                    self.request_frontend_background();
                } else {
                    let initial_source = self.initial_source.clone();
                    self.request_map(initial_source);
                    self.console_map_barrier = !self.startup_commands.is_empty();
                }
                for command in std::mem::take(&mut self.startup_commands) {
                    println!("STARTUP COMMAND: {command}");
                    self.push_console_line(format!("^7] {command}"));
                    self.append_console_commands(split_console_script(&command));
                }
            }
            UserEvent::WaterSplashes(splashes) => self.play_water_splashes(&splashes),
            UserEvent::RenderStats(stats) => {
                self.accumulate_perf_sample(&stats);
                self.video.msaa_samples = stats.msaa_samples;
                self.video.vsync = stats.vsync;
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
                self.threads = stats.threads.map(|thread| ThreadPerfStats {
                    name: crate::thread_activity::slot_label(thread.slot),
                    task: thread.task.label(),
                    active: thread.active,
                    busy_percent: thread.busy_percent,
                });
                // Perf/thread counters refresh at 2 Hz. Send only their Copy data
                // instead of cloning the entire retained UI/console snapshot, and
                // do not send anything when the renderer HUD is not displaying it.
                if self.video.draw_fps != 0 {
                    self.publish_telemetry_ui();
                }
            }
            UserEvent::RendererError(error) => {
                self.restore_gamma_before_restart();
                self.renderer_restart_started = None;
                self.show_window_after_restart();
                eprintln!("Renderer: {error}");
                self.console_status = format!("RENDERER ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));

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
            UserEvent::SteamAudioBakeProgress {
                request_id,
                map_name,
                progress,
            } => {
                if request_id != self.latest_request_id || !self.audio.steam_audio {
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
                        "{map_name}: Steam Audio background bake started; the legacy mixer remains authoritative until baked assets are ready"
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
            UserEvent::MapPrepared {
                request_id,
                started,
                name,
                mut map,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                let frontend_background =
                    self.front_end && self.frontend_background_request_id == Some(request_id);
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
                if self.map_edit_preview_request_id == Some(request_id) {
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
                let cache_started = Instant::now();
                self.prepared_map_cache = Some(PreparedMapCache {
                    label: name.clone(),
                    prepare_options: self.latest_prepare_options,
                    map: Arc::new(map.as_ref().clone()),
                });
                rverbose!(
                    2,
                    "Map restart cache: retained prepared CPU map in {:.1} ms",
                    cache_started.elapsed().as_secs_f64() * 1000.0
                );
                // What is resident is what this map was prepared with, whichever
                // renderer instance loaded it. Enabling grass/ocean beyond that
                // needs the map prepared again.
                self.applied_grass = self.latest_prepare_options.grass;
                self.applied_ocean = self.latest_prepare_options.ocean;
                self.forget_trace();
                self.entity_graph = map.entity_graph.clone();
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
                    }
                }
                self.third_person_camera.reset();
                self.solo_dynamic_models = Arc::new(Vec::new());
                if frontend_background {
                    self.preserve_game_state_on_next_map_upload = false;
                    self.local_server = None;
                    self.spawns = map.spawns.clone();
                    self.spawn_index = 0;
                    if let Some(spawn) = self.spawns.first().copied() {
                        let mut position = spawn.position;
                        position[1] += FRONTEND_CAMERA_EYE_HEIGHT;
                        self.camera =
                            Camera::new_with_fov(position, spawn.yaw, self.camera.cg_fov());
                        self.frontend_cinematic = Some(FrontendCinematic {
                            position,
                            yaw: spawn.yaw,
                            started: Instant::now(),
                            fade_started: None,
                        });
                    } else {
                        self.frontend_cinematic = None;
                    }
                    self.keys.clear();
                    self.movement_keys.clear();
                    self.live_buttons.clear();
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
                            &map.brush_entities,
                            map.visibility
                                .as_ref()
                                .map(jka_assets::bsp::Visibility::area_locator),
                        ) {
                            Ok(mut server) => {
                                server.set_char_color(self.network.char_color);
                                server.set_mouse_input_settings(self.mouse_input);
                                if let Err(error) =
                                    server.set_physics_tick_msec(self.video.physics_msec)
                                {
                                    self.console_status = format!("PHYSICS FPS ERROR: {error}");
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
                self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                self.render_command(RenderCommand::LoadMap {
                    request_id,
                    started,
                    name,
                    map,
                });
                // The quality the renderer runs depends on how this map was
                // prepared, which the settings menu may have changed mid-load.
                self.sync_post_effects();
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
                    self.frontend_background_request_id = None;
                    if let Some(cinematic) = self.frontend_cinematic.as_mut() {
                        // WorldUploaded is emitted only after the renderer has
                        // successfully presented the uploaded map once.
                        cinematic.fade_started = Some(Instant::now());
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
                    self.live_join_ui = None;
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
                        "^5[MAP LOAD]^7 {name} {:.1} ms first-frame | prep {:.1} | bsp {:.1} | collision {:.1} | shaders {:.1} | textures decode {:.1} | geometry {:.1} | render-upload {:.1} | workers {}",
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
                let post_upload_wait_ms =
                    (first_frame_ms - timings.prepare_wall_ms - upload_ms).max(0.0);
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP FIRST FRAME]^7 prep {:.1} ms | render upload {:.1} ms | queue/present wait {:.1} ms",
                        timings.prepare_wall_ms,
                        upload_ms,
                        post_upload_wait_ms,
                    ));
                }
                if crate::logging::renderer_verbose_enabled(2) {
                    self.push_console_line(format!(
                        "^6[MAP PREP DETAIL]^7 grass wall {:.1} ms | grass worker CPU {:.1} ms | gi {:.1} ms | ocean mesh {:.1} ms | acoustics {:.1} ms | PVS plans {:.1} ms",
                        timings.grass_ms,
                        timings.grass_cpu_ms,
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
                let upload_accounted_ms = upload_timings.vertex_buffer_ms
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
                self.console_status =
                    format!("LOADED {name}: {:.3} seconds", first_frame_ms / 1000.0);
                self.push_console_line(format!("^2{}", self.console_status));
                self.loading = None;
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

                // Start the keep/revert clock only after a real world frame has
                // been rendered. This gives the user the full timeout to inspect
                // the new mode instead of spending it on adapter/device/pipeline
                // creation and map upload.
                self.begin_pending_video_confirmation();
                self.publish_ui();
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
                    self.refresh_bound_state();
                    if down {
                        self.execute_binding_press(BindKey::Mouse(button_number));
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
                        self.execute_binding_press(key);
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
                    self.execute_binding_press(key);
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
                self.sync_gamma_focus();
                self.apply_audio_mix();

                if focused {
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
                            | OverlayMode::RaceGhosts
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
                self.refresh_bound_state();
                if event.state == ElementState::Pressed && !event.repeat {
                    if code == KeyCode::Escape {
                        let overlay = self.remembered_in_game_menu_overlay();
                        self.set_overlay(overlay);
                        return;
                    }
                    if code == KeyCode::KeyN && self.modifiers.shift_key() {
                        self.cycle_spawn();
                        return;
                    }
                    if let Some(binding) = self
                        .bindings
                        .get_resolved(
                            keybinds::bind_key_for_code(code),
                            self.spectator_binding_context(),
                        )
                        .map(str::to_owned)
                    {
                        if let Some(player) = &mut self.local_server {
                            for command in keybinds::split_binding_commands(&binding) {
                                let canonical = match command
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("")
                                    .to_ascii_lowercase()
                                    .as_str()
                                {
                                    "+forward" => Some(KeyCode::KeyW),
                                    "+back" => Some(KeyCode::KeyS),
                                    "+moveright" => Some(KeyCode::KeyD),
                                    "+moveleft" => Some(KeyCode::KeyA),
                                    "+moveup" => Some(KeyCode::Space),
                                    "+movedown" => Some(KeyCode::ControlLeft),
                                    _ => None,
                                };
                                if let Some(canonical) = canonical {
                                    player.note_movement_key_press(canonical);
                                }
                            }
                        }
                    }
                    self.execute_binding_press(keybinds::bind_key_for_code(code));
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.captured || self.overlay != OverlayMode::None {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
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
                return;
            }
            let event_at = Instant::now();
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
                    || (self.video.depth_of_field_strength > 0.001 && self.video.dof_autofocus)
                {
                    self.update_crosshair_target(event_at);
                }
                self.publish_live_subframe_view(sample);
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
                        || (self.video.depth_of_field_strength > 0.001 && self.video.dof_autofocus)
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
                                && self.video.dof_autofocus)
                        {
                            self.update_crosshair_target(event_at);
                        }
                        self.publish_snapshot();
                    }
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
        self.tick();
        self.tick_companion_ui();
        if self.quit_requested {
            self.flush_config();
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

impl App {
    /// Flush what must survive and end the process without orderly teardown.
    pub(in crate::app) fn exit_process(&mut self) -> ! {
        self.restore_gamma_before_exit();
        self.flush_config();
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
