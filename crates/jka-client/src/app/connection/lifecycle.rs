//! Connection lifecycle.
use crate::app::{App, Arc, FrontendPage, OverlayMode, PhysicsMapMesh, RenderCommand};

impl App {
    pub(in crate::app) fn request_quit(&mut self) {
        if self.demo_recording.is_some() {
            self.stop_demo_recording();
        }
        // Hide the native window before the render thread begins shutting down.
        // Otherwise Windows can expose one more renderer frame during teardown,
        // which may briefly show the startup/loading splash again.
        self.quit_requested = true;
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
        // Do not wait for the rest of this frame. A console `quit` runs at the
        // start of tick(), so finishing the tick (network, cgame, console UI
        // publish) and another event-loop turn only lengthened the black
        // screen; the menu button runs late in the frame and had no such tail.
        self.exit_process();
    }

    pub(in crate::app) fn disconnect_to_main_menu(&mut self) {
        if self.chat_log_enabled {
            self.chat_log.end("Disconnected");
        }
        // A just-finished session may have created or appended a log after the
        // browser was last opened. Force a background rescan next time the
        // main-menu Chat Logs page is entered.
        self.chat_log_browser_catalog_loaded = false;
        self.chat_log_browser_detail_path = None;
        self.chat_log_browser_detail = None;
        if self.demo_recording.is_some() {
            self.stop_demo_recording();
        }
        // Invalidate any map preparation still in flight. Map worker results carry
        // the request id, so a late completion from the disconnected session will
        // be ignored instead of repopulating the renderer behind the front end.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.pending_frontend_map_launch = false;
        self.frontend_background_request_id = None;
        self.frontend_cinematic = None;
        self.preserve_game_state_on_next_map_upload = false;
        self.loading = None;
        self.static_ao_progress = None;
        self.prepared_map_cache = None;
        self.game_session = None;
        self.live_without_world = false;
        self.demo_without_world = false;
        self.server_password_prompt = None;
        self.missing_map_prompt = None;
        self.demo_missing_map_prompt = None;
        self.live_missing_map_authorized = false;
        self.live_auto_download_requested = false;
        self.live_http_base = None;
        self.live_download = None;
        self.live_join_ui = None;
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        self.pending_live_gamestate = false;
        self.pending_live_server_commands.clear();
        self.live_join_timing = None;
        self.japro_autologin_attempted = None;
        self.restore_startup_game();
        if let Some(mut net) = self.net.take() {
            net.disconnect();
        }
        self.predictor.reset();
        self.live_input = crate::net::LiveInput::default();
        self.scoreboard = None;
        self.scores_showing = false;
        self.last_scores_request = None;
        self.mark_companion_dirty();
        self.center_print = None;
        self.last_center_print.clear();

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
        self.map_name = "MAIN MENU".into();
        self.triangles = 0;
        self.map_distance_cull = crate::camera::DEFAULT_DISTANCE_CULL;
        self.map_authored_sun = None;
        self.map_authored_oceans.clear();
        self.authored_oceans.clear();
        self.authored_ocean_preview = false;
        self.render_command(RenderCommand::SetAuthoredOceans(Vec::new()));
        self.forget_trace();
        self.keys.clear();
        self.movement_keys.clear();
        self.mouse_buttons_down.clear();
        self.noclip_primary_down = false;
        self.noclip_alt_down = false;

        self.front_end = true;
        self.frontend_page = FrontendPage::Main;
        self.menu_selected = 0;
        self.controls_waiting_for_key = false;
        self.render_command(RenderCommand::UnloadMap);
        self.sync_render_fps_cap();
        self.set_overlay(OverlayMode::Game);
        self.console_status = "DISCONNECTED - MAIN MENU".into();
        self.push_console_line("^2DISCONNECTED - RETURNED TO MAIN MENU".to_owned());
        self.publish_snapshot();
        self.publish_ui();
        self.request_frontend_background();
    }

    /// Tear down an active live/local/demo session while transitioning directly
    /// into another demo. Unlike `disconnect_to_main_menu`, this deliberately
    /// does not publish a menu frame, start the frontend BSP, or send UnloadMap.
    /// The previous GPU world is harmless behind the opaque loading-information
    /// screen and is replaced atomically by RenderCommand::LoadMap.
    pub(in crate::app) fn disconnect_for_demo_transition(&mut self) {
        if self.chat_log_enabled {
            self.chat_log.end("Switched to demo playback");
        }
        if self.demo_recording.is_some() {
            self.stop_demo_recording();
        }
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.pending_frontend_map_launch = false;
        self.frontend_background_request_id = None;
        self.frontend_cinematic = None;
        self.preserve_game_state_on_next_map_upload = false;
        self.static_ao_progress = None;
        self.prepared_map_cache = None;
        self.game_session = None;
        self.live_without_world = false;
        self.demo_without_world = false;
        self.server_password_prompt = None;
        self.missing_map_prompt = None;
        self.demo_missing_map_prompt = None;
        self.live_missing_map_authorized = false;
        self.live_auto_download_requested = false;
        self.live_http_base = None;
        self.live_download = None;
        self.live_join_ui = None;
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        self.pending_live_gamestate = false;
        self.pending_live_server_commands.clear();
        self.live_join_timing = None;
        if let Some(mut net) = self.net.take() {
            net.disconnect();
        }
        self.predictor.reset();
        self.live_input = crate::net::LiveInput::default();
        self.scoreboard = None;
        self.scores_showing = false;
        self.last_scores_request = None;
        self.mark_companion_dirty();
        self.center_print = None;
        self.last_center_print.clear();

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
        self.triangles = 0;
        self.map_distance_cull = crate::camera::DEFAULT_DISTANCE_CULL;
        self.map_authored_sun = None;
        self.map_authored_oceans.clear();
        self.authored_oceans.clear();
        self.authored_ocean_preview = false;
        self.forget_trace();
        self.keys.clear();
        self.movement_keys.clear();
        self.mouse_buttons_down.clear();
        self.noclip_primary_down = false;
        self.noclip_alt_down = false;
    }
}
