//! Tick.
use crate::app::{
    scene, ui, App, DemoAdvance, Duration, Instant, OverlayMode, RenderCommand, SessionPhase,
};

/// Integrate a continuously differentiable speed ramp rather than simply
/// starting a sine at t=0 (a sine starts with maximum *velocity*).
/// The camera begins with zero velocity/acceleration and reaches full speed
/// gradually over four seconds; no phase discontinuity at the transition.
fn frontend_motion_phase(elapsed: f32) -> f32 {
    const RAMP: f32 = 4.0;
    let elapsed = elapsed.max(0.0);
    if elapsed >= RAMP {
        elapsed - RAMP * 0.5
    } else {
        let t = elapsed / RAMP;
        // Integral of smootherstep(t) = 6t^5 - 15t^4 + 10t^3.
        RAMP * (t.powi(6) - 3.0 * t.powi(5) + 2.5 * t.powi(4))
    }
}

#[cfg(test)]
mod menu_camera_tests {
    use super::frontend_motion_phase;

    #[test]
    fn camera_starts_at_rest_and_reaches_full_speed_smoothly() {
        let dt = 0.001;
        assert!((frontend_motion_phase(dt) - frontend_motion_phase(0.0)) / dt < 0.0001);
        assert!((frontend_motion_phase(4.0) - 2.0).abs() < 0.0001);
        let before = (frontend_motion_phase(4.0) - frontend_motion_phase(4.0 - dt)) / dt;
        let after = (frontend_motion_phase(4.0 + dt) - frontend_motion_phase(4.0)) / dt;
        assert!((before - 1.0).abs() < 0.01);
        assert!((after - 1.0).abs() < 0.01);
    }
}

