//! Filesystem.
use crate::app::{App, Arc, ColorLutPreset, Path, PathBuf, RenderCommand};

impl App {
    /// DinurdoJK's targeted equivalent of the useful part of OpenJK's
    /// FS_Restart: re-index content that may have appeared on disk and retry
    /// failed registrations, without tearing down the client/game session.
    pub(in crate::app) fn refresh_filesystem(&mut self) {
        self.reset_asset_catalogs_for_game_change();

        let mut retried = 0usize;
        let mut errors = Vec::<String>::new();

        if let Some(presenter) = self.local_player_presenter.as_mut() {
            match presenter.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
        }

        if let Some(session) = self.game_session.as_mut() {
            match session.player_presenter.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
            match session.entity_presenter.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
            match session.weapon_fx.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
            if let Some(sound) = session.sound_presenter.as_mut() {
                match sound.retry_failed_assets() {
                    Ok(count) => retried += count,
                    Err(error) => errors.push(error),
                }
            }
        }

        // A speculative live-session CGame may already exist before it becomes
        // `game_session`. Refresh that long-lived VFS too rather than letting an
        // fs_refresh during connection hand off a stale prepared presenter.
        if let Some(prepared) = self.live_cgame_prepared.as_mut() {
            match prepared.player_presenter.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
            match prepared.entity_presenter.retry_failed_assets() {
                Ok(count) => retried += count,
                Err(error) => errors.push(error),
            }
            if let Err(error) = prepared.fx_assets.refresh() {
                errors.push(format!("FX PREP ASSET REFRESH ERROR: {error}"));
            }
        }

        if errors.is_empty() {
            self.console_status = format!(
                "FS REFRESH COMPLETE: ACTIVE VFS RE-INDEXED, {retried} FAILED REGISTRATION(S) RETRIED"
            );
            self.push_console_line(format!(
                "^2FS_REFRESH:^7 active base/fs_game re-indexed; {retried} failed registration(s) eligible for retry"
            ));
        } else {
            self.console_status = format!("FS REFRESH COMPLETED WITH {} WARNING(S)", errors.len());
            self.push_console_line(format!(
                "^3FS_REFRESH:^7 re-indexed with {} warning(s); {retried} failed registration(s) eligible for retry",
                errors.len()
            ));
            for error in errors {
                self.push_console_line(format!("^3FS_REFRESH:^7 {error}"));
            }
        }
        self.publish_ui();
    }

    /// Re-list `luts/*.cube` for the current game directory, keeping the active
    /// selection by name (its index can shift) and dropping it if it vanished.
    pub(in crate::app) fn refresh_color_luts(&mut self) {
        let active = self.video.color_lut.config_value();
        crate::color_lut::scan_external(&self.base, self.game.as_deref());
        let preset = ColorLutPreset::from_config(active).unwrap_or(ColorLutPreset::Off);
        if preset != self.video.color_lut || matches!(preset, ColorLutPreset::External(_)) {
            self.set_color_lut(preset);
        }
    }

    pub(in crate::app) fn reset_asset_catalogs_for_game_change(&mut self) {
        self.refresh_color_luts();
        self.prepared_map_cache = None;
        self.stringed = None;
        self.ui_catalog.reset();
        self.solo_catalog_loaded = false;
        self.solo_catalog_error = None;
        self.solo_maps.clear();
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
        self.source_map_catalog_loaded = false;
        self.source_map_catalog_error = None;
        self.source_maps.clear();
        self.source_map_levelshot_texture = None;
        self.source_map_levelshot_texture_map = None;
        self.asset_viewer_catalog_loaded = false;
        self.asset_viewer_catalog_error = None;
        self.asset_viewer_entries.clear();
        self.asset_viewer_selected = None;
        self.asset_viewer_shader_file.clear();
        self.asset_viewer_detail_path = None;
        self.asset_viewer_detail = None;
        self.asset_viewer_shader_name = None;
        self.screenshot_catalog_loaded = false;
        self.screenshot_catalog_error = None;
        self.screenshot_entries.clear();
        self.screenshot_selected = None;
        self.screenshot_preview_texture = None;
        self.screenshot_preview_path = None;
        self.screenshot_preview_pending = None;
        self.screenshot_preview_error = None;
        self.chat_log_browser_catalog_loaded = false;
        self.chat_log_browser_catalog_error = None;
        self.chat_log_browser_entries.clear();
        self.chat_log_browser_selected = None;
        self.chat_log_browser_detail_path = None;
        self.chat_log_browser_detail = None;
        self.asset_preview_player_presenter = None;
        self.asset_preview_entity_presenter = None;
        self.asset_preview_model_key = None;
        self.asset_preview_shader_key = None;
        self.asset_preview_fx = None;
        self.asset_preview_viewport_key = None;
        self.profile_catalog_loaded = false;
        self.profile_catalog_error = None;
        self.profile_models.clear();
        self.profile_model_icon_textures.clear();
        self.crosshair_image_textures.clear();
        self.profile_sabers.clear();
        self.profile_preview_key = None;
        self.profile_dynamic_models = Arc::new(Vec::new());
        // Detail texture selection is always material-driven AUTO. A game/mod
        // change invalidates its VFS source, so refresh the renderer-side AUTO
        // cache without enumerating a manual settings list.
        self.render_command(RenderCommand::SetDetailTexture {
            path: "auto".to_owned(),
            game: self.game.clone(),
        });
        self.demo_catalog_loaded = false;
        self.demo_catalog_error = None;
        self.demo_entries.clear();
        self.demo_metadata_generation = self.demo_metadata_generation.wrapping_add(1);
        self.demo_metadata_cache.clear();
        self.demo_metadata_errors.clear();
        self.demo_metadata_inflight.clear();
    }

