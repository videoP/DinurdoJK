//! Session advance.
use crate::app::{
    event_debug, event_workers, local_player_alpha, playerstate_vec3, race_ghost,
    rendering_third_person, sample_player_state, scene, snapshot_discontinuity,
    snapshot_owns_playerstate_events, Arc, ClientFramePerf, DemoAdvance, DemoCameraSample,
    EventCheckDisposition, GameSession, Ghoul2PresentationView, Instant, LivePredictionFrame,
    PresentedEntity, ProtocolSnapshot, SessionPhase, SpectatorCameraMode, SpectatorCameraSettings,
    ThirdPersonSettings, TransientLight,
};

impl GameSession {
    pub(in crate::app) fn advance(
        &mut self,
        now: Instant,
        third_person: ThirdPersonSettings,
        spectator_camera: SpectatorCameraSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        event_workers_enabled: bool,
        ghoul2_view: Option<Ghoul2PresentationView>,
        rt_rigid_casters_enabled: bool,
        blob_shadows_enabled: bool,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        if self.phase != SessionPhase::Playing {
            return Err("DEMO PLAYBACK ADVANCED BEFORE MAP LOAD".to_owned());
        }
        let presentation_time_ms = self
            .timeline
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?
            .target_time_ms(now);
        let target_server_time = presentation_time_ms.floor() as i32;
        self.advance_to_with_prediction(
            target_server_time,
            presentation_time_ms,
            third_person,
            spectator_camera,
            first_person_lightsaber,
            debug_events,
            event_workers_enabled,
            ghoul2_view,
            rt_rigid_casters_enabled,
            blob_shadows_enabled,
            false,
            None,
        )
    }

    pub(in crate::app) fn advance_to(
        &mut self,
        target_server_time: i32,
        third_person: ThirdPersonSettings,
        spectator_camera: SpectatorCameraSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        event_workers_enabled: bool,
        ghoul2_view: Option<Ghoul2PresentationView>,
        rt_rigid_casters_enabled: bool,
        blob_shadows_enabled: bool,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        self.advance_to_with_prediction(
            target_server_time,
            f64::from(target_server_time),
            third_person,
            spectator_camera,
            first_person_lightsaber,
            debug_events,
            event_workers_enabled,
            ghoul2_view,
            rt_rigid_casters_enabled,
            blob_shadows_enabled,
            false,
            None,
        )
    }

