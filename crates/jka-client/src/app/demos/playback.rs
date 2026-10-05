//! Demo playback.
use crate::app::{
    charset_glyph_uv, generic_timer_bar, offset_third_person_view, playerstate_vec3,
    probe_demo_gamestate, rendering_third_person, scene, ui, ActiveReward, App, ApplyLatchedScope,
    Arc, ClientGameState, ClothConfig, Cursor, DemoAdvance, DemoCameraSample, DemoFakeView,
    DemoIndex, DemoReader, DemoViewMode, Duration, DynamicLightsMode, DynamicShadowsMode,
    EntityEvent, FrontendPage, FxGeometryMode, GameSession, Ghoul2PresentationView, Instant,
    KeyCode, LivePredictionFrame, MapLoadingState, MissingMapPrompt, OverlayMode, Path,
    PresentationViewer, PresentedEntity, ProtocolSnapshot, RagdollConfig, ServerMessageDecoder,
    SessionPhase, SpectatorCameraMode, ThirdPersonViewInput, UiScoreEntry, UiScoreboard,
    ViewLatchMode, DEMO_FREE_SPEEDS, DEMO_FREE_SPEED_DEFAULT_INDEX, ET_PLAYER, REWARD_BLOB_MS,
    REWARD_ICON_SIZE, REWARD_TIME_MS,
};

impl GameSession {
    pub(in crate::app) fn set_playback_rate(
        &mut self,
        now: Instant,
        rate: f64,
    ) -> Result<(), String> {
        if !rate.is_finite() || rate < 0.0 || rate > 100.0 {
            return Err("DEMO SPEED MUST BE BETWEEN 0 AND 100 (REVERSE NEEDS CHECKPOINTS)".into());
        }
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;
        timeline.set_rate(now, rate);
        if let Some(sound) = &mut self.sound_presenter {
            sound.set_rate(rate as f32);
        }
        Ok(())
    }

    /// Change the UI playback speed without changing pause state. The play/pause
    /// control owns whether time advances; the speed control owns resume speed.
    pub(in crate::app) fn set_playback_speed(
        &mut self,
        now: Instant,
        speed: f64,
    ) -> Result<(), String> {
        if !speed.is_finite() || speed <= 0.0 || speed > 100.0 {
            return Err("DEMO SPEED MUST BE GREATER THAN 0 AND AT MOST 100".into());
        }
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;
        if timeline.rate == 0.0 {
            timeline.resume_rate = speed;
        } else {
            timeline.set_rate(now, speed);
            if let Some(sound) = &mut self.sound_presenter {
                sound.set_rate(speed as f32);
            }
        }
        Ok(())
    }

    pub(in crate::app) fn toggle_pause(&mut self, now: Instant) -> Result<f64, String> {
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;
        timeline.toggle_pause(now);
        if let Some(sound) = &mut self.sound_presenter {
            sound.set_rate(timeline.rate as f32);
        }
        Ok(timeline.rate)
    }

    pub(in crate::app) fn playback_rate(&self) -> Option<f64> {
        self.timeline.map(|timeline| timeline.rate)
    }

    /// Freeze completed demo playback on the final authoritative snapshot instead
    /// of tearing down the CGame session. Keeping the session alive preserves the
    /// rendered world, camera/follow mode, scoreboard and seek controls so the
    /// user can scrub back and resume from the timeline.
    pub(in crate::app) fn hold_demo_end(&mut self, now: Instant) -> Result<(), String> {
        if self.live || self.local {
            return Ok(());
        }
        let final_server_time = self
            .current_snapshot
            .as_ref()
            .map(|snapshot| snapshot.server_time)
            .ok_or_else(|| "DEMO HAS NO FINAL SNAPSHOT".to_owned())?;
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;

        // Clamp away any wall-clock overshoot from the frame that discovered EOF,
        // then pause. set_rate(0) intentionally leaves resume_rate unchanged.
        timeline.seek(now, f64::from(final_server_time));
        timeline.set_rate(now, 0.0);
        if let Some(sound) = &mut self.sound_presenter {
            sound.set_rate(0.0);
        }
        Ok(())
    }

    pub(in crate::app) fn set_audio_suppressed(&mut self, suppressed: bool) {
        if self.suppress_audio == suppressed {
            return;
        }
        self.suppress_audio = suppressed;
        // Stop anything already queued/looping at the boundary. On resume the
        // destination frame rebuilds valid loops and future one-shots normally.
        if let Some(sound) = &mut self.sound_presenter {
            sound.clear();
        }
    }

    pub(in crate::app) fn demo_duration_ms(&self) -> Option<i32> {
        self.demo_index.as_ref().map(DemoIndex::duration_ms)
    }

    pub(in crate::app) fn demo_elapsed_ms(&self, now: Instant) -> Option<f64> {
        let first = f64::from(self.demo_index.as_ref()?.first_active_server_time?);
        let current = self.timeline?.target_time_ms(now);
        Some((current - first).clamp(0.0, f64::from(self.demo_duration_ms()?.max(0))))
    }

