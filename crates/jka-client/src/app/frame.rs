//! Frame.
use crate::app::{
    rendering_third_person, ui, App, Arc, ClientFramePerf, FrontendPage, InputLatencySample,
    Instant, LocalServer, MapEditor, OverlayMode, PlayerViewPolicyState, RenderCommand,
    RenderSnapshot, SessionPhase, ViewLatchMode,
};

impl App {
    pub(in crate::app) fn subframe_input_view_rotation(&self) -> Option<[f32; 2]> {
        if !self.live_remote_view_forced() {
            if let Some(angles) = self.live_view_angles() {
                return Some([angles[1].to_radians(), -angles[0].to_radians()]);
            }
        }
        self.local_server.as_ref().and_then(|server| {
            if server.view().view_forced != 0 {
                return None;
            }
            let angles = server.subframe_view_angles();
            Some([angles[1].to_radians(), -angles[0].to_radians()])
        })
    }

    pub(in crate::app) fn render_snapshot(&self) -> RenderSnapshot {
        let demo_player_position = self
            .game_session
            .as_ref()
            .and_then(|playback| playback.player_position);
        let dynamic_models = if self.profile_preview_active {
            Arc::clone(&self.profile_dynamic_models)
        } else {
            self.game_session
                .as_ref()
                .map(|playback| {
                    if playback.fx_surfaces.is_empty() {
                        Arc::clone(&playback.dynamic_models)
                    } else {
                        let mut merged = Vec::with_capacity(
                            playback.dynamic_models.len() + playback.fx_surfaces.len(),
                        );
                        merged.extend(playback.dynamic_models.iter().cloned());
                        merged.extend(playback.fx_surfaces.iter().cloned());
                        Arc::new(merged)
                    }
                })
                .unwrap_or_else(|| Arc::clone(&self.solo_dynamic_models))
        };
        let dynamic_models = if self.overlay == OverlayMode::MapEdit {
            if let Some(preview) = self
                .map_editor
                .as_ref()
                .and_then(MapEditor::preview_surface)
            {
                let mut merged = Vec::with_capacity(dynamic_models.len() + 1);
                merged.extend(dynamic_models.iter().cloned());
                merged.push(preview);
                Arc::new(merged)
            } else {
                Arc::clone(&dynamic_models)
            }
        } else {
            dynamic_models
        };
        // CGame owns mover state once it has a snapshot (demo, live, or the
        // solo shim, which spawns brush entities as ET_MOVERs); before that
        // inline models keep their compiled pose.
        let inline_models = self
            .game_session
            .as_ref()
            .filter(|playback| playback.current_snapshot.is_some())
            .map(|playback| Arc::clone(&playback.inline_models));
        let input_view_rotation = self.subframe_input_view_rotation();
        RenderSnapshot {
            model_frame_source: self.network.model_frame_debug.then(|| {
                let playback = self.game_session.as_ref();
                crate::model_frame_log::FrameSource::new(
                    playback
                        .and_then(|s| s.audio_followed_entity.as_ref())
                        .map_or(1023, |e| e.number),
                    playback.and_then(|s| s.player_presenter.viewer_anim_debug()),
                )
            }),
            camera: if self.profile_preview_active
                || (self.front_end && self.frontend_page == FrontendPage::AssetViewer)
            {
                self.asset_preview_camera()
            } else {
                let mut camera = self.camera;
                camera.set_presentation_fov(self.japro_effective_fov(Instant::now()));
                if let Some(playback) = self.game_session.as_ref() {
                    // Render space is Y-up: landing dip and stair smoothing move the eye vertically.
                    camera.position.y += playback.view_offset_z;
                }
                camera
            },
            view_latch: if self.front_end || input_view_rotation.is_none() {
                ViewLatchMode::Disabled
            } else {
                self.render_view_latch
            },
            input_view_rotation,
            dynamic_crosshair_world: self
                .crosshair_target
                .dynamic
                .then_some(self.crosshair_hit_world)
                .flatten(),
            player_position: demo_player_position
                .or_else(|| self.local_server.as_ref().map(LocalServer::world_position)),
            area_mask: self
                .game_session
                .as_ref()
                .and_then(|playback| playback.current_snapshot.as_ref())
                .map(|snapshot| snapshot.area_mask),
            dynamic_models,
            transient_lights: if self.profile_preview_active {
                Arc::new(Vec::new())
            } else {
                self.game_session.as_ref().map_or_else(
                    || Arc::new(Vec::new()),
                    |playback| Arc::clone(&playback.fx_lights),
                )
            },
            screen_fx: self.game_session.as_ref().map_or_else(
                || Arc::new(Vec::new()),
                |playback| Arc::clone(&playback.screen_fx),
            ),
            cloud_foreground: if self.profile_preview_active {
                Arc::new(Vec::new())
            } else {
                self.game_session.as_ref().map_or_else(
                    || Arc::new(Vec::new()),
                    |playback| Arc::clone(&playback.fx_blades),
                )
            },
            inline_models,
            companion_scene: self.companion_scene_view(),
            dof_focus_target: self.dof_focus_target,
            input_latency: self.last_simulated_mouse_input,
            client_perf: self
                .game_session
                .as_ref()
                .map_or_else(ClientFramePerf::default, |playback| playback.client_perf),
            hud: self.current_hud_state(),
            movement_hud: self.current_movement_hud_state(),
            player_names: self.world_player_names(),
            jump_shade: self.current_jump_shade(),
            camera_shake: self
                .game_session
                .as_ref()
                .and_then(|playback| playback.event_presenter.camera_shake()),
        }
    }

