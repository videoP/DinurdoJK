//! Capture.
use crate::app::{playerstate_vec3, scene, App, Arc, FrontendPage, PathBuf, RenderCommand};

impl App {
    pub(in crate::app) fn build_screenshot_metadata(
        &self,
    ) -> crate::screenshot::ScreenshotMetadata {
        let game = self
            .game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        let camera_origin =
            (!self.front_end).then(|| scene::jka_position(self.camera.position.to_array()));
        let view_angles = (!self.front_end).then(|| {
            [
                -self.camera.pitch.to_degrees(),
                self.camera.yaw.to_degrees().rem_euclid(360.0),
                0.0,
            ]
        });
        let player_origin = self
            .live_player_state()
            .and_then(|ps| playerstate_vec3(ps, "origin"));

        let mut players = Vec::new();
        if let Some(session) = self.game_session.as_ref() {
            for client_num in 0..32usize {
                let Some(info) = session
                    .client_game
                    .client_info(client_num, &session.siege_classes)
                else {
                    continue;
                };
                if info.name.is_empty() {
                    continue;
                }
                let score = self.scoreboard.as_ref().and_then(|scoreboard| {
                    scoreboard
                        .entries
                        .iter()
                        .find(|entry| entry.client == client_num as i32)
                });
                players.push(crate::screenshot::ScreenshotPlayer {
                    client_num: client_num as i32,
                    name: info.name,
                    team: info.team,
                    score: score.map(|entry| entry.score),
                    ping: score.map(|entry| entry.ping),
                    time_minutes: score.map(|entry| entry.time),
                });
            }
        }

        let server_time_ms = self
            .game_session
            .as_ref()
            .and_then(|session| {
                session
                    .current_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.server_time)
            })
            .or_else(|| self.net.as_ref().map(|net| net.session().server_time()));
        let map_time_ms = self.game_session.as_ref().and_then(|session| {
            let server_time = server_time_ms?;
            Some(server_time.saturating_sub(session.client_game.match_limits().level_start_time))
        });

