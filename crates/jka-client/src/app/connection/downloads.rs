//! Connection downloads.
use crate::app::{
    mpsc, scene, App, BTreeMap, File, LiveDownloadState, LiveDownloadTransfer, LiveJoinUiPhase,
    PathBuf,
};
use std::io::Write;

impl App {
    /// CL_InitDownloads / FS_ComparePaks. The gamestate supplies the complete
    /// referenced-PK3 list, so this starts only after the pre-connect dialog's
    /// autodownload choice has allowed the gameplay handshake to reach CA_CONNECTED.
    pub(in crate::app) fn begin_live_autodownload(
        &mut self,
        map_name: &str,
        configstrings: &BTreeMap<u16, Vec<u8>>,
    ) {
        if self.live_download.is_some() {
            return;
        }
        let systeminfo = configstrings
            .get(&jka_protocol::session::CS_SYSTEMINFO)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let serverinfo = configstrings.get(&0).map(Vec::as_slice).unwrap_or_default();
        let files = match crate::download::referenced_downloads(
            &self.base,
            self.game.as_deref(),
            systeminfo,
        ) {
            Ok(files) => files,
            Err(error) => {
                self.fail_live_autodownload(map_name, error);
                return;
            }
        };
        if files.is_empty() {
            self.fail_live_autodownload(
                map_name,
                "The server did not advertise any missing referenced PK3s for this map.".to_owned(),
            );
            return;
        }

        let server_allows_legacy =
            jka_protocol::commands::info_value(serverinfo, b"sv_allowDownload")
                .is_none_or(|value| jka_protocol::commands::atoi(value) != 0);
        let server_allows_http =
            jka_protocol::commands::info_value(serverinfo, b"sv_httpDownloads")
                .is_none_or(|value| jka_protocol::commands::atoi(value) != 0);
        let http_base = if self.network.allow_http_downloads && server_allows_http {
            self.live_http_base.clone()
        } else {
            None
        };
        let can_legacy = self.network.allow_legacy_downloads && server_allows_legacy;
        devprintln!(
            1,
            "[DOWNLOAD] transports: client http={} legacy={}, server http={} legacy={}, endpoint={}",
            self.network.allow_http_downloads,
            self.network.allow_legacy_downloads,
            server_allows_http,
            server_allows_legacy,
            http_base.as_deref().unwrap_or("none")
        );
        if http_base.is_none() && !can_legacy {
            self.fail_live_autodownload(
                map_name,
                "No allowed download transport is available (HTTP not advertised/allowed and legacy downloads unavailable).".to_owned(),
            );
            return;
        }

        self.push_console_line(format!(
            "^5DOWNLOAD:^7 {} missing referenced PK3{} for {map_name}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ));
        self.live_download = Some(LiveDownloadState {
            map_name: map_name.to_owned(),
            files,
            index: 0,
            http_base,
            server_allows_legacy,
            transfer: LiveDownloadTransfer::Idle,
            received: 0,
            total: None,
            status: "Preparing download...".into(),
        });
        self.egui_repaint_requested = true;
        self.start_live_download_current();
    }

    pub(in crate::app) fn start_live_download_current(&mut self) {
        let complete = self
            .live_download
            .as_ref()
            .is_some_and(|state| state.index >= state.files.len());
        if complete {
            self.finish_live_autodownload();
            return;
        }

        let (spec, http_base, can_legacy) = {
            let Some(state) = self.live_download.as_ref() else {
                return;
            };
            let Some(spec) = state.current().cloned() else {
                return;
            };
            (
                spec,
                state
                    .http_base
                    .clone()
                    .filter(|_| self.network.allow_http_downloads),
                state.server_allows_legacy && self.network.allow_legacy_downloads,
            )
        };

        if let Some(http_base) = http_base {
            let (tx, rx) = mpsc::channel();
            crate::download::spawn_http_download(http_base, spec.clone(), tx);
            if let Some(state) = self.live_download.as_mut() {
                state.received = 0;
                state.total = None;
                state.status = format!("HTTP: {}", spec.remote_name);
                state.transfer = LiveDownloadTransfer::Http { rx };
            }
            self.console_status = format!(
                "DOWNLOADING {} (HTTP)",
                spec.remote_name.to_ascii_uppercase()
            );
            self.push_console_line(format!("^5HTTP DOWNLOAD:^7 {}", spec.remote_name));
            self.egui_repaint_requested = true;
            return;
        }

        if can_legacy {
            self.start_live_legacy_download(spec);
            return;
        }

        let map_name = self
            .live_download
            .as_ref()
            .map(|state| state.map_name.clone())
            .unwrap_or_default();
        self.fail_live_autodownload(
            &map_name,
            "No enabled download transport is available.".to_owned(),
        );
    }

    pub(in crate::app) fn start_live_legacy_download(
        &mut self,
        spec: crate::download::DownloadSpec,
    ) {
        if let Some(parent) = spec.local_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                let map = self
                    .live_download
                    .as_ref()
                    .map(|state| state.map_name.clone())
                    .unwrap_or_default();
                self.fail_live_autodownload(
                    &map,
                    format!("Could not create {}: {error}", parent.display()),
                );
                return;
            }
        }
        let temp_path = crate::download::temp_path_for(&spec.local_path);
        let file = match File::create(&temp_path) {
            Ok(file) => file,
            Err(error) => {
                let map = self
                    .live_download
                    .as_ref()
                    .map(|state| state.map_name.clone())
                    .unwrap_or_default();
                self.fail_live_autodownload(
                    &map,
                    format!("Could not create {}: {error}", temp_path.display()),
                );
                return;
            }
        };
        if let Some(state) = self.live_download.as_mut() {
            state.received = 0;
            state.total = None;
            state.status = format!("Legacy JKA: {}", spec.remote_name);
            state.transfer = LiveDownloadTransfer::Legacy {
                file,
                temp_path,
                expected_block: 0,
                received: 0,
                total: None,
            };
        }
        let command = format!("download {}", spec.remote_name);
        let send_result = self
            .net
            .as_mut()
            .map_or(Ok(()), |net| net.send_reliable_now(command.as_bytes()));
        if let Err(error) = send_result {
            let map = self
                .live_download
                .as_ref()
                .map(|state| state.map_name.clone())
                .unwrap_or_default();
            self.fail_live_autodownload(&map, error);
            return;
        }
        self.console_status = format!(
            "DOWNLOADING {} (LEGACY)",
            spec.remote_name.to_ascii_uppercase()
        );
        self.push_console_line(format!("^5LEGACY DOWNLOAD:^7 {}", spec.remote_name));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn poll_live_download(&mut self) {
        let events = match self.live_download.as_ref().map(|state| &state.transfer) {
            Some(LiveDownloadTransfer::Http { rx }) => rx.try_iter().collect::<Vec<_>>(),
            _ => return,
        };
        for event in events {
            match event {
                crate::download::HttpEvent::Started { total } => {
                    if let Some(state) = self.live_download.as_mut() {
                        state.total = total;
                    }
                }
                crate::download::HttpEvent::Progress { received, total } => {
                    if let Some(state) = self.live_download.as_mut() {
                        state.received = received;
                        state.total = total.or(state.total);
                    }
                    self.egui_repaint_requested = true;
                }
                crate::download::HttpEvent::Finished(Ok(())) => {
                    if let Some(state) = self.live_download.as_mut() {
                        state.index += 1;
                        state.transfer = LiveDownloadTransfer::Idle;
                        state.received = 0;
                        state.total = None;
                    }
                    self.start_live_download_current();
                }
                crate::download::HttpEvent::Finished(Err(error)) => {
                    let fallback = self.live_download.as_ref().is_some_and(|state| {
                        state.server_allows_legacy && self.network.allow_legacy_downloads
                    });
                    if fallback {
                        let spec = self
                            .live_download
                            .as_ref()
                            .and_then(LiveDownloadState::current)
                            .cloned();
                        if let Some(state) = self.live_download.as_mut() {
                            state.transfer = LiveDownloadTransfer::Idle;
                            state.status = format!("HTTP failed; falling back to legacy: {error}");
                        }
                        self.push_console_line(format!(
                            "^3HTTP download failed:^7 {error}; trying legacy JKA download."
                        ));
                        if let Some(spec) = spec {
                            self.start_live_legacy_download(spec);
                        }
                    } else {
                        let map = self
                            .live_download
                            .as_ref()
                            .map(|state| state.map_name.clone())
                            .unwrap_or_default();
                        self.fail_live_autodownload(&map, error);
                    }
                }
            }
        }
    }

    pub(in crate::app) fn handle_live_download_block(
        &mut self,
        block: jka_protocol::server::DownloadBlock,
    ) {
        if !matches!(
            self.live_download.as_ref().map(|state| &state.transfer),
            Some(LiveDownloadTransfer::Legacy { .. })
        ) {
            if let Some(net) = self.net.as_mut() {
                let _ = net.send_reliable_now(b"stopdl");
            }
            return;
        }
        if let Some(error) = block.error.as_ref() {
            let message = String::from_utf8_lossy(error).into_owned();
            let map = self
                .live_download
                .as_ref()
                .map(|state| state.map_name.clone())
                .unwrap_or_default();
            self.fail_live_autodownload(&map, format!("Server refused download: {message}"));
            return;
        }

        let mut ack = None;
        let mut completed = None::<(PathBuf, PathBuf, i32)>;
        let mut failure = None;
        if let Some(state) = self.live_download.as_mut() {
            let completion = state
                .current()
                .map(|spec| (spec.local_path.clone(), spec.checksum));
            if let LiveDownloadTransfer::Legacy {
                file,
                temp_path,
                expected_block,
                received,
                total,
            } = &mut state.transfer
            {
                if block.block < *expected_block {
                    ack = Some(block.block);
                } else if block.block > *expected_block {
                    return;
                } else {
                    if let Some(size) = block.file_size {
                        if size < 0 {
                            failure = Some("Server reported a negative download size.".to_owned());
                        } else if size as u64 > 2 * 1024 * 1024 * 1024 {
                            failure = Some(
                                "Server package exceeds the 2 GiB download safety limit."
                                    .to_owned(),
                            );
                        } else {
                            *total = Some(size as u64);
                            state.total = *total;
                        }
                    }
                    if failure.is_none() {
                        if let Err(error) = file.write_all(&block.data) {
                            failure = Some(format!("Writing download failed: {error}"));
                        } else {
                            *received += block.data.len() as u64;
                            state.received = *received;
                            ack = Some(block.block);
                            *expected_block = (*expected_block).wrapping_add(1);
                            if block.data.is_empty() {
                                if let Some((final_path, checksum)) = completion {
                                    completed = Some((temp_path.clone(), final_path, checksum));
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(error) = failure {
            let map = self
                .live_download
                .as_ref()
                .map(|state| state.map_name.clone())
                .unwrap_or_default();
            self.fail_live_autodownload(&map, error);
            return;
        }
        if let Some(block) = ack {
            let command = format!("nextdl {block}");
            let send_result = self
                .net
                .as_mut()
                .map_or(Ok(()), |net| net.send_reliable_now(command.as_bytes()));
            if let Err(error) = send_result {
                let map = self
                    .live_download
                    .as_ref()
                    .map(|state| state.map_name.clone())
                    .unwrap_or_default();
                self.fail_live_autodownload(&map, error);
                return;
            }
        }
        if let Some((temp_path, final_path, checksum)) = completed {
            // Drop/close the file before the Windows rename.
            if let Some(state) = self.live_download.as_mut() {
                state.transfer = LiveDownloadTransfer::Idle;
            }
            if let Err(error) = crate::download::verify_pk3_checksum(&temp_path, checksum) {
                let _ = std::fs::remove_file(&temp_path);
                let map = self
                    .live_download
                    .as_ref()
                    .map(|state| state.map_name.clone())
                    .unwrap_or_default();
                self.fail_live_autodownload(&map, error);
                return;
            }
            if let Err(error) = std::fs::rename(&temp_path, &final_path) {
                let map = self
                    .live_download
                    .as_ref()
                    .map(|state| state.map_name.clone())
                    .unwrap_or_default();
                self.fail_live_autodownload(
                    &map,
                    format!("Installing {} failed: {error}", final_path.display()),
                );
                return;
            }
            if let Some(state) = self.live_download.as_mut() {
                state.index += 1;
                state.received = 0;
                state.total = None;
            }
            self.start_live_download_current();
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn finish_live_autodownload(&mut self) {
        let Some(state) = self.live_download.take() else {
            return;
        };
        let map_name = state.map_name;
        let source = scene::MapSource::Bsp(map_name.clone());
        if let Err(error) =
            scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source)
        {
            self.live_auto_download_requested = false;
            self.live_join_ui = None;
            if let Some(net) = self.net.as_mut() {
                let _ = net.send_reliable_now(b"stopdl");
            }
            self.show_missing_map_prompt(
                &map_name,
                format!("Downloads completed, but the map is still unavailable: {error}"),
            );
            return;
        }

        self.live_auto_download_requested = false;
        self.live_missing_map_authorized = false;
        if let Some(join_ui) = self.live_join_ui.as_mut() {
            join_ui.phase = LiveJoinUiPhase::Synchronizing;
            join_ui.detail =
                "Refreshing downloaded assets and requesting a fresh gamestate...".into();
        }
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        // A downloaded PK3 can contribute strings as well as map/render assets.
        // Force the post-download prep to rebuild from the refreshed search path.
        self.stringed = None;
        self.start_live_cgame_prep();
        self.console_status = format!("DOWNLOAD COMPLETE: {map_name}; REFRESHING GAMESTATE...");
        self.push_console_line(format!(
            "^2DOWNLOAD COMPLETE:^7 {map_name}; requesting fresh gamestate."
        ));
        let finish_result = self
            .net
            .as_mut()
            .map_or(Ok(()), |net| net.send_reliable_now(b"donedl"));
        if let Err(error) = finish_result {
            self.push_console_line(format!("^1Could not finish download handshake: {error}"));
            self.disconnect_to_main_menu();
            return;
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn fail_live_autodownload(&mut self, map_name: &str, error: String) {
        if let Some(state) = self.live_download.take() {
            if let LiveDownloadTransfer::Legacy { temp_path, .. } = state.transfer {
                let _ = std::fs::remove_file(temp_path);
            }
        }
        self.live_auto_download_requested = false;
        self.live_join_ui = None;
        if let Some(net) = self.net.as_mut() {
            let _ = net.send_reliable_now(b"stopdl");
        }
        self.push_console_line(format!("^1AUTODOWNLOAD FAILED:^7 {error}"));
        self.show_missing_map_prompt(map_name, format!("Autodownload failed: {error}"));
    }
}
