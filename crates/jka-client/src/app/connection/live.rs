//! Connection live.
use crate::app::{
    scene, App, Arc, Duration, FrontendPage, GameSession, HashSet, Instant, LiveJoinTiming,
    OverlayMode, PhysicsMapMesh, RenderCommand, SessionPhase,
};

impl App {
    pub(in crate::app) fn connect_to_server(&mut self, target: &str) {
        if self.net.is_some() || self.game_session.is_some() {
            self.disconnect_to_main_menu();
        }
        self.disconnect_notice = None;
        self.server_password_prompt = None;
        self.japro_autologin_attempted = None;
        // Start before DNS/socket setup so the join metric really measures from
        // the user's connect action to the first rendered world frame.
        self.live_join_timing = Some(LiveJoinTiming::new());
        self.missing_map_prompt = None;
        self.live_missing_map_authorized = false;
        self.live_auto_download_requested = false;
        self.live_http_base = None;
        self.live_download = None;
        self.live_join_ui = None;
        let userinfo = self.network.userinfo(&self.solo_client_info.model_cvar());
        match crate::net::NetClient::connect_preflight(target, userinfo, self.network.net_port) {
            Ok(net) => {
                // NET_OpenIP parity: if the requested port was occupied and we
                // scanned upward, expose the actual bound port through net_port.
                self.network.net_port = net.local_port();
                let resolved_server = net.session().server();
                self.push_console_line(format!("^7{target} resolved to {}", resolved_server));
                self.console_status = format!("CONNECTING TO {}...", target.to_ascii_uppercase());
                self.reconnect_target = Some(target.to_owned());
                self.live_input = crate::net::LiveInput::default();
                self.predictor.reset();
                self.pending_frontend_map_launch = false;
                self.pending_live_server_commands.clear();
                self.start_live_cgame_prep();
                self.net = Some(net);
                self.begin_chat_log_for_current_connection();
                self.publish_ui();
            }
            Err(error) => {
                self.live_join_timing = None;
                self.console_status = error.clone();
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    /// CL_Rcon_f: send the rest of the console line to the connected server, or
    /// to `rconAddress` while not connected.
    pub(in crate::app) fn send_rcon(&mut self, command: &str) {
        if self.network.rcon_password.is_empty() {
            self.push_console_line("You must set 'rconpassword' before issuing an rcon command.");
            return;
        }
        let target = if self.live_connected() {
            self.net.as_ref().map(|net| net.session().server())
        } else if self.network.rcon_address.is_empty() {
            self.push_console_line(
                "You must either be connected,\nor set the 'rconAddress' cvar\nto issue rcon commands",
            );
            return;
        } else {
            match crate::net::resolve_server(&self.network.rcon_address) {
                Ok(address) => Some(address),
                Err(error) => {
                    self.push_console_line(format!("^1{error}"));
                    return;
                }
            }
        };
        let Some(target) = target else { return };
        if self
            .rcon
            .as_ref()
            .is_none_or(|channel| channel.target() != target)
        {
            match crate::net::RconChannel::open(target) {
                Ok(channel) => self.rcon = Some(channel),
                Err(error) => {
                    self.push_console_line(format!("^1{error}"));
                    return;
                }
            }
        }
        if let Some(Err(error)) = self
            .rcon
            .as_ref()
            .map(|channel| channel.send(&self.network.rcon_password, command))
        {
            self.push_console_line(format!("^1{error}"));
        }
    }

    /// Prints the `print` replies to an earlier `rcon`.
    pub(in crate::app) fn poll_rcon(&mut self) {
        let Some(replies) = self.rcon.as_ref().map(crate::net::RconChannel::poll) else {
            return;
        };
        for reply in replies {
            let text = self.translate_server_text(&reply);
            for line in text.lines().filter(|line| !line.is_empty()) {
                self.push_console_line(line.to_owned());
            }
        }
    }

    /// Per-frame network work: CL_PacketEvent for every queued datagram,
    /// CL_SetCGameTime, CL_CreateNewCommands and CL_SendCmd.
    pub(in crate::app) fn tick_network(&mut self, frame: Duration) {
        self.poll_rcon();
        self.poll_live_download();
        let events = {
            let Some(net) = self.net.as_mut() else { return };
            net.pump()
        };
        self.poll_live_cgame_prep();
        for event in events {
            self.handle_session_event(event);
            if self.net.is_none() {
                return;
            }
        }
        self.poll_live_cgame_prep();
        if self.pending_live_gamestate && self.live_cgame_prep_rx.is_none() {
            self.pending_live_gamestate = false;
            self.start_live_gamestate();
            if self.net.is_none() {
                return;
            }
        }
        if self.game_session.as_ref().is_some_and(|session| {
            session.live && session.phase == SessionPhase::WaitingForSnapshot
        }) {
            self.begin_live_session();
        }
        if self.scoreboard_should_show() || self.companion_scoreboard_visible() {
            self.request_scores_if_due(false);
        }
        let Some(net) = self.net.as_mut() else { return };
        let realtime = net.realtime();
        net.session_mut()
            .set_cgame_time(realtime, self.network.time_nudge);
        if net.state() >= jka_protocol::session::ConnectionState::Primed {
            // Subframe CL_MouseMove already advanced cl.viewangles on each raw
            // mouse event. Client-frame work only packages the current angles.
            let playing = self.overlay == OverlayMode::None && self.captured;
            if let Some(ps) = self
                .game_session
                .as_ref()
                .filter(|s| s.live)
                .and_then(|s| s.current_snapshot.as_ref())
            {
                let ps = ps.player_state.clone();
                self.live_input.sync_selection(&ps);
            }
            self.apply_out_of_ammo_changes();
            let empty = HashSet::new();
            let buttons = crate::net::CommandButtons {
                active: if playing { &self.live_buttons } else { &empty },
                forced_moveup: playing && self.japro_flipkick_moveup,
                any_key: playing && (!self.keys.is_empty() || !self.mouse_buttons_down.is_empty()),
                // jaPRO CL_CmdButtons: `Key_GetCatcher() || (unfocused &&
                // cl_chatBubbleUnfocused)`, then `cl_chatBubbleSelf`.
                talking: (self.overlay != OverlayMode::None
                    || (!self.window_focused && self.network.chat_bubble_unfocused))
                    && self.network.chat_bubble_self,
            };
            // CL_CreateNewCommands runs once per client frame; our tick rate
            // follows input events, so pace commands explicitly. Mouse keeps
            // accumulating in cl.viewangles between commands.
            let now = Instant::now();
            let rate = f64::from(self.network.command_rate.max(15));
            let session_time = self
                .net
                .as_ref()
                .expect("checked above")
                .session()
                .server_time();
            if self.network.command_pacing {
                // Race physics steps by the gap between usercmds, so stamp them on an exact
                // cl.serverTime cadence (a stock client at com_maxfps 125 sends 8 ms gaps).
                // Pacing on the wall clock but stamping cl.serverTime, which advances in
                // 0/1/2 ms lumps per tick, gave gaps of 5-10 ms around the same mean.
                let interval = 1000.0 / rate;
                let now_ms = f64::from(session_time);
                // More than one command per tick only to catch up after a 2+ ms tick gap
                // at 1-2 ms intervals.
                for _ in 0..3 {
                    let slot = match self.next_command_time {
                        // First command, time went backwards (map restart), or we fell behind:
                        // restart the cadence from now instead of bursting to catch up.
                        None => Some((session_time, now_ms + interval)),
                        Some(next)
                            if now_ms < next - interval * 4.0 || now_ms - next > interval * 3.0 =>
                        {
                            Some((session_time, now_ms + interval))
                        }
                        Some(next) if now_ms >= next => {
                            Some((next.floor() as i32, next + interval))
                        }
                        Some(_) => None,
                    };
                    let Some((stamp, next)) = slot else { break };
                    let net = self.net.as_mut().expect("checked above");
                    let previous = net
                        .session()
                        .command(net.session().cmd_number())
                        .map(|cmd| cmd.server_time);
                    // Stay strictly after the previous command and never in the future.
                    let stamp = previous.map_or(stamp, |previous| stamp.max(previous + 1));
                    if stamp > session_time {
                        break;
                    }
                    self.next_command_time = Some(next);
                    let cmd = self.live_input.create_cmd(&buttons);
                    net.session_mut().create_command_at(cmd, stamp);
                }
            } else {
                let interval = Duration::from_secs_f64(1.0 / rate);
                let due = self
                    .last_command_at
                    .is_none_or(|last| now.saturating_duration_since(last) >= interval);
                if due {
                    self.last_command_at = Some(match self.last_command_at {
                        // Keep a steady cadence without accumulating backlog.
                        Some(last) if now.saturating_duration_since(last) < interval * 2 => {
                            last + interval
                        }
                        _ => now,
                    });
                    let cmd = self.live_input.create_cmd(&buttons);
                    let net = self.net.as_mut().expect("checked above");
                    net.session_mut().create_command(cmd);
                }
            }
            let server_time = self
                .net
                .as_ref()
                .expect("checked above")
                .session()
                .server_time();
            let mut provisional = self.live_input.preview_cmd(&buttons);
            provisional.server_time = server_time;
            self.live_provisional = Some(provisional);
        } else {
            self.live_provisional = None;
        }
        let net = self.net.as_mut().expect("checked above");
        net.session_mut()
            .set_packet_dup(self.network.packet_dup as i32);
        net.session_mut()
            .send_commands(realtime, self.network.max_packets as i32);
        net.flush();
    }

    pub(in crate::app) fn handle_session_event(
        &mut self,
        event: jka_protocol::session::SessionEvent,
    ) {
        use jka_protocol::session::{ConnectionState, SessionEvent};
        match event {
            SessionEvent::StateChanged(state) => {
                let text = match state {
                    ConnectionState::Connecting => "Awaiting challenge...",
                    ConnectionState::Challenging => "Awaiting connection...",
                    ConnectionState::Connected => "Awaiting gamestate...",
                    ConnectionState::Primed => "Awaiting snapshot...",
                    ConnectionState::Active => "Connected.",
                    ConnectionState::Disconnected => "Disconnected.",
                };
                self.console_status = text.to_ascii_uppercase();
                println!("NET: {state:?} - {text}");
                self.push_console_line(format!("^5{text}"));
                if state == ConnectionState::Connected {
                    if let Some(address) = self.net.as_ref().map(|net| net.session().server()) {
                        if let Err(error) = self.server_browser.remember_history(address) {
                            eprintln!("SERVER BROWSER: could not save history: {error}");
                        }
                    }
                }
            }
            SessionEvent::ServerInfo(info) => self.handle_live_server_info(info),
            SessionEvent::Print(text) => {
                let text = self.translate_server_text(&text);
                for line in text.lines().filter(|line| !line.is_empty()) {
                    self.push_console_line(line.to_owned());
                }
                // A wrong/missing g_password is rejected by the game module as
                // a connectionless `print`, not as SessionEvent::Disconnected.
                // Keep the console copy, but surface a password retry dialog
                // instead of leaving the user stuck at "Awaiting connection".
                let awaiting_connect = self.net.as_ref().is_some_and(|net| {
                    matches!(
                        net.session().state(),
                        ConnectionState::Connecting | ConnectionState::Challenging
                    )
                });
                if awaiting_connect && Self::is_server_password_rejection(&text) {
                    let target = self
                        .reconnect_target
                        .clone()
                        .or_else(|| {
                            self.net
                                .as_ref()
                                .map(|net| net.session().server().to_string())
                        })
                        .unwrap_or_default();
                    self.show_server_password_prompt(
                        target,
                        Some(crate::logging::strip_jka_colors(&text)),
                    );
                }
            }
            SessionEvent::Gamestate => {
                let elapsed = self
                    .live_join_timing
                    .as_ref()
                    .map(LiveJoinTiming::elapsed_ms);
                if let (Some(timing), Some(elapsed)) = (self.live_join_timing.as_mut(), elapsed) {
                    if timing.gamestate_ms.is_none() {
                        timing.gamestate_ms = Some(elapsed);
                        devprintln!(1, "[JOIN] gamestate received at +{elapsed:.1} ms");
                    }
                }
                self.start_live_gamestate();
            }
            SessionEvent::ServerCommand(command) => {
                // Decoder configstrings are current before this event is delivered.
                if command.text.starts_with(b"cs 0 ") || command.text.starts_with(b"bcs2 0 ") {
                    self.send_userinfo();
                }
                if let Some(session) = self.game_session.as_mut().filter(|session| session.live) {
                    session.client_game.queue_server_command(command);
                } else if self.net.is_some() {
                    // A gamestate can beat the CPU prep worker. Keep reliable
                    // commands received in that overlap window instead of
                    // dropping them just because GameSession is not built yet.
                    if self.pending_live_server_commands.len() >= 128 {
                        self.pending_live_server_commands.pop_front();
                    }
                    self.pending_live_server_commands.push_back(command);
                }
            }
            SessionEvent::Snapshot(snapshot) => {
                if let Some(session) = self.game_session.as_mut().filter(|session| session.live) {
                    if session.live_snapshots.len() >= 128 {
                        session.live_snapshots.pop_front();
                    }
                    session.live_snapshots.push_back(snapshot);
                }
            }
            SessionEvent::DemoMessage {
                sequence,
                payload,
                full_snapshot,
            } => {
                self.record_demo_message(sequence, payload, full_snapshot);
            }
            SessionEvent::Download(block) => self.handle_live_download_block(block),
            SessionEvent::MapChange => {
                // CG_MapChange: sticks until the next gamestate builds a fresh session.
                if let Some(session) = self.game_session.as_mut().filter(|session| session.live) {
                    session.map_change = true;
                }
            }
            SessionEvent::Disconnected(reason) => {
                let reason = self.translate_server_text(reason.as_bytes());
                println!("NET: disconnected: {reason}");
                self.push_console_line(format!("^1{reason}"));
                if self.chat_log_enabled {
                    self.chat_log.end(format!(
                        "Disconnected: {}",
                        crate::logging::strip_jka_colors(&reason)
                    ));
                }
                self.net = None;
                self.disconnect_to_main_menu();
                self.console_status = reason.to_ascii_uppercase();
                self.push_console_line(format!("^3Use ^7reconnect^3 to rejoin."));
                // Mirrors jaPRO/stock JKA's com_errorMessage -> error_popmenu: an
                // involuntary drop (kick/ban/server message) gets a popup the
                // player actually sees, not just a line buried in the console.
                self.disconnect_notice = Some(crate::logging::strip_jka_colors(&reason));
            }
        }
    }

    /// Clear any locally loaded BSP state while preserving the active live/demo session.
    pub(in crate::app) fn prepare_worldless_session(&mut self, map_name: &str) {
        // Invalidate any previous/front-end map job so a late worker result cannot
        // repopulate a stale world behind the active session.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.pending_frontend_map_launch = false;
        self.frontend_background_request_id = None;
        self.frontend_cinematic = None;
        self.preserve_game_state_on_next_map_upload = false;
        self.loading = None;
        self.static_ao_progress = None;
        self.prepared_map_cache = None;
        self.live_join_ui = None;

        self.local_server = None;
        self.map_collision = None;
        self.map_mark_surfaces = None;
        self.map_static_models = Arc::default();
        self.map_visibility = None;
        self.map_physics_collision = PhysicsMapMesh::default();
        self.map_movement = None;
        self.third_person_camera.reset();
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.spawns.clear();
        self.spawn_index = 0;
        self.map_name = format!("{map_name} (WORLD MISSING)");
        self.triangles = 0;
        self.map_distance_cull = crate::camera::DEFAULT_DISTANCE_CULL;
        self.map_authored_sun = None;
        self.map_authored_oceans.clear();
        self.authored_oceans.clear();
        self.authored_ocean_preview = false;
        self.render_command(RenderCommand::SetAuthoredOceans(Vec::new()));
        self.forget_trace();
        self.front_end = false;
        self.frontend_page = FrontendPage::Main;
        self.menu_selected = 0;
        self.render_command(RenderCommand::UnloadMap);
        self.sync_render_fps_cap();
        self.set_overlay(OverlayMode::None);
    }

    /// Live equivalent of finishing downloads with no BSP: CGame/entities keep
    /// running, while prediction falls back to snapshot interpolation because
    /// map collision is absent.
    pub(in crate::app) fn prime_live_session_without_world(
        &mut self,
        map_name: &str,
        reason: &str,
    ) {
        self.prepare_worldless_session(map_name);
        self.live_without_world = true;
        self.demo_without_world = false;

        self.console_status = format!("CONNECTED WITHOUT MAP: {map_name}");
        self.push_console_line(format!(
            "^3Map unavailable locally; continuing without world geometry:^7 {reason}"
        ));
        self.push_console_line(
            "^3Prediction is disabled until a local BSP is loaded; player/entity presentation remains active."
                .to_owned(),
        );
        self.prime_live_session(map_name);
        self.publish_snapshot();
        self.publish_ui();
    }

    /// CL_ParseGamestate -> CL_InitCGame: a new CGame for the new gamestate.
    pub(in crate::app) fn start_live_gamestate(&mut self) {
        self.send_userinfo();
        // CL_SystemInfoChanged: the gamestate systeminfo is authoritative for
        // fs_game. Inspect it before accepting any overlapped presenter/map prep
        // that may have been built speculatively from infoResponse.
        let (
            map_name,
            configstrings,
            command_sequence,
            client_num,
            pure,
            server_name,
            fs_game,
            checksum_feed,
        ) = {
            let Some(net) = self.net.as_ref() else { return };
            let decoder = net.session().decoder();
            let Some(map_name) = decoder.map_name() else {
                self.push_console_line("^1Gamestate has no mapname.");
                return;
            };
            let fs_game = decoder
                .configstrings
                .get(&jka_protocol::session::CS_SYSTEMINFO)
                .and_then(|info| jka_protocol::commands::info_value(info, b"fs_game"))
                .unwrap_or_default()
                .to_vec();
            (
                map_name,
                decoder.configstrings.clone(),
                decoder.server_command_sequence,
                decoder.client_number,
                net.session().sv_pure(),
                net.server_name.clone(),
                fs_game,
                decoder.checksum_feed,
            )
        };
        match self.set_session_fs_game(&fs_game, "gamestate systeminfo") {
            Ok(true) => self.start_live_cgame_prep(),
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1Rejected server fs_game: {error}"));
                self.disconnect_to_main_menu();
                return;
            }
        }
        self.update_chat_log_live_metadata(&map_name);

        let source = scene::MapSource::Bsp(map_name.clone());
        if let Err(error) =
            scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source)
        {
            if self.live_auto_download_requested {
                self.begin_live_autodownload(&map_name, &configstrings);
                return;
            }
            if !self.live_missing_map_authorized {
                self.show_missing_map_prompt(&map_name, error);
                return;
            }
        } else {
            self.live_auto_download_requested = false;
        }

        self.poll_live_cgame_prep();
        if self.live_cgame_prepared.is_none() && self.live_cgame_prep_rx.is_some() {
            if !self.pending_live_gamestate {
                devprintln!(
                    1,
                    "[JOIN] gamestate arrived before CGame prep; waiting for worker"
                );
                if crate::logging::developer_enabled(1) {
                    self.push_console_line(
                        "^5[JOIN]^7 gamestate received; finishing overlapped CGame prep".to_owned(),
                    );
                }
            }
            self.pending_live_gamestate = true;
            return;
        }
        self.pending_live_gamestate = false;

        if pure {
            // TaystJK CL_SendPureChecksums: sent on every pure server, whatever the
            // fs_game or gamename. Without a `cp` the server silently ignores all
            // usercmds and the client hangs awaiting a snapshot.
            match crate::pure::build_pure_command(
                &self.base,
                self.game.as_deref(),
                checksum_feed,
                &map_name,
            ) {
                Ok(response) => {
                    let send_result = self
                        .net
                        .as_mut()
                        .ok_or_else(|| "network session disappeared".to_owned())
                        .and_then(|net| net.send_reliable_now(&response.command));
                    match send_result {
                        Ok(()) => self.push_console_line(format!(
                            "^2sv_pure:^7 TaystJK pure reply sent ({} general PK3s; {}).",
                            response.reported_paks, response.detail
                        )),
                        Err(error) => self.push_console_line(format!(
                            "^1sv_pure: failed to send pure checksum response: {error}"
                        )),
                    }
                }
                Err(error) => self
                    .push_console_line(format!("^1sv_pure: stock PK3 reply unavailable: {error}")),
            }
        }

        let mut session = if let Some(prepared) = self.live_cgame_prepared.take() {
            let mut session = GameSession::new(
                format!("live:{server_name}"),
                server_name.clone(),
                Vec::new(),
                prepared.siege_classes,
                prepared.player_presenter,
                prepared.entity_presenter,
                crate::cgame::weapon_fx::WeaponFx::new(prepared.fx_assets),
                self.forced_player_models.clone(),
            );
            session
                .weapon_fx
                .set_modern_sabers(self.video.modern_sabers);
            session
                .weapon_fx
                .set_saber_impact_fx(self.video.saber_impact_fx);
            session.weapon_fx.set_saber_marks(self.video.saber_marks);
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
                .player_presenter
                .set_saber_team_colors(self.saber_team_colors);
            session
                .player_presenter
                .set_saber_staff_multi_color(self.saber_staff_multi_color);
            session.weapon_fx.set_saber_trail(self.saber_trail);
            session.weapon_fx.set_continuous_fx_fps(self.video.fx_fps);
            session.weapon_fx.set_fx_fps_scope(self.video.fx_fps_scope);
            session.weapon_fx.set_fx_physics(self.video.fx_physics);
            session.weapon_fx.set_fx_lod(
                self.video.fx_lod,
                self.video.fx_count_scale,
                self.video.fx_lod_scale,
            );
            session.apply_client_options(
                self.screen_shake,
                self.audio.game,
                self.video.footprints,
                self.japro_cg,
                self.network.plugin_disable,
            );
            self.attach_live_sound_presenter(&mut session);
            session
        } else {
            match self.build_game_session(
                format!("live:{server_name}"),
                server_name.clone(),
                Vec::new(),
            ) {
                Ok(session) => session,
                Err(error) => {
                    self.push_console_line(format!("^1{error}"));
                    self.disconnect_to_main_menu();
                    return;
                }
            }
        };
        session.live = true;
        session.local = false;
        session.map_name = Some(map_name.clone());
        if let Err(error) = session
            .player_presenter
            .set_physics_map_mesh(&self.map_physics_collision)
        {
            eprintln!("RAPIER MAP COLLISION ERROR: {error}");
        }
        session
            .client_game
            .reset_gamestate(&configstrings, command_sequence);
        while let Some(command) = self.pending_live_server_commands.pop_front() {
            session.client_game.queue_server_command(command);
        }
        self.game_session = Some(session);
        self.predictor.reset();
        self.local_server = None;
        println!(
            "NET: gamestate {map_name} clientNum={client_num} configstrings={}",
            configstrings.len()
        );
        self.push_console_line(format!("^2Gamestate: {map_name} (client {client_num})"));
        self.try_japro_autologin();

        if self.loaded_map_matches(&map_name) {
            if self.front_end {
                self.pending_frontend_map_launch = false;
                self.front_end = false;
                self.frontend_page = FrontendPage::Main;
                self.menu_selected = 0;
                self.sync_render_fps_cap();
            }
            devprintln!(
                1,
                "[JOIN] adopting already-rendered prefetched map {map_name}"
            );
            self.live_join_ui = None;
            self.egui_repaint_requested = true;
            self.prime_live_session(&map_name);
            self.publish_snapshot();
            self.publish_ui();
            return;
        }

        if self.loading_map_matches(&map_name) {
            self.live_without_world = false;
            // If MapPrepared has not fired yet this follows the normal launch
            // path. If it already fired, WorldUploaded below performs the same
            // front-end transition before priming.
            if self.front_end {
                self.pending_frontend_map_launch = true;
            }
            devprintln!(1, "[JOIN] reusing in-flight map prefetch for {map_name}");
            if crate::logging::developer_enabled(1) {
                self.push_console_line(format!("^5[JOIN]^7 reusing in-flight {map_name} prefetch"));
            }
            return;
        }

        let source = scene::MapSource::Bsp(map_name.clone());
        match scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source) {
            Ok(()) => {
                self.live_without_world = false;
                self.request_map(source);
            }
            Err(error) if self.live_missing_map_authorized => {
                self.prime_live_session_without_world(&map_name, &error);
            }
            Err(error) => {
                self.show_missing_map_prompt(&map_name, error);
            }
        }
    }