    /// Resolve a game-relative write without letting the feature decide whether
    /// it belongs to base or the current server/demo fs_game.
    pub(in crate::app) fn game_write_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref()).join(relative)
    }

    pub(in crate::app) fn set_session_fs_game(
        &mut self,
        value: &[u8],
        source: &str,
    ) -> Result<bool, String> {
        self.set_session_fs_game_inner(value, source, false)
    }

    /// Demo transitions already have a full-screen loading-information screen.
    /// Keep the last GPU world resident behind it until the replacement BSP is
    /// uploaded; clearing the renderer here creates a visible fog-color frame.
    pub(in crate::app) fn set_session_fs_game_preserving_rendered_world(
        &mut self,
        value: &[u8],
        source: &str,
    ) -> Result<bool, String> {
        self.set_session_fs_game_inner(value, source, true)
    }

    pub(in crate::app) fn set_session_fs_game_inner(
        &mut self,
        value: &[u8],
        source: &str,
        preserve_rendered_world: bool,
    ) -> Result<bool, String> {
        let game = jka_assets::pk3::resolve_fs_game_directory(&self.base, value)?;
        if self.game == game {
            return Ok(false);
        }
        let missing_game = game
            .as_ref()
            .filter(|path| !path.is_dir())
            .map(|path| path.display().to_string());
        let previous = self
            .game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        let next = game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        self.game = game;
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        if !preserve_rendered_world {
            self.loading = None;
        }
        self.static_ao_progress = None;
        self.reset_asset_catalogs_for_game_change();
        self.map_collision = None;
        self.map_mark_surfaces = None;
        self.map_static_models = Arc::default();
        self.map_visibility = None;
        self.map_movement = None;
        self.spawns.clear();
        self.live_without_world = false;
        self.demo_without_world = false;
        self.map_name = "MAIN MENU".into();
        if !preserve_rendered_world {
            self.render_command(RenderCommand::UnloadMap);
        }
        devprintln!(1, "FS_GAME: {source}: {previous} -> {next}");
        self.push_console_line(format!("^5FS_GAME:^7 {next} ^8({source})"));
        if let Some(path) = missing_game {
            self.push_console_line(format!(
                "^3FS_GAME:^7 mod directory is not installed locally: {path}; base fallback remains available"
            ));
        }
        Ok(true)
    }

    /// A local (solo) game mounts the `fs_game` cvar (default `japro`) over base,
    /// like `+set fs_game japro`, so jaPRO-only assets resolve as they do on a
    /// jaPRO server. An explicit `--game` startup directory wins, and a missing
    /// directory leaves base alone. Must run before the map request so the map
    /// worker sees the game dir.
    pub(in crate::app) fn enter_local_game_dir(&mut self) {
        if self.startup_game.is_some() {
            return;
        }
        let wanted = self.network.fs_game.clone();
        if let Err(error) = self.set_session_fs_game(wanted.as_bytes(), "local game") {
            self.push_console_line(format!("^3FS_GAME:^7 local game stays on base: {error}"));
        }
    }

    pub(in crate::app) fn restore_startup_game(&mut self) {
        if self.game == self.startup_game {
            return;
        }
        self.game = self.startup_game.clone();
        self.reset_asset_catalogs_for_game_change();
        let name = self
            .game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        devprintln!(1, "FS_GAME: restored startup game {name}");
    }
}
