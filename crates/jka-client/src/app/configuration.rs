//! Configuration.
use crate::app::{config, App};

impl App {
    /// Explicit saves (writeconfig, video confirmation, quit) retain their
    /// synchronous durability semantics. Older queued saves complete first,
    /// so they cannot overwrite the explicitly flushed current settings.
    pub(in crate::app) fn flush_config(&mut self) {
        self.queue_config_save();
        if !self.background_jobs.flush_io(std::time::Duration::from_secs(1)) {
            eprintln!("Timed out waiting for queued config/background I/O");
        }
        self.poll_background_results();
    }

    /// Debounced periodic save: never perform filesystem I/O on the tick path.
    pub(in crate::app) fn queue_config_save(&mut self) {
        // Offline captures may override video/game settings for a comparison;
        // never persist them over the interactive user's configuration.
        // Inside Render Studio the renderer runs a Studio graphics profile;
        // gameplay values were flushed on entry and are restored on exit.
        if super::lifecycle::headless() || self.render_studio.graphics_gameplay.is_some() { return; }
        if !self.config_dirty {
            return;
        }

        // Restart-sensitive settings are latched. Never persist a merely selected
        // resolution/fullscreen/backend, and never persist an unconfirmed mode.
        // If the process dies while the keep/revert dialog is visible, the next
        // launch therefore comes back in the last known-good video mode.
        let stable = self
            .video_confirmation
            .map(|confirmation| confirmation.previous)
            .or(self.pending_video_confirmation)
            .or_else(|| {
                self.pending_renderer_restart.map(|pending| {
                    if pending.confirm_on_change {
                        pending.previous
                    } else {
                        // A non-confirming restart is a rollback/recovery. Its
                        // target is already the last known-good mode.
                        pending.target
                    }
                })
            })
            .unwrap_or_else(|| self.applied_video_mode());
        let mut settings = self.video;
        settings.fullscreen = stable.fullscreen;
        settings.renderer_backend = stable.renderer_backend;
        settings.gpu_adapter = stable.gpu_adapter;
        settings.resolution = stable.resolution;
        // Grass can be disabled live. Preserve a pending enable as latched
        // until the renderer has actually been restarted with the required map
        // resources, but persist live disables immediately.
        settings.grass = if self.video.grass && !self.applied_grass {
            self.applied_grass
        } else {
            self.video.grass
        };
        // Archive the selected ocean preference even before Apply has prepared
        // the current world's clipmaps. The next launch prepares with this value;
        // substituting applied_ocean here silently lost pending enables on quit.
        // OpenJK archives the pending CVAR_LATCH value. Overlay only console
        // latches here; unconfirmed menu display changes above remain last-good.
        self.apply_latched_values_to_settings(&mut settings);

        let presentation = config::ClientPresentationSettings {
            game_launch_screen: self.game_launch_screen,
            third_person: self.third_person,
            spectator_camera: self.spectator_camera,
            first_person_lightsaber: self.first_person_lightsaber,
            saber_trail: self.saber_trail,
            saber_team_colors: self.saber_team_colors,
            saber_staff_multi_color: self.saber_staff_multi_color,
            team_overlay: self.team_overlay,
            score_deaths: self.score_deaths,
            score_spectator_guess: self.score_spectator_guess,
            draw_scores: self.draw_scores,
            draw_score: self.draw_score,
            draw_score_x: self.draw_score_x,
            draw_score_y: self.draw_score_y,
            draw_enemy_info: self.draw_enemy_info,
            mouse: self.mouse_input,
            fov: self.camera.cg_fov(),
            model: self.solo_client_info.model_cvar(),
            force_model: self.force_model.clone(),
            crosshair: self.crosshair,
            player_names: self.player_names,
            hud_layout: self.hud_layout,
            movement_keys: self.movement_keys_hud,
            strafe_helper: self.strafe_helper,
            strafe_trail: self.strafe_trails.settings.clone(),
            race_ghost_alpha: self.race_ghost_alpha,
            race_ghost_name: self.race_ghost_name,
            race_ghost_trail: self.race_ghost_trail,
            race_ghost_velocity_delta: self.race_ghost_velocity_delta,
            race_ghost_distance_delta: self.race_ghost_distance_delta,
            race_ghost_demo_base_url: self.race_ghost_demo_base_url.clone(),
            console_timestamps: self.console_timestamps,
            console_scale: self.console_scale,
            console_notify_lines: self.console_notify_lines,
            console_notify_words: self.console_notify_words.clone(),
            console_notify_connect: self.console_notify_connect,
            console_notify_vote: self.console_notify_vote,
            console_notify_beep: self.console_notify_beep,
            console_suggest: self.console_suggest,
            chatbox_completion: self.chatbox_completion,
            clean_chatbox: self.clean_chatbox,
            streamer: self.streamer,
            chat_log: self.chat_log_enabled,
            ui_vgs: self.ui_vgs,
            jump_height_shade: self.jump_height_shade,
            screen_shake: self.screen_shake,
            head_look: self.head_look,
            swim_tilt: self.swim_tilt,
            swim_turn_bank: self.swim_turn_bank,
            move_lean: self.move_lean,
            zoom_fov: self.japro_zoom_fov,
            view_bob: self.view_bob,
            fk_duration: self.japro_fk_duration,
            fk_first_jump_duration: self.japro_fk_first_jump_duration,
            fk_second_jump_delay: self.japro_fk_second_jump_delay,
            killfeed: self.killfeed_settings,
            scoreboard: self.scoreboard_settings,
            chat: self.chat_settings,
            network: self.network.clone(),
            japro: self.japro_cg,
            master_servers: self.server_browser.master_servers.clone(),
            server_filters: self.server_browser.filters_string(),
        };
        let path = self.config_path.clone();
        let fs_roots = self.fs_roots.clone();
        let bindings = self.bindings.clone();
        let audio = self.audio.clone();
        let generation = self.config_save_generation.wrapping_add(1);
        let tx = self.background_result_tx.clone();
        match self.background_jobs.io(move || {
            // Report serialization panics as a failed save instead of letting
            // the shared queue swallow the panic and silently lose settings.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                config::save_video_settings(&path, settings, &bindings, &presentation, &audio)?;
                // Keep writeconfig faithful to the cvar console even though
                // path discovery at startup uses the exe-local bootstrap file.
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .append(true).open(&path).map_err(|e| e.to_string())?;
                let value = |path: &Option<std::path::PathBuf>|
                    path.as_ref().map_or(String::new(), |p| p.display().to_string());
                writeln!(file, "\n// Filesystem paths (restart required; mirrored in DinurdoJK-paths.ini)")
                    .map_err(|e| e.to_string())?;
                for (name, value) in [
                    ("fs_homepath", value(&fs_roots.homepath)),
                    ("fs_cdpath", value(&fs_roots.cdpath)),
                    ("fs_forcegame", fs_roots.forcegame.unwrap_or_default()),
                ] {
                    writeln!(file, "seta {name} \"{value}\"").map_err(|e| e.to_string())?;
                }
                Ok(())
            }))
            .unwrap_or_else(|_| Err("config serialization panicked".to_owned()));
            let _ = tx.send(super::background_jobs::JobResult::ConfigSaved { generation, result });
        }) {
            Ok(()) => {
                self.config_save_generation = generation;
                self.config_dirty = false;
                self.last_config_write = std::time::Instant::now();
            }
            Err(error) => {
                eprintln!("Could not queue config save: {error}");
                self.last_config_write = std::time::Instant::now();
            },
        }
    }
}

impl App {
    pub(in crate::app) fn poll_background_results(&mut self) {
        while let Ok(done) = self.background_result_rx.try_recv() {
            match done {
                super::background_jobs::JobResult::ConfigSaved { generation, result } => {
                    if let Err(error) = result {
                        eprintln!("Config save failed: {error}");
                        // A newer scheduled save takes precedence over a failed
                        // earlier write; don't resurrect stale config state.
                        if generation == self.config_save_generation {
                            self.config_dirty = true;
                            self.last_config_write = std::time::Instant::now();
                        }
                    }
                }
                super::background_jobs::JobResult::ServerDump { path, result } => match result {
                    Ok(()) => self.push_console_path_line(
                        format!("^2serverdump: wrote {} and appended to servers.log", path.display()),
                        path,
                    ),
                    Err(error) => self.push_console_line(format!("^1serverdump: couldn't write files: {error}")),
                },
            }
        }
    }
}
