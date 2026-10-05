//! Input.
use crate::app::{
    keybinds, App, Arc, BindKey, CursorGrabMode, DemoViewMode, ElementState, FrontendPage, HashSet,
    Instant, KeyCode, KeyEvent, OverlayMode, RenderCommand,
};

impl App {
    pub(in crate::app) fn set_capture(&mut self, captured: bool) {
        if !captured {
            self.noclip_primary_down = false;
            self.noclip_alt_down = false;
        }
        if self.captured == captured {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        if captured && self.overlay == OverlayMode::None {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if grabbed.is_ok() {
                window.set_cursor_visible(false);
                self.captured = true;
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.captured = false;
        }
    }

    pub(in crate::app) fn sync_profile_preview_mode(&mut self, active: bool) {
        if self.profile_preview_active == active {
            return;
        }
        self.profile_preview_active = active;
        if active {
            self.profile_name_input = self.network.name.clone();
            self.profile_preview_started = Instant::now();
        }
        self.profile_force_preview_pending = None;
        self.profile_force_preview_active = None;
        self.profile_force_preview_started = Instant::now();
        self.profile_preview_key = None;
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewMode(active));
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        if !active {
            self.profile_dynamic_models = Arc::new(Vec::new());
        }
    }

    pub(in crate::app) fn set_overlay(&mut self, overlay: OverlayMode) {
        let leaving_console =
            self.overlay == OverlayMode::Console && overlay != OverlayMode::Console;
        self.overlay = overlay;
        self.sync_profile_preview_mode(
            self.overlay == OverlayMode::Game
                && if self.front_end {
                    self.frontend_page == FrontendPage::Profile
                } else {
                    self.menu_selected == 2
                },
        );
        self.egui_repaint_requested = true;
        if leaving_console {
            self.console_input.clear();
            self.console_cursor = 0;
            self.console_history_index = None;
        }
        if overlay != OverlayMode::Console {
            self.console_search_open = false;
        }
        self.last_mouse_motion_at = None;
        if let Some(player) = &mut self.local_server {
            player.pause();
        }
        if overlay != OverlayMode::None {
            self.keys.clear();
            self.movement_keys.clear();
            self.mouse_buttons_down.clear();
            self.noclip_primary_down = false;
            self.noclip_alt_down = false;
            self.set_capture(false);
        } else if self.demo_playback_active() {
            self.sync_demo_camera_capture();
        } else {
            self.set_capture(true);
        }
        self.publish_ui();
    }

    pub(in crate::app) fn controls_page_active(&self) -> bool {
        self.overlay == OverlayMode::Game
            && if self.front_end {
                self.frontend_page == FrontendPage::Controls
            } else {
                self.menu_selected == 3
                    || (self.menu_selected == 6
                        && self.ui_mod() == Some(crate::net::mod_support::ServerMod::Japro))
            }
    }

    pub(in crate::app) fn overlay_after_console(&self) -> OverlayMode {
        if self.front_end && self.overlay_before_console == OverlayMode::None {
            OverlayMode::Game
        } else {
            self.overlay_before_console
        }
    }

    pub(in crate::app) fn frontend_back(&mut self) {
        if !self.front_end {
            return;
        }
        if self.overlay == OverlayMode::Video {
            self.frontend_page = FrontendPage::Main;
            self.set_overlay(OverlayMode::Game);
            return;
        }
        if self.frontend_page == FrontendPage::Profile {
            self.sync_profile_preview_mode(false);
        } else if self.frontend_page == FrontendPage::AssetViewer {
            self.render_command(RenderCommand::SetAssetPreviewMode(false));
            self.render_command(RenderCommand::SetAssetPreviewViewport(None));
            self.asset_preview_viewport_key = None;
            self.clear_asset_preview_runtime();
        }
        self.frontend_page = match self.frontend_page {
            FrontendPage::Main => FrontendPage::Main,
            FrontendPage::Play
            | FrontendPage::Profile
            | FrontendPage::Controls
            | FrontendPage::ChatLogs
            | FrontendPage::DeveloperTools
            | FrontendPage::Screenshots => FrontendPage::Main,
            FrontendPage::ServerBrowser | FrontendPage::SoloGame | FrontendPage::PlayDemo => {
                FrontendPage::Play
            }
            FrontendPage::AssetViewer | FrontendPage::MapViewer => FrontendPage::DeveloperTools,
        };
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app) fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
        self.last_config_write = Instant::now();
    }