impl App {
    pub(in crate::app) fn tick_frontend_cinematic(&mut self, now: Instant) {
        if !self.front_end || self.loading.is_some() {
            return;
        }
        let Some(cinematic) = self.frontend_cinematic else {
            return;
        };

        // Keep the camera safely at a real duel spawn and animate only the view.
        // This gives the menu a living 3D background without pathing a camera
        // through BSP walls or requiring gameplay collision/input.
        // The first presented 3D frame is exactly the camera pose saved in the
        // scene cache. Hold that pose through the image->world crossfade, then
        // begin the animation from phase zero (no position or angle jump).
        let seconds = cinematic.fade_started.map_or(0.0, |ready| {
            now.saturating_duration_since(self.frontend_visual_handoff_start(ready) + Duration::from_secs_f32(
                crate::app::egui_menu::FRONTEND_SCENE_FADE_IN_SECS,
            ))
            .as_secs_f32()
        });
        let sway = crate::scene::menu_stage(super::constants::FRONTEND_BACKGROUND_MAP)
            .is_none_or(|stage| stage.camera_sway);
        let phase = if sway { frontend_motion_phase(seconds) } else { 0.0 };
        self.camera.position = glam::Vec3::from_array(cinematic.position);
        self.camera.position.y += (phase * 0.32).sin() * 0.65;
        self.camera.yaw = cinematic.yaw + (phase * 0.115).sin() * 0.28;
        self.camera.pitch = cinematic.pitch + (phase * 0.09).sin() * 0.025;

        // A broken/missing model must not leave a cold first-run menu black
        // indefinitely; never persist a snapshot without a settled model.
        if cinematic.fade_started.is_none()
            && cinematic.world_ready_at.is_some_and(|ready| {
                now.saturating_duration_since(ready) >= Duration::from_secs(5)
            })
        {
            if let Some(active) = self.frontend_cinematic.as_mut() {
                active.fade_started = Some(now);
            }
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app) fn tick(&mut self) {
        self.studio_cfg_check_exit();
        self.restore_frontend_world_after_studio();
        if self.extensions.poll() {
            self.render_command(RenderCommand::SetExtensionHud(self.extensions.placed_widgets(), self.extensions.native_hidden, self.extensions.hud_images.clone()));
            if self.overlay == OverlayMode::HudEdit {
                self.egui_repaint_requested = true;
            }
        }
        self.poll_background_results();
        // Still drain map-worker results needed to make the replacement frame,
        // but do not run CGame, local Pmove, demo advancement, audio or network
        // usercmd generation while the graphics subsystem is unavailable.
        if self.renderer_restart_suspended {
            self.previous_tick = Instant::now();
            return;
        }
        let input_diag_notices = self
            .render
            .as_ref()
            .map(|render| render.take_input_diagnostic_notices())
            .unwrap_or_default();
        for line in input_diag_notices {
            self.push_console_line(line);
        }
        let console_changed = self.sync_console_log();
        if console_changed && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
        let commands_executed = self.process_console_command_buffer();
        // /vid_restart may have executed inside Cbuf_Execute; stop this very
        // frame before it can send one more +attack or advance local physics.
        if self.renderer_restart_suspended {
            self.previous_tick = Instant::now();
            return;
        }
        // A console `quit` runs here, mid-tick, after request_quit has hidden
        // the window. Running the rest of the frame (UI publish, network, render
        // hand-off) against a hidden exclusive-fullscreen window is what made
        // console quit slower than the menu button. Leave immediately.
        if self.quit_requested {
            return;
        }
        if commands_executed && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
        self.poll_race_ghost_loads();
        self.poll_race_ghost_web();
        self.sync_coastal_debug();
        self.sync_coastal_live();
        let trail_lines = self.strafe_trails.poll();
        let trail_changed = !trail_lines.is_empty();
        for line in trail_lines {
            self.push_console_line(line);
        }
        if trail_changed
            && matches!(
                self.overlay,
                OverlayMode::Console | OverlayMode::StrafeTrails
            )
        {
            self.publish_ui();
        }
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::MainTick,
        );
        let now = Instant::now();
        let dt = now.duration_since(self.previous_tick);
        self.previous_tick = now;

        if self.sun_ray_sent {
            self.sync_sun_ray();
        }

        // Loose source-map live reload is developer-only work and remains idle
        // unless a `.map` editor exists. The 10 Hz metadata poll itself is cheap;
        // actual parsing/reconstruction is triggered only after a stable save.
        self.tick_source_map_disk_reload(now);
        self.tick_display_transition(now);

        if let Some(confirmation) = self.video_confirmation {
            let remaining = confirmation.deadline.saturating_duration_since(now);
            let seconds = (remaining.as_secs() + if remaining.subsec_nanos() != 0 { 1 } else { 0 })
                .min(u32::MAX as u64) as u32;
            if seconds == 0 {
                self.revert_video_settings("NOT CONFIRMED IN TIME");
                return;
            }
            if seconds != confirmation.shown_seconds {
                if let Some(active) = &mut self.video_confirmation {
                    active.shown_seconds = seconds;
                }
                self.publish_ui();
            }
        }

        // TaystJK CG_DoAsync runs each CGame frame before the input command is
        // consumed, so synthetic +moveup/-moveup affects this frame's usercmd.
        self.tick_japro_flipkick();
        self.tick_network(dt);
        self.tick_japro_userinfo_updates();
        self.tick_demo_scrub_preview(now);
        let studio_scene_started = Instant::now();
        self.tick_render_studio_demo_preview(now);
        if let Some(export) = self.render_studio.export.as_mut() {
            export.app_cost(super::render_studio_profile::StudioAppCost::Scene,
                studio_scene_started.elapsed().as_secs_f64() * 1000.0);
        }
        self.tick_render_studio_audio();
        self.tick_menu_ambience();

        // `/map`/`/devmap` now runs a tiny authoritative one-client server.
        // Advance that authority first, publish a decoded snapshot, then let
        // the exact same CGame frame used by remote servers consume it below.
        let local_is_playing = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local && session.phase == SessionPhase::Playing);
        let mut deformation_stamps = Vec::new();
        let mut local_movement_error = None;
        if local_is_playing
            && matches!(self.overlay, OverlayMode::None | OverlayMode::DevRadial | OverlayMode::EmoteRadial)
            && self.captured
        {
            // Local play originates the same CL_CreateCmd action payload as
            // remote play. The old offline path only forwarded attack/alt
            // while noclip was enabled, so normal +attack never reached Pmove.
            if let Some(ps) = self
                .game_session
                .as_ref()
                .filter(|session| session.local)
                .and_then(|session| session.current_snapshot.as_ref())
                .map(|snapshot| snapshot.player_state.clone())
            {
                self.live_input.sync_selection(&ps);
            }
            self.apply_out_of_ammo_changes();
            let command_buttons = crate::net::CommandButtons {
                active: &self.live_buttons,
                forced_moveup: self.japro_flipkick_moveup,
                idrive: self.network.idrive,
                any_key: !self.keys.is_empty() || !self.mouse_buttons_down.is_empty(),
                talking: false,
            };
            let local_cmd = crate::net::movement_cmd(&self.live_input.create_cmd(&command_buttons));

            if let Some(server) = &mut self.local_server {
                server.set_idrive(self.network.idrive);
                server.set_mouse_input_settings(self.mouse_input);
                match server.update(dt, &self.movement_keys, (0.0, 0.0), local_cmd) {
                    Ok(stamps) => deformation_stamps = stamps,
                    Err(error) => local_movement_error = Some(error),
                }
            }
            if local_movement_error.is_none() {
                self.push_local_snapshot(false);
            }
        }
        if let Some(error) = local_movement_error {
            self.console_status = format!("MOVEMENT ERROR: {error}");
            self.set_overlay(OverlayMode::Game);
        }

