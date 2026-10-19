//! Input.
use crate::app::{
    keybinds, App, Arc, BindKey, CursorGrabMode, DemoViewMode, ElementState, FrontendPage, HashSet,
    Instant, KeyCode, KeyEvent, MouseButton, OverlayMode, RenderCommand,
};

impl App {
    /// Connection/download dialogs are drawn even when the normal gameplay
    /// overlay is None (for example, after a server map change). They must own
    /// the OS pointer just like an ordinary egui menu does.
    pub(in crate::app) fn connection_modal_active(&self) -> bool {
        self.server_password_prompt.is_some()
            || self.missing_map_prompt.is_some()
            || self.demo_missing_map_prompt.is_some()
            || self.live_download.is_some()
            || self.live_join_ui.is_some()
    }

    pub(in crate::app) fn set_capture(&mut self, captured: bool) {
        // A focus regain, left click, or set_overlay(None) must not re-lock the
        // cursor while a map/download modal is open over live gameplay.
        let captured = captured
            && matches!(self.overlay, OverlayMode::None | OverlayMode::DevRadial | OverlayMode::EmoteRadial)
            && self.window_focused
            && !self.connection_modal_active();
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
        if captured {
            let grab_mode = if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
                Some("locked")
            } else if window.set_cursor_grab(CursorGrabMode::Confined).is_ok() {
                Some("confined")
            } else {
                None
            };
            if let Some(grab_mode) = grab_mode {
                window.set_cursor_visible(false);
                self.captured = true;
                self.mouse_grab_mode = grab_mode;
            } else if matches!(self.overlay, OverlayMode::DevRadial | OverlayMode::EmoteRadial) {
                // No cursor even if the platform cannot lock/confine it; raw
                // motion still drives the wheel while the window has focus.
                window.set_cursor_visible(false);
                self.captured = true;
                self.mouse_grab_mode = "none";
            } else {
                self.captured = false;
                self.mouse_grab_mode = "none";
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.captured = false;
            self.mouse_grab_mode = "none";
        }
    }

    /// Enter the single Profile experience. Both front-end and in-game routes
    /// use this same initialization; studio/old tab names are command aliases.
    pub(in crate::app) fn open_profile_studio(&mut self, tab: Option<&str>) {
        self.profile_selected_section = match tab {
            Some("force") => 2,
            Some("visibility") | Some("cosmetics") => 4,
            _ => 1, // Look includes sabers and the player name.
        };
        self.profile_studio_started = Instant::now();
        self.profile_studio_last_frame = Instant::now();
        self.profile_studio_spin_velocity = 0.0;
        self.profile_studio_head_look = [0.0; 2];
        self.profile_studio_world_anchors = [None; 7];
        self.profile_studio_cosmetic_group = 0;
        self.profile_preview_key = None;
    }

