//! Connection prepare.
use crate::app::{
    mpsc, scene, thread, App, Instant, LiveCgamePrepared, LiveJoinTiming, LiveJoinUiPhase,
    LiveJoinUiState, MissingMapPrompt,
};

impl App {
    pub(in crate::app) fn start_live_cgame_prep(&mut self) {
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        self.pending_live_gamestate = false;

        let base = self.base.clone();
        let game = self.game.clone();
        let pbr = self.video.pbr;
        let allow_asset_overrides = self.video.allow_asset_overrides;
        let load_stringed = self.stringed.is_none();
        let (tx, rx) = mpsc::channel();
        let spawn = thread::Builder::new()
            .name("jka-live-cgame-prep".into())
            .spawn(move || {
                let started = Instant::now();
                let result =
                    App::build_cgame_cpu_assets(&base, game.as_deref(), pbr, allow_asset_overrides)
                        .map(
                            |(siege_classes, player_presenter, entity_presenter, fx_assets)| {
                                let stringed = if load_stringed {
                                    jka_assets::pk3::AssetSearchPath::open_game(
                                        &base,
                                        game.as_deref(),
                                    )
                                    .ok()
                                    .map(|mut assets| {
                                        crate::cgame::stringed::StringEd::load(&mut assets)
                                    })
                                } else {
                                    None
                                };
                                LiveCgamePrepared {
                                    siege_classes,
                                    player_presenter,
                                    entity_presenter,
                                    fx_assets,
                                    stringed,
                                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                                }
                            },
                        );
                let _ = tx.send(result);
            });
        match spawn {
            Ok(_) => self.live_cgame_prep_rx = Some(rx),
            Err(error) => {
                eprintln!("CGAME PREP: could not start worker: {error}");
                self.push_console_line(format!(
                    "^3CGAME PREP:^7 worker unavailable ({error}); will build at gamestate"
                ));
            }
        }
    }