        let session_is_playing = self
            .game_session
            .as_ref()
            .is_some_and(|playback| playback.phase == SessionPhase::Playing);
        if session_is_playing {
            let advance_result = if self.demo_video_export.is_some() {
                self.tick_demo_video_export(now)
            } else {
                self.tick_demo_playback(now, dt)
            };
            match advance_result {
                Ok(DemoAdvance::Running) => {}
                Ok(DemoAdvance::Completed) => {
                    let summary = self.game_session.as_ref().map(|playback| {
                        format!(
                            "DEMO COMPLETE: {} messages, {} snapshots",
                            playback.messages, playback.snapshots
                        )
                    });
                    if let Some(summary) = summary {
                        println!("{summary}");
                        self.push_console_line(format!("^2{summary}"));
                    }
                    if let Some(playback) = self.game_session.as_mut() {
                        if let Err(error) = playback.hold_demo_end(now) {
                            self.console_status = error;
                            self.push_console_line(format!("^1{}", self.console_status));
                        } else {
                            self.console_status = "DEMO COMPLETE - PAUSED ON FINAL FRAME".into();
                        }
                    }
                    self.publish_transient_ui();
                }
                Err(error) => {
                    self.console_status = error;
                    self.push_console_line(format!("^1{}", self.console_status));
                    eprintln!("{}", self.console_status);
                    if self
                        .game_session
                        .as_ref()
                        .is_some_and(|session| session.live)
                    {
                        // ERR_DROP: a CGame failure ends remote and local live
                        // sessions instead of reviving the legacy solo path.
                        self.disconnect_to_main_menu();
                        return;
                    }
                    self.game_session = None;
                    self.set_overlay(OverlayMode::Game);
                    self.publish_ui();
                }
            }
            // TaystJK reads player state from each CG_Draw2D frame. Our
            // renderer owns a separate thread, so publish only the compact
            // state. No command at all when idle, except the one transition
            // that clears previously active artwork.
            let (zoom_scope, rocket_lock) = self.combat_overlay_state();
            let active = zoom_scope.is_some() || rocket_lock.is_some();
            if active || self.combat_ui_was_active {
                self.render_command(RenderCommand::SetCombatUi {
                    zoom_scope,
                    rocket_lock,
                });
            }
            self.combat_ui_was_active = active;
            self.drain_cgame_notices();
            self.drain_siege_server_actions();
            // jaPRO calls CG_DoAsync at the end of each CGame frame, after
            // CG_DrawActive. Queue any due vstr now; it executes next App tick.
            self.tick_japro_do();
            let duel_mini_scores = self.draw_scores == 3
                && self
                    .game_session
                    .as_ref()
                    .is_some_and(|session| session.client_game.gametype() == 3);
            if self.scoreboard_should_show()
                || self.companion_scoreboard_visible()
                || duel_mini_scores
            {
                // Demo scores are indexed locally; the server's pushed duel
                // and intermission score pushes update live boards. Do not
                // issue background `score` requests, which can suppress chat
                // through the server's shared flood-protection interval.
                self.refresh_demo_scoreboard(now);
            }
        } else if self.combat_ui_was_active {
            // Demo stopped/paused, or the live session ended: clear once.
            self.render_command(RenderCommand::SetCombatUi {
                zoom_scope: None,
                rocket_lock: None,
            });
            self.combat_ui_was_active = false;
        }