    /// CL_DownloadsComplete after the map is up: CA_PRIMED, usercmds start.
    /// Local `/map`/`/devmap` uses the same state transition, but its snapshot
    /// source is the in-process authority rather than netchan.
    pub(in crate::app) fn prime_live_session(&mut self, loaded_map: &str) {
        let loaded_qpath = loaded_map
            .strip_suffix(".bsp")
            .or_else(|| loaded_map.strip_suffix(".map"))
            .unwrap_or(loaded_map);
        let is_local = {
            let Some(session) = self.game_session.as_mut().filter(|session| session.live) else {
                return;
            };
            if session.phase != SessionPhase::WaitingForMap
                || !session
                    .map_name
                    .as_deref()
                    .is_some_and(|map| map.eq_ignore_ascii_case(loaded_qpath))
            {
                return;
            }
            session.phase = SessionPhase::WaitingForSnapshot;
            session.local
        };

        if is_local {
            // The local server already queued an active snapshot during map prep.
            // There is no network pump to trigger CL_FirstSnapshot, so enter it
            // directly once the render world is ready.
            self.begin_live_session();
            if self
                .game_session
                .as_ref()
                .is_some_and(|session| session.local && session.phase == SessionPhase::Playing)
            {
                self.set_overlay(OverlayMode::None);
            }
        } else {
            self.local_server = None;
            if let Some(net) = self.net.as_mut() {
                let realtime = net.realtime();
                net.session_mut().set_primed(realtime);
                net.flush();
            }
            self.set_overlay(OverlayMode::None);
        }
    }