    /// Rebuild the scoreboard from demo-indexed `scores` commands instead of
    /// trying to ask a non-existent historical server for fresh data. Team/top
    /// scores come from the *current* CS_SCORES1/2 state so they remain exact
    /// between sparse recorded scoreboard payloads and after seeks.
    pub(in crate::app) fn demo_scoreboard_ui(
        &self,
        now: Instant,
        score_deaths_mode: i32,
    ) -> Option<UiScoreboard> {
        if self.live {
            return None;
        }
        let elapsed_ms = self.demo_elapsed_ms(now)?.round() as i32;
        let gametype = self.client_game.gametype();
        let limits = self.client_game.match_limits();
        let sample = self.demo_index.as_ref()?.scoreboard_at(elapsed_ms);
        let current_team_scores = [limits.scores1, limits.scores2];
        let team_scores = if self
            .client_game
            .configstring(crate::cgame::CS_SCORES1)
            .is_some()
            || self
                .client_game
                .configstring(crate::cgame::CS_SCORES2)
                .is_some()
        {
            current_team_scores
        } else {
            sample.map_or(current_team_scores, |sample| sample.team_scores)
        };
        let mut entries = sample
            .map(|sample| {
                sample
                    .entries
                    .iter()
                    .map(|entry| {
                        // Client slot is the stable wire identity. If a user
                        // renamed or changed teams after this score payload,
                        // present the current CS_PLAYERS identity while keeping
                        // the score/ping/time values from the recorded command.
                        let current = usize::try_from(entry.client).ok().and_then(|client| {
                            self.client_game.client_info(client, &self.siege_classes)
                        });
                        let counted = usize::try_from(entry.client)
                            .ok()
                            .filter(|&client| client < self.client_deaths.len())
                            .map(|client| self.client_deaths[client]);
                        let deaths = match score_deaths_mode {
                            0 => None,
                            1 => entry.deaths,
                            2 => entry.deaths.or(counted),
                            3 => counted,
                            _ => entry.deaths,
                        };
                        UiScoreEntry {
                            client: entry.client,
                            name: current
                                .as_ref()
                                .map_or_else(|| entry.name.clone(), |info| info.name.clone()),
                            score: entry.score,
                            deaths,
                            ping: entry.ping,
                            time: entry.time,
                            team: current.as_ref().map_or(entry.team, |info| info.team),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // A recorded score payload may be a couple of seconds old. The POV's
        // own PERS_SCORE is carried in every playerState, so refresh that row
        // when it already exists without fabricating ping/time for missing rows.
        if let Some(snapshot) = self.current_snapshot.as_ref() {
            if let Some(client) = snapshot.player_state.field_i32("clientNum") {
                if let Some(entry) = entries.iter_mut().find(|entry| entry.client == client) {
                    if let Some(score) = snapshot.player_state.persistant.first() {
                        entry.score = *score;
                    }
                }
            }
        }

        let show_deaths = score_deaths_mode != 0
            && !matches!(gametype, 3 | 4 | 8)
            && entries.iter().any(|entry| entry.deaths.is_some());
        Some(UiScoreboard {
            team_scores,
            team_game: gametype >= 6,
            spectator_scores: matches!(gametype, 3 | 4),
            show_deaths,
            entries,
        })
    }

    pub(in crate::app) fn demo_followed_client(&self) -> Option<i32> {
        self.current_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
    }

    pub(in crate::app) fn demo_visible_players(&self) -> Vec<(i32, String)> {
        let authoritative = self.demo_followed_client();
        let mut out = Vec::<(i32, String)>::new();
        for client in authoritative.into_iter().chain(
            self.presented_entities
                .iter()
                .filter(|entity| entity.entity_type == ET_PLAYER)
                .map(|entity| i32::from(entity.number)),
        ) {
            if out.iter().any(|(existing, _)| *existing == client) {
                continue;
            }
            let name = usize::try_from(client)
                .ok()
                .and_then(|client| self.client_game.client_info(client, &self.siege_classes))
                .map(|info| info.name)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("CLIENT {client}"));
            out.push((client, name));
        }
        out.sort_by_key(|(client, _)| *client);
        out
    }

    pub(in crate::app) fn demo_fake_view(&self, client_num: i32) -> Option<DemoFakeView> {
        let entity = self.presented_entities.iter().find(|entity| {
            entity.entity_type == ET_PLAYER && i32::from(entity.number) == client_num
        })?;
        // Remote demos do not carry another client's playerState/viewheight, so this
        // is intentionally a presentation approximation. Standing eye height is the
        // JKA player-view default; authoritative POV keeps the exact playerState path.
        let mut eye = entity.origin;
        eye[2] += 26.0;
        Some(DemoFakeView {
            client_num,
            native_origin: entity.origin,
            eye_position: scene::render_position(eye),
            view_angles: entity.angles,
            velocity: [
                entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
            ],
            health: entity.state.field_i32("health").unwrap_or(100),
        })
    }

    pub(in crate::app) fn demo_area_mask(&self) -> Option<[u8; 32]> {
        self.current_snapshot
            .as_ref()
            .map(|snapshot| snapshot.area_mask)
    }

    pub(in crate::app) fn reset_demo_stream_for_seek(&mut self) {
        self.reader = DemoReader::new(Cursor::new(self.demo_bytes.clone()));
        self.decoder = ServerMessageDecoder::new();
        self.messages = 0;
        self.snapshots = 0;
        self.phase = SessionPhase::WaitingForMap;
        self.map_name = None;
        self.pending_snapshot = None;
        self.current_snapshot = None;
        self.next_snapshot = None;
        self.first_server_time = None;
        self.timeline = None;
        self.eof = false;
        self.player_position = None;
        self.demo_spectator_camera_mode = None;
        self.client_deaths = [0; 32];
        self.client_game = ClientGameState::new();
        self.presented_entities.clear();
        self.event_presenter.clear();
        self.reward_active = None;
        self.reward_queue.clear();
        if let Some(sound) = &mut self.sound_presenter {
            sound.clear();
        }
        self.weapon_fx.reset_for_seek(0);
        self.player_presenter.reset_for_seek();
        self.fx_draws.clear();
        self.fx_lights = Arc::new(Vec::new());
        self.fx_blades = Arc::new(Vec::new());
        self.fx_surfaces = Arc::new(Vec::new());
        self.dynamic_models = Arc::new(Vec::new());
        self.inline_models = Arc::new(Vec::new());
        self.clear_event_stats();
    }

    pub(in crate::app) fn seek_demo(
        &mut self,
        now: Instant,
        elapsed_ms: i32,
    ) -> Result<DemoCameraSample, String> {
        if self.live {
            return Err("DEMO SEEK IS NOT AVAILABLE FOR LIVE SESSIONS".into());
        }
        self.seeking_demo = true;
        let result = self.seek_demo_inner(now, elapsed_ms);
        self.seeking_demo = false;
        result
    }

    pub(in crate::app) fn seek_demo_inner(
        &mut self,
        now: Instant,
        elapsed_ms: i32,
    ) -> Result<DemoCameraSample, String> {
        let (first, duration) = {
            let index = self
                .demo_index
                .as_ref()
                .ok_or_else(|| "DEMO TIMELINE INDEX IS UNAVAILABLE".to_owned())?;
            let first = index
                .first_active_server_time
                .ok_or_else(|| "DEMO TIMELINE HAS NO ACTIVE START".to_owned())?;
            (first, index.duration_ms())
        };
        let elapsed_ms = elapsed_ms.clamp(0, duration);
        let target = first.saturating_add(elapsed_ms);
        let (rate, resume_rate) = self
            .timeline
            .map(|timeline| (timeline.rate, timeline.resume_rate))
            .unwrap_or((1.0, 1.0));

        self.reset_demo_stream_for_seek();
        let _map = self.read_until_map()?;
        let _ = self.begin_after_map_load(now)?;

        while self
            .next_snapshot
            .as_ref()
            .is_some_and(|snapshot| target >= snapshot.server_time)
        {
            self.client_game.transition_snapshot(target)?;
            self.current_snapshot = self.next_snapshot.take();
            self.next_snapshot = self.read_next_snapshot()?;
            self.client_game
                .set_next_snapshot(self.next_snapshot.as_ref())?;
        }

        // Seeking reconstructs authoritative CGame state, but transient effects
        // from the replay-to-target pass must not burst onto the destination frame.
        // Keep the one persistent client-side statistic TaystJK needs for
        // cg_scoreDeaths 2/3 so seeking does not reset the displayed count.
        for event in self.client_game.drain_presentation_events() {
            if event.event == EntityEvent::EV_OBITUARY {
                if let Some(target) = event
                    .state
                    .field_i32("otherEntityNum")
                    .and_then(|target| usize::try_from(target).ok())
                    .filter(|&target| target < self.client_deaths.len())
                {
                    self.client_deaths[target] = self.client_deaths[target].saturating_add(1);
                }
            }
        }
        let _ = self.client_game.drain_event_check_traces();
        // Replayed server commands are historical at the seek destination. The App
        // reconstructs chat/centerprint explicitly from the indexed demo timeline.
        let _ = self.client_game.drain_notices();
        self.event_presenter.clear();
        if let Some(sound) = &mut self.sound_presenter {
            sound.clear();
            sound.set_rate(rate as f32);
        }
        self.weapon_fx.reset_for_seek(target);
        self.weapon_fx.begin_frame(target);
        self.presented_entities = self.client_game.present_entities_at(f64::from(target))?;
        let sample = self
            .sample_at(f64::from(target), target, true)
            .ok_or_else(|| "DEMO SEEK TARGET HAS NO CAMERA PLAYERSTATE".to_owned())?;
        self.player_position = Some(glam::Vec3::from_array(sample.player_position));
        if let Some(timeline) = &mut self.timeline {
            timeline.seek(now, f64::from(target));
            timeline.rate = rate;
            timeline.resume_rate = resume_rate;
        }
        Ok(sample)
    }

    pub(in crate::app) fn read_until_map(&mut self) -> Result<String, String> {
        loop {
            // A normal JKA gamestate packet ends before the first snapshot, but
            // preserve a snapshot if a compatible producer places both in one
            // server message. The shared decoder has already consumed the whole
            // packet, so dropping that snapshot here would move playback's first
            // visible frame forward by one message.
            if let Some(snapshot) = self.read_next_message()? {
                if self.pending_snapshot.is_none() {
                    self.pending_snapshot = Some(snapshot);
                }
            }
            if let Some(map) = self.map_name.clone().or_else(|| self.decoder.map_name()) {
                self.map_name = Some(map.clone());
                return Ok(map);
            }
            if self.eof {
                return Err(format!(
                    "DEMO REACHED EOF BEFORE GAMESTATE MAP: {}",
                    self.qpath
                ));
            }
        }
    }
}

impl App {
    pub(in crate::app) fn play_selected_demo(&mut self) {
        self.play_selected_demo_from(None);
    }

    pub(in crate::app) fn play_selected_demo_at(&mut self, elapsed_ms: i32) {
        self.play_selected_demo_from(Some(elapsed_ms.max(0)));
    }

    pub(in crate::app) fn play_selected_demo_from(&mut self, start_elapsed_ms: Option<i32>) {
        // Demo presentation assets are opened before the demo's map request, so
        // apply map-scoped CVAR_LATCH values at this boundary too.
        self.apply_latched_console_cvars(ApplyLatchedScope::MapLoad);
        self.sync_pbr();
        if self.demo_entries.is_empty() {
            self.console_status = "NO DEMO SELECTED".into();
            return;
        }
        self.demo_selected = self.demo_selected.min(self.demo_entries.len() - 1);
        let demo_name = self.demo_entries[self.demo_selected].demo_name.clone();
        self.play_demo_named_from(&demo_name, start_elapsed_ms);
    }

    pub(in crate::app) fn play_demo_named(&mut self, argument: &str) {
        self.play_demo_named_from(argument, None);
    }

    pub(in crate::app) fn play_demo_named_from(
        &mut self,
        argument: &str,
        start_elapsed_ms: Option<i32>,
    ) {
        let argument = argument.trim().trim_matches('"');
        if argument.is_empty() {
            self.push_console_line("^3demo <demoname>".to_owned());
            return;
        }
        let mut name = argument.replace('\\', "/");
        if let Some(stripped) = name.strip_prefix("demos/") {
            name = stripped.to_owned();
        }
        let demo_name = name.strip_suffix(".dm_26").unwrap_or(&name).to_owned();
        if demo_name.is_empty()
            || Path::new(&demo_name).components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            self.push_console_line("^1demo name must stay inside demos/".to_owned());
            return;
        }
        let qpath = format!("demos/{demo_name}.dm_26");
        const MAX_DEMO_FILE_BYTES: usize = 512 * 1024 * 1024;

        // Open through the *current* VFS before disconnecting. This preserves
        // mod-local demo lookup when /demo is issued while connected to that mod.
        println!("DEMO: {qpath}");
        let mut assets =
            match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                Ok(assets) => assets,
                Err(error) => {
                    self.console_status = format!("DEMO ASSET PATH ERROR: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                    return;
                }
            };
        let asset = match assets.read(&qpath, MAX_DEMO_FILE_BYTES) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                self.console_status = format!("DEMO NOT FOUND: {qpath}");
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
            Err(error) => {
                self.console_status = format!("DEMO READ ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };
        let source = asset.source.display().to_string();
        crate::logging::write_line_with_path(
            crate::logging::Level::Info,
            format_args!("DEMO SOURCE: {source} ({} bytes)", asset.bytes.len()),
            asset.source.clone(),
        );
        let probe = match probe_demo_gamestate(&asset.bytes) {
            Ok(probe) => probe,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };

        // OpenJK disconnects the old client before pumping the demo to CA_PRIMED,
        // but CA_LOADING/CA_PRIMED always render a loading-information screen. Our
        // old path used the generic "return to main menu" disconnect, which sent
        // UnloadMap and briefly exposed the renderer's no-world fog clear. Tear
        // down authority without touching the currently presented GPU world.
        let transitioning_from_active_session = self.net.is_some()
            || self.game_session.is_some()
            || self.local_server.is_some()
            || !self.front_end;
        if transitioning_from_active_session {
            self.disconnect_for_demo_transition();
        }
        if let Err(error) =
            self.set_session_fs_game_preserving_rendered_world(&probe.fs_game, "demo gamestate")
        {
            self.console_status = format!("DEMO FS_GAME ERROR: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
            if transitioning_from_active_session {
                self.disconnect_to_main_menu();
            }
            return;
        }

        // Establish CA_LOADING-equivalent presentation before the expensive CGame
        // asset build. `unknownmap_mp` is resident on the renderer, so the next
        // present is intentional even if presenter/VFS setup takes a while.
        self.loading = Some(MapLoadingState::new(
            self.latest_request_id.wrapping_add(1),
            probe.map_name.clone(),
            self.game.as_deref(),
        ));
        self.console_status = format!("LOADING DEMO MAP {}...", probe.map_name);
        self.publish_ui();

        let mut playback = match self.build_game_session(qpath.clone(), source, asset.bytes) {
            Ok(playback) => playback,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                self.loading = None;
                self.disconnect_to_main_menu();
                return;
            }
        };
        let map_name = match playback.read_until_map() {
            Ok(map_name) => map_name,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                eprintln!("{}", self.console_status);
                self.loading = None;
                self.disconnect_to_main_menu();
                return;
            }
        };
        if Self::normalized_bsp_name(&map_name) != Self::normalized_bsp_name(&probe.map_name) {
            eprintln!(
                "DEMO GAMESTATE PROBE MAP MISMATCH: probe={} playback={}",
                probe.map_name, map_name
            );
        }

        self.console_status = format!("DEMO GAMESTATE READY: {qpath} -> {map_name}");
        self.push_console_line(format!("^2{}", self.console_status));
        println!("{}", self.console_status);
        // A console-row launch stores its relative timestamp only after all demo
        // open/gamestate work has succeeded, so a failed launch cannot poison the
        // next ordinary /demo command.
        self.demo_launch_seek_ms = start_elapsed_ms.map(|value| value.max(0));
        let map_source = scene::MapSource::Bsp(map_name.clone());
        match scene::verify_map_source_exists(&self.base, self.game.as_deref(), &map_source) {
            Ok(()) => {
                self.game_session = Some(playback);
                self.request_map(map_source);
            }
            Err(error) => {
                self.console_status = format!("DEMO MAP NOT FOUND: {map_name}");
                self.push_console_line(format!(
                    "^3Demo map is not installed locally:^7 {map_name}"
                ));
                self.game_session = Some(playback);
                // The missing-map prompt is the next authoritative screen; do
                // not leave the opaque retained loading UI above the egui dialog.
                self.loading = None;
                self.demo_missing_map_prompt = Some(MissingMapPrompt {
                    map_name,
                    reason: error,
                });
                self.egui_repaint_requested = true;
                self.publish_ui();
            }
        }
    }

    pub(in crate::app) fn choose_demo_missing_map_exit(&mut self) {
        let map_name = self
            .demo_missing_map_prompt
            .take()
            .map(|prompt| prompt.map_name)
            .unwrap_or_else(|| "unknown map".to_owned());
        self.game_session = None;
        self.live_without_world = false;
        self.demo_without_world = false;
        self.restore_startup_game();
        self.front_end = true;
        self.frontend_page = FrontendPage::PlayDemo;
        self.menu_selected = 0;
        self.set_overlay(OverlayMode::Game);
        self.console_status = format!("DEMO ABORTED - MAP NOT INSTALLED: {map_name}");
        self.push_console_line(format!("^3{}", self.console_status));
        self.publish_snapshot();
        self.publish_ui();
        self.request_frontend_background();
    }

    pub(in crate::app) fn choose_demo_missing_map_continue(&mut self) {
        let Some(prompt) = self.demo_missing_map_prompt.take() else {
            return;
        };
        let map_name = prompt.map_name;
        self.prepare_worldless_session(&map_name);
        self.live_without_world = false;
        self.demo_without_world = true;
        if let Some(playback) = self.game_session.as_mut() {
            let _ = playback
                .player_presenter
                .set_physics_map_mesh(&self.map_physics_collision);
        }
        self.console_status = format!("DEMO PLAYBACK WITHOUT MAP: {map_name}");
        self.push_console_line(format!(
            "^3Continuing demo without BSP geometry:^7 {}",
            prompt.reason
        ));
        self.push_console_line("^3Recorded snapshots, players, camera and FX continue; world geometry and map-dependent presentation may be missing.".to_owned());
        self.start_demo_after_map_load(&map_name);
        self.publish_snapshot();
        self.publish_ui();
    }

    pub(in crate::app) fn start_demo_after_map_load(&mut self, loaded_map: &str) {
        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.live)
        {
            self.prime_live_session(loaded_map);
            return;
        }
        let should_start = self.game_session.as_ref().is_some_and(|playback| {
            let loaded_qpath = loaded_map
                .strip_suffix(".bsp")
                .or_else(|| loaded_map.strip_suffix(".map"))
                .unwrap_or(loaded_map);
            playback.phase == SessionPhase::WaitingForMap
                && playback
                    .map_name
                    .as_deref()
                    .is_some_and(|map| map.eq_ignore_ascii_case(loaded_qpath))
        });
        if !should_start {
            return;
        }

        let now = Instant::now();
        let launch_seek_ms = self.demo_launch_seek_ms.take();
        let result = {
            let playback = self.game_session.as_mut().expect("checked above");
            match playback.begin_after_map_load(now) {
                Ok(mut sample) => {
                    let seek_error = if let Some(elapsed_ms) = launch_seek_ms {
                        match playback.seek_demo(now, elapsed_ms) {
                            Ok(seek_sample) => {
                                sample = seek_sample;
                                None
                            }
                            Err(error) => Some(error),
                        }
                    } else {
                        None
                    };
                    if let Some(error) = seek_error {
                        Err(error)
                    } else {
                        Ok((sample, playback.qpath.clone(), playback.source.clone()))
                    }
                }
                Err(error) => Err(error),
            }
        };
        match result {
            Ok((sample, qpath, source)) => {
                // Demo playback is authoritative. Do not run the local movement
                // simulator on top of the recorded playerstate.
                self.local_server = None;
                self.demo_scrub_dragging = false;
                self.demo_scrub_fraction = None;
                self.demo_scrub_resume_rate = 1.0;
                self.demo_scrub_last_seek = None;
                self.demo_scrub_last_applied_fraction = None;
                self.demo_view_mode = DemoViewMode::Authoritative;
                self.demo_free_follow_anchor = None;
                self.demo_view_outside_authoritative = false;
                self.demo_free_speed_index = DEMO_FREE_SPEED_DEFAULT_INDEX;
                if let Some(elapsed_ms) = launch_seek_ms {
                    self.restore_demo_transients(elapsed_ms, now);
                }
                self.apply_demo_camera(sample);
                // This runs from the WorldUploaded handler, which clears the
                // loading-information screen with its own publish_ui() right
                // after returning here. Without publishing the corrected demo
                // camera now, the render thread keeps drawing from whatever
                // stale camera was live before the map load (e.g. the frontend
                // menu flythrough) until the next regular per-tick snapshot -
                // uncovered by the just-cleared loading screen, that stale view
                // is usually outside the map and exposes the fog clear color.
                self.publish_snapshot();
                self.sync_demo_camera_capture();
                self.previous_tick = now;
                self.console_status = format!(
                    "DEMO PLAYING: {qpath} serverTime={} source={source}",
                    sample.server_time
                );
                self.push_console_line(format!("^2{}", self.console_status));
                println!("{}", self.console_status);
            }
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                eprintln!("{}", self.console_status);
                self.game_session = None;
            }
        }
    }

    pub(in crate::app) fn apply_demo_camera(&mut self, sample: DemoCameraSample) {
        self.render_view_latch = ViewLatchMode::Disabled;
        let live_follow = sample.following
            && self
                .game_session
                .as_ref()
                .is_some_and(|session| session.live);
        let demo_follow = self
            .game_session
            .as_ref()
            .is_some_and(|session| !session.live)
            && self.demo_view_mode != DemoViewMode::Free;
        let spectator_view = live_follow || demo_follow;
        if sample.teleported && spectator_view {
            self.spectator_camera_state.reset();
            self.third_person_camera.reset();
        }
        let spectator_mode = spectator_view.then_some(self.spectator_camera.mode);
        let render_third_person = match spectator_mode {
            Some(SpectatorCameraMode::FirstPerson) => false,
            Some(SpectatorCameraMode::ThirdPerson) | Some(SpectatorCameraMode::Orbit) => true,
            None => rendering_third_person(
                self.third_person,
                self.first_person_lightsaber,
                sample.policy,
            ),
        };
        let latch_input_available = sample.policy.health > 0
            && sample.policy.vehicle_num == 0
            && spectator_mode.is_none()
            && self.subframe_input_view_rotation().is_some();
        if render_third_person {
            let mut third_person = self.third_person;
            let mut camera_angles = sample.view_angles;
            if let Some(mode) = spectator_mode {
                third_person.enabled = true;
                match mode {
                    SpectatorCameraMode::FirstPerson => unreachable!("handled above"),
                    SpectatorCameraMode::ThirdPerson => {
                        third_person.angle = if self.spectator_camera.motion_direction {
                            // TaystJK cg_thirdPersonAngle -1 semantics: use
                            // the followed player's presented velocity directly.
                            -1.0
                        } else {
                            0.0
                        };
                    }
                    SpectatorCameraMode::Orbit => {
                        third_person.angle = 0.0;
                        third_person.range = self.spectator_camera.orbit_range;
                        third_person.camera_damp = 1.0;
                        camera_angles =
                            self.spectator_camera_state.orbit_angles(sample.view_angles);
                    }
                }
            }
            if self.live_buttons.contains("+zoom") {
                // TaystJK CG_OffsetThirdPersonView: +zoom changes chase-camera
                // range directly using the integer cg_zoomFov value.
                let zoom = self.japro_zoom_fov_integer();
                third_person.range = if zoom < 1 {
                    third_person.range * third_person.range
                } else if zoom > 176 {
                    third_person.range * third_person.range / 176.0
                } else {
                    third_person.range * third_person.range / zoom as f32
                };
            }
            if sample.policy.vehicle_num != 0 {
                // OpenJK bypasses ordinary target/location damping while riding
                // a vehicle, then applies any cameraOverride authored by the
                // resolved ext_data/vehicles/*.veh definition.
                third_person.target_damp = 1.0;
                third_person.camera_damp = 1.0;
                if let Some(vehicle) = self.game_session.as_ref().and_then(|session| {
                    session.ridden_vehicle_definition(sample.policy.vehicle_num)
                }) {
                    let camera = vehicle.camera;
                    if camera.override_enabled {
                        third_person.range = camera.range;
                        third_person.horz_offset = camera.horz_offset;
                        if camera.pitch_dependent_vert_offset {
                            // Match OpenJK's AT-ST-style paired vertical +
                            // pitch hacks when this legacy vehicle flag is set.
                            let pitch = sample.view_angles[0];
                            third_person.pitch_offset = pitch * -0.75;
                            third_person.vert_offset = if pitch > 0.0 {
                                (130.0 - pitch * 10.0).max(-170.0)
                            } else if pitch < 0.0 {
                                (130.0 - pitch * 5.0).min(130.0)
                            } else {
                                30.0
                            };
                        } else {
                            third_person.pitch_offset = camera.pitch_offset;
                            third_person.vert_offset = camera.vert_offset;
                        }
                    } else if vehicle
                        .vehicle_type
                        .as_deref()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("VH_ANIMAL"))
                    {
                        // OpenJK puts animal riders at zero vertical offset
                        // when the vehicle does not author a cameraOverride.
                        third_person.vert_offset = 0.0;
                    }
                }
            }
            if let Some(world) = self.map_collision.as_mut() {
                let view = offset_third_person_view(
                    world,
                    third_person,
                    &mut self.third_person_camera,
                    ThirdPersonViewInput {
                        origin: sample.native_origin,
                        view_angles: camera_angles,
                        velocity: sample.velocity,
                        view_height: sample.view_height,
                        // Orbit is spectator-owned. Do not let OpenJK's dead-camera
                        // yaw override replace the local orbit yaw when the followed
                        // player is dead (or a reconstructed demo POV reports 0 HP).
                        health: if spectator_mode == Some(SpectatorCameraMode::Orbit) {
                            sample.policy.health.max(1)
                        } else {
                            sample.policy.health
                        },
                        dead_yaw: sample.dead_yaw,
                        client_num: sample.client_num,
                        time: sample.presentation_time_ms,
                        teleported: sample.teleported,
                    },
                );
                self.camera.position = glam::Vec3::from_array(scene::render_position(view.origin));
                self.camera.yaw = view.angles[1].to_radians();
                self.camera.pitch = -view.angles[0].to_radians();
                if latch_input_available {
                    if let Some(latch) = view.late_latch {
                        self.render_view_latch = ViewLatchMode::ThirdPerson(latch);
                    }
                }
                return;
            }
        }
        self.camera.position = glam::Vec3::from_array(sample.eye_position);
        self.camera.yaw = sample.view_angles[1].to_radians();
        self.camera.pitch = -sample.view_angles[0].to_radians();
        if latch_input_available {
            self.render_view_latch = ViewLatchMode::Direct;
        }
    }

    pub(in crate::app) fn apply_demo_fake_camera(
        &mut self,
        sample: DemoCameraSample,
        fake: DemoFakeView,
    ) {
        self.render_view_latch = ViewLatchMode::Disabled;
        match self.spectator_camera.mode {
            SpectatorCameraMode::FirstPerson => {
                self.camera.position = glam::Vec3::from_array(fake.eye_position);
                self.camera.yaw = fake.view_angles[1].to_radians();
                self.camera.pitch = -fake.view_angles[0].to_radians();
            }
            SpectatorCameraMode::ThirdPerson | SpectatorCameraMode::Orbit => {
                let mut third_person = self.third_person;
                third_person.enabled = true;
                let mut camera_angles = fake.view_angles;
                match self.spectator_camera.mode {
                    SpectatorCameraMode::FirstPerson => unreachable!("handled above"),
                    SpectatorCameraMode::ThirdPerson => {
                        third_person.angle = if self.spectator_camera.motion_direction {
                            -1.0
                        } else {
                            0.0
                        };
                    }
                    SpectatorCameraMode::Orbit => {
                        third_person.angle = 0.0;
                        third_person.range = self.spectator_camera.orbit_range;
                        third_person.camera_damp = 1.0;
                        camera_angles = self.spectator_camera_state.orbit_angles(fake.view_angles);
                    }
                }
                if self.live_buttons.contains("+zoom") {
                    let zoom = self.japro_zoom_fov_integer();
                    third_person.range = if zoom < 1 {
                        third_person.range * third_person.range
                    } else if zoom > 176 {
                        third_person.range * third_person.range / 176.0
                    } else {
                        third_person.range * third_person.range / zoom as f32
                    };
                }
                if let Some(world) = self.map_collision.as_mut() {
                    let view = offset_third_person_view(
                        world,
                        third_person,
                        &mut self.third_person_camera,
                        ThirdPersonViewInput {
                            origin: fake.native_origin,
                            view_angles: camera_angles,
                            velocity: fake.velocity,
                            // Remote EntityState has no viewheight/dead_yaw.
                            // 26 is JKA's standing viewheight and matches the
                            // first-person approximation used by demo_fake_view.
                            view_height: 26,
                            // Same rule for reconstructed remote demo POVs: Orbit
                            // owns yaw even if remote health is unavailable/zero.
                            health: if self.spectator_camera.mode == SpectatorCameraMode::Orbit {
                                fake.health.max(1)
                            } else {
                                fake.health
                            },
                            dead_yaw: fake.view_angles[1],
                            client_num: fake.client_num,
                            time: sample.presentation_time_ms,
                            teleported: false,
                        },
                    );
                    self.camera.position =
                        glam::Vec3::from_array(scene::render_position(view.origin));
                    self.camera.yaw = view.angles[1].to_radians();
                    self.camera.pitch = -view.angles[0].to_radians();
                } else {
                    self.camera.position = glam::Vec3::from_array(fake.eye_position);
                    self.camera.yaw = fake.view_angles[1].to_radians();
                    self.camera.pitch = -fake.view_angles[0].to_radians();
                }
            }
        }
    }

    pub(in crate::app) fn demo_point_visible_from_authoritative(
        &self,
        sample: DemoCameraSample,
        target_render: [f32; 3],
    ) -> bool {
        let Some(visibility) = self.map_visibility.as_ref() else {
            return true;
        };
        let mut source = sample.native_origin;
        source[2] += sample.view_height as f32;
        let target = scene::jka_position(target_render);
        let Some(target_cluster) = visibility.cluster_at(target) else {
            return false;
        };
        if !visibility.visible(visibility.cluster_at(source), &[target_cluster]) {
            return false;
        }
        if let (Some(area), Some(mask)) = (
            visibility.area_at(target),
            self.game_session
                .as_ref()
                .and_then(GameSession::demo_area_mask),
        ) {
            if area < 256 && mask[area / 8] & (1 << (area % 8)) != 0 {
                return false;
            }
        }
        true
    }

    pub(in crate::app) fn update_demo_free_camera(&mut self, dt: Duration) {
        // +speed is reserved in demos as a temporary cursor/timeline modifier.
        // While held, suspend free-camera translation; mouse-look is suspended
        // too because sync_demo_camera_capture releases the cursor.
        if self.live_buttons.contains("+speed") {
            return;
        }
        let mut movement = glam::Vec3::ZERO;
        // Noclip/FPS semantics: forward/back follow the complete view vector,
        // including pitch. Strafing remains horizontal and perpendicular to yaw,
        // while +moveup/+movedown are explicit world-up/world-down controls.
        let forward = self.camera.forward();
        let (sin_yaw, cos_yaw) = self.camera.yaw.sin_cos();
        let right = glam::Vec3::new(sin_yaw, 0.0, cos_yaw);
        // movement_keys stores semantic +forward/+back/etc as canonical keys,
        // so rebinding movement still drives demo free spec correctly.
        if self.movement_keys.contains(&KeyCode::KeyW) {
            movement += forward;
        }
        if self.movement_keys.contains(&KeyCode::KeyS) {
            movement -= forward;
        }
        if self.movement_keys.contains(&KeyCode::KeyD) {
            movement += right;
        }
        if self.movement_keys.contains(&KeyCode::KeyA) {
            movement -= right;
        }
        if self.movement_keys.contains(&KeyCode::Space) {
            movement += glam::Vec3::Y;
        }
        if self.movement_keys.contains(&KeyCode::ControlLeft) {
            movement -= glam::Vec3::Y;
        }
        if movement.length_squared() > 0.0 {
            let speed =
                DEMO_FREE_SPEEDS[self.demo_free_speed_index.min(DEMO_FREE_SPEEDS.len() - 1)];
            self.camera.position += movement.normalize() * speed * dt.as_secs_f32();
        }
    }

    pub(in crate::app) fn adjust_demo_free_speed(&mut self, direction: i32) {
        if !self.demo_playback_active() || self.demo_view_mode != DemoViewMode::Free {
            return;
        }
        let last = DEMO_FREE_SPEEDS.len().saturating_sub(1) as i32;
        self.demo_free_speed_index =
            (self.demo_free_speed_index as i32 + direction).clamp(0, last) as usize;
        let speed = DEMO_FREE_SPEEDS[self.demo_free_speed_index];
        self.console_status = format!("DEMO FREE CAMERA SPEED: {speed:.0} U/S");
        self.publish_transient_ui();
    }

    pub(in crate::app) fn apply_demo_view(
        &mut self,
        sample: DemoCameraSample,
        dt: Duration,
    ) -> i32 {
        self.demo_view_outside_authoritative = false;
        match self.demo_view_mode {
            DemoViewMode::Authoritative => {
                self.apply_demo_camera(sample);
                sample.client_num
            }
            DemoViewMode::Follow(client) if client == sample.client_num => {
                self.demo_view_mode = DemoViewMode::Authoritative;
                self.apply_demo_camera(sample);
                sample.client_num
            }
            DemoViewMode::Follow(client) => {
                let fake = self
                    .game_session
                    .as_ref()
                    .and_then(|session| session.demo_fake_view(client));
                if let Some(fake) = fake.filter(|view| {
                    self.demo_point_visible_from_authoritative(sample, view.eye_position)
                }) {
                    self.apply_demo_fake_camera(sample, fake);
                    fake.client_num
                } else {
                    self.demo_view_mode = DemoViewMode::Authoritative;
                    self.sync_demo_camera_capture();
                    self.console_status =
                        "DEMO FOLLOW TARGET LEFT RECORDED VISIBILITY; RETURNED TO RECORDED POV"
                            .into();
                    self.apply_demo_camera(sample);
                    sample.client_num
                }
            }
            DemoViewMode::Free => {
                self.update_demo_free_camera(dt);
                self.demo_view_outside_authoritative = !self
                    .demo_point_visible_from_authoritative(sample, self.camera.position.to_array());
                sample.client_num
            }
        }
    }

    pub(in crate::app) fn sync_demo_camera_capture(&mut self) {
        let view_mode = self.demo_view_mode;
        if let Some(session) = self.game_session.as_mut() {
            let before = session.client_game.presentation_client_num();
            session
                .client_game
                .set_presentation_viewer(if session.live || session.local {
                    PresentationViewer::Snapshot
                } else {
                    match view_mode {
                        DemoViewMode::Authoritative => PresentationViewer::Snapshot,
                        DemoViewMode::Follow(client) => PresentationViewer::Client(client),
                        DemoViewMode::Free => PresentationViewer::None,
                    }
                });
            if before != session.client_game.presentation_client_num() {
                // These are viewer-local transients. Never carry the recorder's
                // pickup/timer/reward/no-ammo state across a demo POV switch.
                session.pending_out_of_ammo.clear();
                session.pending_weapon_select.clear();
                session.item_pickup = None;
                session.timer_bar = None;
                session.reward_active = None;
                session.reward_queue.clear();
                session.force_flash_until = None;
            }
        }
        if !self.demo_playback_active() || self.overlay != OverlayMode::None {
            return;
        }
        // Camera input owns the mouse in every demo view. +speed is the one
        // temporary UI modifier: reveal/release the cursor while held so the
        // timeline can be clicked or scrubbed, then recapture on release.
        self.set_capture(!self.live_buttons.contains("+speed") && !self.demo_scrub_dragging);
    }

    pub(in crate::app) fn reset_demo_view_authoritative(&mut self) {
        self.demo_view_mode = DemoViewMode::Authoritative;
        self.demo_free_follow_anchor = None;
        self.demo_view_outside_authoritative = false;
        self.spectator_camera_state.reset();
        self.third_person_camera.reset();
        self.sync_demo_camera_capture();
        self.publish_transient_ui();
    }

    pub(in crate::app) fn enter_demo_free_view(&mut self) {
        if !self.demo_playback_active() || self.demo_view_mode == DemoViewMode::Free {
            return;
        }
        // Remember the POV we detached from so attack/alt-attack in Free Spec
        // advance relative to that player instead of restarting at the recorder.
        self.demo_free_follow_anchor =
            self.game_session
                .as_ref()
                .and_then(|session| match self.demo_view_mode {
                    DemoViewMode::Follow(client) => Some(client),
                    DemoViewMode::Authoritative => session.demo_followed_client(),
                    DemoViewMode::Free => None,
                });
        // Detach from the exact currently rendered first/third/orbit camera.
        // Keeping camera.position/yaw/pitch untouched avoids a visual pop.
        self.demo_view_mode = DemoViewMode::Free;
        self.demo_view_outside_authoritative = false;
        self.spectator_camera_state.reset();
        self.third_person_camera.reset();
        self.sync_demo_camera_capture();
        let speed = DEMO_FREE_SPEEDS[self.demo_free_speed_index];
        self.console_status =
            format!("DEMO CAMERA: FREE · {speed:.0} U/S (MOVE BINDS, WEAPNEXT/WEAPPREV SPEED)");
        self.publish_transient_ui();
    }

    pub(in crate::app) fn cycle_demo_follow(&mut self, direction: i32) {
        let Some(session) = self.game_session.as_ref().filter(|session| !session.live) else {
            return;
        };
        let players = session.demo_visible_players();
        let Some(authoritative) = session.demo_followed_client() else {
            return;
        };
        if players.is_empty() {
            return;
        }
        let current = match self.demo_view_mode {
            DemoViewMode::Follow(client) => client,
            DemoViewMode::Free => self.demo_free_follow_anchor.unwrap_or(authoritative),
            DemoViewMode::Authoritative => authoritative,
        };
        let index = players
            .iter()
            .position(|(client, _)| *client == current)
            .unwrap_or(0) as i32;
        let next_index = (index + direction).rem_euclid(players.len() as i32) as usize;
        let (next, name) = players[next_index].clone();
        self.demo_free_follow_anchor = Some(next);
        self.spectator_camera_state.reset();
        self.third_person_camera.reset();
        if next == authoritative {
            self.demo_view_mode = DemoViewMode::Authoritative;
            self.console_status = format!("DEMO CAMERA: RECORDED POV ({name})");
        } else {
            self.demo_view_mode = DemoViewMode::Follow(next);
            self.console_status = format!("DEMO CAMERA: FAKE POV {name} (CLIENT {next})");
        }
        self.sync_demo_camera_capture();
        self.publish_transient_ui();
    }

    /// The current camera as a Ghoul2 LOD/cull view, for presenters that need
    /// screen-size LOD selection.
    pub(in crate::app) fn current_ghoul2_view(&self) -> Option<Ghoul2PresentationView> {
        self.window.as_ref().map(|window| {
            let size = window.inner_size();
            let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
            Ghoul2PresentationView::new(
                self.camera.position.to_array(),
                self.camera.forward().to_array(),
                self.camera.fov_y_for_viewport(size.width, size.height),
                aspect,
                self.effective_distance_cull(),
            )
        })
    }

    pub(in crate::app) fn tick_demo_playback(
        &mut self,
        now: Instant,
        dt: Duration,
    ) -> Result<DemoAdvance, String> {
        let client_frame_started = Instant::now();
        // Player presentation happens inside GameSession::advance, before the
        // final camera is applied for this frame. Pass the current rendered view
        // shape/pose as a hint: advance() replaces its pose with the exact current
        // sample in first person, while third person retains this previous-frame
        // collision/orbit camera because that state lives here in App.
        let ghoul2_view = self.current_ghoul2_view();
        let local_frame = self
            .local_server
            .as_ref()
            .map(|server| (server.presentation_time(), server.subframe_view_angles()));
        let demo_view_mode = self.demo_view_mode;
        // The network-owned slot remains "us" even when snapshot presentation is
        // following another player. VGS needs both identities: presentation for
        // spatial/listener behavior, connection ownership for our own radio copy.
        let connection_client_num = self
            .net
            .as_ref()
            .and_then(|net| u16::try_from(net.session().client_num()).ok());
        let (advance, sample, debug_lines) = {
            let playback = self
                .game_session
                .as_mut()
                .ok_or_else(|| "DEMO PLAYBACK STATE DISAPPEARED".to_owned())?;
            playback
                .client_game
                .set_presentation_viewer(if playback.live || playback.local {
                    PresentationViewer::Snapshot
                } else {
                    match demo_view_mode {
                        DemoViewMode::Authoritative => PresentationViewer::Snapshot,
                        DemoViewMode::Follow(client) => PresentationViewer::Client(client),
                        DemoViewMode::Free => PresentationViewer::None,
                    }
                });
            playback.demo_spectator_camera_mode =
                if playback.live || self.demo_view_mode == DemoViewMode::Free {
                    None
                } else {
                    Some(self.spectator_camera.mode)
                };
            playback.demo_hidden_view_client = if playback.live
                || self.spectator_camera.mode != SpectatorCameraMode::FirstPerson
            {
                None
            } else {
                match self.demo_view_mode {
                    DemoViewMode::Follow(client) => Some(client),
                    _ => None,
                }
            };
            playback
                .weapon_fx
                .set_modern_sabers(self.video.modern_sabers);
            playback
                .weapon_fx
                .set_saber_impact_fx(self.video.saber_impact_fx);
            playback.weapon_fx.set_saber_marks(self.video.saber_marks);
            // Collision ownership is map-scoped, not frame-scoped. Rebinding it here
            // used to clear WeaponFx's per-blade wall-contact history every frame,
            // so OpenJK-style saber marks could never connect oldPos -> newPos.
            // build_game_session/map-load paths already install the current world.
            playback.weapon_fx.set_saber_trail(self.saber_trail);
            playback.weapon_fx.set_continuous_fx_fps(self.video.fx_fps);
            playback.weapon_fx.set_fx_fps_scope(self.video.fx_fps_scope);
            playback.weapon_fx.set_fx_physics(self.video.fx_physics);
            playback.weapon_fx.set_fx_lod(
                self.video.fx_lod,
                self.video.fx_count_scale,
                self.video.fx_lod_scale,
            );
            playback
                .player_presenter
                .set_skinning_mode(self.video.ghoul2_skinning);
            playback
                .player_presenter
                .set_early_frustum_cull(self.video.ghoul2_early_cull);
            playback
                .entity_presenter
                .set_draw_map_models(self.video.draw_map_models);
            playback
                .player_presenter
                .set_lod_bias(self.video.ghoul2_lod_bias);
            playback
                .player_presenter
                .set_ghoul2_anim_smooth(self.video.ghoul2_anim_smooth);
            playback
                .player_presenter
                .set_lod_scale(self.video.lod_scale);
            playback.player_presenter.set_jiggle_config(
                self.video.client_physics && self.video.jiggle_physics,
                self.video.client_physics_hz,
                self.video.client_physics_max_substeps,
                self.video.jiggle_solver,
                self.video.jiggle_strength,
                self.video.jiggle_breast_strength,
                self.video.jiggle_glute_strength,
                self.video.jiggle_stiffness,
                self.video.jiggle_damping,
                self.video.jiggle_glute_lift,
                self.video.jiggle_jp_stiffness,
                self.video.jiggle_jp_drag,
                self.video.jiggle_jp_air_drag,
                self.video.jiggle_jp_stretch,
                self.video.jiggle_jp_soften,
                self.video.jiggle_jp_gravity,
            );
            playback.player_presenter.set_cloth_config(ClothConfig {
                enabled: self.video.client_physics && self.video.cloth_physics,
                hz: self.video.client_physics_hz,
                max_substeps: self.video.client_physics_max_substeps,
                body_collision: self.video.cloth_body_collision,
                body_clearance: self.video.cloth_body_clearance,
                air_resistance: self.video.cloth_air_resistance,
                turn_response: self.video.cloth_turn_response,
                animation_influence: self.video.cloth_animation_influence,
                wind_velocity: [0.0; 3],
            });
            playback.player_presenter.set_cloth_wind(
                self.video
                    .cloth_wind
                    .then_some(self.video.weather_wind.sanitize()),
            );
            playback.player_presenter.set_ragdoll_config(RagdollConfig {
                physics_enabled: self.video.client_physics,
                enabled: self.video.client_physics && self.video.ragdolls,
                hz: self.video.client_physics_hz,
                max_substeps: self.video.client_physics_max_substeps,
                ccd: self.video.client_physics_ccd,
                sleeping: self.video.client_physics_sleeping,
                max_ragdolls: self.video.ragdoll_max,
                lifetime_seconds: self.video.ragdoll_lifetime,
                dismemberment: self.video.dismemberment,
                max_detached_limbs: self.video.dismember_max,
                detached_limb_lifetime_seconds: self.video.dismember_lifetime,
                self_collision: self.video.ragdoll_self_collision,
                weapon_impulses: self.video.physics_weapon_impulses,
                explosion_impulses: self.video.physics_explosion_impulses,
                force_impulses: self.video.physics_force_impulses,
                debug: self.video.physics_debug_draw,
                stats: self.video.physics_stats,
            });
            let rt_rigid_casters_enabled = self.video.dynamic_shadows
                == DynamicShadowsMode::RayTraced
                || self.video.dynamic_lights == DynamicLightsMode::RayTracedHardware;
            let blob_shadows_enabled = self.video.dynamic_shadows == DynamicShadowsMode::Blob;
            playback
                .weapon_fx
                .set_rt_lighting(self.video.dynamic_lights == DynamicLightsMode::RayTracedHardware);
            if let Some(sound) = playback.sound_presenter.as_mut() {
                sound.set_connection_client_num(if playback.live && !playback.local {
                    connection_client_num
                } else {
                    None
                });
            }
            playback
                .player_presenter
                .set_local_cosmetics(if playback.local {
                    self.network.cosmetics
                } else {
                    0
                });
            playback
                .player_presenter
                .set_sent_cosmetics(if playback.live && !playback.local {
                    self.network.cosmetics
                } else {
                    0
                });
            playback.race_ghost_alpha = self.race_ghost_alpha;
            let (advance, sample) = if playback.local {
                let Some((server_time, _)) = local_frame else {
                    return Err("LOCAL SESSION HAS NO SERVER AUTHORITY".into());
                };
                if let Some(server) = self.local_server.as_mut() {
                    server.set_foot_bolts(playback.player_presenter.viewer_foot_bolts());
                }
                // Solo step events are stamped with the tick the eye interpolates over.
                playback
                    .event_presenter
                    .view_kick_mut()
                    .set_step_lead_ms(self.video.physics_msec as i32);
                playback.advance_to(
                    server_time,
                    self.third_person,
                    self.spectator_camera,
                    self.first_person_lightsaber,
                    self.cg_debug_events,
                    self.cg_event_workers,
                    ghoul2_view,
                    rt_rigid_casters_enabled,
                    blob_shadows_enabled,
                )?
            } else if playback.live {
                let Some(net) = self.net.as_ref() else {
                    return Err("LIVE SESSION HAS NO CONNECTION".into());
                };
                let server_time = net.session().server_time();
                let predictor = &mut self.predictor;
                predictor.set_saber_movement(
                    playback.predicted_saber_movement(net.session().client_num()),
                );
                predictor.set_foot_bolts(playback.player_presenter.viewer_foot_bolts());
                let movement = self.map_movement.as_ref();
                let world = &mut self.map_collision;
                let settings = &self.network;
                let session = net.session();
                let provisional = self.live_provisional;
                let mut logged_failure = false;
                let mut predict = |snap: &ProtocolSnapshot,
                                   next: Option<&ProtocolSnapshot>,
                                   entities: &[PresentedEntity]| {
                    // OpenJK vehicle pmove is a separate path. Until that is ported,
                    // never feed a piloting playerstate through the on-foot predictor;
                    // use snapshot interpolation instead and discard stale prediction.
                    if snap.player_state.field_i32("m_iVehicleNum").unwrap_or(0) != 0 {
                        predictor.reset();
                        return Ok(None);
                    }
                    let (Some(movement), Some(world)) = (movement, world.as_mut()) else {
                        return Ok(None);
                    };
                    let input = crate::net::PredictionInput {
                        session,
                        snap,
                        next,
                        time: server_time,
                        movement,
                        world,
                        entities,
                        settings,
                        provisional,
                    };
                    let previous_display = predictor
                        .predicted()
                        .cloned()
                        .unwrap_or_else(|| snap.player_state.clone());
                    if let Err(error) = predictor.predict(input) {
                        if !logged_failure {
                            eprintln!("PREDICTION FAILED (falling back to interpolation): {error}");
                            logged_failure = true;
                        }
                        return Ok(None);
                    }
                    let error = predictor.view_error(server_time, settings.error_decay);
                    let Some(display) = predictor.predicted().cloned() else {
                        return Ok(None);
                    };
                    Ok(Some(LivePredictionFrame {
                        display,
                        previous_display,
                        error,
                    }))
                };
                playback.advance_to_with_prediction(
                    server_time,
                    f64::from(server_time),
                    self.third_person,
                    self.spectator_camera,
                    self.first_person_lightsaber,
                    self.cg_debug_events,
                    self.cg_event_workers,
                    ghoul2_view,
                    rt_rigid_casters_enabled,
                    blob_shadows_enabled,
                    settings.no_predict,
                    Some(&mut predict),
                )?
            } else {
                playback.advance(
                    now,
                    self.third_person,
                    self.spectator_camera,
                    self.first_person_lightsaber,
                    self.cg_debug_events,
                    self.cg_event_workers,
                    ghoul2_view,
                    rt_rigid_casters_enabled,
                    blob_shadows_enabled,
                )?
            };
            (advance, sample, playback.take_event_debug_lines())
        };
        for line in debug_lines {
            self.push_console_line(line);
        }
        let mut sample = sample;
        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local)
        {
            // Subframe input remains presentation-only. The authoritative
            // local snapshots still contain fixed-Pmove angles, while the camera
            // can consume mouse motion immediately between those steps.
            if let Some((_, angles)) = local_frame {
                sample.view_angles = angles;
            }
        } else if !self.live_remote_view_forced() {
            if let Some(angles) = self.live_view_angles() {
                sample.view_angles = angles;
            }
        }
        let listener_entity = if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.live)
        {
            self.apply_demo_camera(sample);
            sample.client_num
        } else {
            self.apply_demo_view(sample, dt)
        };
        // Cinematic strafehelper keeps the exact TaystJK angle/colour decisions,
        // but submits them through the world FX path. Compute before borrowing
        // game_session mutably so current_movement_hud_state can read whichever
        // predicted/followed player is actually driving the HUD this frame.
        let cinematic_strafe_rays = if self.strafe_helper.flags & ui::SHELPER_CINEMATIC != 0
            && !self.video.skip_ui
            && self.loading.is_none()
            && matches!(
                self.overlay,
                OverlayMode::None
                    | OverlayMode::Chat
                    | OverlayMode::Vgs
                    | OverlayMode::HudEdit
                    | OverlayMode::CameraEdit
                    | OverlayMode::MapEdit
            ) {
            let hud = self.current_movement_hud_state();
            crate::strafehelper::strafe_world_rays(&self.strafe_helper, &hud, self.video.fps_cap)
        } else {
            Vec::new()
        };
        // Use the final camera (including third-person orbit/free/fake follow) for spatial audio.
        if let Some(playback) = &mut self.game_session {
            // FX sprites/lines face the final render view.
            let view = crate::fx::draw::FxView::from_camera(&self.camera);
            playback.weapon_fx.set_view(view.origin, view.axis[1]);
            if let Some(window) = self.window.as_ref() {
                let size = window.inner_size();
                let (viewport_width, viewport_height) = (size.width.max(1), size.height.max(1));
                let fov_y = self
                    .camera
                    .fov_y_for_viewport(viewport_width, viewport_height);
                playback.weapon_fx.set_lod_view(
                    view.origin,
                    viewport_height as f32 / (2.0 * (fov_y * 0.5).tan()).max(1e-3),
                );
            }
            let flare = if self.video.flares {
                let (viewport_width, viewport_height) = self
                    .window
                    .as_ref()
                    .map(|window| {
                        let size = window.inner_size();
                        (size.width.max(1), size.height.max(1))
                    })
                    .unwrap_or((640, 480));
                let flare_fov_y = self
                    .camera
                    .fov_y_for_viewport(viewport_width, viewport_height);
                let flare_fov_x = 2.0
                    * ((flare_fov_y * 0.5).tan() * viewport_width as f32 / viewport_height as f32)
                        .atan();
                playback.weapon_fx.saber_clash_flare(
                    &view,
                    flare_fov_x.to_degrees(),
                    flare_fov_y.to_degrees(),
                )
            } else {
                None
            };
            // jaPRO strafe trails are presentation-only. Loaded CFGs are spatially
            // culled here and then use the normal FxDraw::Line batching path. Live
            // player sampling and race logging are likewise client-side only.
            let trail_time = playback
                .current_snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.server_time);
            for entity in &playback.presented_entities {
                if entity.entity_type == ET_PLAYER {
                    self.strafe_trails.sample_player(
                        usize::from(entity.number),
                        entity.origin,
                        trail_time,
                    );
                }
            }
            if let Some(snapshot) = playback.current_snapshot.as_ref() {
                let ps = &snapshot.player_state;
                let own_client = ps.field_i32("clientNum").unwrap_or(sample.client_num);
                if let (Ok(client), Some(origin)) =
                    (usize::try_from(own_client), playerstate_vec3(ps, "origin"))
                {
                    self.strafe_trails.sample_player(client, origin, trail_time);
                    let race_active = playback.client_game.is_japro()
                        && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0
                        && ps.field_i32("duelTime").unwrap_or(0) != 0;
                    self.strafe_trails
                        .log_point_if_active(origin, trail_time, race_active);
                }
            }
            let (trail_viewport_width, trail_viewport_height) = self
                .window
                .as_ref()
                .map(|window| {
                    let size = window.inner_size();
                    (size.width.max(1), size.height.max(1))
                })
                .unwrap_or((640, 480));
            let trail_fov_y = self
                .camera
                .fov_y_for_viewport(trail_viewport_width, trail_viewport_height);
            let trail_tan_half_y = (trail_fov_y * 0.5).tan().max(1e-3);
            let trail_tan_half_x =
                trail_tan_half_y * trail_viewport_width as f32 / trail_viewport_height as f32;
            let trail_view = crate::strafe_trail::TrailView {
                origin: view.origin,
                forward: view.axis[0],
                left: view.axis[1],
                up: view.axis[2],
                tan_half_fov_x: trail_tan_half_x,
                tan_half_fov_y: trail_tan_half_y,
                focal_length_pixels: trail_viewport_height as f32 / (2.0 * trail_tan_half_y),
            };
            self.strafe_trails
                .append_draws(trail_view, trail_time, &mut playback.fx_draws);