    pub(in crate::app) fn poll_live_cgame_prep(&mut self) {
        let result = match self.live_cgame_prep_rx.as_ref() {
            Some(rx) => rx.try_recv(),
            None => return,
        };
        match result {
            Ok(Ok(mut prepared)) => {
                self.live_cgame_prep_rx = None;
                if self.stringed.is_none() {
                    if let Some(table) = prepared.stringed.take() {
                        devprintln!(
                            1,
                            "STRINGED: {} strings preloaded on CGame worker",
                            table.len()
                        );
                        self.stringed = Some(table);
                    }
                }
                if let Some(timing) = self.live_join_timing.as_mut() {
                    let elapsed = timing.elapsed_ms();
                    timing.cgame_ready_ms = Some(elapsed);
                }
                devprintln!(
                    1,
                    "[JOIN] CGame CPU prep ready in {:.1} ms",
                    prepared.elapsed_ms
                );
                self.live_cgame_prepared = Some(prepared);
            }
            Ok(Err(error)) => {
                self.live_cgame_prep_rx = None;
                eprintln!("CGAME PREP: {error}");
                self.push_console_line(format!(
                    "^3CGAME PREP:^7 {error}; falling back to synchronous init"
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.live_cgame_prep_rx = None;
                self.push_console_line(
                    "^3CGAME PREP:^7 worker ended without a result; falling back to synchronous init"
                        .to_owned(),
                );
            }
        }
    }

    pub(in crate::app) fn normalized_bsp_name(name: &str) -> String {
        let normalized = name.trim().replace('\\', "/");
        let normalized = normalized.strip_prefix("maps/").unwrap_or(&normalized);
        let normalized = normalized
            .strip_suffix(".bsp")
            .or_else(|| normalized.strip_suffix(".map"))
            .unwrap_or(normalized);
        normalized.to_ascii_lowercase()
    }

    pub(in crate::app) fn loading_map_matches(&self, map_name: &str) -> bool {
        let wanted = Self::normalized_bsp_name(map_name);
        self.loading
            .as_ref()
            .is_some_and(|loading| Self::normalized_bsp_name(&loading.name) == wanted)
    }

    pub(in crate::app) fn loaded_map_matches(&self, map_name: &str) -> bool {
        !self.live_without_world
            && self.loading.is_none()
            && Self::normalized_bsp_name(&self.map_name) == Self::normalized_bsp_name(map_name)
    }

    pub(in crate::app) fn handle_live_server_info(&mut self, info: jka_protocol::ServerInfo) {
        let elapsed_ms = self
            .live_join_timing
            .as_ref()
            .map(LiveJoinTiming::elapsed_ms);
        if let Some(timing) = self.live_join_timing.as_mut() {
            timing.server_info_ms = elapsed_ms;
        }

        if let Some(server) = self.net.as_ref().map(|net| net.session().server()) {
            self.live_http_base = crate::download::advertised_http_base(&info, server);
            if let Some(base) = self.live_http_base.as_deref() {
                devprintln!(1, "[DOWNLOAD] infoResponse HTTP endpoint: {base}");
            }
        }

        // TaystJK checks `needpass` before joining from the browser and opens
        // password_request instead of knowingly sending an empty password. Our
        // speculative getinfo preflight lets direct `/connect` do the same thing
        // before its gameplay connect packet is sent.
        let needs_password = info
            .get(b"needpass")
            .is_some_and(|value| !value.is_empty() && value != b"0");
        if needs_password && self.network.password.is_empty() {
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
                Some("This server requires a password.".to_owned()),
            );
            return;
        }

        let Some(map_bytes) = info.get(b"mapname") else {
            if let Some(net) = self.net.as_mut() {
                net.resume_connect();
            }
            return;
        };
        let map_name = String::from_utf8_lossy(map_bytes).into_owned();
        devprintln!(
            1,
            "[JOIN] infoResponse protocol={} map={} at +{:.1} ms",
            info.protocol,
            map_name,
            elapsed_ms.unwrap_or(0.0)
        );
        if !info.is_protocol_26() {
            if let Some(net) = self.net.as_mut() {
                net.resume_connect();
            }
            return;
        }

        let advertised_game = info.get(b"game").unwrap_or_default();
        match self.set_session_fs_game(advertised_game, "server info") {
            Ok(true) => self.start_live_cgame_prep(),
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1Rejected server fs_game: {error}"));
                self.disconnect_to_main_menu();
                return;
            }
        }

        if self.loaded_map_matches(&map_name) || self.loading_map_matches(&map_name) {
            devprintln!(
                1,
                "[JOIN] map {map_name} already loaded/in flight; no prefetch restart"
            );
            if let Some(net) = self.net.as_mut() {
                net.resume_connect();
            }
            return;
        }

        let source = scene::MapSource::Bsp(map_name.clone());
        match scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source) {
            Ok(()) => {
                if let Some(timing) = self.live_join_timing.as_mut() {
                    timing.prefetched_map = Some(map_name.clone());
                }
                if crate::logging::developer_enabled(1) {
                    self.push_console_line(format!(
                        "^5[JOIN]^7 prefetching {map_name} during handshake"
                    ));
                }
                self.prefetch_live_map(source);
                if let Some(net) = self.net.as_mut() {
                    net.resume_connect();
                }
            }
            Err(error) => {
                devprintln!(1, "[JOIN] map preflight blocked: {error}");
                self.console_status = format!("MAP NOT FOUND: {map_name}");
                self.push_console_line(format!("^3[JOIN]^7 map not found locally: {map_name}"));
                self.missing_map_prompt = Some(MissingMapPrompt {
                    map_name,
                    reason: error,
                });
                self.egui_repaint_requested = true;
                self.publish_ui();
                // Intentionally leave ClientSession's preflight pause engaged.
                // The dialog explicitly chooses abort, connect-anyway, or
                // autodownload before the gameplay connect packet is sent.
            }
        }
    }

    pub(in crate::app) fn choose_missing_map_abort(&mut self) {
        self.missing_map_prompt = None;
        self.live_auto_download_requested = false;
        self.live_missing_map_authorized = false;
        self.disconnect_to_main_menu();
        self.console_status = "CONNECTION ABORTED - MAP NOT INSTALLED".into();
    }

    pub(in crate::app) fn choose_missing_map_connect_anyway(&mut self) {
        let map = self.missing_map_prompt.take().map(|prompt| prompt.map_name);
        self.live_missing_map_authorized = true;
        self.live_auto_download_requested = false;
        self.console_status = map.as_deref().map_or_else(
            || "CONNECTING WITHOUT LOCAL MAP...".into(),
            |map| format!("CONNECTING WITHOUT {map}..."),
        );
        self.continue_live_after_missing_map_choice();
    }

    pub(in crate::app) fn choose_missing_map_autodownload(&mut self) {
        if !self.network.allow_http_downloads && !self.network.allow_legacy_downloads {
            self.console_status = "AUTODOWNLOAD IS DISABLED IN SETUP > NETWORK".into();
            self.push_console_line(
                "^3Enable HTTP downloads and/or Legacy server downloads in Setup > Network."
                    .to_owned(),
            );
            self.egui_repaint_requested = true;
            return;
        }
        let map_name = self
            .missing_map_prompt
            .as_ref()
            .map(|prompt| prompt.map_name.clone())
            .unwrap_or_else(|| "server map".to_owned());
        self.live_join_ui = Some(LiveJoinUiState {
            map_name,
            phase: LiveJoinUiPhase::CheckingContent,
            detail: "Requesting the server package list...".into(),
        });
        self.missing_map_prompt = None;
        self.live_missing_map_authorized = false;
        self.live_auto_download_requested = true;
        self.console_status = "CONNECTING TO GET SERVER DOWNLOAD LIST...".into();
        self.continue_live_after_missing_map_choice();
    }

    pub(in crate::app) fn continue_live_after_missing_map_choice(&mut self) {
        let state = self.net.as_ref().map(crate::net::NetClient::state);
        if state.is_some_and(|state| state < jka_protocol::session::ConnectionState::Connected) {
            if let Some(net) = self.net.as_mut() {
                net.resume_connect();
            }
            return;
        }
        if state.is_some() {
            // Fallback path for servers that did not answer the speculative
            // infoResponse: the gamestate itself discovered the missing map.
            self.start_live_gamestate();
        }
    }

    pub(in crate::app) fn show_missing_map_prompt(
        &mut self,
        map_name: &str,
        reason: impl Into<String>,
    ) {
        self.missing_map_prompt = Some(MissingMapPrompt {
            map_name: map_name.to_owned(),
            reason: reason.into(),
        });
        self.console_status = format!("MAP NOT FOUND: {map_name}");
        self.egui_repaint_requested = true;
        self.publish_ui();
    }
}