    pub(in crate::app) fn begin_live_session(&mut self) {
        let is_local = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local);
        let result = match self.game_session.as_mut() {
            Some(session) => session.begin_live(),
            None => return,
        };
        match result {
            Ok(true) => {
                self.previous_tick = Instant::now();
                if is_local {
                    devprintln!(
                        1,
                        "LOCAL SERVER: first active snapshot; shared CGame running"
                    );
                    self.push_console_line("^2Entered local game.");
                } else {
                    devprintln!(1, "NET: first active snapshot; CGame running");
                    self.push_console_line("^2Entered the game.");
                }
                let summary = if !is_local {
                    self.live_join_timing.as_mut().map(|timing| {
                        let active_ms = timing.elapsed_ms();
                        format!(
                            "[JOIN] active +{active_ms:.1} ms | info {} | cgame {} | gamestate {} | first-world {}",
                            timing.server_info_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                            timing.cgame_ready_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                            timing.gamestate_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                            timing.first_world_frame_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                        )
                    })
                } else {
                    None
                };
                if let Some(summary) = summary {
                    devprintln!(1, "{summary}");
                    if crate::logging::developer_enabled(1) {
                        self.push_console_line(format!("^5{}", summary));
                    }
                }
            }
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                self.disconnect_to_main_menu();
            }
        }
    }
}
