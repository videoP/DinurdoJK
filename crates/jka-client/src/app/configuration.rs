//! Configuration.
use crate::app::{config, App};

impl App {
    pub(in crate::app) fn flush_config(&mut self) {
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
        settings.resolution = stable.resolution;
        // Grass/ocean can be disabled live. Preserve a pending enable as latched
        // until the renderer has actually been restarted with the required map
        // resources, but persist live disables immediately.
        settings.grass = if self.video.grass && !self.applied_grass {
            self.applied_grass
        } else {
            self.video.grass
        };
        settings.ocean = if self.video.ocean && !self.applied_ocean {
            self.applied_ocean
        } else {
            self.video.ocean
        };
        // OpenJK archives the pending CVAR_LATCH value. Overlay only console
        // latches here; unconfirmed menu display changes above remain last-good.
        self.apply_latched_values_to_settings(&mut settings);

        let presentation = config::ClientPresentationSettings {
            third_person: self.third_person,
            spectator_camera: self.spectator_camera,
            first_person_lightsaber: self.first_person_lightsaber,
            saber_trail: self.saber_trail,
            saber_team_colors: self.saber_team_colors,
            saber_staff_multi_color: self.saber_staff_multi_color,
            team_overlay: self.team_overlay,
            score_deaths: self.score_deaths,
            draw_scores: self.draw_scores,
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
            console_suggest: self.console_suggest,
            chatbox_completion: self.chatbox_completion,
            chat_log: self.chat_log_enabled,
            ui_vgs: self.ui_vgs,
            jump_height_shade: self.jump_height_shade,
            screen_shake: self.screen_shake,
            zoom_fov: self.japro_zoom_fov,
            fk_duration: self.japro_fk_duration,
            fk_first_jump_duration: self.japro_fk_first_jump_duration,
            fk_second_jump_delay: self.japro_fk_second_jump_delay,
            network: self.network.clone(),
            japro: self.japro_cg,
            master_servers: self.server_browser.master_servers.clone(),
        };
        match config::save_video_settings(
            &self.config_path,
            settings,
            &self.bindings,
            &presentation,
            &self.audio,
        ) {
            Ok(()) => self.config_dirty = false,
            Err(error) => eprintln!("Config save failed: {error}"),
        }
    }
}