        crate::screenshot::ScreenshotMetadata {
            version: crate::screenshot::METADATA_VERSION,
            captured_at: crate::screenshot::capture_timestamp_local(),
            captured_unix_ms: crate::screenshot::unix_time_ms(),
            game,
            map_name: (!self.front_end && !self.map_name.is_empty()).then(|| {
                self.map_name
                    .strip_suffix(".bsp")
                    .or_else(|| self.map_name.strip_suffix(".map"))
                    .unwrap_or(&self.map_name)
                    .replace('\\', "/")
            }),
            server_name: self.current_server_hostname(),
            server_address: self
                .net
                .as_ref()
                .map(|net| net.session().server().to_string()),
            player_name: self.network.name.clone(),
            players,
            camera_origin,
            player_origin,
            view_angles,
            map_time_ms,
            server_time_ms,
            fov: self.camera.cg_fov(),
            third_person: self.third_person.enabled,
            crosshair: None,
        }
    }

    pub(in crate::app) fn request_screenshot(&mut self) {
        self.render_command(RenderCommand::Screenshot {
            directory: self.game_write_path("screenshots"),
            metadata: self.build_screenshot_metadata(),
        });
    }

    /// Configure an image path supplied on the process command line (for
    /// example by dragging a screenshot onto DinurdoJK.exe). Metadata-bearing
    /// screenshots launch directly into their saved viewpoint; ordinary images
    /// simply open selected in the browser.
    pub fn configure_startup_screenshot(
        &mut self,
        path: PathBuf,
        metadata: Option<crate::screenshot::ScreenshotMetadata>,
    ) {
        if let Some(metadata) = metadata.filter(|metadata| metadata.can_go_to_spot()) {
            self.pending_screenshot_jump = Some(metadata);
        } else {
            self.frontend_page = FrontendPage::Screenshots;
            self.startup_screenshot_select = Some(path);
            self.screenshot_catalog_loaded = false;
        }
    }

    pub(in crate::app) fn launch_screenshot_spot(
        &mut self,
        metadata: crate::screenshot::ScreenshotMetadata,
    ) {
        let Some(map_name) = metadata
            .map_name
            .as_deref()
            .map(str::trim)
            .filter(|map| !map.is_empty())
            .map(str::to_owned)
        else {
            self.console_status = "SCREENSHOT HAS NO MAP NAME".into();
            return;
        };
        let Some(_origin) = metadata.player_origin.or(metadata.camera_origin) else {
            self.console_status = "SCREENSHOT HAS NO SAVED POSITION".into();
            return;
        };
        if metadata.view_angles.is_none() {
            self.console_status = "SCREENSHOT HAS NO SAVED VIEW ANGLES".into();
            return;
        }

        let fs_game =
            if metadata.game.trim().is_empty() || metadata.game.eq_ignore_ascii_case("base") {
                Vec::new()
            } else {
                metadata.game.as_bytes().to_vec()
            };
        if let Err(error) = self.set_session_fs_game(&fs_game, "screenshot spot") {
            self.console_status = format!("SCREENSHOT MOD UNAVAILABLE: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
            return;
        }

        let source = scene::MapSource::Bsp(map_name.clone());
        if let Err(error) =
            scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source)
        {
            self.console_status = format!("SCREENSHOT MAP NOT FOUND: {map_name}");
            self.push_console_line(format!("^3{}:^7 {error}", self.console_status));
            self.publish_ui();
            return;
        }

        // Screenshot Browser is frontend-only, but explicitly clear any stale
        // authority so this always becomes a local solo world.
        self.net = None;
        self.game_session = None;
        self.local_server = None;
        self.predictor.reset();
        self.live_without_world = false;
        self.demo_without_world = false;
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.pending_screenshot_jump = Some(metadata);
        self.request_map(source);
        self.console_status = format!("LOADING SCREENSHOT SPOT: {map_name}");
        self.publish_ui();
    }

    /// Apply a pending screenshot viewpoint after the local server exists.
    /// Local solo starts as a free spectator, so saved positions are restored
    /// without requiring a valid grounded spawn. Joined players use noclip.
    pub(in crate::app) fn apply_pending_screenshot_jump(&mut self, loaded_name: &str) {
        let Some(metadata) = self.pending_screenshot_jump.take() else {
            return;
        };
        let Some(saved_map) = metadata.map_name.as_deref() else {
            return;
        };
        if Self::normalized_bsp_name(saved_map) != Self::normalized_bsp_name(loaded_name) {
            self.push_console_line(format!(
                "^3SCREENSHOT SPOT:^7 saved map {} does not match loaded {}",
                saved_map, loaded_name
            ));
            return;
        }
        let Some(origin) = metadata.player_origin.or(metadata.camera_origin) else {
            return;
        };
        let Some(angles) = metadata.view_angles else {
            return;
        };

        let mut refreshed_snapshot = None;
        let result = if let Some(server) = self.local_server.as_mut() {
            // Local solo starts as a free spectator. `teleport` intentionally
            // supports both spectator and joined-player states; noclip is only
            // relevant once the user has joined the local game.
            let _ = server.set_noclip(true);
            match server.teleport(origin, angles) {
                Ok(()) => {
                    server.camera(&mut self.camera);
                    if metadata.fov.is_finite() {
                        self.camera.set_cg_fov(metadata.fov.clamp(1.0, 179.0));
                    }
                    refreshed_snapshot = server.take_snapshot(true);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else {
            Err("local server was not created for the screenshot map".into())
        };
        if let Some(snapshot) = refreshed_snapshot {
            if let Some(session) = self.game_session.as_mut() {
                session.snapshots = session.snapshots.saturating_add(1);
                session.live_snapshots.push_back(snapshot);
            }
        }

        match result {
            Ok(()) => {
                self.third_person_camera.reset();
                self.console_status = format!(
                    "SCREENSHOT SPOT: {} @ {:.1} {:.1} {:.1} (FREE CAMERA)",
                    saved_map, origin[0], origin[1], origin[2]
                );
                self.push_console_line(format!("^2{}", self.console_status));
            }
            Err(error) => {
                self.console_status = format!("SCREENSHOT SPOT FAILED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
    }

    pub(in crate::app) fn request_clipboard_capture(&mut self) {
        self.render_command(RenderCommand::CopyFrameToClipboard);
    }
}