        if self.force_select_until.is_some_and(|until| now >= until) {
            self.force_select_until = None;
            self.publish_transient_ui();
        }
        if self.weapon_select_until.is_some_and(|until| now >= until) {
            self.weapon_select_until = None;
            self.publish_transient_ui();
        }
        if self.inventory_select_until.is_some_and(|until| now >= until) {
            self.inventory_select_until = None;
            self.publish_transient_ui();
        }

        self.tick_water_wetness(now);
        self.refresh_authored_oceans();
        if !session_is_playing {
            self.tick_frontend_cinematic(now);
            self.update_solo_player_view_and_presentation();
        }

        // Spectators have no local mouse-motion event to run the event-rate
        // crosshair trace. Trace from this frame's freshly presented follow
        // sample *before* publishing RenderSnapshot; doing it afterward leaves
        // dynamic_crosshair_world one main tick behind only while spectating.
        self.update_crosshair_target(now);
        self.update_model_placement();
        if self.video.depth_of_field_strength > 0.001
            && self.video.dof_mode == ui::DofMode::On && self.video.dof_autofocus {
            if let Some(hit) = self.crosshair_hit_world {
                self.dof_focus_target = self
                    .camera
                    .position
                    .distance(glam::Vec3::from_array(hit))
                    .clamp(32.0, 16_384.0);
            } else if let Some(world) = self.map_collision.as_mut() {
                use jka_movement::TraceWorld;
                let start = scene::jka_position(self.camera.position.to_array());
                let direction = scene::jka_position(self.camera.forward().to_array());
                let end = std::array::from_fn(|axis| start[axis] + direction[axis] * 16_384.0);
                let result = world.trace(jka_movement::TraceQuery {
                    start,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    end,
                    pass_entity: -1,
                    mask: 0x1 | 0x1000,
                });
                let hit = glam::Vec3::from_array(scene::render_position(result.end));
                self.dof_focus_target = self.camera.position.distance(hit).clamp(32.0, 16_384.0);
            }
        }
        for stamp in deformation_stamps {
            crate::surface_deformation::trace_stamp(stamp);
            self.render_command(RenderCommand::AddSurfaceDeformation(stamp));
        }
        // Read-only bridge: accepted events are drained in delivery order; only
        // clone playerstate ~60 Hz, never per render frame. All Rhai work is
        // performed by the extension worker, not App::tick.
        let extension_events = self.game_session.as_mut()
            .map(|session| std::mem::take(&mut session.extension_events))
            .unwrap_or_default();
        self.extensions.queue_events(extension_events);
        if self.extensions.wants_frame(now) {
            let state = self.game_session.as_ref()
                .filter(|session| session.phase == SessionPhase::Playing)
                .map(|session| crate::extensions::GameStateSnapshot {
                    presented: session.presented_player_state()
                        .filter(|ps| ps.field_i32("clientNum") == session.client_game.presentation_client_num())
                        .cloned(),
                    authoritative: session.client_game.presentation_player_state().cloned(),
                    map_name: session.map_name.clone().unwrap_or_default(),
                    server_time: session.cgame_time,
                    viewer_client: session.client_game.presentation_client_num(),
                    is_live: session.live || session.local,
                    is_demo: !session.live && !session.local,
                    screen_size: self.window.as_ref().map_or([0, 0], |w| {
                        let size = w.inner_size();
                        [size.width, size.height]
                    }),
                });
            self.extensions.submit_frame(now, state);
        }
        self.update_player_name_visibility(now);
        self.ensure_inline_instance_capacity();
        let snapshot_started = Instant::now();
        self.publish_snapshot();
        if let Some(export) = self.render_studio.export.as_mut() {
            export.app_cost(super::render_studio_profile::StudioAppCost::Snapshot,
                snapshot_started.elapsed().as_secs_f64() * 1000.0);
        }
        self.queue_demo_video_capture();
        self.tick_render_studio_export();