    pub(in crate::app) fn refresh_bound_state(&mut self) {
        let spectator_context = self.spectator_binding_context();
        let mut movement_keys = HashSet::with_capacity(8);
        let mut primary = false;
        let mut alt = false;
        let mut live_buttons = HashSet::new();

        let mut apply = |binding: &str| {
            for command in keybinds::split_binding_commands(binding) {
                let verb = command.split_whitespace().next().unwrap_or("");
                if verb.starts_with('+') {
                    live_buttons.insert(verb.to_ascii_lowercase());
                }
                match verb.to_ascii_lowercase().as_str() {
                    "+forward" => {
                        movement_keys.insert(KeyCode::KeyW);
                    }
                    "+back" => {
                        movement_keys.insert(KeyCode::KeyS);
                    }
                    "+moveleft" => {
                        movement_keys.insert(KeyCode::KeyA);
                    }
                    "+moveright" => {
                        movement_keys.insert(KeyCode::KeyD);
                    }
                    "+moveup" => {
                        movement_keys.insert(KeyCode::Space);
                    }
                    "+movedown" => {
                        movement_keys.insert(KeyCode::ControlLeft);
                    }
                    "+speed" => {
                        movement_keys.insert(KeyCode::ShiftLeft);
                    }
                    "+attack" => primary = true,
                    "+altattack" => alt = true,
                    _ => {}
                }
            }
        };

        for code in &self.keys {
            if let Some(binding) = self
                .bindings
                .get_resolved(keybinds::bind_key_for_code(*code), spectator_context)
            {
                apply(binding);
            }
        }
        for button in &self.mouse_buttons_down {
            if let Some(binding) = self
                .bindings
                .get_resolved(BindKey::Mouse(*button), spectator_context)
            {
                apply(binding);
            }
        }

        let was_scores_showing = self.scores_showing;
        let was_demo_ui_modifier = self.live_buttons.contains("+speed");
        let was_zoomed = self.live_buttons.contains("+zoom");
        let is_zoomed = live_buttons.contains("+zoom");
        if is_zoomed != was_zoomed {
            // TaystJK CG_ZoomDown_f / CG_ZoomUp_f record cg.zoomTime when the
            // held +zoom state changes; keep the same transition epoch here.
            self.japro_zoom_transition_at = Some(Instant::now());
        }
        self.movement_keys = movement_keys;
        self.live_buttons = live_buttons;
        self.noclip_primary_down = primary;
        self.noclip_alt_down = alt;
        self.scores_showing = self.live_buttons.contains("+scores");
        if self.demo_playback_active()
            && was_demo_ui_modifier != self.live_buttons.contains("+speed")
        {
            self.sync_demo_camera_capture();
        }
        if self.scores_showing && !was_scores_showing {
            // TaystJK's CG_ScoresDown_f does not send `score` or clear the
            // cached board during demo playback. Rebuild from the demo index;
            // live play keeps the normal ~2 second score request cadence.
            if !self.refresh_demo_scoreboard(Instant::now()) {
                self.request_scores_if_due(true);
            }
        }
        if self.scores_showing != was_scores_showing {
            self.publish_ui();
        }
    }