    /// `predict` is CG_PredictPlayerState: given this frame's snap/nextSnap it
    /// returns cg.predictedPlayerState plus the decaying view error.
    pub(in crate::app) fn advance_to_with_prediction(
        &mut self,
        target_server_time: i32,
        presentation_time_ms: f64,
        third_person: ThirdPersonSettings,
        spectator_camera: SpectatorCameraSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        event_workers_enabled: bool,
        ghoul2_view: Option<Ghoul2PresentationView>,
        rt_rigid_casters_enabled: bool,
        blob_shadows_enabled: bool,
        no_predict: bool,
        predict: Option<
            &mut dyn FnMut(
                &ProtocolSnapshot,
                Option<&ProtocolSnapshot>,
                &[PresentedEntity],
            ) -> Result<Option<LivePredictionFrame>, String>,
        >,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        self.client_perf = ClientFramePerf::default();
        self.player_presenter.begin_perf_frame();
        self.player_presenter
            .set_rt_shadow_casters_enabled(rt_rigid_casters_enabled);
        self.player_presenter
            .begin_blob_shadow_frame(blob_shadows_enabled);
        self.client_perf.ghoul2_skinning_mode = self.player_presenter.skinning_mode();
        let snapshot_started = Instant::now();
        let current_time = self
            .current_snapshot
            .as_ref()
            .map(|snapshot| snapshot.server_time)
            .ok_or_else(|| "DEMO HAS NO CURRENT SNAPSHOT".to_owned())?;
        // Live cl.serverTime can trail the newest snapshot right after the
        // first one or a time reset; CG_ProcessSnapshots simply holds there.
        let target_server_time = if self.live {
            target_server_time.max(current_time)
        } else {
            target_server_time
        };
        let presentation_time_ms = if self.live {
            presentation_time_ms.max(f64::from(current_time))
        } else {
            presentation_time_ms
        };
        if target_server_time < current_time {
            return Err(format!(
                "DEMO REVERSE/SEEK REQUIRES CHECKPOINT RESTORE: target {target_server_time} < current snapshot {current_time}"
            ));
        }

        if let (Some(current), Some(next)) = (&self.current_snapshot, &self.next_snapshot) {
            if next.server_time < current.server_time {
                return Err(format!(
                    "DEMO SERVER TIME WENT BACKWARDS: message {} time {} after message {} time {}",
                    next.message_num, next.server_time, current.message_num, current.server_time
                ));
            }
        }

        // CG_ProcessSnapshots: with no nextSnap, try to read one before
        // deciding to extrapolate. Live snapshots arrive between frames.
        if self.next_snapshot.is_none() {
            self.next_snapshot = self.read_next_snapshot()?;
            if self.next_snapshot.is_some() {
                self.client_game
                    .set_next_snapshot(self.next_snapshot.as_ref())?;
            }
        }

        let mut camera_teleported = false;
        while self
            .next_snapshot
            .as_ref()
            .is_some_and(|snapshot| target_server_time >= snapshot.server_time)
        {
            if let (Some(current), Some(next)) = (&self.current_snapshot, &self.next_snapshot) {
                camera_teleported |= snapshot_discontinuity(current, next);
                // TaystJK CG_TransitionSnapshot: when client-side movement
                // prediction does not own the presented playerState, committed
                // snapshots must drive CG_TransitionPlayerState themselves.
                // PMF_FOLLOW is especially important here: Predictor::predict
                // intentionally returns no predicted state while spectating a
                // player, so otherwise their predictable EV_JUMP / EV_FALL /
                // step events never reach the shared event/audio path.
                if snapshot_owns_playerstate_events(
                    self.live,
                    self.local,
                    no_predict,
                    self.client_game.synchronous_clients(),
                    &next.player_state,
                ) {
                    self.client_game.transition_predicted_player_state(
                        &next.player_state,
                        &current.player_state,
                        next.server_time,
                    )?;
                }
            }
            self.client_game.transition_snapshot(target_server_time)?;
            self.current_snapshot = self.next_snapshot.take();
            self.next_snapshot = self.read_next_snapshot()?;
            self.client_game
                .set_next_snapshot(self.next_snapshot.as_ref())?;
        }

        // CG_ExecuteNewServerCommands runs before entity presentation. In
        // OpenJK, reliable ircg/rcg commands mutate Ghoul2 instances before
        // CG_AddPacketEntities sees the corresponding body/respawn state.
        for command in self.client_game.drain_ghoul2_commands() {
            self.player_presenter.apply_ghoul2_server_command(command);
        }
        for action in self.client_game.drain_audio_server_actions() {
            if let Some(sound) = self.sound_presenter.as_mut() {
                match action {
                    crate::cgame::CgameServerAction::KillLoopSounds { entities } => {
                        for entity in entities {
                            sound.kill_looping_sound(entity);
                        }
                    }
                    crate::cgame::CgameServerAction::RestartMapMusic => {
                        sound.restart_map_music(&self.client_game);
                    }
                    _ => unreachable!("drain_audio_server_actions returned a non-audio action"),
                }
            }
        }

        // FX_AdjustTime: effects spawned by this frame's events and entities
        // start at cg.time.
        self.weapon_fx.begin_frame(target_server_time);
        self.presented_entities = self.client_game.present_entities_at(presentation_time_ms)?;
        let predicted = match (predict, self.current_snapshot.as_ref()) {
            (Some(predict), Some(current)) => predict(
                current,
                self.next_snapshot.as_ref(),
                &self.presented_entities,
            )?,
            _ => None,
        };
        if let Some(prediction) = &predicted {
            // CG_TransitionPlayerState / CG_CheckPlayerstateEvents on the state
            // being presented, so events land on the frame they are drawn.
            self.client_game.transition_predicted_player_state(
                &prediction.display,
                &prediction.previous_display,
                target_server_time,
            )?;
        }
        let sample = match &predicted {
            Some(prediction) => sample_player_state(
                &prediction.display,
                target_server_time,
                camera_teleported,
                prediction.error,
            ),
            None => self.sample_at(presentation_time_ms, target_server_time, camera_teleported),
        }
        .ok_or_else(|| "DEMO CURRENT SNAPSHOT HAS NO CAMERA PLAYERSTATE".to_owned())?;
        self.client_perf.snapshot_ms = snapshot_started.elapsed().as_secs_f64() * 1000.0;

        // CG_AddEntities adds cg.predictedPlayerEntity separately from packet
        // entities. Audio has the same ownership rule: the local/followed
        // player's loops and entity position must still exist even though
        // present_entities() intentionally skips that packet entity.
        let followed_entity = match &predicted {
            Some(prediction) => crate::cgame::presented_player_state_entity(&prediction.display),
            None => self
                .client_game
                .present_followed_player_at(presentation_time_ms),
        };
        self.audio_followed_entity.clone_from(&followed_entity);

        // Presentation needs a renderer view before App::apply_demo_camera runs.
        // First person can use this frame's exact sampled eye immediately. Third
        // person needs collision/orbit state owned by App, so it uses the previous
        // rendered third-person camera as a one-frame hint. Never reuse that hint
        // across a teleport/snapshot discontinuity.
        let sample_third_person = if self.live && sample.following {
            spectator_camera.mode != SpectatorCameraMode::FirstPerson
        } else if let Some(mode) = self.demo_spectator_camera_mode {
            mode != SpectatorCameraMode::FirstPerson
        } else {
            rendering_third_person(third_person, first_person_lightsaber, sample.policy)
        };
        let ghoul2_view = if sample.teleported {
            None
        } else if !sample_third_person {
            ghoul2_view.map(|view| {
                let yaw = sample.view_angles[1].to_radians();
                let pitch = -sample.view_angles[0].to_radians();
                let (sy, cy) = yaw.sin_cos();
                let (sp, cp) = pitch.sin_cos();
                view.with_pose(sample.eye_position, [cy * cp, sp, -sy * cp])
            })
        } else {
            ghoul2_view
        };
        let audio_started = Instant::now();
        if !self.suppress_audio {
            if let Some(sound) = &mut self.sound_presenter {
                sound.update_loops(
                    &self.client_game,
                    &self.siege_classes,
                    &self.presented_entities,
                    followed_entity.as_ref(),
                    sample.client_num as u16,
                    scene::jka_position(sample.eye_position),
                    target_server_time,
                );
                let yaw = sample.view_angles[1].to_radians();
                sound.frame(
                    crate::audio::Listener {
                        entity: sample.client_num as u16,
                        origin: scene::jka_position(sample.eye_position),
                        ahead: [yaw.cos(), yaw.sin(), 0.0],
                        up: [0.0, 0.0, 1.0],
                        left: [-yaw.sin(), yaw.cos(), 0.0],
                    },
                    &self.presented_entities,
                    followed_entity.as_ref(),
                );
            }
        }
        self.client_perf.audio_ms += audio_started.elapsed().as_secs_f64() * 1000.0;

        let events_started = Instant::now();
        // OpenJK's cg_debugEvents only prints accepted CG_EntityEvent cases.
        // Keep level 1 similarly compact; level 2 additionally explains why
        // candidate events were suppressed by CG_CheckEvents.
        let traces = self.client_game.drain_event_check_traces();
        for trace in &traces {
            match trace.disposition {
                EventCheckDisposition::Duplicate => {
                    self.event_suppressed_duplicate =
                        self.event_suppressed_duplicate.saturating_add(1);
                }
                EventCheckDisposition::Zero => {
                    self.event_suppressed_zero = self.event_suppressed_zero.saturating_add(1);
                }
                EventCheckDisposition::Accepted | EventCheckDisposition::NoEvent => {}
            }
        }
        if debug_events >= 2 {
            for trace in &traces {
                if let Some(line) = event_debug::trace_line(trace) {
                    self.push_event_debug_line(line);
                }
            }
        }

        let event_count = self.client_game.presentation_event_count();
        if event_workers_enabled && event_count > 1 {
            let queued_events = self.client_game.drain_presentation_events();
            let sound_prep = self
                .sound_presenter
                .as_ref()
                .map(crate::cgame::sound_presenter::SoundPresenter::prep_data);
            let fx_saber_definitions = self.weapon_fx.saber_definitions();
            let prepared_batch = event_workers::prepare_batch(
                queued_events,
                &self.client_game,
                &self.siege_classes,
                sound_prep,
                fx_saber_definitions,
                true,
            );
            self.client_perf.event_prepare_ms = prepared_batch.stats.wall_ms;
            self.client_perf.event_worker_jobs = prepared_batch.stats.jobs;
            self.client_perf.event_worker_threads = prepared_batch.stats.pool_threads;
            self.client_perf.event_worker_parallel = prepared_batch.stats.parallel;

            let prepared_events = prepared_batch.events;
            if !self.suppress_audio {
                if let Some(sound_presenter) = self.sound_presenter.as_mut() {
                    let decode_jobs = sound_presenter.stage_prepared_asset_decodes(
                        prepared_events
                            .iter()
                            .filter(|prepared| {
                                target_server_time.saturating_sub(prepared.event.server_time) <= 250
                            })
                            .filter_map(|prepared| prepared.sound.as_ref()),
                        &self.client_game,
                        &self.siege_classes,
                    );
                    let decoded = event_workers::decode_sound_batch(decode_jobs, true);
                    self.client_perf.event_sound_decode_ms = decoded.stats.wall_ms;
                    self.client_perf.event_sound_decode_jobs = decoded.stats.jobs;
                    self.client_perf.event_sound_decode_parallel = decoded.stats.parallel;
                    sound_presenter.queue_predecoded_assets(decoded.results);
                } else {
                    self.client_perf.event_sound_decode_ms = 0.0;
                    self.client_perf.event_sound_decode_jobs = 0;
                    self.client_perf.event_sound_decode_parallel = false;
                }
            } else {
                self.client_perf.event_sound_decode_ms = 0.0;
                self.client_perf.event_sound_decode_jobs = 0;
                self.client_perf.event_sound_decode_parallel = false;
            }

            for prepared in prepared_events {
                self.dispatch_prepared_event(prepared, target_server_time, debug_events);
            }
        } else {
            // Exact single-thread baseline: no temporary batch allocation. The
            // same pure preparation functions run inline, then side effects are
            // applied immediately in receive-queue order.
            let mut prep_ms = 0.0;
            let mut jobs = 0u32;
            while let Some(event) = self.client_game.pop_presentation_event() {
                let sound_prep = self
                    .sound_presenter
                    .as_ref()
                    .map(crate::cgame::sound_presenter::SoundPresenter::prep_data);
                let fx_saber_definitions = self.weapon_fx.saber_definitions();
                let prep_started = Instant::now();
                let prepared = event_workers::prepare_inline(
                    event,
                    &self.client_game,
                    &self.siege_classes,
                    sound_prep,
                    fx_saber_definitions,
                );
                prep_ms += prep_started.elapsed().as_secs_f64() * 1000.0;
                jobs = jobs.saturating_add(1);
                self.dispatch_prepared_event(prepared, target_server_time, debug_events);
            }
            self.client_perf.event_prepare_ms = prep_ms;
            self.client_perf.event_worker_jobs = jobs;
            self.client_perf.event_worker_threads = 0;
            self.client_perf.event_worker_parallel = false;
            self.client_perf.event_sound_decode_ms = 0.0;
            self.client_perf.event_sound_decode_jobs = 0;
            self.client_perf.event_sound_decode_parallel = false;
        }
        if !self.logged_entity_summary {
            let summary = self.client_game.summarize_entities();
            devprintln!(
                2,
                "DEMO ENTITY TYPES: total={} players={} npcs={} movers={} missiles={} items={} eventEntities={} other={}",
                summary.total,
                summary.players,
                summary.npcs,
                summary.movers,
                summary.missiles,
                summary.items,
                summary.event_entities,
                summary.other,
            );
            devprintln!(
                2,
                "DEMO GAME RULES: gametype={} weaponDisable=0x{:08x}",
                self.client_game.gametype(),
                self.client_game.weapon_disable_mask() as u32,
            );
            for entity in self
                .presented_entities
                .iter()
                .filter(|entity| entity.entity_type == crate::cgame::ET_PLAYER)
            {
                let client_num = entity
                    .state
                    .field_i32("clientNum")
                    .unwrap_or(i32::from(entity.number));
                if client_num < 0 {
                    continue;
                }
                match self
                    .client_game
                    .client_info(client_num as usize, &self.siege_classes)
                {
                    Some(info) => devprintln!(
                        2,
                        "  ET_PLAYER entity={} clientNum={} name={:?} model={}/{} glm={} skin={} siegeclass={:?}",
                        entity.number,
                        client_num,
                        info.name,
                        info.model_name,
                        info.skin_name,
                        info.model_qpath(),
                        info.skin_qpath(),
                        info.siege_class,
                    ),
                    None => devprintln!(
                        2,
                        "  ET_PLAYER entity={} clientNum={} has no CS_PLAYERS configstring",
                        entity.number, client_num
                    ),
                }
            }
            self.logged_entity_summary = true;
        }
        self.client_perf.events_ms = events_started.elapsed().as_secs_f64() * 1000.0;

        self.player_position = Some(glam::Vec3::from_array(sample.player_position));
        self.update_race_timer(
            target_server_time,
            predicted.as_ref().map(|prediction| &prediction.display),
        );
        // CG_AddLagometerFrameInfo: cg.time - cg.latestSnapshotTime, where the latest
        // snapshot is the newest one received, not just the newest one read.
        let latest_snapshot_time = self
            .live_snapshots
            .back()
            .map(|snapshot| snapshot.server_time)
            .or_else(|| {
                self.next_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.server_time)
            })
            .or_else(|| {
                self.current_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.server_time)
            })
            .unwrap_or(target_server_time);
        self.lagometer
            .add_frame_info(target_server_time, latest_snapshot_time);
        self.vote_ui = crate::vote::read(
            |index| {
                self.client_game
                    .configstring(index)
                    .map(crate::cgame::bytes_to_lossless_ascii)
            },
            target_server_time,
        );

        // CG_AddEntities adds cg.predictedPlayerEntity for the local player.
        let followed_entity = match &predicted {
            Some(prediction) => crate::cgame::presented_player_state_entity(&prediction.display),
            None => self
                .client_game
                .present_followed_player_at(presentation_time_ms),
        };
        let followed_entity_num = followed_entity.as_ref().map(|entity| entity.number);
        // forceGripCripple is one of the few exact victim-side grip signals in
        // vanilla playerState. Remote entityState does not transmit
        // forceGripEntityNum, so PlayerPresenter reconstructs those targets
        // from each active gripper's view trace.
        let local_force_gripped = predicted
            .as_ref()
            .map(|prediction| {
                prediction
                    .display
                    .field_i32("fd.forceGripCripple")
                    .unwrap_or(0)
                    != 0
            })
            .or_else(|| {
                self.current_snapshot.as_ref().map(|snapshot| {
                    snapshot
                        .player_state
                        .field_i32("fd.forceGripCripple")
                        .unwrap_or(0)
                        != 0
                })
            })
            .unwrap_or(false);
        self.player_presenter.prepare_force_grip_targets(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_force_gripped,
            target_server_time,
        );
        // PMF_TIME_KNOCKBACK is available only for the local/followed
        // playerState. Remote victims are confirmed from event-correlated
        // entity trajectory changes inside PlayerPresenter.
        const PMF_TIME_KNOCKBACK: i32 = 64;
        let local_knockback = predicted
            .as_ref()
            .map(|prediction| {
                prediction.display.field_i32("pm_flags").unwrap_or(0) & PMF_TIME_KNOCKBACK != 0
            })
            .or_else(|| {
                self.current_snapshot.as_ref().map(|snapshot| {
                    snapshot.player_state.field_i32("pm_flags").unwrap_or(0) & PMF_TIME_KNOCKBACK
                        != 0
                })
            })
            .unwrap_or(false);
        self.player_presenter.prepare_impulse_ragdolls(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_knockback,
            target_server_time,
        );

        // Broadsword historically advanced ragdoll state immediately before
        // Ghoul2 submission. Keep the same presentation ownership: Rapier is
        // stepped once here, while OpenJK snapshot/pmove state stays untouched.
        self.player_presenter
            .begin_physics_frame(target_server_time);

        let entity_present_started = Instant::now();
        let (mut dynamic_models, inline_models, dispatch_summary) =
            self.entity_presenter.present_snapshot_entities(
                &self.presented_entities,
                &self.client_game,
                target_server_time,
                &mut self.player_presenter,
                &mut self.weapon_fx,
                rt_rigid_casters_enabled,
            );
        self.client_perf.entity_present_ms =
            entity_present_started.elapsed().as_secs_f64() * 1000.0;
        self.inline_models = Arc::new(inline_models);
        if !self.logged_dispatch_summary {
            devprintln!(
                2,
                "DEMO ENTITY DISPATCH: general={} movers={} items={} missiles={} npcs={} fx={} other={} renderedSurfaces={}",
                dispatch_summary.general,
                dispatch_summary.movers,
                dispatch_summary.items,
                dispatch_summary.missiles,
                dispatch_summary.npcs,
                dispatch_summary.fx,
                dispatch_summary.other,
                dispatch_summary.rendered_surfaces,
            );
            self.logged_dispatch_summary = true;
        }
        self.player_presenter
            .set_external_live_entities(self.race_ghosts.iter().map(|ghost| ghost.entity_num));
        let player_present_started = Instant::now();
        dynamic_models.extend(
            self.player_presenter.present_snapshot_players(
                &self.presented_entities,
                &self.client_game,
                &self.siege_classes,
                target_server_time,
                followed_entity_num,
                self.demo_hidden_view_client
                    .and_then(|client| u16::try_from(client).ok()),
                ghoul2_view,
                self.forced_player_models.as_ref(),
            ),
        );
        // TaystJK CG_Player: spawn protection is an authoritative eFlags bit
        // and the shell is a separate MD3 refEntity, not a tint on Ghoul2.
        // Use the same presentation view that selected player LODs; in first
        // person it was replaced above with this frame's exact sampled eye.
        let sphere_view_origin = ghoul2_view
            .map(crate::cgame::player_presenter::Ghoul2PresentationView::jka_position)
            .unwrap_or_else(|| scene::jka_position(sample.eye_position));
        dynamic_models.extend(self.entity_presenter.present_invulnerability_bubbles(
            &self.presented_entities,
            followed_entity.as_ref(),
            &self.client_game,
            sphere_view_origin,
        ));
        self.client_perf.player_present_ms =
            player_present_started.elapsed().as_secs_f64() * 1000.0;
        // EternalJK keeps the local Ghoul2 instance active for first-person
        // saber/melee so the attached hilt and blade bolts keep presenting,
        // but cg_fpls mode 3 turns the player-body surfaces off. Other weapons
        // use the separate CG_AddViewWeapon-style first-person viewmodel path.
        const WP_MELEE: i32 = 2;
        const WP_SABER: i32 = 3;
        // EternalJK only applies cg_fpls to the local non-followed player;
        // spectator/follow presentation disables it rather than putting the
        // followed player's Ghoul2 instance around the camera.
        let first_person_saber = !sample_third_person
            && first_person_lightsaber
            && !sample.following
            && matches!(sample.policy.weapon, WP_MELEE | WP_SABER);
        let render_followed_body = sample_third_person || first_person_saber;
        let followed_started = Instant::now();
        let intermission = sample.policy.pm_type == crate::cgame::PM_INTERMISSION;
        if let Some(entity) = followed_entity
            .filter(|entity| !crate::cgame::suppressed_during_intermission(intermission, entity))
        {
            let client_num = entity
                .state
                .field_i32("clientNum")
                .unwrap_or(sample.client_num);
            if client_num >= 0 {
                if let Some(info) = self
                    .client_game
                    .client_info(client_num as usize, &self.siege_classes)
                {
                    let look_target_origin =
                        if entity.state.field_i32("hasLookTarget").unwrap_or(0) != 0 {
                            let look_target = entity.state.field_i32("lookTarget").unwrap_or(-1);
                            self.presented_entities
                                .iter()
                                .find(|candidate| i32::from(candidate.number) == look_target)
                                .map(|candidate| candidate.origin)
                        } else {
                            None
                        };
                    self.player_presenter.queue_player_sprites(
                        &entity,
                        &self.client_game,
                        target_server_time,
                        render_followed_body,
                    );
                    dynamic_models.extend(self.player_presenter.present_player_entity(
                        &entity,
                        &info,
                        target_server_time,
                        local_player_alpha(third_person.alpha),
                        look_target_origin,
                        sample.teleported,
                        render_followed_body,
                        first_person_saber,
                        // Without a view the presenter cannot pick a Ghoul2 LOD,
                        // so the local player's own model would stay at LOD 0.
                        ghoul2_view,
                    )?);
                    // Snapshot players already submit their thrown sabers in
                    // present_snapshot_players. The local/followed player can
                    // instead be synthesized from playerState (and therefore
                    // absent from the packet-entity list), so mirror OpenJK's
                    // CG_Player saberEntityNum path here only for that case.
                    if !self
                        .presented_entities
                        .iter()
                        .any(|candidate| candidate.number == entity.number)
                    {
                        dynamic_models.extend(
                            self.player_presenter.present_thrown_saber_for_player(
                                &entity,
                                &info,
                                &self.presented_entities,
                                &self.client_game,
                                target_server_time,
                                local_player_alpha(third_person.alpha),
                            )?,
                        );
                    }
                    // EternalJK/OpenJK CG_AddViewWeapon. This runs for every
                    // ordinary first-person weapon, including saber/melee; FPLS
                    // additionally keeps the local Ghoul2 saber/body instance above.
                    // The bg_itemlist view_model is attached to the invisible
                    // _hand.md3 tag parent with RF_DEPTHHACK semantics.
                    const TEAM_SPECTATOR: i32 = 3;
                    if !sample_third_person
                        && sample.policy.team != TEAM_SPECTATOR
                        && !intermission
                        && sample.policy.zoom_mode == 0
                        && !matches!(
                            jka_movement::animation_name(sample.policy.torso_anim),
                            Some("BOTH_BUTTON_HOLD")
                        )
                    {
                        let (hand_frame, hand_oldframe, hand_backlerp) =
                            self.player_presenter.view_weapon_frames(
                                entity.number,
                                target_server_time,
                                sample.policy.torso_anim,
                                sample.policy.force_hand_extend,
                            );
                        // CG_CalculateWeaponPosition operates in native JKA space.
                        // `sample.eye_position` is already converted to render space,
                        // while present_view_weapon/present_view_md3 performs the
                        // render-space conversion at final vertex submission. Convert
                        // back here so the view weapon is not transformed twice.
                        let mut weapon_view_origin = scene::jka_position(sample.eye_position);
                        weapon_view_origin[2] += self
                            .event_presenter
                            .view_kick()
                            .offset(target_server_time, true);
                        dynamic_models.extend(
                            self.entity_presenter.present_view_weapon(
                                entity.number,
                                sample.policy.weapon,
                                weapon_view_origin,
                                sample.view_angles,
                                sample.velocity,
                                sample.bob_cycle,
                                target_server_time,
                                self.event_presenter
                                    .view_kick()
                                    .weapon_land_offset(target_server_time),
                                hand_frame,
                                hand_oldframe,
                                hand_backlerp,
                            )?,
                        );
                    }
                }
            }
        }
        self.client_perf.followed_player_ms = followed_started.elapsed().as_secs_f64() * 1000.0;

        // `/rGhost`: the reference player is presentation-only and is driven by
        // the live player's own duelTime race clock. It never enters collision,
        // prediction, entity dispatch, audio, events, or server-visible state.
        self.race_ghost_live_sample = None;
        if self.live && !self.race_ghosts.is_empty() {
            let ghost_present_started = Instant::now();
            let live_race_clock = predicted
                .as_ref()
                .map(|prediction| &prediction.display)
                .or_else(|| {
                    self.current_snapshot
                        .as_ref()
                        .map(|snapshot| &snapshot.player_state)
                })
                .map(|player_state| {
                    (
                        player_state.field_i32("duelTime").unwrap_or(0),
                        player_state.stats[crate::japro_cg::STAT_RACEMODE] != 0,
                        playerstate_vec3(player_state, "origin"),
                        playerstate_vec3(player_state, "velocity"),
                    )
                });
            if let Some((live_duel_time, live_race_mode, live_origin, live_velocity)) =
                live_race_clock
            {
                if live_duel_time > 0 && live_race_mode {
                    if let (Some(origin), Some(velocity)) = (live_origin, live_velocity) {
                        self.race_ghost_live_sample =
                            Some(race_ghost::RaceGhostVisualSample { origin, velocity });
                    }
                }
                let mut ghost_errors = Vec::new();
                for ghost in &mut self.race_ghosts {
                    if let Some((entity, force_reset, _elapsed_ms)) =
                        ghost.sample(live_duel_time, live_race_mode, target_server_time)
                    {
                        match self.player_presenter.present_race_ghost_entity(
                            &entity,
                            &ghost.info,
                            target_server_time,
                            self.race_ghost_alpha,
                            force_reset,
                            ghoul2_view,
                        ) {
                            Ok(mut draws) => dynamic_models.append(&mut draws),
                            Err(error) => ghost_errors.push(format!(
                                "RACE GHOST presentation {}: {error}",
                                ghost.track.demo_name
                            )),
                        }
                    }
                }
                for error in ghost_errors {
                    self.push_event_debug_line(error);
                }
            } else {
                for ghost in &mut self.race_ghosts {
                    ghost.reset_sync();
                }
            }
            self.client_perf.player_present_ms +=
                ghost_present_started.elapsed().as_secs_f64() * 1000.0;
        }

        let cosmetics = self.player_presenter.drain_cosmetic_draws();
        if !cosmetics.is_empty() {
            dynamic_models.extend(self.entity_presenter.present_cosmetics(cosmetics));
        }
        // CG_DrawMiscStaticModels: client-placed map props, culled to the view.
        dynamic_models.extend(self.entity_presenter.present_static_models(ghoul2_view));
        if let Some(blob_shadows) = self.player_presenter.finish_blob_shadow_frame() {
            dynamic_models.push(blob_shadows);
        }
        let fx_started = Instant::now();
        dynamic_models.extend(self.event_presenter.present(target_server_time));
        self.view_offset_z = self
            .event_presenter
            .view_kick()
            .offset(target_server_time, !render_followed_body);
        self.dynamic_models = Arc::new(dynamic_models);
        // Force-power visuals CG_Player queued for this frame.
        for request in self.player_presenter.drain_fx_requests() {
            self.weapon_fx.player_fx(&request);
        }
        // CG_PlayerFootsteps: the sound, dust and print of each footfall the
        // animations found. The local solo player's snow belongs to Snowflow.
        let stages = self.player_presenter.footstep_stages();
        for impact in self.player_presenter.drain_footsteps() {
            if stages.sounds && !self.suppress_audio {
                if let Some(sound) = &mut self.sound_presenter {
                    let variant =
                        crate::cgame::footsteps::sound_variant(&impact, target_server_time);
                    let step = crate::cgame::footsteps::material_step(impact.material);
                    sound.play_footstep(
                        &step.sound.path(impact.foot.heavy(), variant),
                        impact.entity,
                        impact.position,
                    );
                }
            }
            self.weapon_fx
                .footstep(&impact, stages, self.local && impact.entity < 32);
        }
        // FX_AddScheduledEffects + FX_Add after every CGame submission.
        let fx_frame = self.weapon_fx.end_frame();
        if self.screen_shake_level >= 1 {
            let view = scene::jka_position(sample.eye_position);
            for shake in &fx_frame.shakes {
                self.event_presenter.do_camera_shake(
                    view,
                    shake.origin,
                    shake.intensity,
                    shake.radius,
                    shake.time_ms,
                );
            }
        }
        self.fx_draws = fx_frame.draws;
        self.fx_blades = Arc::new(
            fx_frame
                .blades
                .iter()
                .map(|blade| crate::renderer::CloudForegroundBlade {
                    start: scene::render_position(blade.start),
                    end: scene::render_position(blade.end),
                    radius: blade.radius,
                })
                .collect(),
        );
        self.fx_lights = Arc::new(
            fx_frame
                .lights
                .iter()
                .filter(|light| {
                    light.radius.is_finite()
                        && light.radius > 0.0
                        && light.origin.iter().all(|value| value.is_finite())
                        && light.rgb.iter().all(|value| value.is_finite())
                })
                .map(|light| TransientLight {
                    kind: light.kind,
                    position: scene::render_position(light.origin),
                    color: light.rgb.map(|value| value.max(0.0)),
                    radius: light.radius,
                    // OpenJK's FX primitive passes RGB plus radius to
                    // RE_AddLightToScene; the renderer's point-light intensity
                    // scale of 1.0 is the matching neutral source strength.
                    intensity: 1.0,
                    segment: light.segment.map(|ends| ends.map(scene::render_position)),
                    blade_segments: light.blade_segments.as_ref().map(|blades| {
                        blades
                            .iter()
                            .map(|blade| crate::fx::system::FxLightSegment {
                                endpoints: blade.endpoints.map(scene::render_position),
                                ..*blade
                            })
                            .collect::<Vec<_>>()
                            .into()
                    }),
                })
                .collect(),
        );
        if !self.suppress_audio {
            if let Some(sound) = &mut self.sound_presenter {
                for fx_sound in &fx_frame.sounds {
                    sound.play_fx_sound(&fx_sound.qpath, fx_sound.origin);
                }
            }
        }
        self.client_perf.fx_ms = fx_started.elapsed().as_secs_f64() * 1000.0;
        let ghoul2 = self.player_presenter.perf_stats();
        self.client_perf.ghoul2_pose_ms = ghoul2.pose_ms;
        self.client_perf.ghoul2_motion_pose_ms = ghoul2.motion_pose_ms;
        self.client_perf.ghoul2_skin_ms = ghoul2.skin_ms;
        self.client_perf.ghoul2_bolt_ms = ghoul2.bolt_ms;
        self.client_perf.ghoul2_pose_evals = ghoul2.pose_evals;
        self.client_perf.ghoul2_motion_pose_evals = ghoul2.motion_pose_evals;
        self.client_perf.ghoul2_bolt_queries = ghoul2.bolt_queries;
        self.client_perf.ghoul2_surfaces = ghoul2.surfaces_skinned;
        self.client_perf.ghoul2_vertices = ghoul2.vertices_skinned;
        self.client_perf.ghoul2_frustum_tests = ghoul2.frustum_tests;
        self.client_perf.ghoul2_frustum_culled = ghoul2.frustum_culled;
        self.client_perf.ghoul2_lod_counts = ghoul2.lod_counts;
        self.client_perf.dynamic_surfaces =
            u32::try_from(self.dynamic_models.len()).unwrap_or(u32::MAX);
        self.client_perf.dynamic_vertices = self
            .dynamic_models
            .iter()
            .map(|surface| surface.vertex_count() as u64)
            .sum();
        self.client_perf.dynamic_indices = self
            .dynamic_models
            .iter()
            .map(|surface| surface.index_count() as u64)
            .sum();

        let completed = self.eof
            && self.next_snapshot.is_none()
            // Once App freezes the demo at EOF, subsequent render frames should
            // simply keep presenting that final snapshot rather than repeatedly
            // reporting completion. Seeking backwards and resuming makes rate > 0
            // again, so reaching EOF later still produces one completion edge.
            && self.timeline.is_some_and(|timeline| timeline.rate > 0.0)
            && self
                .current_snapshot
                .as_ref()
                .is_some_and(|snapshot| target_server_time >= snapshot.server_time);
        Ok((
            if completed {
                DemoAdvance::Completed
            } else {
                DemoAdvance::Running
            },
            sample,
        ))
    }
}