        let follow_name = self.current_follow_name();
        let game_timer = self.game_timer_text();
        let mini_scores = self.mini_scores_ui();
        let hud_score = self.hud_score_ui();
        let enemy_info = self.enemy_info_ui();
        let race_timer = self
            .game_session
            .as_ref()
            .and_then(|session| session.race_timer_ui.clone());
        let vote_line = self.vote_hud_line();
        let team_overlay = self.team_overlay_ui();
        self.update_speedometer(now);
        self.update_variometer();
        self.update_lagometer();
        if follow_name != self.follow_name_shown
            || game_timer != self.game_timer_shown
            || mini_scores != self.mini_scores_shown
            || hud_score != self.hud_score_shown
            || enemy_info != self.enemy_info_shown
            || race_timer != self.race_timer_shown
            || vote_line != self.vote_line_shown
            || team_overlay != self.team_overlay_shown
            || self.speedometer_ui != self.speedometer_shown
            || self.lagometer_ui != self.lagometer_shown
        {
            self.follow_name_shown = follow_name;
            self.game_timer_shown = game_timer;
            self.mini_scores_shown = mini_scores;
            self.hud_score_shown = hud_score;
            self.enemy_info_shown = enemy_info;
            self.race_timer_shown = race_timer;
            self.vote_line_shown = vote_line;
            self.team_overlay_shown = team_overlay;
            self.speedometer_shown = self.speedometer_ui.clone();
            self.lagometer_shown = self.lagometer_ui.clone();
            self.publish_transient_ui();
        }
        self.tick_follow_fastest(now);

        let refresh_chat = !self.chat_lines.is_empty()
            && now.duration_since(self.last_chat_refresh) >= Duration::from_millis(250);
        if refresh_chat {
            let demo_now = self.current_demo_elapsed_ms(now);
            // Keep lines for the typing view (cg_chatHistoryTime), not just
            // while they are on screen (cg_chatFadeTime).
            let keep_s = self.chat_settings.history_time.max(self.chat_settings.fade_time).max(1.0);
            self.chat_lines
                .retain(|line| match (demo_now, line.demo_elapsed_ms) {
                    (Some(now_ms), Some(created_ms)) => now_ms - created_ms < f64::from(keep_s) * 1000.0,
                    _ => now.saturating_duration_since(line.created) < Duration::from_secs_f32(keep_s),
                });
            self.last_chat_refresh = now;
        }
        let refresh_center = self.center_print.is_some()
            && now.duration_since(self.last_center_refresh) >= Duration::from_millis(50);
        if refresh_center {
            let demo_now = self.current_demo_elapsed_ms(now);
            if self.center_print.as_ref().is_some_and(|print| {
                match (demo_now, print.demo_elapsed_ms) {
                    (Some(now_ms), Some(created_ms)) => now_ms - created_ms >= 3_000.0,
                    _ => now.saturating_duration_since(print.created) >= Duration::from_secs(3),
                }
            }) {
                self.center_print = None;
            }
            self.last_center_refresh = now;
        }
        // TaystJK does not fade notify rows: it stops drawing a row once its age
        // exceeds con_notifytime. Refresh lightly until that boundary passes so
        // expiration does not require rebuilding retained UI every frame.
        let notify_lifetime_ms = f64::from(self.console_notify_time) * 1000.0;
        let demo_now = self.current_demo_elapsed_ms(now);
        let notify_clock_active = self.console_notify.back().is_some_and(|line| {
            if notify_lifetime_ms < 0.0 {
                return false;
            }
            let age_ms = match (demo_now, line.demo_elapsed_ms) {
                (Some(now_ms), Some(created_ms)) => (now_ms - created_ms).max(0.0),
                _ => now.saturating_duration_since(line.created).as_secs_f64() * 1000.0,
            };
            age_ms <= notify_lifetime_ms + 100.0
        });
        let refresh_console_notify = notify_clock_active
            && now.duration_since(self.last_console_notify_refresh) >= Duration::from_millis(50);
        if refresh_console_notify {
            self.last_console_notify_refresh = now;
        }
        let refresh_demo_timeline = self.demo_playback_active()
            && (self.demo_scrub_dragging
                || now.saturating_duration_since(self.last_demo_timeline_refresh)
                    >= Duration::from_millis(16));
        if refresh_demo_timeline {
            self.last_demo_timeline_refresh = now;
        }
        self.tick_match_stats(now);
        let refresh_chat_anim = self.tick_chat_anim(now);
        let refresh_killfeed = self.tick_killfeed(now);
        let refresh_scoreboard = self.tick_scoreboard_ui();
        if refresh_chat || refresh_center || refresh_console_notify || refresh_demo_timeline
            || refresh_killfeed || refresh_scoreboard || refresh_chat_anim
        {
            // Fade/timeline updates are intentionally isolated from retained UI.
            // This avoids cloning console/menu state and rebuilding unrelated HUD
            // geometry at render frequency.
            self.publish_transient_ui();
        }