    /// CG_OutOfAmmoChange for EV_NOAMMO events the session queued.
    pub(in crate::app) fn apply_out_of_ammo_changes(&mut self) {
        let auto_switch = self.audio.game.auto_switch;
        let Some(session) = self.game_session.as_mut() else {
            return;
        };
        if session.pending_out_of_ammo.is_empty() && session.pending_weapon_select.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut session.pending_out_of_ammo);
        let picked = std::mem::take(&mut session.pending_weapon_select);
        let Some(snapshot) = session.current_snapshot.as_ref() else {
            return;
        };
        if auto_switch > 0 {
            for (old, dry) in pending {
                let mut ps = snapshot.player_state.clone();
                if dry > 0 {
                    ps.stats[4] &= !(1 << dry);
                }
                self.live_input.out_of_ammo_change(&ps, old, auto_switch);
            }
        }
        for tag in picked {
            self.live_input
                .pickup_autoswitch(&snapshot.player_state, tag, auto_switch);
        }
    }

    pub(in crate::app) fn publish_snapshot(&self) {
        if let Some(render) = &self.render {
            render.publish(self.render_snapshot());
        }
    }

    /// Feed the render-only latest-view mailbox directly from live mouse input.
    /// The mailbox always contains raw local viewangles; RenderSnapshot::view_latch
    /// decides whether they map directly to first person or through the cheap
    /// no-trace portion of the existing OpenJK third-person camera.
    pub(in crate::app) fn publish_live_subframe_view(&self, input: InputLatencySample) {
        if self.live_remote_view_forced() {
            return;
        }
        let Some(view_angles) = self.live_view_angles() else {
            return;
        };
        if let Some(render) = &self.render {
            render.publish_subframe_view_rotation(
                view_angles[1].to_radians(),
                -view_angles[0].to_radians(),
                self.crosshair_target
                    .dynamic
                    .then_some(self.crosshair_hit_world)
                    .flatten(),
                input,
            );
        }
    }

    pub(in crate::app) fn publish_local_subframe_view(&self, input: InputLatencySample) -> bool {
        let Some(server) = self.local_server.as_ref() else {
            return false;
        };
        let player_view = server.view();
        if player_view.view_forced != 0 {
            return false;
        }
        let entity_view = server.entity_view();
        let policy = PlayerViewPolicyState::from_entity_view(entity_view, player_view.legs_timer);
        if rendering_third_person(self.third_person, self.first_person_lightsaber, policy)
            && !matches!(self.render_view_latch, ViewLatchMode::ThirdPerson(_))
        {
            // Around a wall, during camera damping, or on the first camera frame,
            // keep the pre-existing local fallback that reruns the authoritative
            // CGame camera instead of using an untraced third-person orbit.
            return false;
        }
        let view_angles = server.subframe_view_angles();
        if let Some(render) = &self.render {
            render.publish_subframe_view_rotation(
                view_angles[1].to_radians(),
                -view_angles[0].to_radians(),
                self.crosshair_target
                    .dynamic
                    .then_some(self.crosshair_hit_world)
                    .flatten(),
                input,
            );
            return true;
        }
        false
    }

    pub(in crate::app) fn publish_ui(&self) {
        if self.settings_batch_open {
            self.ui_publish_pending.set(true);
            return;
        }
        self.render_command(RenderCommand::SetUi(self.ui_snapshot()));
    }

    /// Start a batch of settings changes. The renderer defers pipeline-variant
    /// activation and this side coalesces UI snapshots until `end_settings_batch`.
    pub(in crate::app) fn begin_settings_batch(&mut self, title: &str) {
        self.settings_batch_open = true;
        self.render_command(RenderCommand::BatchBegin(title.to_owned()));
    }

    pub(in crate::app) fn end_settings_batch(&mut self) {
        self.settings_batch_open = false;
        if self.ui_publish_pending.take() {
            self.publish_ui();
        }
        self.render_command(RenderCommand::BatchEnd);
    }

    pub(in crate::app) fn force_select_ui(&self, now: Instant) -> Option<ui::UiForceSelect> {
        let until = self.force_select_until?;
        if now >= until {
            return None;
        }
        let ps = self.live_player_state()?;
        if ps.stats[0] <= 0 {
            return None;
        }
        let known_bits = ps.field_i32("fd.forcePowersKnown").unwrap_or(0) as u32;
        Some(ui::UiForceSelect {
            selected: self.live_input.force_select,
            known_bits,
        })
    }

    pub(in crate::app) fn publish_transient_ui(&self) {
        let (mut chat_lines, mut center_print) = self.transient_ui_snapshot();
        let demo_timeline = self.demo_timeline_ui(Instant::now());
        let prediction_debug = self.prediction_debug_ui();
        let mut crosshair_target = self.crosshair_target.clone();
        let mut follow_name = self.current_follow_name();
        let mut race_timer = self
            .game_session
            .as_ref()
            .and_then(|session| session.race_timer_ui.clone());
        let mut vote_line = self.vote_hud_line();
        let mut speedometer = self.speedometer_ui.clone();
        let team_overlay = self.team_overlay_ui();
        let scoreboard = self.visible_scoreboard();
        let scoreboard_focus_client = self.scoreboard_focus_client();
        let force_select = self.force_select_ui(Instant::now());
        if self.overlay == OverlayMode::HudEdit {
            self.hud_edit_transient_samples(
                &mut chat_lines,
                &mut center_print,
                &mut follow_name,
                &mut vote_line,
                &mut crosshair_target,
                &mut race_timer,
                &mut speedometer,
            );
        }
        self.render_command(RenderCommand::SetTransientUi {
            chat_lines,
            center_print,
            demo_timeline,
            prediction_debug,
            crosshair_target,
            force_select,
            follow_name,
            game_timer: self.game_timer_text(),
            mini_scores: self.mini_scores_ui(),
            race_timer,
            vote_line,
            scoreboard,
            scoreboard_focus_client,
            speedometer,
            lagometer: self.lagometer_ui.clone(),
            team_overlay,
        });
    }

    pub(in crate::app) fn publish_telemetry_ui(&self) {
        self.render_command(RenderCommand::SetUiTelemetry {
            perf: self.perf,
            threads: self.threads,
        });
    }

    pub(in crate::app) fn render_command(&self, command: RenderCommand) {
        if let Some(render) = &self.render {
            render.command(command);
        }
    }

    pub(in crate::app) fn demo_playback_active(&self) -> bool {
        self.game_session.as_ref().is_some_and(|session| {
            !session.live && session.phase == SessionPhase::Playing && session.demo_index.is_some()
        })
    }
}
