//! Session advance.
use crate::app::{
    event_debug, event_workers, local_player_alpha, playerstate_vec3, race_ghost,
    attacker_damage_transition, rendering_third_person, scene, snapshot_discontinuity,
    snapshot_owns_playerstate_events, Arc,
    ClientFramePerf, DemoAdvance, DemoCameraSample, EventCheckDisposition, GameSession,
    Ghoul2PresentationView, Instant, LivePredictionFrame, PlayerPresentationInput, PresentedEntity,
    ProtocolSnapshot, SessionPhase, SpectatorCameraMode, SpectatorCameraSettings, ThirdPersonSettings,
    TransientLight,
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
        blob_shadow_settings: crate::ui::PlayerBlobShadowSettings,
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
            blob_shadow_settings,
            false,
            None,
            None,
        )
    }

    pub(in crate::app) fn advance_to(
        &mut self,
        target_server_time: i32,
        view_angles: [f32; 3],
        third_person: ThirdPersonSettings,
        spectator_camera: SpectatorCameraSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        event_workers_enabled: bool,
        ghoul2_view: Option<Ghoul2PresentationView>,
        rt_rigid_casters_enabled: bool,
        blob_shadow_settings: crate::ui::PlayerBlobShadowSettings,
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
            blob_shadow_settings,
            false,
            None,
            Some(PlayerPresentationInput::Local(view_angles)),
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
        blob_shadow_settings: crate::ui::PlayerBlobShadowSettings,
        no_predict: bool,
        predict: Option<
            &mut dyn FnMut(
                &ProtocolSnapshot,
                Option<&ProtocolSnapshot>,
                &[PresentedEntity],
            ) -> Result<Option<LivePredictionFrame>, String>,
        >,
        input: Option<PlayerPresentationInput>,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        self.client_perf = ClientFramePerf::default();
        self.player_presenter.begin_perf_frame();
        self.player_presenter
            .set_rt_shadow_casters_enabled(rt_rigid_casters_enabled);
        self.player_presenter
            .begin_blob_shadow_frame(blob_shadow_settings);
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
                // CG_TransitionPlayerState -> CG_DamageFeedback -> cg.attackerTime.
                // Server damageEvent/count already exist in our protocol PlayerState.
                if attacker_damage_transition(&current.player_state, &next.player_state) {
                    self.attacker_damage_seen = true;
                    if !self.seeking_demo {
                        crate::extensions::enqueue_damage(
                            &mut self.extension_events, &mut self.extension_last_damage,
                            &next.player_state, next.server_time,
                        );
                    }
                }
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
        // Studio "Hide other players": drop other clients before any
        // presentation, so their bodies, sabers, shadows and sounds all go.
        if let Some(keep) = self.studio_visible_players.as_ref() {
            self.presented_entities.retain(|e| e.entity_type != crate::cgame::ET_PLAYER
                || usize::from(e.number) >= crate::strafe_trail::MAX_CLIENTS || keep.contains(&e.number));
        }
        let predicted = match (predict, self.current_snapshot.as_ref()) {
            (Some(predict), Some(current)) => predict(
                current,
                self.next_snapshot.as_ref(),
                &self.presented_entities,
            )?,
            _ => None,
        };
        if let Some(prediction) = &predicted {
            // Only recorded commands commit predictable events or other discrete state.
            // Unsent render Pmove can never dispatch one-shot audio/effects.
            let now = &prediction.committed;
            let before = &prediction.previous_committed;
            if attacker_damage_transition(before, now) {
                self.attacker_damage_seen = true;
                if !self.seeking_demo {
                    crate::extensions::enqueue_damage(
                        &mut self.extension_events, &mut self.extension_last_damage,
                        now, target_server_time,
                    );
                }
            }
            self.client_game.transition_predicted_player_state(
                now,
                before,
                target_server_time,
            )?;
        }
        let sample = self
            .update_presented_player_state(
                presentation_time_ms,
                target_server_time,
                camera_teleported,
                predicted.as_ref(),
                input,
            )
            .ok_or_else(|| "DEMO CURRENT SNAPSHOT HAS NO CAMERA PLAYERSTATE".to_owned())?;
        self.client_perf.snapshot_ms = snapshot_started.elapsed().as_secs_f64() * 1000.0;

        // CG_AddEntities adds cg.predictedPlayerEntity separately from packet
        // entities. Audio has the same ownership rule: the local/followed
        // player's loops and entity position must still exist even though
        // present_entities() intentionally skips that packet entity.
        let followed_entity = self
            .presented_player_state()
            .and_then(crate::cgame::presented_player_state_entity);
        self.audio_followed_entity.clone_from(&followed_entity);

        // Presentation needs a renderer view before App::apply_demo_camera runs.
        // First person can use this frame's exact sampled eye immediately. Third
        // person needs collision/orbit state owned by App, so it uses the previous
        // rendered third-person camera as a one-frame hint. Never reuse that hint
        // across a teleport/snapshot discontinuity.
        // `sample` is still sourced from the snapshot playerState in demos even
        // when presentation has detached from that player. Keep camera mode and
        // first-person body ownership separate: fake-follow and freecam must not
        // hide the recorder merely because its playerState supplied the sample.
        let presentation_client = self.client_game.presentation_client_num();
        let sample_owns_camera = presentation_client == Some(sample.client_num);
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
        self.entity_presenter.update_md3_lod_view(ghoul2_view);
        self.entity_presenter.update_entity_area_mask(
            self.current_snapshot.as_ref().map(|snapshot| snapshot.area_mask),
        );
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
                // TaystJK CG_Init invokes CG_StartMusic(qfalse) from the
                // initial CS_MUSIC gamestate. Live sessions previously never
                // called our update_announcer, so only a later `cs 2` update
                // could start music. This also restores normal announcer ticks.
                sound.update_announcer(&self.client_game, &self.siege_classes, target_server_time);
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
                        "  ET_PLAYER entity={} clientNum={} name=\"{}\" model={}/{} glm={} skin={} siegeclass={:?}",
                        entity.number,
                        client_num,
                        crate::logging::jka_color_scope(&info.name, '7'),
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
        // Presentation records, once per entity rather than once per model
        // surface. Explicit identity also covers first/third-person prediction.
        let actors = Arc::make_mut(&mut self.ocean_actors);
        actors.clear();
        if self.ocean_interaction_enabled || self.lake_wakes_enabled || self.water_wake_mode != 0 {
        if sample.policy.pm_type < 2 && self.presented_player_state.as_ref()
            .is_none_or(|ps| ps.field_i32("m_iVehicleNum").unwrap_or(0) == 0) {
            actors.push(crate::ocean::interaction::ActorSample {
                id: sample.client_num as u16, origin: sample.player_position,
                velocity: Some(scene::render_position(sample.velocity)),
                head_height: sample.view_height as f32, time_ms: target_server_time,
                continuity: self.presented_player_state.as_ref().and_then(|ps| ps.field_i32("eFlags")).unwrap_or(0) & 8,
            });
        }
        for entity in self.presented_entities.iter().filter(|e| e.entity_type == 1) {
            if actors.len() >= 64 { break; }
            if actors.iter().any(|a| a.id == entity.number)
                || entity.state.field_i32("eFlags").unwrap_or(0) & 1 != 0
                || entity.state.field_i32("m_iVehicleNum").unwrap_or(0) != 0 { continue; }
            if self.client_game.client_info(usize::from(entity.number), &self.siege_classes).is_some_and(|info| info.team == 3) { continue; }
            actors.push(crate::ocean::interaction::ActorSample {
                id: entity.number, origin: scene::render_position(entity.origin), head_height: 26.0,
                velocity: match [entity.state.field_f32("pos.trDelta[0]"),entity.state.field_f32("pos.trDelta[1]"),entity.state.field_f32("pos.trDelta[2]")] {
                    [Some(x),Some(y),Some(z)]=>Some(scene::render_position([x,y,z])), _=>None,
                },
                time_ms: target_server_time, continuity: entity.state.field_i32("eFlags").unwrap_or(0) & 8,
            });
        }
        }
        self.update_race_timer(target_server_time);
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

        let followed_entity_num = followed_entity.as_ref().map(|entity| entity.number);
        // forceGripCripple is one of the few exact victim-side grip signals in
        // vanilla playerState. Remote entityState does not transmit
        // forceGripEntityNum, so PlayerPresenter reconstructs those targets
        // from each active gripper's view trace.
        let local_force_gripped = self
            .presented_player_state()
            .is_some_and(|ps| ps.field_i32("fd.forceGripCripple").unwrap_or(0) != 0);
        self.player_presenter.prepare_force_grip_targets(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_force_gripped,
            target_server_time,
        );
        // PMF_TIME_KNOCKBACK is available only for our local/followed
        // playerState (not for remote entityState). Read the received snapshot,
        // rather than the predicted/display state, for server-owned evidence.
        const PMF_TIME_KNOCKBACK: i32 = 64;
        let local_knockback = self
            .current_snapshot
            .as_ref()
            .is_some_and(|snap| {
                snap.player_state.field_i32("pm_flags").unwrap_or(0) & PMF_TIME_KNOCKBACK != 0
            });
        self.player_presenter.prepare_impulse_ragdolls(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_knockback,
            target_server_time,
        );

        // Broadsword historically advanced ragdoll state immediately before
        // Ghoul2 submission. Keep the same presentation ownership: Rapier is
        // stepped once here, while OpenJK snapshot/pmove state stays untouched.
        // Server EV_DEBRIS was prepared by the same reference FX event path.
        // Promote its existing chunk models into the *same* Rapier world used
        // for ragdolls before the fixed step; if unavailable, keep exact JKA
        // ballistic chunks (for example while the map collider is loading).
        let new_debris = self.weapon_fx.take_debris_spawns();
        if !new_debris.is_empty() {
            let promoted = self.player_presenter
                .spawn_physics_debris(&new_debris, target_server_time);
            self.weapon_fx.resolve_debris_promotion(promoted);
        }
        let new_gibs = self.weapon_fx.take_gib_spawns();
        if !new_gibs.is_empty() {
            let promoted = self.player_presenter
                .spawn_physics_debris(&new_gibs, target_server_time);
            self.weapon_fx.resolve_gib_promotion(promoted);
        }
        // Unlike MD3 gibs, glass keeps its tessellated FX quad presentation.
        // Only the centre/orientation gets handed to Rapier; a capped remainder
        // stays in the legacy FX physics path with no duplicate quads.
        let new_glass = self.weapon_fx.take_glass_spawns();
        if !new_glass.is_empty() {
            let promoted = self.player_presenter
                .spawn_physics_glass(&new_glass, target_server_time);
            self.weapon_fx.resolve_glass_promotion(promoted);
        }
        let local_hull_height = self.presented_player_state().map_or(40.0, |ps| {
            let crouched = ps.field_i32("pm_flags").unwrap_or(0)
                & (jka_movement::PMF_DUCKED | jka_movement::PMF_ROLLING) != 0;
            if crouched {
                ps.field_i32("crouchheight").filter(|height| *height > 0).unwrap_or(16) as f32
            } else {
                ps.field_i32("standheight").filter(|height| *height > 0).unwrap_or(40) as f32
            }
        });
        self.player_presenter.prepare_corpse_player_contacts(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_hull_height,
            sample.policy.pm_type < 2,
            target_server_time,
        );
        self.player_presenter
            .begin_physics_frame(target_server_time);

        // TaystJK CG_General uses cg.refdef.viewangles for the local E-Web
        // gunner only in first person; snapshot owner angles for everybody else.
        self.entity_presenter.set_emplaced_first_person_view(
            if sample_owns_camera && !sample_third_person {
                followed_entity.as_ref().map(|owner| (owner.number, sample.view_angles))
            } else { None },
        );
        // CG_PlayerState emplacedIndex explicitly names the mounted gun.
        // We suppress the local ET_PLAYER snapshot and some servers omit the
        // occupied-gun marker, so CG_General's reverse owner lookup alone
        // cannot reliably drive Bone02 for the locally controlled E-Web.
        let local_emplaced_gunner = if sample_owns_camera {
            self.presented_player_state().and_then(|ps| {
                if ps.field_i32("weapon") != Some(17) { return None; }
                let gun_num = u16::try_from(ps.field_i32("emplacedIndex")?).ok()?;
                if gun_num == 0 || gun_num >= 1023 { return None; }
                let angles = if sample_third_person {
                    followed_entity.as_ref()?.angles
                } else {
                    sample.view_angles
                };
                Some((gun_num, angles))
            })
        } else { None };
        self.entity_presenter.set_local_emplaced_gunner(local_emplaced_gunner);
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
        self.player_presenter.set_external_live_entities(
            self.race_ghosts.iter().map(|ghost| ghost.entity_num)
                .chain(self.studio_external_player_entities.iter().copied()),
        );
        // TaystJK CG_BreathPuffs queries S_GetVoiceVolume each frame, and
        // switches 3-second breathing to 300 ms during a spoken voice line.
        // Only the small set of players (plus a synthesized local player) is
        // sampled, and absent audio backends report no active voice.
        let talking = self.sound_presenter.as_ref().map(|sound| {
            self.presented_entities.iter().map(|e| e.number)
                .chain(followed_entity_num)
                .filter(|&id| sound.voice_active(id))
                .collect::<std::collections::HashSet<_>>()
        }).unwrap_or_default();
        self.player_presenter.set_breath_talking(talking);
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
        // TaystJK CG_Player: seeker drones are independent orbiting MD3s;
        // playerState can synthesize a local owner absent from the snapshot.
        dynamic_models.extend(self.entity_presenter.present_seeker_drones(
            &self.presented_entities, followed_entity.as_ref(), target_server_time,
            self.client_game.rendering_intermission(),
        ));
        // TaystJK CG_Player: boon/enlightenment/ysalamiri and spawn
        // protection are separate MD3 refEntities, not Ghoul2 body tints.
        // Use the same presentation view that selected player LODs; in first
        // person it was replaced above with this frame's exact sampled eye.
        let sphere_view_origin = ghoul2_view
            .map(crate::cgame::player_presenter::Ghoul2PresentationView::jka_position)
            .unwrap_or_else(|| scene::jka_position(sample.eye_position));
        dynamic_models.extend(self.entity_presenter.present_player_spheres(
            &self.presented_entities,
            followed_entity.as_ref(),
            &self.client_game,
            sphere_view_origin,
        ));
        // CG_PlayerHitFX: armor-absorbed hits flash a half-shield on the victim
        // (never on your own first-person body).
        dynamic_models.extend(self.entity_presenter.present_shield_hits(
            &self.presented_entities,
            followed_entity.as_ref(),
            target_server_time,
            if sample_owns_camera && !sample_third_person {
                followed_entity_num
            } else {
                None
            },
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
        let first_person_saber = sample_owns_camera
            && !sample_third_person
            && first_person_lightsaber
            && !sample.following
            && matches!(sample.policy.weapon, WP_MELEE | WP_SABER);
        // The snapshot/predicted entity is special only while it actually owns
        // the camera. In demo fake-follow and freecam it is just another world
        // player and must be submitted so spectator outlines (and ordinary world
        // rendering) can see it.
        let render_followed_body = !sample_owns_camera || sample_third_person || first_person_saber;
        let followed_started = Instant::now();
        let intermission = sample.policy.pm_type == crate::cgame::PM_INTERMISSION;
        if let Some(entity) = followed_entity
            // TaystJK CG_AddPacketEntities synthesizes predictedPlayerEntity,
            // then dispatches it via CG_AddCEntity. ET_INVISIBLE (notably the
            // PM_SPECTATOR used by NPC-control mods) does NOT invoke CG_Player.
            // Keep the entity for audio/state bookkeeping above; only suppress
            // its visual branch, including sprites, sabers and view weapons.
            .filter(crate::cgame::predicted_player_dispatches_cg_player)
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
                        sample.policy.pm_type == 0, // PM_NORMAL, never PM_NOCLIP (3).
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
                    if sample_owns_camera
                        && !sample_third_person
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
            let live_race_clock = self.presented_player_state().map(|player_state| {
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
                            false, false,
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
            self.entity_presenter.set_material_eye(sphere_view_origin);
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
        self.weapon_fx.set_fx_light_promotion_level(self.promote_fx_lights);
        let mut fx_frame = self.weapon_fx.end_frame();
        // A Decal{} with ghoul2decals may also mark an animated character.
        // This is a model-space projection, not an extra billboard in the FX pass.
        self.player_presenter.add_authored_ghoul2_decals(&fx_frame.ghoul2_decals, target_server_time);

        let (emote_draws, emote_lights) = self.player_presenter.drain_emote_fx();
        fx_frame.draws.extend(emote_draws);
        fx_frame.lights.extend(emote_lights);
        // CG_EntityEffects runs once per presented entity on every cgame frame,
        // irrespective of the optional EFX promotion setting.
        fx_frame.lights.extend(self.presented_entities.iter().filter_map(
            crate::cgame::weapon_fx::WeaponFx::constant_entity_light,
        ));
        fx_frame.draws.extend(self.player_presenter.physics_glass_draws(target_server_time));
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
        let projectile_emitters = self.weapon_fx.take_projectile_light_requests();
        if self.promote_fx_lights == 2 {
            // Effects' animated primitive draws can expire between fxFPS ticks.
            // The live missile position is sampled per presentation frame.
            // Resolve all material evidence using the map-worker's catalog.
            let mut anchored = Vec::new();
            for (effect, origin, _direction) in projectile_emitters {
                if let Some(candidate) = self.fx_light_promoter.projectile_candidate(&effect, origin) {
                    anchored.push(candidate);
                }
            }
            // A live projectile emitter supersedes nearby trail samples from
            // the same shader. The existing spatial merge remains unchanged.
            fx_frame.auto_light_candidates.retain(|trail| {
                !anchored.iter().any(|current| {
                    current.shader.eq_ignore_ascii_case(&trail.shader)
                        && current.kind == trail.kind
                        && current.origin.iter().zip(trail.origin).map(|(a, b)| (*a - b) * (*a - b)).sum::<f32>() < 120.0 * 120.0
                })
            });
            fx_frame.auto_light_candidates.extend(anchored);
        }
        if self.promote_fx_lights > 0 {
            let promoter = &mut self.fx_light_promoter;
            let presenter = &mut self.entity_presenter;
            let mut resolve = |shader: &str| presenter.fx_material_stages(shader);
            let promoted = promoter.promote(
                &fx_frame.auto_light_candidates, &mut resolve, target_server_time, self.promote_fx_lights,
            );
            fx_frame.lights.extend(promoted);
        }
        if crate::fx::diagnostics::enabled() {
            use crate::fx::diagnostics as diag;
            diag::set(diag::PROMOTION_LEVEL, self.promote_fx_lights as u64);
            diag::add(diag::FX_DRAW_TOTAL, fx_frame.draws.len() as u64);
            diag::add(diag::FX_DRAW_LINES, fx_frame.draws.iter()
                .filter(|draw| matches!(draw, crate::fx::system::FxDraw::Line { .. })).count() as u64);
            diag::report_if_due();
        }
        self.gpu_commands = fx_frame.gpu_sparks.take();
        self.gpu_draw_indices = std::mem::take(&mut fx_frame.gpu_sprite_draw_indices);
        self.fx_runner_draws = fx_frame.runner_draws;
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
                .filter_map(TransientLight::from_fx)
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
        // OpenJK assigns cg.time from the CGame frame's serverTime. Keep the
        // successful frame time for jaPRO asynchronous commands such as `/do`.
        self.cgame_time = target_server_time;
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