        if self.video.draw_entities {
            self.rebuild_entity_markers();
        }
        self.tick_egui_menu();

        if self.config_dirty && self.last_config_write.elapsed() >= Duration::from_millis(250) {
            self.queue_config_save();
        }
    }

    fn sync_coastal_debug(&mut self) {
        let mode = self.coastal_debug_mode;
        let job = self
            .prepared_map_cache
            .as_ref()
            .and_then(|cache| cache.map.coastal_bathymetry.as_ref())
            .cloned();
        let pointer = job
            .as_ref()
            .map_or(0, |job| std::sync::Arc::as_ptr(job) as usize);
        let status = job.as_ref().map_or(2, |job| job.debug_status_tag());
        let key = (pointer, status, mode);
        if self.coastal_debug_sent == Some(key) {
            return;
        }
        let (mesh, lines) = if mode == 0 {
            (None, Vec::new())
        } else if let Some(job) = job {
            job.debug_view(mode, &self.promoted_oceans)
        } else {
            (
                None,
                vec!["No coastal geometry job for this map (enable ocean and load a BSP with promoted water).".into()],
            )
        };
        self.coastal_debug_sent = Some(key);
        self.render_command(RenderCommand::SetCoastalDebugMesh(mesh));
        for line in lines {
            self.push_console_line(line);
        }
    }

    fn sync_coastal_live(&mut self) {
        let job=self.prepared_map_cache.as_ref().and_then(|c|c.map.coastal_bathymetry.as_ref()).cloned();
        let pointer=job.as_ref().map_or(0,|j|std::sync::Arc::as_ptr(j) as usize);
        let status=job.as_ref().map_or(2,|j|j.debug_status_tag());
        let key=(pointer,status,self.coastal_enabled,self.coastal_pool);
        if self.coastal_live_sent==Some(key) {return;}
        let atlas=job.as_ref().and_then(|j| {
            let result=j.status.try_lock().ok()?;
            if let crate::ocean::bathymetry::Status::Ready {atlas,..}=&*result {Some(std::sync::Arc::clone(atlas))} else {None}
        });
        // Retry publication if the terminal result was temporarily locked.
        if status==1 && atlas.is_none() {return;}
        self.coastal_live_sent=Some(key);
        if self.coastal_enabled {
            self.push_console_line(if atlas.is_some() {
                format!("Coastal GPU solver requested: pool {}. See COASTAL LIVE in latest.log for selection/fallback; r_coastal 0 restores FFT.",
                    if self.coastal_pool==0 {"automatic".to_owned()} else {self.coastal_pool.to_string()})
            } else if status==0 {"Coastal GPU solver waiting for bathymetry bake.".into()}
                else {"Coastal unavailable: load a BSP with r_ocean 1 so its bathymetry can be prepared.".into()});
        }
        let upstream_source=job.as_ref().map(|j|std::sync::Arc::clone(&j.upstream_shore));
        self.render_command(RenderCommand::SetCoastalWater {enabled:self.coastal_enabled,pool:self.coastal_pool,atlas,upstream_source});
    }
}
