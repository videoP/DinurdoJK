//! Tick.
use crate::app::{
    scene, App, DemoAdvance, Duration, Instant, OverlayMode, RenderCommand, SessionPhase,
};

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
        let seconds = now
            .saturating_duration_since(cinematic.started)
            .as_secs_f32();
        self.camera.position = glam::Vec3::from_array(cinematic.position);
        self.camera.position.y += (seconds * 0.32).sin() * 0.65;
        self.camera.yaw = cinematic.yaw + (seconds * 0.115).sin() * 0.28;
        self.camera.pitch = (-2.0_f32).to_radians() + (seconds * 0.09).sin() * 0.025;
    }

    pub(in crate::app) fn tick(&mut self) {
        let console_changed = self.sync_console_log();
        if console_changed && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
        let commands_executed = self.process_console_command_buffer();
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
        self.tick_demo_scrub_preview(now);

        // `/map`/`/devmap` now runs a tiny authoritative one-client server.
        // Advance that authority first, publish a decoded snapshot, then let
        // the exact same CGame frame used by remote servers consume it below.
        let local_is_playing = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local && session.phase == SessionPhase::Playing);
        let mut deformation_stamps = Vec::new();
        let mut local_movement_error = None;
        if local_is_playing && self.overlay == OverlayMode::None && self.captured {
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
                any_key: !self.keys.is_empty() || !self.mouse_buttons_down.is_empty(),
                talking: false,
            };
            let local_cmd = crate::net::movement_cmd(&self.live_input.create_cmd(&command_buttons));

            if let Some(server) = &mut self.local_server {
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
            match self.tick_demo_playback(now, dt) {
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
            self.drain_cgame_notices();
            let duel_mini_scores = self.draw_scores == 3
                && self
                    .game_session
                    .as_ref()
                    .is_some_and(|session| session.client_game.gametype() == 3);
            if self.scoreboard_should_show()
                || self.companion_scoreboard_visible()
                || duel_mini_scores
            {
                let demo_scores = self.refresh_demo_scoreboard(now);
                if duel_mini_scores && !demo_scores {
                    // Tayst's duel HUD reads clientInfo.score; keep our equivalent
                    // score records fresh even when the full scoreboard is hidden.
                    self.request_scores_if_due(false);
                }
            }
        }

        if self.force_select_until.is_some_and(|until| now >= until) {
            self.force_select_until = None;
            self.publish_transient_ui();
        }

        self.refresh_authored_oceans();
        if !session_is_playing {
            self.tick_frontend_cinematic(now);
            self.update_solo_player_view_and_presentation();
        }
        if self.video.depth_of_field_strength > 0.001 && self.video.dof_autofocus {
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
        self.update_player_name_visibility(now);
        self.publish_snapshot();

        let follow_name = self.current_follow_name();
        let game_timer = self.game_timer_text();
        let mini_scores = self.mini_scores_ui();
        let race_timer = self
            .game_session
            .as_ref()
            .and_then(|session| session.race_timer_ui.clone());
        let vote_line = self.vote_hud_line();
        let team_overlay = self.team_overlay_ui();
        self.update_speedometer(now);
        self.update_lagometer();
        if follow_name != self.follow_name_shown
            || game_timer != self.game_timer_shown
            || mini_scores != self.mini_scores_shown
            || race_timer != self.race_timer_shown
            || vote_line != self.vote_line_shown
            || team_overlay != self.team_overlay_shown
            || self.speedometer_ui != self.speedometer_shown
            || self.lagometer_ui != self.lagometer_shown
        {
            self.follow_name_shown = follow_name;
            self.game_timer_shown = game_timer;
            self.mini_scores_shown = mini_scores;
            self.race_timer_shown = race_timer;
            self.vote_line_shown = vote_line;
            self.team_overlay_shown = team_overlay;
            self.speedometer_shown = self.speedometer_ui.clone();
            self.lagometer_shown = self.lagometer_ui.clone();
            self.publish_transient_ui();
        }
        self.tick_follow_fastest(now);
        self.update_crosshair_target(now);

        let refresh_chat = !self.chat_lines.is_empty()
            && now.duration_since(self.last_chat_refresh) >= Duration::from_millis(250);
        if refresh_chat {
            let demo_now = self.current_demo_elapsed_ms(now);
            self.chat_lines
                .retain(|line| match (demo_now, line.demo_elapsed_ms) {
                    (Some(now_ms), Some(created_ms)) => now_ms - created_ms < 10_000.0,
                    _ => now.saturating_duration_since(line.created) < Duration::from_secs(10),
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
        let refresh_demo_timeline = self.demo_playback_active()
            && (self.demo_scrub_dragging
                || now.saturating_duration_since(self.last_demo_timeline_refresh)
                    >= Duration::from_millis(16));
        if refresh_demo_timeline {
            self.last_demo_timeline_refresh = now;
        }
        if refresh_chat || refresh_center || refresh_demo_timeline {
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
            self.flush_config();
        }
    }
}