    pub(in crate::app) fn sync_profile_preview_mode(&mut self, active: bool) {
        if self.profile_preview_active == active {
            return;
        }
        // Only commit a selected model when the entire Profile menu closes,
        // not while clicking tiles, switching Profile sections, or pressing APPLY.
        // This route is shared by ESC, Back, top-level tabs, and uipage.
        if !active {
            if let Some(model) = self.profile_model_staged.take() {
                if !model.eq_ignore_ascii_case(&self.solo_client_info.model_cvar()) {
                    if let Err(error) = self.set_console_cvar("model", &model) {
                        self.console_status = error;
                    }
                }
            }
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
        let previous_overlay = self.overlay;
        let leaving_console =
            self.overlay == OverlayMode::Console && overlay != OverlayMode::Console;
        let toggling_console = self.overlay != overlay
            && (self.overlay == OverlayMode::Console || overlay == OverlayMode::Console);
        // TaystJK Con_ToggleConsole_f calls Con_ClearNotify before changing the
        // console catcher, so console command/output rows do not leak back onto
        // gameplay after opening or closing the solid console.
        if toggling_console {
            self.console_notify.clear();
            self.last_console_notify_refresh = Instant::now();
        }
        self.overlay = overlay;
        // Console is a temporary input/paint layer over its originating menu.
        // It must not close Profile (which clears the isolated character stage
        // and commits staged model changes), only the menu's actual exit does.
        if overlay != OverlayMode::Console {
            self.sync_profile_preview_mode(
                self.overlay == OverlayMode::Game
                    && if self.front_end {
                        self.frontend_page == FrontendPage::Profile
                    } else {
                        self.menu_selected == 2
                    },
            );
        }
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
        // The wheel is a non-modal overlay over gameplay: opening/closing it
        // must not clear physical keys, release +movement sources, reset the
        // solo command clock, or interrupt a locally held movement key.
        let wheel_gameplay_transition = matches!(
            (previous_overlay, overlay),
            (OverlayMode::None, OverlayMode::DevRadial | OverlayMode::EmoteRadial)
                | (OverlayMode::DevRadial | OverlayMode::EmoteRadial, OverlayMode::None)
        );
        if !wheel_gameplay_transition {
            if let Some(player) = &mut self.local_server {
                player.pause();
            }
        }
        if wheel_gameplay_transition {
            if matches!(overlay, OverlayMode::DevRadial | OverlayMode::EmoteRadial) {
                self.set_capture(true);
            } else if self.demo_playback_active() {
                self.sync_demo_camera_capture();
            } else {
                self.set_capture(true);
            }
        } else if overlay != OverlayMode::None {
            // Equivalent to TaystJK Key_ClearStates: menus/console must not
            // leave a gameplay kbutton latched if its physical release is
            // consumed by UI input routing.
            self.release_all_pressed_bindings();
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

    /// Open or close the `/dev` wheel without losing the overlay that owned
    /// input before it. When invoked in the console, return to what was under
    /// the console, not the console itself.
    pub(in crate::app) fn toggle_dev_radial(&mut self) {
        if self.overlay == OverlayMode::DevRadial {
            self.close_dev_radial();
            return;
        }
        // The Dev bind acts as a Back key while either inspection screen is
        // open. It dismisses that screen without opening a wheel underneath.
        match self.overlay {
            OverlayMode::Trace => {
                self.close_trace_overlay();
                return;
            }
            OverlayMode::EntityGraph | OverlayMode::ShaderRemap | OverlayMode::ModelPicker | OverlayMode::BmodelPicker | OverlayMode::MapMods => {
                self.set_overlay(OverlayMode::None);
                return;
            }
            _ => {}
        }
        self.dev_radial_return_overlay = if self.overlay == OverlayMode::Console {
            self.overlay_after_console()
        } else {
            self.overlay
        };
        // Keep Trace selected on open without letting a one-count mouse
        // twitch swing the wheel to a different sector. This is a virtual
        // stick anchor, not a visible cursor or an input dead zone.
        self.dev_radial_offset = [0.0, -0.28];
        self.set_overlay(OverlayMode::DevRadial);
        // Also capture when invoked from the console/menu, where a normal
        // overlay transition intentionally released gameplay input.
        self.set_capture(true);
    }

    pub(in crate::app) fn close_dev_radial(&mut self) {
        self.set_overlay(self.dev_radial_return_overlay);
    }

    pub(in crate::app) fn controls_page_active(&self) -> bool {
        self.overlay == OverlayMode::Game
            && if self.front_end {
                self.frontend_page == FrontendPage::Controls
            } else {
                self.menu_selected == 3
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
        let mut movement_keys = HashSet::with_capacity(8);
        for command in &self.live_buttons {
            match command.as_str() {
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
                _ => {}
            }
        }
        self.movement_keys = movement_keys;
        self.noclip_primary_down = self.live_buttons.contains("+attack");
        self.noclip_alt_down = self.live_buttons.contains("+altattack");
        self.scores_showing = self.live_buttons.contains("+scores");
    }

    fn sync_button_transitions(
        &mut self,
        was_scores_showing: bool,
        was_demo_ui_modifier: bool,
        was_zoomed: bool,
    ) {
        let is_zoomed = self.live_buttons.contains("+zoom");
        if is_zoomed != was_zoomed {
            // TaystJK CG_ZoomDown_f / CG_ZoomUp_f record cg.zoomTime when the
            // held +zoom state changes; keep the same transition epoch here.
            self.japro_zoom_transition_at = Some(Instant::now());
        }
        self.refresh_bound_state();
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

    /// Port of TaystJK/OpenJK IN_KeyDown/IN_KeyUp ownership semantics. The
    /// command buffer supplies an internal `@bind:<key>` source for physical
    /// binds; commands typed manually have no source and `-foo` clears all
    /// sources, matching IN_KeyUp's console-un-sticking behavior.
    pub(in crate::app) fn execute_input_button_command(&mut self, command: &str) -> bool {
        let mut words = command.split_whitespace();
        let Some(raw_verb) = words.next() else {
            return false;
        };
        let verb = raw_verb.to_ascii_lowercase();
        let Some(stem) = verb.strip_prefix('+').or_else(|| verb.strip_prefix('-')) else {
            return false;
        };
        if !keybinds::is_supported_button_stem(stem) {
            return false;
        }
        let is_down = verb.starts_with('+');
        let plus_verb = format!("+{stem}");
        let source = words
            .find(|word| word.starts_with("@bind:"))
            .map(str::to_owned);

        // TaystJK CG_ZoomDown/Up: while Disruptor alternate singlefire
        // is selected, +zoom is a held altattack, not an FOV change.
        if plus_verb == "+zoom" {
            let redirect_source = format!("@bind:disruptorzoom:{}", source.as_deref().unwrap_or("@manual"));
            if !is_down {
                self.execute_input_button_command(&format!("-altattack {redirect_source}"));
            } else if self.live_input.single_fire_alt
                && self.current_player_state().and_then(|ps| ps.field_i32("weapon")) == Some(6)
            {
                self.execute_input_button_command(&format!("+altattack {redirect_source}"));
                return true;
            }
        }

        // Demo mouse attack bindings are POV controls in this client. Keep the
        // behavior, but execute it at Cbuf time so `wait` also orders it.
        if self.demo_playback_active() && is_down {
            let follow_direction = if plus_verb == "+attack" {
                Some(1)
            } else if plus_verb == "+altattack" {
                Some(-1)
            } else {
                None
            };
            if let Some(direction) = follow_direction {
                if !self.live_buttons.contains("+speed") {
                    self.cycle_demo_follow(direction);
                }
                return true;
            }
        }

        let was_scores_showing = self.scores_showing;
        let was_demo_ui_modifier = self.live_buttons.contains("+speed");
        let was_zoomed = self.live_buttons.contains("+zoom");
        let was_active = self.live_buttons.contains(&plus_verb);

        if is_down {
            let source = source.unwrap_or_else(|| "@manual".to_owned());
            let inserted = self
                .live_button_sources
                .entry(plus_verb.clone())
                .or_default()
                .insert(source);
            if !inserted {
                return true;
            }
            if !was_active {
                self.live_buttons.insert(plus_verb.clone());
                // IN_KeyDown wasPressed: a tap shorter than a frame still fires.
                self.live_input.note_pressed(&plus_verb);
                if let Some(player) = &mut self.local_server {
                    if let Some(key) = keybinds::canonical_movement_key(&plus_verb) {
                        player.note_movement_key_press(key);
                    }
                }
                if plus_verb == "+moveup"
                    && self.demo_playback_active()
                    && self.demo_view_mode != DemoViewMode::Free
                {
                    self.enter_demo_free_view();
                }
            }
        } else if let Some(source) = source {
            let mut remove_entry = false;
            if let Some(sources) = self.live_button_sources.get_mut(&plus_verb) {
                sources.remove(&source);
                remove_entry = sources.is_empty();
            }
            if remove_entry {
                self.live_button_sources.remove(&plus_verb);
                self.live_buttons.remove(&plus_verb);
            }
        } else {
            // TaystJK IN_KeyUp with no key argument is an explicit unstick.
            self.live_button_sources.remove(&plus_verb);
            self.live_buttons.remove(&plus_verb);
        }

        self.sync_button_transitions(was_scores_showing, was_demo_ui_modifier, was_zoomed);
        true
    }

    /// TaystJK CG_SingleFireDown/Up, CG_NorollDown/Up, and CG_GrappleDown/Up.
    /// All synthesized held buttons use separate bind owners so releasing a
    /// helper cannot release the player's actual attack/crouch/use key.
    pub(in crate::app) fn tayst_special_button(&mut self, command: &str) {
        let mut parts = command.split_whitespace();
        let Some(verb) = parts.next() else { return; };
        let verb = verb.to_ascii_lowercase();
        let source = parts.find(|part| part.starts_with("@bind:"))
            .unwrap_or("@manual");
        let kind = verb.trim_start_matches(['+', '-']);
        let owner = format!("@bind:{kind}:{source}");
        let down = verb.starts_with('+');
        match kind {
            "singlefire" => {
                if down {
                    let ps = self.current_player_state();
                    let disruptor = ps.and_then(|ps| ps.field_i32("weapon")) == Some(6);
                    let dead = ps.is_some_and(|ps| ps.stats[0] <= 0);
                    let button = if self.live_input.single_fire_alt && !disruptor && !dead {
                        "+altattack"
                    } else {
                        "+attack"
                    };
                    self.execute_input_button_command(&format!("{button} {owner}"));
                } else {
                    self.execute_input_button_command(&format!("-attack {owner}"));
                    self.execute_input_button_command(&format!("-altattack {owner}"));
                }
            }
            "duck" => {
                let restricted = self.japro_do_restricted() || self.japro_do_race_restricted();
                let commands = if down {
                    if restricted {
                        vec![format!("+speed {owner}"), "wait 2".into(),
                             "-moveup".into(), format!("+movedown {owner}"),
                             format!("-speed {owner}")]
                    } else {
                        vec![format!("+speed {owner}"), "-moveup".into(),
                             "wait 1".into(), format!("+movedown {owner}"),
                             format!("-speed {owner}")]
                    }
                } else if restricted {
                    vec![format!("-movedown {owner}"), format!("-speed {owner}")]
                } else {
                    vec!["wait 1".into(), format!("-movedown {owner}"),
                         format!("-speed {owner}")]
                };
                self.insert_console_commands_front(commands);
            }
            "grapple" => {
                self.execute_input_button_command(&format!("{}button12 {owner}", if down { "+" } else { "-" }));
                if !down && self.active_server_mod() == crate::net::mod_support::ServerMod::Japlus {
                    // JA+ release intentionally sends a brief USE pulse.
                    self.insert_console_commands_front(vec![
                        format!("+use {owner}"), "wait 1".into(), format!("-use {owner}"),
                    ]);
                }
            }
            _ => {}
        }
    }

    /// TaystJK CG_Lowjump_f: one cg frame of +moveup, or a two-frame
    /// legacy wait sequence on jaPRO race / RESTRICT_DO servers.
    pub(in crate::app) fn tayst_lowjump(&mut self) {
        let frames = if self.japro_do_restricted() || self.japro_do_race_restricted() { 2 } else { 1 };
        self.insert_console_commands_front(vec![
            "+moveup @bind:lowjump".into(), format!("wait {frames}"),
            "-moveup @bind:lowjump".into(),
        ]);
    }

    /// CG_ToggleSingleFire_f redirects a currently held singlefire binding
    /// to the other fire button without stealing independently held buttons.
    pub(in crate::app) fn redirect_held_singlefire(&mut self) {
        let sources = ["+attack", "+altattack"].into_iter()
            .filter_map(|button| self.live_button_sources.get(button))
            .flat_map(|owners| owners.iter())
            .filter(|source| source.starts_with("@bind:singlefire:"))
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let disruptor = self.current_player_state()
            .and_then(|ps| ps.field_i32("weapon")) == Some(6);
        let dead = self.current_player_state().is_some_and(|ps| ps.stats[0] <= 0);
        let next = if self.live_input.single_fire_alt && !disruptor && !dead {
            "+altattack"
        } else { "+attack" };
        for source in sources {
            self.execute_input_button_command(&format!("-attack {source}"));
            self.execute_input_button_command(&format!("-altattack {source}"));
            self.execute_input_button_command(&format!("{next} {source}"));
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

    /// `/emote` uses precisely the same press/release treatment as `/dev`.
    pub(in crate::app) fn toggle_emote_radial(&mut self) {
        if self.overlay == OverlayMode::EmoteRadial {
            self.close_emote_radial();
            return;
        }
        self.dev_radial_return_overlay = if self.overlay == OverlayMode::Console {
            self.overlay_after_console()
        } else {
            self.overlay
        };
        self.emote_radial_category = None;
        self.dev_radial_offset = [0.0, -0.28];
        self.set_overlay(OverlayMode::EmoteRadial);
        self.set_capture(true);
    }

    pub(in crate::app) fn close_emote_radial(&mut self) {
        self.emote_radial_category = None;
        self.set_overlay(self.dev_radial_return_overlay);
    }

    pub(in crate::app) fn key_bound_to_emote(&self, code: KeyCode) -> bool {
        self.bindings
            .get_resolved(keybinds::bind_key_for_code(code), self.spectator_binding_context())
            .is_some_and(|binding| keybinds::binding_contains_command(binding, "emote"))
    }

    pub(in crate::app) fn mouse_button_bound_to_emote(&self, button: MouseButton) -> bool {
        let number = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::Back => 4,
            MouseButton::Forward => 5,
            _ => return false,
        };
        self.bindings
            .get_resolved(BindKey::Mouse(number), self.spectator_binding_context())
            .is_some_and(|binding| keybinds::binding_contains_command(binding, "emote"))
    }

    /// The same key used to open `/dev` must be able to close it while egui
    /// owns the cursor; see `route_egui_window_event` and raw key dispatch.
    pub(in crate::app) fn key_bound_to_dev(&self, code: KeyCode) -> bool {
        self.bindings
            .get_resolved(
                keybinds::bind_key_for_code(code),
                self.spectator_binding_context(),
            )
            .is_some_and(|binding| keybinds::binding_contains_command(binding, "dev"))
    }

    /// Dev can also be bound to a mouse button; its *release* must not
    /// disappear into egui's normal button swallowing path.
    pub(in crate::app) fn mouse_button_bound_to_dev(&self, button: MouseButton) -> bool {
        let number = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::Back => 4,
            MouseButton::Forward => 5,
            _ => return false,
        };
        self.bindings
            .get_resolved(BindKey::Mouse(number), self.spectator_binding_context())
            .is_some_and(|binding| keybinds::binding_contains_command(binding, "dev"))
    }

    /// Keep held gameplay buttons responsive while the wheel is up. Unlike
    /// ordinary key dispatch, only '+' commands are allowed here: typing a
    /// non-button command while inspecting a wheel must not open another UI
    /// or activate an unintended tool.
    pub(in crate::app) fn handle_dev_wheel_gameplay_key(
        &mut self,
        code: KeyCode,
        state: ElementState,
        repeat: bool,
    ) {
        let key = keybinds::bind_key_for_code(code);
        if state == ElementState::Released {
            self.keys.remove(&code);
            self.execute_binding_release_from(key, BindKey::Keyboard(code));
            return;
        }
        self.keys.insert(code);
        if repeat {
            return;
        }
        let Some(binding) = self
            .bindings
            .get_resolved(key, self.spectator_binding_context())
            .map(str::to_owned)
        else {
            return;
        };
        let source = format!("@bind:key:{code:?}");
        let presses = keybinds::split_binding_commands(&binding)
            .into_iter()
            .filter(|command| {
                command.split_whitespace().next().is_some_and(|verb| verb.starts_with('+'))
            })
            .map(|command| format!("{command} {source}"))
            .collect();
        self.append_console_commands(presses);
    }

    pub(in crate::app) fn execute_binding_press(&mut self, key: BindKey) {
        self.execute_binding_press_from(key, key);
    }

    pub(in crate::app) fn execute_binding_press_from(
        &mut self,
        key: BindKey,
        source_key: BindKey,
    ) {
        let spectator_context = self.spectator_binding_context();
        let Some(binding) = self
            .bindings
            .get_resolved(key, spectator_context)
            .map(str::to_owned)
        else {
            return;
        };
        // Binding lookup intentionally normalizes left/right modifiers, but a
        // kbutton source must identify the physical key. Use Debug for keyboard
        // codes so Left/Right Ctrl/Shift/Alt remain independent sources.
        let source = match source_key {
            BindKey::Keyboard(code) => format!("@bind:key:{code:?}"),
            _ => format!("@bind:{}", keybinds::key_name(source_key)),
        };
        let mut buffered = Vec::new();
        for command in keybinds::split_binding_commands(&binding) {
            let verb = command.split_whitespace().next().unwrap_or("");
            if verb.starts_with('+') {
                buffered.push(format!("{command} {source}"));
            } else {
                buffered.push(command.to_owned());
            }
        }
        // TaystJK CL_ParseBinding appends the complete bind to Cbuf, including
        // +commands. That ordering is what makes `+speed; wait 2; +movedown`
        // activate crouch only after the wait has elapsed.
        self.append_console_commands(buffered);
    }

    pub(in crate::app) fn execute_binding_release(&mut self, key: BindKey) {
        self.execute_binding_release_from(key, key);
    }

    pub(in crate::app) fn execute_binding_release_from(
        &mut self,
        key: BindKey,
        source_key: BindKey,
    ) {
        let spectator_context = self.spectator_binding_context();
        let Some(binding) = self
            .bindings
            .get_resolved(key, spectator_context)
            .map(str::to_owned)
        else {
            return;
        };
        let source = match source_key {
            BindKey::Keyboard(code) => format!("@bind:key:{code:?}"),
            _ => format!("@bind:{}", keybinds::key_name(source_key)),
        };
        let releases = keybinds::split_binding_commands(&binding)
            .into_iter()
            .filter_map(|command| {
                let verb = command.split_whitespace().next().unwrap_or("");
                verb.strip_prefix('+')
                    .map(|stem| format!("-{stem} {source}"))
            })
            .collect();
        self.append_console_commands(releases);
    }

    pub(in crate::app) fn execute_binding_tap(&mut self, key: BindKey) {
        self.execute_binding_press(key);
        self.execute_binding_release(key);
    }

    pub(in crate::app) fn release_all_pressed_bindings(&mut self) {
        let keyboard: Vec<_> = self.keys.iter().copied().collect();
        let mouse: Vec<_> = self.mouse_buttons_down.iter().copied().collect();
        for code in keyboard {
            self.execute_binding_release_from(
                keybinds::bind_key_for_code(code),
                BindKey::Keyboard(code),
            );
        }
        for button in mouse {
            self.execute_binding_release(BindKey::Mouse(button));
        }
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
