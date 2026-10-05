//! Shell.
use crate::app::egui_menu::{
    confirmation_dialog, rail_group, rail_item, theme, App, ApplyVideoPath, Duration,
    EguiRenderData, ElementState, FrontendPage, Instant, KeyCode, KeyEvent, MapEditor, MouseButton,
    OverlayMode, PhysicalKey, RenderCommand, VideoSection, Window, WindowEvent, CONTENT_MAX_W,
    GUTTER, PROFILE_COSMETICS, PROFILE_FORCE, PROFILE_IDENTITY, PROFILE_MODEL, PROFILE_SABER,
    SETUP_TABS, SETUP_TAB_AUDIO, SETUP_TAB_CAMERA, SETUP_TAB_GAME, SETUP_TAB_INTERFACE,
    SETUP_TAB_NETWORK, SETUP_TAB_VIDEO, TOP_CONTROLS, TOP_ITEMS, TOP_PROFILE, TOP_RESUME,
    TOP_SETUP,
};

impl App {
    pub(in crate::app) fn reset_egui_for_window(&mut self, window: &Window) {
        self.egui_ctx = egui::Context::default();
        theme::apply(&self.egui_ctx);
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        ));
        self.egui_last_frame = Instant::now() - Duration::from_millis(100);
        self.egui_repaint_requested = true;
        self.egui_renderer_active = false;
        self.egui_apply_video_requested = false;
        self.map_edit_toolbar_rect = None;
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
    }

    /// egui owns every menu overlay, not just the Video page.
    pub(in crate::app) fn egui_menu_active(&self) -> bool {
        let blocking_download_ui = self.server_password_prompt.is_some()
            || self.missing_map_prompt.is_some()
            || self.demo_missing_map_prompt.is_some()
            || self.live_download.is_some()
            || self.live_join_ui.is_some();
        let asset_viewer_behind_console = self.front_end
            && self.frontend_page == FrontendPage::AssetViewer
            && self.overlay == OverlayMode::Console;
        (blocking_download_ui
            || asset_viewer_behind_console
            || matches!(
                self.overlay,
                OverlayMode::Game
                    | OverlayMode::Video
                    | OverlayMode::Vgs
                    | OverlayMode::HudEdit
                    | OverlayMode::CameraEdit
                    | OverlayMode::MapEdit
                    | OverlayMode::EntityGraph
                    | OverlayMode::Trace
                    | OverlayMode::StrafeTrails
                    | OverlayMode::RaceGhosts
            ))
            && self.video_confirmation.is_none()
            && (blocking_download_ui || self.loading.is_none())
            && !self.video.skip_ui
            && self.window.is_some()
            && self.render.is_some()
    }

    /// Whether an egui frame needs to run at all this tick: either a real menu
    /// overlay (`egui_menu_active`) or just the passive `r_drawEntities` label
    /// painter, which must coexist with live gameplay (no menu, no stolen
    /// input) rather than opening anything.
    pub(in crate::app) fn egui_paint_needed(&self) -> bool {
        self.egui_menu_active()
            || (self.video.draw_entities
                && self.video_confirmation.is_none()
                && self.loading.is_none()
                && !self.video.skip_ui
                && self.window.is_some()
                && self.render.is_some())
    }

    /// The post-apply countdown suspends the menu but still owns the pointer.
    pub(in crate::app::egui_menu) fn egui_confirmation_active(&self) -> bool {
        self.video_confirmation.is_some()
            && !self.video.skip_ui
            && self.window.is_some()
            && self.render.is_some()
    }

    /// Feed winit events to egui. Returns `true` when egui has taken the event.
    ///
    /// A few keys stay application-global (console, screenshot, back-out), and
    /// control rebinding needs the raw winit key/button rather than egui's
    /// logical key, so those events are handed back to the legacy handlers.
    pub(in crate::app) fn route_egui_window_event(&mut self, event: &WindowEvent) -> bool {
        let confirmation = self.egui_confirmation_active();
        // Asset Viewer remains rendered behind the legacy console, but the
        // console must retain all keyboard/mouse input while it is open.
        if !confirmation && self.overlay == OverlayMode::Console {
            return false;
        }
        if !confirmation && !self.egui_menu_active() {
            return false;
        }
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let Some(state) = self.egui_state.as_mut() else {
            return false;
        };

        // The countdown dialog is pointer-driven in egui, but its Enter/Esc/Y/N
        // shortcuts stay with the legacy handler that owns the timer.
        if confirmation {
            if matches!(event, WindowEvent::KeyboardInput { .. }) {
                return false;
            }
            let response = state.on_window_event(window, event);
            self.egui_repaint_requested |= response.repaint;
            return response.consumed;
        }

        // VGS keyboard ownership is the TaystJK ownerdraw hotkey table rather
        // than egui focus/navigation. Feed every key back to the raw handler;
        // pointer events still belong to the egui-rendered menu.
        if self.overlay == OverlayMode::Vgs && matches!(event, WindowEvent::KeyboardInput { .. }) {
            return false;
        }

        // While capturing a bind, the raw event must reach `bind_control_key`
        // untouched; egui never sees it so it cannot steal focus mid-capture.
        if self.controls_waiting_for_key
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
            )
        {
            return false;
        }

        // Ctrl+C in the Trace popup is an application shortcut: it exports the
        // complete selected diagnostic record, not just selectable egui text.
        // Keep it out of egui so its generic copy handling cannot swallow (or
        // later overwrite) the clipboard payload before `window_event` handles it.
        if self.overlay == OverlayMode::Trace
            && self.modifiers.control_key()
            && matches!(
                event,
                WindowEvent::KeyboardInput {
                    event: KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::KeyC),
                        ..
                    },
                    ..
                }
            )
        {
            return false;
        }

        let response = state.on_window_event(window, event);
        self.egui_repaint_requested |= response.repaint;

        // `egui_winit::Response::consumed` is not a reliable hit-test for every
        // pointer-button event. In Map Edit that used to let a toolbar press
        // fall through and start the 3D brush picker underneath (the giveaway
        // was clicking SAVE and seeing "No brush under cursor"). Own presses
        // inside the toolbar explicitly; releases may still reach the raw path
        // so a viewport drag ending over the toolbar can finish cleanly.
        if self.overlay == OverlayMode::MapEdit
            && matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    ..
                }
            )
        {
            let pixels_per_point = self.egui_ctx.pixels_per_point().max(0.001);
            let pointer = egui::pos2(
                self.cursor_position.0 as f32 / pixels_per_point,
                self.cursor_position.1 as f32 / pixels_per_point,
            );
            if self
                .map_edit_toolbar_rect
                .is_some_and(|rect| rect.expand(2.0).contains(pointer))
            {
                return true;
            }
        }

        if self.overlay == OverlayMode::MapEdit
            && matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                }
            )
            && self.map_editor.as_ref().is_some_and(MapEditor::is_dragging)
        {
            // A drag that began in the viewport still belongs to the editor if
            // the cursor happens to cross the toolbar before LMB is released.
            return false;
        }

        if let WindowEvent::KeyboardInput { event, .. } = event {
            let PhysicalKey::Code(code) = event.physical_key else {
                return true;
            };
            if code == KeyCode::Escape
                && (self.server_password_prompt.is_some()
                    || self.missing_map_prompt.is_some()
                    || self.demo_missing_map_prompt.is_some()
                    || self.live_download.is_some()
                    || self.live_join_ui.is_some())
            {
                // The missing-map/download flow is modal. Do not let the
                // ordinary in-game Escape handler obscure it.
                return true;
            }
            if matches!(
                code,
                KeyCode::PrintScreen | KeyCode::Backquote | KeyCode::Escape
            ) {
                return false;
            }
            // The key that opened the Trace popup closes it too (same toggle
            // `trace_surface_center` implements), so it has to reach the raw
            // keybind dispatch instead of being swallowed here like any other
            // key while a menu is "open".
            if self.overlay == OverlayMode::Trace
                && event.state == ElementState::Pressed
                && !event.repeat
                && self.key_bound_to_trace(code)
            {
                return false;
            }
            // Everything else belongs to egui while a menu is open.
            return true;
        }

        response.consumed
    }

    pub(in crate::app) fn tick_egui_menu(&mut self) {
        self.poll_server_browser_events();
        self.poll_ui_catalog();
        if self.tick_egui_confirmation() {
            return;
        }
        if !self.egui_paint_needed() {
            if self.egui_renderer_active {
                self.render_command(RenderCommand::SetEgui(None));
                self.egui_renderer_active = false;
            }
            return;
        }

        // The render thread reuses the most recent tessellated UI every frame.
        // Rebuilding egui at ~120 Hz keeps interaction fluid without coupling
        // the main/event thread to a 300-1000 FPS uncapped renderer.
        if !self.egui_repaint_requested && self.egui_last_frame.elapsed() < Duration::from_millis(8)
        {
            return;
        }
        self.run_egui_menu_frame();
    }

    pub(in crate::app::egui_menu) fn run_egui_menu_frame(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(mut state) = self.egui_state.take() else {
            return;
        };

        // Zoom has to be settled before `take_egui_input`, which derives the
        // frame's screen rect from the context's points-per-pixel.
        let ctx = self.egui_ctx.clone();
        let size = window.inner_size();
        let zoom = theme::zoom_for_height(size.height as f32 / window.scale_factor() as f32);
        if (ctx.zoom_factor() - zoom).abs() > 0.001 {
            ctx.set_zoom_factor(zoom);
        }
        let raw_input = state.take_egui_input(&window);
        let egui::FullOutput {
            platform_output,
            mut textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = {
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Main,
                crate::thread_activity::Task::UiBuild,
            );
            ctx.run_ui(raw_input, |ui| self.build_egui_menu(ui))
        };
        state.handle_platform_output(&window, platform_output);
        self.egui_state = Some(state);

        // Applying display/backend changes recreates both the wgpu renderer and
        // egui state. Defer that until Context::run has fully returned so this
        // frame cannot overwrite the freshly-reset egui integration.
        let mut after_paint = None;
        if std::mem::take(&mut self.egui_apply_video_requested) {
            match self.begin_apply_video_settings() {
                Some(ApplyVideoPath::RestartRenderer) => {
                    // The current egui context/renderer are about to be discarded, so
                    // these deltas intentionally have no target. Clear them explicitly
                    // before dropping to satisfy egui's integration contract.
                    textures_delta.clear();
                    self.restart_renderer_internal(true);
                    return;
                }
                // Live display and map-data changes keep the renderer and egui
                // alive, so this frame must still be delivered; apply after it.
                path @ (Some(ApplyVideoPath::LiveDisplay) | Some(ApplyVideoPath::ReprepareMap)) => {
                    after_paint = path;
                }
                None => {}
            }
        }

        let paint_jobs = {
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Main,
                crate::thread_activity::Task::UiTessellate,
            );
            ctx.tessellate(shapes, pixels_per_point)
        };
        self.render_command(RenderCommand::SetEgui(Some(EguiRenderData {
            paint_jobs,
            textures_delta,
            pixels_per_point,
        })));
        self.egui_renderer_active = true;
        self.egui_repaint_requested = false;
        self.egui_last_frame = Instant::now();
        match after_paint {
            Some(ApplyVideoPath::LiveDisplay) => self.apply_display_live(true),
            Some(ApplyVideoPath::ReprepareMap) => self.reprepare_map_in_place(),
            _ => {}
        }
    }

    // ------------------------------------------------------------- chrome --

    pub(in crate::app::egui_menu) fn build_egui_menu(&mut self, ui: &mut egui::Ui) {
        if self.video.draw_entities {
            self.egui_entity_labels(ui);
        }
        if !self.egui_menu_active() {
            // Only the label painter wanted this frame; no menu to open.
            return;
        }
        if self.server_password_prompt.is_some() {
            self.egui_server_password_dialog(ui);
            return;
        }
        if self.missing_map_prompt.is_some() {
            self.egui_missing_map_dialog(ui);
            return;
        }
        if self.demo_missing_map_prompt.is_some() {
            self.egui_demo_missing_map_dialog(ui);
            return;
        }
        if self.live_join_ui.is_some() {
            self.egui_live_join_pipeline(ui);
            return;
        }
        if self.live_download.is_some() {
            self.egui_download_dialog(ui);
            return;
        }
        if self.overlay == OverlayMode::Vgs {
            self.egui_vgs_menu(ui);
            return;
        }
        if self.overlay == OverlayMode::HudEdit {
            self.egui_hud_editor(ui);
            return;
        }
        if self.overlay == OverlayMode::CameraEdit {
            self.egui_camera_editor(ui);
            return;
        }
        if self.overlay == OverlayMode::MapEdit {
            self.egui_map_editor(ui);
            return;
        }
        if self.overlay == OverlayMode::EntityGraph {
            self.egui_entity_graph(ui);
            return;
        }
        if self.overlay == OverlayMode::Trace {
            self.egui_trace_menu(ui);
            return;
        }
        if self.overlay == OverlayMode::StrafeTrails {
            self.egui_strafe_trails(ui);
            return;
        }
        if self.overlay == OverlayMode::RaceGhosts {
            self.egui_race_ghosts(ui);
            return;
        }
        if self.front_end {
            self.build_egui_frontend_menu(ui);
            return;
        }
        self.egui_top_bar(ui);
        if self.overlay == OverlayMode::Video {
            self.egui_setup_tabs(ui);
        }
        self.egui_footer(ui);
        self.egui_body(ui);
    }

    pub(in crate::app::egui_menu) fn egui_vgs_menu(&mut self, root: &mut egui::Ui) {
        let menu = self.vgs_menu;
        let mut action = None;
        let area = egui::Area::new(egui::Id::new("tayst_vgs"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(18.0, 28.0))
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(250.0);
                        theme::page_title(
                            ui,
                            "VGS",
                            if menu == crate::vgs::Menu::Main {
                                ""
                            } else {
                                menu.title()
                            },
                        );
                        for item in menu.items() {
                            if theme::ghost_button(ui, item.label).clicked() {
                                action = Some(item.action);
                            }
                            ui.add_space(4.0);
                        }
                    });
            });

        // TaystJK's menuDef uses outOfBoundsClick and closes the entire VGS
        // menu, just like Escape. Preserve that rather than inventing a back
        // stack for nested groups.
        let clicked_outside = root.ctx().input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !area.response.rect.contains(pos))
        });

        if let Some(action) = action {
            self.activate_vgs_action(action);
        } else if clicked_outside {
            self.vgs_menu = crate::vgs::Menu::Main;
            self.set_overlay(OverlayMode::None);
        }
    }

    pub(in crate::app::egui_menu) fn egui_top_bar(&mut self, root: &mut egui::Ui) {
        let selected = if self.overlay == OverlayMode::Video {
            TOP_SETUP
        } else {
            self.menu_selected
        };
        egui::Panel::top("jka_menu_top")
            .exact_size(theme::TOP_BAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show_inside(root, |ui| {
                let width = ui.available_width();
                let item_w = width / TOP_ITEMS.len() as f32;
                let mut clicked = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, item) in TOP_ITEMS.iter().enumerate() {
                        if theme::nav_item(
                            ui,
                            item,
                            13.5,
                            item_w,
                            theme::TOP_BAR_H,
                            index == selected,
                        )
                        .clicked()
                        {
                            clicked = Some(index);
                        }
                    }
                });
                let rect = ui.max_rect();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.left(), rect.bottom() - 1.0),
                        egui::vec2(rect.width(), 1.0),
                    ),
                    egui::CornerRadius::ZERO,
                    theme::LINE,
                );
                if let Some(index) = clicked {
                    self.select_top_menu(index);
                }
            });
    }

    pub(in crate::app::egui_menu) fn select_top_menu(&mut self, index: usize) {
        self.controls_waiting_for_key = false;
        if index == TOP_SETUP {
            self.menu_selected = TOP_SETUP;
            if self.overlay != OverlayMode::Video {
                self.setup_selected = SETUP_TAB_VIDEO;
                self.set_overlay(OverlayMode::Video);
                return;
            }
        } else {
            self.menu_selected = index;
            if self.overlay != OverlayMode::Game {
                self.set_overlay(OverlayMode::Game);
                return;
            }
        }
        self.sync_profile_preview_mode(
            !self.front_end
                && self.overlay == OverlayMode::Game
                && self.menu_selected == TOP_PROFILE,
        );
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    /// `uipage <name> [tab]`: jump straight to a menu page. A scripting aid
    /// for screenshotting every page (`+uipage setup audio +screenshot`).
    pub(in crate::app) fn open_ui_page(&mut self, args: &[&str]) {
        let name = args.first().map(|word| word.to_ascii_lowercase());
        let tab = args.get(1).map(|word| word.to_ascii_lowercase());
        self.controls_waiting_for_key = false;
        if self.front_end {
            let page = match name.as_deref() {
                Some("main") => FrontendPage::Main,
                Some("play") => FrontendPage::Play,
                Some("servers") => FrontendPage::ServerBrowser,
                Some("solo") => FrontendPage::SoloGame,
                Some("demo") => FrontendPage::PlayDemo,
                Some("profile") => {
                    self.profile_selected_section = match tab.as_deref() {
                        Some("model") => PROFILE_MODEL,
                        Some("force") => PROFILE_FORCE,
                        Some("saber") => PROFILE_SABER,
                        Some("cosmetics") => PROFILE_COSMETICS,
                        _ => PROFILE_IDENTITY,
                    };
                    self.profile_preview_key = None;
                    FrontendPage::Profile
                }
                Some("controls") => FrontendPage::Controls,
                Some("chatlogs") | Some("chatlog") => FrontendPage::ChatLogs,
                Some("devtools") => FrontendPage::DeveloperTools,
                Some("assets") => FrontendPage::AssetViewer,
                Some("screenshots") | Some("shots") => FrontendPage::Screenshots,
                Some("maps") => FrontendPage::MapViewer,
                Some("setup") => {
                    self.setup_selected = Self::setup_tab_index(tab.as_deref());
                    self.set_overlay(OverlayMode::Video);
                    return;
                }
                _ => {
                    self.console_status =
                        "USAGE: uipage main|play|servers|solo|demo|profile|controls|chatlogs|devtools|assets|screenshots|maps|setup [tab]".into();
                    return;
                }
            };
            self.frontend_page = page;
            self.set_overlay(OverlayMode::Game);
            return;
        }
        match name.as_deref() {
            Some("setup") => {
                self.menu_selected = TOP_SETUP;
                self.setup_selected = Self::setup_tab_index(tab.as_deref());
                self.set_overlay(OverlayMode::Video);
            }
            Some(page @ ("resume" | "servers" | "profile" | "controls" | "vote" | "mod")) => {
                if page == "profile" {
                    self.profile_selected_section = match tab.as_deref() {
                        Some("model") => PROFILE_MODEL,
                        Some("force") => PROFILE_FORCE,
                        Some("saber") => PROFILE_SABER,
                        Some("cosmetics") => PROFILE_COSMETICS,
                        _ => PROFILE_IDENTITY,
                    };
                    self.profile_preview_key = None;
                }
                self.menu_selected = match page {
                    "resume" => TOP_RESUME,
                    "servers" => 1,
                    "profile" => TOP_PROFILE,
                    "controls" => TOP_CONTROLS,
                    "vote" => 5,
                    _ => 6,
                };
                self.set_overlay(OverlayMode::Game);
            }
            _ => {
                self.console_status =
                    "USAGE: uipage resume|servers|profile|controls|vote|mod|setup [game|camera|video|audio|network|interface]".into();
            }
        }
    }

    pub(in crate::app::egui_menu) fn setup_tab_index(tab: Option<&str>) -> usize {
        tab.and_then(|tab| {
            SETUP_TABS
                .iter()
                .position(|name| name.eq_ignore_ascii_case(tab))
        })
        .unwrap_or(SETUP_TAB_VIDEO)
    }

    pub(in crate::app) fn remembered_in_game_menu_overlay(&mut self) -> OverlayMode {
        // All top-level destinations share the same remembered selection.
        // Setup is the one exception in rendering only: its body lives in the
        // Video overlay, so reopening it must not route through egui_page()'s
        // normal Game-page switch.
        if self.menu_selected == TOP_SETUP {
            return OverlayMode::Video;
        }
        if self.menu_selected >= TOP_ITEMS.len() {
            self.menu_selected = TOP_RESUME;
        }
        OverlayMode::Game
    }

    pub(in crate::app::egui_menu) fn egui_setup_tabs(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("jka_setup_tabs")
            .exact_size(theme::TAB_BAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE_ALT)
                    .inner_margin(egui::Margin::symmetric(0, 0)),
            )
            .show_inside(root, |ui| {
                let mut clicked = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // `nav_item` centres its label, so back off half a label's
                    // worth of padding to land the first tab's text on GUTTER.
                    ui.add_space(GUTTER - 10.0);
                    for (index, tab) in SETUP_TABS.iter().enumerate() {
                        let width = 20.0 + tab.len() as f32 * 9.0;
                        if theme::nav_item(
                            ui,
                            tab,
                            12.5,
                            width,
                            theme::TAB_BAR_H,
                            index == self.setup_selected,
                        )
                        .clicked()
                        {
                            clicked = Some(index);
                        }
                    }
                });
                let rect = ui.max_rect();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.left(), rect.bottom() - 1.0),
                        egui::vec2(rect.width(), 1.0),
                    ),
                    egui::CornerRadius::ZERO,
                    theme::LINE,
                );
                if let Some(index) = clicked {
                    if self.setup_selected != index {
                        self.setup_selected = index;
                        self.egui_repaint_requested = true;
                        self.publish_ui();
                    }
                }
            });
    }

    pub(in crate::app::egui_menu) fn egui_footer(&mut self, root: &mut egui::Ui) {
        let on_video = self.overlay == OverlayMode::Video && self.setup_selected == SETUP_TAB_VIDEO;
        let restart = on_video && self.video_restart_required();
        let pending_video_changes = if restart {
            self.pending_video_changes()
        } else {
            Vec::new()
        };
        debug_assert_eq!(restart, !pending_video_changes.is_empty());
        // Only prepared map data is staged: Apply reloads the map in place and
        // leaves the renderer and window alone.
        let map_only = restart && !self.display_restart_required() && self.map_reprep_wanted();
        egui::Panel::bottom("jka_menu_footer")
            .exact_size(theme::FOOTER_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(0, 0)),
            )
            .show_inside(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), 1.0)),
                    egui::CornerRadius::ZERO,
                    theme::LINE,
                );
                ui.horizontal_centered(|ui| {
                    ui.add_space(GUTTER);
                    let hint = if map_only {
                        "Map changes are staged. Apply Video Settings to reload the map."
                    } else if restart {
                        "Renderer changes are staged. Apply Video Settings to rebuild required resources."
                    } else if self.front_end {
                        "ESC returns to the main menu.  ` opens the console."
                    } else if on_video {
                        "Settings apply immediately.  ESC closes Setup."
                    } else {
                        "ESC goes back.  ` opens the console."
                    };
                    theme::label(
                        ui,
                        theme::plain(
                            hint,
                            11.5,
                            if restart { theme::WARNING } else { theme::TEXT_FAINT },
                        ),
                    );
                    if restart {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(GUTTER);
                            let button_text = format!(
                                "APPLY VIDEO SETTINGS ({})",
                                pending_video_changes.len()
                            );
                            let response = theme::primary_button(ui, &button_text).on_hover_ui(|ui| {
                                ui.set_max_width(420.0);
                                theme::glow_label(
                                    ui,
                                    &format!(
                                        "{} pending video {}",
                                        pending_video_changes.len(),
                                        if pending_video_changes.len() == 1 {
                                            "change"
                                        } else {
                                            "changes"
                                        }
                                    ),
                                    12.5,
                                    theme::WARNING,
                                );
                                ui.add_space(4.0);
                                for change in &pending_video_changes {
                                    theme::glow_label(
                                        ui,
                                        &format!("{} - {}", change.label, change.detail),
                                        11.5,
                                        theme::TEXT_DIM,
                                    );
                                }
                            });
                            if response.clicked() {
                                self.egui_apply_video_requested = true;
                            }
                        });
                    }
                });
            });
    }

    pub(in crate::app::egui_menu) fn egui_body(&mut self, root: &mut egui::Ui) {
        let on_video = self.overlay == OverlayMode::Video && self.setup_selected == SETUP_TAB_VIDEO;
        if on_video {
            self.egui_video_rail(root);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show_inside(root, |ui| {
                // Keep the Servers page at the same 1120-point canvas used by
                // the front-end browser so the two versions have identical
                // column geometry and interaction positions. Profile remains a
                // full-width special case for its renderer-owned preview.
                let server_page = self.overlay == OverlayMode::Game && self.menu_selected == 1;
                let profile_page =
                    self.overlay == OverlayMode::Game && self.menu_selected == TOP_PROFILE;
                let content_w = if server_page {
                    ui.available_width().min(1120.0)
                } else if profile_page {
                    ui.available_width()
                } else {
                    ui.available_width().min(CONTENT_MAX_W)
                };
                let left = if server_page {
                    ui.max_rect().center().x - content_w * 0.5
                } else {
                    ui.max_rect().left()
                };
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, ui.max_rect().top()),
                    egui::pos2(left + content_w, ui.max_rect().bottom()),
                );
                // The Video page draws straight over the live scene -- its
                // captions carry their own halo instead (see `theme::glow_text`).
                // Every other page sits on the renderer's blurred backdrop,
                // where a solid panel reads better.
                let profile_preview_page =
                    self.overlay == OverlayMode::Game && self.menu_selected == TOP_PROFILE;
                let frame = if on_video {
                    egui::Frame::new().fill(theme::VIDEO_SCRIM)
                } else if profile_preview_page {
                    // The right half is a renderer-owned 3D viewport; do not cover
                    // it with the normal opaque menu body. The Profile controls
                    // draw their own surface on the left.
                    egui::Frame::new().fill(egui::Color32::TRANSPARENT)
                } else {
                    egui::Frame::new()
                        .fill(theme::PANEL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                };
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("jka_menu_body")
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui| {
                        frame
                            .inner_margin(egui::Margin::symmetric(GUTTER as i8, 18))
                            .show(ui, |ui| {
                                ui.set_width(content_w - GUTTER * 2.0);
                                ui.set_min_height(ui.available_height());
                                self.egui_page(ui);
                            });
                    },
                );
            });
    }

    pub(in crate::app::egui_menu) fn egui_page(&mut self, ui: &mut egui::Ui) {
        if self.overlay == OverlayMode::Video {
            match self.setup_selected {
                SETUP_TAB_GAME => self.egui_game_settings(ui),
                SETUP_TAB_CAMERA => self.egui_camera_page(ui),
                SETUP_TAB_VIDEO => self.egui_video_page(ui),
                SETUP_TAB_AUDIO => self.egui_audio_page(ui),
                SETUP_TAB_NETWORK => self.egui_network_page(ui),
                SETUP_TAB_INTERFACE => self.egui_interface_page(ui),
                _ => unreachable!("setup tab index is bounded by SETUP_TABS"),
            }
            return;
        }

        match self.menu_selected {
            TOP_RESUME => self.egui_resume_page(ui),
            1 => self.egui_server_browser_page(ui),
            TOP_PROFILE => self.egui_profile_page(ui),
            TOP_CONTROLS => self.egui_controls_page(ui),
            5 => self.egui_vote_page(ui),
            6 => self.egui_mod_settings(ui),
            _ => {}
        }
    }

    // ---------------------------------------------------------- video rail --

    pub(in crate::app::egui_menu) fn egui_video_rail(&mut self, root: &mut egui::Ui) {
        let mut target = None;
        egui::Panel::left("jka_video_rail")
            .exact_size(theme::NAV_W)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::RAIL_FILL)
                    .inner_margin(egui::Margin::symmetric(0, 12)),
            )
            .show_inside(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.right() - 1.0, rect.top()),
                        egui::vec2(1.0, rect.height()),
                    ),
                    egui::CornerRadius::ZERO,
                    theme::LINE,
                );
                ui.spacing_mut().item_spacing.y = 0.0;
                for (title, entries) in [
                    ("RENDERING", &VideoSection::RENDERING[..]),
                    ("ENVIRONMENT", &VideoSection::ENVIRONMENT[..]),
                    ("TOOLS", &VideoSection::TOOLS[..]),
                ] {
                    rail_group(ui, title);
                    for (section, label) in entries.iter().copied() {
                        if rail_item(ui, label, section == self.video_section).clicked() {
                            target = Some(section);
                        }
                    }
                    ui.add_space(10.0);
                }
            });
        if let Some(section) = target {
            if self.video_section != section {
                self.video_section = section;
                self.egui_repaint_requested = true;
                self.publish_ui();
            }
        }
    }

    // --------------------------------------------------------- confirmation --

    /// The post-apply "keep these settings?" countdown. Drawn as its own egui
    /// frame because the rest of the menu is suspended while it is up.
    pub(in crate::app::egui_menu) fn tick_egui_confirmation(&mut self) -> bool {
        if !self.egui_confirmation_active() {
            return false;
        }
        let Some(seconds) = self.video_confirmation_seconds() else {
            return false;
        };
        let Some(window) = self.window.clone() else {
            return false;
        };
        let Some(mut state) = self.egui_state.take() else {
            return false;
        };
        if !self.egui_repaint_requested
            && self.egui_last_frame.elapsed() < Duration::from_millis(33)
        {
            self.egui_state = Some(state);
            return true;
        }

        let raw_input = state.take_egui_input(&window);
        let ctx = self.egui_ctx.clone();
        let mut keep = None;
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = ctx.run_ui(raw_input, |ui| {
            keep = confirmation_dialog(ui.ctx(), seconds);
        });
        state.handle_platform_output(&window, platform_output);
        self.egui_state = Some(state);

        let paint_jobs = ctx.tessellate(shapes, pixels_per_point);
        self.render_command(RenderCommand::SetEgui(Some(EguiRenderData {
            paint_jobs,
            textures_delta,
            pixels_per_point,
        })));
        self.egui_renderer_active = true;
        self.egui_repaint_requested = false;
        self.egui_last_frame = Instant::now();

        match keep {
            Some(true) => self.confirm_video_settings(),
            Some(false) => self.revert_video_settings("REJECTED"),
            None => {}
        }
        true
    }
}