    /// Whether `code` is currently bound to the `trace` command, so
    /// `route_egui_window_event` can let it fall through to the raw keybind
    /// dispatch instead of being swallowed as "a menu is open" input while
    /// the Trace popup is up — the same key needs to close it again.
    pub(in crate::app) fn key_bound_to_trace(&self, code: KeyCode) -> bool {
        self.bindings
            .get_resolved(
                keybinds::bind_key_for_code(code),
                self.spectator_binding_context(),
            )
            .is_some_and(|binding| keybinds::binding_contains_command(binding, "trace"))
    }

    pub(in crate::app) fn execute_binding_press(&mut self, key: BindKey) {
        let spectator_context = self.spectator_binding_context();
        let Some(binding) = self
            .bindings
            .get_resolved(key, spectator_context)
            .map(str::to_owned)
        else {
            return;
        };
        let mut buffered = Vec::new();
        for command in keybinds::split_binding_commands(&binding) {
            if command.starts_with('+') {
                let button_verb = command.split_whitespace().next().unwrap_or("");
                if self.demo_playback_active() {
                    let follow_direction = if button_verb.eq_ignore_ascii_case("+attack") {
                        Some(1)
                    } else if button_verb.eq_ignore_ascii_case("+altattack") {
                        Some(-1)
                    } else {
                        None
                    };
                    if let Some(direction) = follow_direction {
                        // +speed is the demo cursor/timeline modifier. While it is
                        // held, mouse buttons belong to UI interaction, not POV cycling.
                        if !self.live_buttons.contains("+speed") {
                            self.cycle_demo_follow(direction);
                        }
                        continue;
                    }
                }
                if button_verb.eq_ignore_ascii_case("+moveup")
                    && self.demo_playback_active()
                    && self.demo_view_mode != DemoViewMode::Free
                {
                    self.enter_demo_free_view();
                }
                // +scores is a local CG kbutton, not a usercmd button bit. The
                // held state is derived by refresh_bound_state above.
                if command
                    .split_whitespace()
                    .next()
                    .is_some_and(|verb| verb.eq_ignore_ascii_case("+scores"))
                {
                    continue;
                }
                // IN_KeyDown wasPressed: a tap shorter than a frame still fires.
                if let Some(verb) = command.split_whitespace().next() {
                    self.live_input.note_pressed(verb);
                }
                continue;
            }
            buffered.push(command.to_owned());
        }
        // Non-button commands use the same Cbuf path as console scripts, so a
        // binding like `echo one; wait; echo two` observes frame-aware wait.
        self.append_console_commands(buffered);
    }

    pub(in crate::app) fn bind_control_key(&mut self, key: BindKey) {
        let Some(action) = keybinds::control_action(self.controls_selected) else {
            return;
        };
        let command = action.command;
        if keybinds::is_spectator_selection(self.controls_selected) {
            self.bindings.unbind_spectator_command(command);
            self.bindings.set_spectator(key, command);
        } else {
            self.bindings.unbind_command(command);
            self.bindings.set(key, command);
        }
        self.controls_waiting_for_key = false;
        self.refresh_bound_state();
        self.mark_config_dirty();
        self.console_status = format!("BOUND {} TO {}", keybinds::key_name(key), command);
        self.publish_ui();
    }

    pub(in crate::app) fn unbind_selected_control(&mut self) {
        if let Some(action) = keybinds::control_action(self.controls_selected) {
            let command = action.command;
            let changed = if keybinds::is_spectator_selection(self.controls_selected) {
                self.bindings.unbind_spectator_command(command)
            } else {
                self.bindings.unbind_command(command)
            };
            if changed > 0 {
                self.refresh_bound_state();
                self.mark_config_dirty();
            }
        }
        self.controls_waiting_for_key = false;
        self.publish_ui();
    }