            // Two same-hue additive ribbons produce a soft coloured glow with a
            // brighter coloured centre. Deliberately do not submit RGBcore1 (or
            // any white line): Cinematic is saber-ish glow, not a saber blade.
            // RGBglow1 is jaPRO's tintable glow asset and EntityPresenter already
            // synthesizes it from stock JKA saber art when the jaPRO texture is
            // unavailable.
            for ray in &cinematic_strafe_rays {
                let alpha = ray.color[3].clamp(0.0, 1.0);
                let rgba = |gain: f32| {
                    let channel = |value: f32| {
                        (value.clamp(0.0, 1.0) * alpha * gain * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    };
                    [
                        channel(ray.color[0]),
                        channel(ray.color[1]),
                        channel(ray.color[2]),
                        255,
                    ]
                };
                playback.fx_draws.push(crate::fx::system::FxDraw::Line {
                    start: ray.start,
                    end: ray.end,
                    width: ray.width * 2.75,
                    rgba: rgba(0.30),
                    shader: "gfx/effects/sabers/RGBglow1".to_owned(),
                });
                playback.fx_draws.push(crate::fx::system::FxDraw::Line {
                    start: ray.start,
                    end: ray.end,
                    width: ray.width,
                    rgba: rgba(0.90),
                    shader: "gfx/effects/sabers/RGBglow1".to_owned(),
                });
            }
            if self.race_ghost_trail {
                for ghost in &mut playback.race_ghosts {
                    self.strafe_trails.append_prebuilt_draws(
                        &mut ghost.track.strafe_trail,
                        trail_view,
                        &mut playback.fx_draws,
                    );
                }
            }

            playback.client_perf.fx_draws =
                u32::try_from(playback.fx_draws.len()).unwrap_or(u32::MAX);
            playback.client_perf.fx_sprites = 0;
            playback.client_perf.fx_oriented_quads = 0;
            playback.client_perf.fx_lines = 0;
            playback.client_perf.fx_quads = 0;
            playback.client_perf.fx_meshes = 0;
            playback.client_perf.fx_cylinders = 0;
            for draw in &playback.fx_draws {
                match draw {
                    crate::fx::system::FxDraw::Sprite { .. }
                    | crate::fx::system::FxDraw::SaberGlow { .. } => {
                        playback.client_perf.fx_sprites += 1
                    }
                    crate::fx::system::FxDraw::OrientedQuad { .. } => {
                        playback.client_perf.fx_oriented_quads += 1
                    }
                    crate::fx::system::FxDraw::Line { .. } => playback.client_perf.fx_lines += 1,
                    crate::fx::system::FxDraw::Quad { .. } => playback.client_perf.fx_quads += 1,
                    crate::fx::system::FxDraw::Mesh { .. }
                    | crate::fx::system::FxDraw::LitMesh { .. } => {
                        playback.client_perf.fx_meshes += 1
                    }
                    crate::fx::system::FxDraw::Cylinder { .. } => {
                        playback.client_perf.fx_cylinders += 1
                    }
                }
            }
            // TaystJK CG_DrawReward advances its FIFO when REWARD_TIME expires
            // and starts the next announcer sound at that exact handoff.
            if playback
                .reward_active
                .is_some_and(|reward| reward.started.elapsed().as_millis() as f32 >= REWARD_TIME_MS)
            {
                playback.reward_active =
                    playback.reward_queue.pop_front().map(|spec| ActiveReward {
                        spec,
                        started: Instant::now(),
                    });
                if !playback.suppress_audio {
                    if let (Some(reward), Some(sound)) =
                        (playback.reward_active, playback.sound_presenter.as_mut())
                    {
                        sound.play_announcer_sound(reward.spec.sound);
                    }
                }
            }

            let fx_tessellate_started = Instant::now();
            let entity_presenter = &mut playback.entity_presenter;
            playback.screen_fx = Arc::new(flare.map_or_else(Vec::new, |flare| {
                entity_presenter
                    .fx_material_stages("gfx/effects/saberFlare")
                    .into_iter()
                    .map(|material| crate::fx::draw::ScreenFxDraw {
                        rect: flare.rect,
                        uv_rect: [0.0, 0.0, 1.0, 1.0],
                        color: flare.color,
                        material,
                        anchor: crate::fx::draw::ScreenFxAnchor::Stretch,
                    })
                    .collect()
            }));
            // CG_DrawGenericTimerBar: the dispenser toss cooldown.
            if let Some((started, duration)) = playback.timer_bar {
                let elapsed = started.elapsed().as_millis() as i32;
                if elapsed >= duration {
                    playback.timer_bar = None;
                } else {
                    let mut draws = playback.screen_fx.as_ref().clone();
                    generic_timer_bar(&mut draws, (duration - elapsed) as f32 / duration as f32);
                    playback.screen_fx = Arc::new(draws);
                }
            }
            // CG_DrawPickupItem: the last picked-up item's icon, faded over 3 s.
            if let Some((icon, started)) = playback.item_pickup.clone() {
                let age = started.elapsed().as_millis() as f32;
                if age >= 3000.0 {
                    playback.item_pickup = None;
                } else {
                    let alpha = if 3000.0 - age < 200.0 {
                        (3000.0 - age) / 200.0
                    } else {
                        1.0
                    };
                    let icons: Vec<_> = entity_presenter
                        .fx_material_stages(&icon)
                        .into_iter()
                        .map(|material| crate::fx::draw::ScreenFxDraw {
                            rect: [585.0, 480.0 - 160.0, 48.0, 48.0],
                            uv_rect: [0.0, 0.0, 1.0, 1.0],
                            color: [1.0, 1.0, 1.0, alpha],
                            material,
                            anchor: crate::fx::draw::ScreenFxAnchor::Right,
                        })
                        .collect();
                    let mut draws = playback.screen_fx.as_ref().clone();
                    draws.extend(icons);
                    playback.screen_fx = Arc::new(draws);
                }
            }
            // TaystJK CG_DrawReward: 3 s FIFO entries, 200 ms blob scale at
            // both ends, repeated medals below ten, and a small charset count
            // once the cumulative award counter reaches double digits.
            if let Some(active) = playback.reward_active {
                let age = active.started.elapsed().as_millis() as f32;
                if age < REWARD_TIME_MS {
                    let remaining = REWARD_TIME_MS - age;
                    let alpha = if remaining < REWARD_BLOB_MS {
                        remaining / REWARD_BLOB_MS
                    } else {
                        1.0
                    };
                    let scale = if age <= REWARD_BLOB_MS {
                        age / REWARD_BLOB_MS
                    } else if remaining <= REWARD_BLOB_MS {
                        remaining / REWARD_BLOB_MS
                    } else {
                        1.0
                    };
                    let icon_size = REWARD_ICON_SIZE * scale.clamp(0.0, 1.0);
                    let y = 56.0 + (REWARD_ICON_SIZE - icon_size) * 0.5;
                    let mut draws = playback.screen_fx.as_ref().clone();
                    let icon_materials = entity_presenter
                        .fx_material_stages_if_present(active.spec.shader)
                        .unwrap_or_else(|| match active.spec.fallback_shader {
                            Some(fallback) => entity_presenter.fx_material_stages(fallback),
                            None => entity_presenter.fx_material_stages(active.spec.shader),
                        });
                    let count = active.spec.count.max(1);
                    if count >= 10 {
                        let x = 320.0 - icon_size * 0.5;
                        for material in &icon_materials {
                            draws.push(crate::fx::draw::ScreenFxDraw {
                                rect: [
                                    x,
                                    y,
                                    (icon_size - 4.0).max(0.0),
                                    (icon_size - 4.0).max(0.0),
                                ],
                                uv_rect: [0.0, 0.0, 1.0, 1.0],
                                color: [1.0, 1.0, 1.0, alpha],
                                material: material.clone(),
                                anchor: crate::fx::draw::ScreenFxAnchor::Center,
                            });
                        }

                        let label = count.to_string();
                        let mut x = (640.0 - 8.0 * label.len() as f32) * 0.5;
                        let charset = entity_presenter.fx_material_stages("gfx/2d/charsgrid_med");
                        for glyph in label.bytes() {
                            let uv_rect = charset_glyph_uv(glyph);
                            for material in &charset {
                                // CG_DrawStringExt's small-font shadow.
                                draws.push(crate::fx::draw::ScreenFxDraw {
                                    rect: [x + 2.0, 106.0, 8.0, 16.0],
                                    uv_rect,
                                    color: [0.0, 0.0, 0.0, alpha],
                                    material: material.clone(),
                                    anchor: crate::fx::draw::ScreenFxAnchor::Center,
                                });
                                draws.push(crate::fx::draw::ScreenFxDraw {
                                    rect: [x, 104.0, 8.0, 16.0],
                                    uv_rect,
                                    color: [1.0, 1.0, 1.0, alpha],
                                    material: material.clone(),
                                    anchor: crate::fx::draw::ScreenFxAnchor::Center,
                                });
                            }
                            x += 8.0;
                        }
                    } else {
                        let mut x = 320.0 - count as f32 * icon_size * 0.5;
                        for _ in 0..count {
                            for material in &icon_materials {
                                draws.push(crate::fx::draw::ScreenFxDraw {
                                    rect: [
                                        x,
                                        y,
                                        (icon_size - 4.0).max(0.0),
                                        (icon_size - 4.0).max(0.0),
                                    ],
                                    uv_rect: [0.0, 0.0, 1.0, 1.0],
                                    color: [1.0, 1.0, 1.0, alpha],
                                    material: material.clone(),
                                    anchor: crate::fx::draw::ScreenFxAnchor::Center,
                                });
                            }
                            x += icon_size;
                        }
                    }
                    playback.screen_fx = Arc::new(draws);
                }
            }

            let fx_surfaces = match self.video.fx_geometry {
                FxGeometryMode::Cpu => {
                    crate::fx::draw::tessellate(&playback.fx_draws, &view, &mut |shader| {
                        entity_presenter.fx_material_stages(shader)
                    })
                }
                FxGeometryMode::CpuWorkers => {
                    crate::fx::draw::tessellate_workers(&playback.fx_draws, &view, &mut |shader| {
                        entity_presenter.fx_material_stages(shader)
                    })
                }
                FxGeometryMode::Gpu
                    if !matches!(self.video.dynamic_shadows, DynamicShadowsMode::RayTraced)
                        && self.video.dynamic_lights != DynamicLightsMode::RayTracedHardware =>
                {
                    crate::fx::draw::tessellate_gpu_particles(
                        &playback.fx_draws,
                        &view,
                        &mut |shader| entity_presenter.fx_material_stages(shader),
                    )
                }
                // The hardware-RT dynamic preparation owns a separate shared vertex/index
                // stream and does not consume the instanced FX buffer yet. Preserve all RT
                // paths until FX sprites get a dedicated integration there.
                FxGeometryMode::Gpu => {
                    crate::fx::draw::tessellate_workers(&playback.fx_draws, &view, &mut |shader| {
                        entity_presenter.fx_material_stages(shader)
                    })
                }
            };
            playback.client_perf.fx_render_surfaces =
                u32::try_from(fx_surfaces.len()).unwrap_or(u32::MAX);
            playback.client_perf.fx_cpu_geom_surfaces = 0;
            playback.client_perf.fx_cpu_vertices = 0;
            playback.client_perf.fx_cpu_indices = 0;
            playback.client_perf.fx_gpu_sprite_batches = 0;
            playback.client_perf.fx_gpu_sprite_instances = 0;
            for surface in &fx_surfaces {
                if let Some(sprites) = surface.fx_gpu_sprites.as_ref() {
                    playback.client_perf.fx_gpu_sprite_batches =
                        playback.client_perf.fx_gpu_sprite_batches.saturating_add(1);
                    playback.client_perf.fx_gpu_sprite_instances = playback
                        .client_perf
                        .fx_gpu_sprite_instances
                        .saturating_add(u32::try_from(sprites.instances.len()).unwrap_or(u32::MAX));
                } else {
                    playback.client_perf.fx_cpu_geom_surfaces =
                        playback.client_perf.fx_cpu_geom_surfaces.saturating_add(1);
                    playback.client_perf.fx_cpu_vertices = playback
                        .client_perf
                        .fx_cpu_vertices
                        .saturating_add(surface.vertices.len() as u64);
                    playback.client_perf.fx_cpu_indices = playback
                        .client_perf
                        .fx_cpu_indices
                        .saturating_add(surface.indices.len() as u64);
                }
            }
            playback.fx_surfaces = Arc::new(fx_surfaces);
            playback.client_perf.fx_tessellate_ms =
                fx_tessellate_started.elapsed().as_secs_f64() * 1000.0;
            let audio_started = Instant::now();
            if !playback.suppress_audio {
                if let Some(sound) = &mut playback.sound_presenter {
                    let yaw = self.camera.yaw;
                    sound.frame(
                        crate::audio::Listener {
                            entity: listener_entity.max(0) as u16,
                            origin: scene::jka_position(self.camera.position.to_array()),
                            ahead: [yaw.cos(), yaw.sin(), 0.0],
                            up: [0.0, 0.0, 1.0],
                            left: [-yaw.sin(), yaw.cos(), 0.0],
                        },
                        &playback.presented_entities,
                        playback.audio_followed_entity.as_ref(),
                    );
                    if let Some(snapshot) = playback.client_game.current_snapshot() {
                        sound.update_announcer(
                            &playback.client_game,
                            &playback.siege_classes,
                            snapshot.server_time,
                        );
                    }
                }
            }
            playback.client_perf.audio_ms += audio_started.elapsed().as_secs_f64() * 1000.0;
            playback.client_perf.total_ms = client_frame_started.elapsed().as_secs_f64() * 1000.0;
        }
        if self.network.hitch_record {
            self.record_hitch_frame(now, dt);
        }
        if self.network.prediction_debug || self.network.prediction_miss_highlight {
            // Diagnostics are deliberately opt-in. While active, publish the
            // volatile overlay every client frame so a one-frame miss and its
            // 350 ms edge flash cannot be lost between ordinary UI updates.
            self.publish_transient_ui();
        }
        Ok(advance)
    }
}
