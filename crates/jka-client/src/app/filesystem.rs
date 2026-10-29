//! Filesystem.
use crate::app::{App, Arc, ColorLutPreset, FullscreenMode, Path, PathBuf, RenderCommand, RendererBackend};

impl App {
    /// DinurdoJK's targeted equivalent of the useful part of OpenJK's
    /// FS_Restart: re-index content that may have appeared on disk and retry
    /// failed registrations, without tearing down the client/game session.
    pub(in crate::app) fn restart_filesystem(&mut self) {
        // Explicit asset refresh invalidates parked sources. A normal fs_game
        // transition keeps them: each entry owns its own game/VFS identity.
        self.pk3_manager_loaded = false;
        self.invalidate_studio_sources();
        self.reset_asset_catalogs_for_game_change();
        if self.front_end && self.frontend_page == crate::app::FrontendPage::RenderStudio {
            self.render_studio_update_backdrop();
        }

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
        // fs_restart during connection hand off a stale prepared presenter.
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
                "FS RESTART COMPLETE: ACTIVE VFS RE-INDEXED, {retried} FAILED REGISTRATION(S) RETRIED"
            );
            self.push_console_line(format!(
                "^2FS_RESTART:^7 mounted filesystem layers re-indexed; {retried} failed registration(s) eligible for retry"
            ));
        } else {
            self.console_status = format!("FS RESTART COMPLETED WITH {} WARNING(S)", errors.len());
            self.push_console_line(format!(
                "^3FS_RESTART:^7 re-indexed with {} warning(s); {retried} failed registration(s) eligible for retry",
                errors.len()
            ));
            for error in errors {
                self.push_console_line(format!("^3FS_RESTART:^7 {error}"));
            }
        }
        self.publish_ui();
    }

    /// Destructive filesystem refresh: replace CPU CGame asset owners and then
    /// use the existing vid_restart state machine to release renderer-owned GPU
    /// resources and reprepare the BSP against the new PK3 search order.
    ///
    /// Snapshot/decode/prediction state stays in GameSession. Only the asset
    /// presenters (including the worker-owned FX cache and decoded audio) are
    /// replaced. This is a user-triggered operation, never a frame hot-path.
    pub(in crate::app) fn rebuild_filesystem(&mut self) -> Result<(), String> {
        // A second restart in the middle of an in-flight shutdown can strand
        // both the renderer and the map loader. Do not drop cached assets until
        // the existing video transaction can actually accept a restart.
        if self.video_restart_in_flight() || self.renderer_restart_suspended {
            return Err("video restart is already in progress".into());
        }
        if self.window.is_none() || self.render.is_none() {
            return Err("renderer/window is not ready for a full rebuild".into());
        }

        // The ordinary vid_restart path refuses an unavailable requested
        // exclusive mode. Check before touching CPU assets, so a pending bad
        // video setting cannot leave a half-rebuilt renderer/session.
        if self.video.fullscreen == FullscreenMode::Exclusive
            && self.video.renderer_backend != RendererBackend::Dx12
            && self.window.as_ref().is_some_and(|window| {
                Self::exclusive_video_mode(window, self.video.resolution).is_none()
            })
        {
            return Err("requested exclusive video mode is unavailable".into());
        }

        // Prepare new CGame owners first. If an archive was essential and its
        // removal makes the new namespace invalid, keep the existing session
        // intact rather than partially replacing presenters.
        let rebuilt = if self.game_session.is_some() {
            Some(Self::build_cgame_cpu_assets(
                &self.base,
                self.game.as_deref(),
                self.video.pbr,
                self.video.allow_asset_overrides,
            )?)
        } else {
            None
        };

        // The normal fs_restart still handles every non-destructive catalog and
        // VFS refresh. It deliberately does *not* evict successful registrations.
        self.restart_filesystem();

        // Discard jobs that were prepared against the previous generation of
        // mounts. An incoming gamestate must still be processed once its new
        // speculative CGame completes.
        let was_preparing = self.live_cgame_prepared.is_some()
            || self.live_cgame_prep_rx.is_some();
        if was_preparing {
            let pending_gamestate = self.pending_live_gamestate;
            self.start_live_cgame_prep();
            self.pending_live_gamestate = pending_gamestate;
        }

        if let Some((siege_catalog, siege_classes, player, entity, fx_assets)) = rebuilt {
            // Detach the old session to allow the same canonical setup path as
            // a new CGame; all network, demo, and simulation state is retained.
            if let Some(mut session) = self.game_session.take() {
                session.siege_catalog = siege_catalog;
                session.siege_classes = siege_classes;
                session.player_presenter = player;
                session.entity_presenter = entity;
                session.weapon_fx = crate::cgame::weapon_fx::WeaponFx::new(fx_assets);
                session.sound_presenter = None;
                session.dynamic_models = Arc::new(Vec::new());
                session.fx_draws.clear();
                session.fx_runner_draws.clear();
                session.fx_light_promoter.invalidate_materials();
                self.configure_game_session(&mut session, self.game.as_deref(), true);
                session.player_presenter.set_shader_remaps(self.shader_remaps.clone());
                session.entity_presenter.set_shader_remaps(self.shader_remaps.clone());
                self.game_session = Some(session);
            }
        }
        self.local_player_presenter = None;
        self.menu_emote_sound = None;
        crate::emotes::stop();
        crate::emotes::refresh_catalog();

        // A pending map job might still be decoding content from an earlier
        // archive namespace. Its result must never repopulate the CPU cache.
        // The new vid_restart/map request uses a newer request ID.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.prepared_map_cache = None;
        self.restart_renderer();
        self.push_console_line(
            "^2FS_REBUILD:^7 CPU CGame assets replaced; renderer and world reload queued".to_owned(),
        );
        Ok(())
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
        self.extensions.set_game(self.game.clone());
        self.refresh_color_luts();
        self.prepared_map_cache = None;
        self.stringed = None;
        self.ui_catalog.reset();
        self.render_studio.cinematic_preview=None;
        self.render_studio.cinematics=Default::default();
        if let Some(world)=self.render_studio.bsp_world.take() {
            self.render_command(RenderCommand::RemoveStudioSource(world.id));
        }
        self.profile_background_maps=None;
        self.render_studio.bsp_maps=None;
        self.profile_background_requested=None;
        self.profile_background_available=false;
        self.render_command(RenderCommand::SetAssetPreviewBackground(None));
        *self.console_map_suggest_cache.borrow_mut() = None;
        *self.console_arg_suggest_cache.borrow_mut() = None;
        self.console_file_catalog_loaded = false;
        self.console_files = Default::default();
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
        // The model VFS and its renderer registrations are being replaced;
        // old locally placed MD3 instances must not keep stale asset pointers.
        self.model_placement = Default::default();
        self.asset_viewer_cache = None;
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
        self.demo_catalog_pending = false;
        self.demo_catalog_generation = self.demo_catalog_generation.wrapping_add(1);
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
        jka_assets::pk3::game_write_directory(&self.base, self.game.as_deref()).join(relative)
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
        // Keep only a completed, immutable CPU map as a *candidate* seed.
        // The map worker validates BSP bytes and per-resource/stage hashes;
        // this never reuses old GPU resources or old resolved lightmaps.
        self.cross_game_map_seed = self.prepared_map_cache.take();
        self.game = game;
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.map_loader_latest_requested.store(
            self.latest_request_id,
            std::sync::atomic::Ordering::Release,
        );
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

    /// A local (solo) game mounts the `fs_game` cvar over base; the default is
    /// empty, which leaves the base directory active. `+set fs_game japro` can
    /// select a mod explicitly. An explicit `--game` startup directory wins,
    /// and a missing directory leaves base alone. Must run before the map request
    /// so the map worker sees the game dir.
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