    pub(in crate::app) fn handle_bind_console_command(&mut self, command: &str) -> bool {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else {
            return false;
        };
        if verb.eq_ignore_ascii_case("bind") || verb.eq_ignore_ascii_case("bindspec") {
            let spectator = verb.eq_ignore_ascii_case("bindspec");
            let label = if spectator { "bindspec" } else { "bind" };
            match words.as_slice() {
                [_] => self.push_console_line(format!(
                    "^3{label} <key> [command]^7 - attach a {}command to a key",
                    if spectator { "spectator-only " } else { "" }
                )),
                [_, key_name] => match keybinds::parse_key(key_name) {
                    Some(key) => {
                        let binding = if spectator {
                            self.bindings.get_spectator(key)
                        } else {
                            self.bindings.get(key)
                        }
                        .map(str::to_owned);
                        match binding {
                            Some(binding) => self.push_console_line(format!(
                                "^7\"{}\" = \"{}\"{}",
                                keybinds::key_name(key),
                                binding,
                                if spectator { " ^5[spectator]" } else { "" }
                            )),
                            None if spectator => self.push_console_line(format!(
                                "^7\"{}\" has no spectator override (inherits normal bind)",
                                keybinds::key_name(key)
                            )),
                            None => self.push_console_line(format!(
                                "^7\"{}\" is not bound",
                                keybinds::key_name(key)
                            )),
                        }
                    }
                    None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
                },
                [_, key_name, rest @ ..] => match keybinds::parse_key(key_name) {
                    Some(key) => {
                        if spectator {
                            self.bindings.set_spectator(key, rest.join(" "));
                        } else {
                            self.bindings.set(key, rest.join(" "));
                        }
                        self.refresh_bound_state();
                        self.mark_config_dirty();
                    }
                    None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
                },
                [] => unreachable!(),
            }
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("unbind") || verb.eq_ignore_ascii_case("unbindspec") {
            let spectator = verb.eq_ignore_ascii_case("unbindspec");
            let label = if spectator { "unbindspec" } else { "unbind" };
            if words.len() != 2 {
                self.push_console_line(format!(
                    "^3{label} <key>^7 - remove a {}key binding",
                    if spectator { "spectator-only " } else { "" }
                ));
            } else if let Some(key) = keybinds::parse_key(&words[1]) {
                if spectator {
                    self.bindings.unbind_spectator(key);
                } else {
                    self.bindings.unbind(key);
                }
                self.refresh_bound_state();
                self.mark_config_dirty();
            } else {
                self.push_console_line(format!("^1\"{}\" isn't a valid key", words[1]));
            }
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("unbindall") || verb.eq_ignore_ascii_case("unbindspecall") {
            if verb.eq_ignore_ascii_case("unbindspecall") {
                self.bindings.clear_spectator();
            } else {
                self.bindings.clear();
            }
            self.refresh_bound_state();
            self.mark_config_dirty();
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("bindlist") || verb.eq_ignore_ascii_case("bindspeclist") {
            let spectator = verb.eq_ignore_ascii_case("bindspeclist");
            let entries = if spectator {
                self.bindings.spectator_sorted()
            } else {
                self.bindings.sorted()
            };
            for (key, binding) in entries {
                self.push_console_line(format!(
                    "^7{} \"{}\"{}",
                    keybinds::key_name(key),
                    binding,
                    if spectator { " ^5[spectator]" } else { "" }
                ));
            }
            return true;
        }
        false
    }

    /// Keyboard for the Game-side menus. egui owns navigation and activation;
    /// only the two things egui cannot do land here: backing out of the menu,
    /// and capturing a raw key/button while rebinding a control.
    pub(in crate::app) fn handle_game_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.controls_page_active() && self.controls_waiting_for_key {
            if event.repeat {
                return;
            }
            if code == KeyCode::Escape {
                self.controls_waiting_for_key = false;
                self.egui_repaint_requested = true;
                self.publish_ui();
            } else {
                self.bind_control_key(keybinds::bind_key_for_code(code));
            }
            return;
        }

        if code != KeyCode::Escape {
            return;
        }

        if self.front_end {
            self.frontend_back();
        } else {
            self.set_overlay(OverlayMode::None);
        }
    }
}
