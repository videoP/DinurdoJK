//! Frontend.
use crate::app::egui_menu::{
    menu_action, theme, App, FrontendPage, OverlayMode, RenderCommand, CONTENT_MAX_W,
    FRONTEND_SCENE_FADE_IN_SECS, GUTTER, SETUP_TAB_VIDEO,
};
use crate::app::frontend_scene_cache::{
    smoothstep01, FRONTEND_CHROME_FADE_IN_SECS, FRONTEND_STAGE_FADE_IN_SECS,
};

impl App {
    pub(in crate::app::egui_menu) fn build_egui_frontend_menu(&mut self, ui: &mut egui::Ui) {
        self.egui_frontend_scene_fade(ui);
        self.egui_frontend_header(ui);
        if self.egui_display_overlay() == OverlayMode::Video {
            self.egui_setup_tabs(ui);
            self.egui_footer(ui);
            self.egui_body(ui);
        } else {
            self.egui_frontend_footer(ui);
            self.egui_frontend_body(ui);
        }
        self.egui_disconnect_notice(ui);
        self.egui_frontend_2d_fade(ui);
    }

    /// Fade the 2D chrome in on the shared stage clock (see `FrontendStage`).
    /// The UI remains interactive as the scrim is just a paint layer, not a
    /// hit-testable Area or blocking loading overlay. Until the renderer has
    /// presented the first dark frame the scrim is fully opaque.
    fn egui_frontend_2d_fade(&self, ui: &egui::Ui) {
        // Background decoding/preparation stays asynchronous. A preferred-page
        // launch must never wait behind the scene's presentation clock, even
        // when navigating back to Main before the scene is ready.
        if self.frontend_skip_chrome_fade || self.frontend_page == FrontendPage::Main {
            return;
        }
        let ctx = ui.ctx();
        let now = std::time::Instant::now();
        let shown = self.frontend_stage.brightness(now, FRONTEND_CHROME_FADE_IN_SECS);
        if shown >= 1.0 { return; }
        let alpha = ((1.0 - shown) * 255.0).round() as u8;
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("frontend_2d_initial_fade_scrim"),
        ))
        .rect_filled(
            ctx.content_rect(),
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(alpha),
        );
        ctx.request_repaint();
    }

    /// jaPRO/stock JKA show a kick/ban/drop reason as a popup over the main menu
    /// (`com_errorMessage` -> `error_popmenu`) instead of only printing it to a
    /// console the player may not have open. This is the equivalent here.
    pub(in crate::app::egui_menu) fn egui_disconnect_notice(&mut self, root: &mut egui::Ui) {
        let Some(reason) = self.disconnect_notice.clone() else {
            return;
        };
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("disconnect_notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(480.0);
                        theme::page_title(ui, "DISCONNECTED", "The server closed the connection.");
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(&reason).color(theme::TEXT));
                        ui.add_space(16.0);
                        if theme::primary_button(ui, "OK").clicked() {
                            dismiss = true;
                        }
                    });
            });
        if dismiss {
            self.disconnect_notice = None;
        }
    }

    /// Paint the menu's 3D stage under the chrome: the cached photograph (while
    /// the live scene is not yet shown) and a black scrim over both.
    ///
    /// Two independent quantities, one clock each, never stacked:
    /// * `handoff` (live-3D readiness, `fade_started`) cross-fades the cached
    ///   image out. It is evaluated even when the scrim is fully opaque.
    /// * `darkness` is the startup brightness. With a cached image it follows the
    ///   presentation-driven stage clock; with none (cold start) the live scene
    ///   itself fades in from black on the same handoff.
    /// The scrim is painted after the image and the live scene, before the chrome.
    pub(in crate::app::egui_menu) fn egui_frontend_scene_fade(&self, root: &mut egui::Ui) {
        if matches!(
            self.frontend_page,
            FrontendPage::Profile | FrontendPage::AssetViewer | FrontendPage::Screenshots
        ) {
            return;
        }
        let now = std::time::Instant::now();
        let handoff = self
            .frontend_cinematic
            .and_then(|cinematic| cinematic.fade_started)
            .map_or(0.0, |started| {
                let started = self.frontend_visual_handoff_start(started);
                smoothstep01(now.saturating_duration_since(started).as_secs_f32()
                    / FRONTEND_SCENE_FADE_IN_SECS)
            });
        let image_expected =
            self.frontend_scene_texture.is_some() || self.frontend_stage.cache_pending;
        // Main displays the art-directed scene without artificial darkening.
        // Retain the cached still -> live-3D handoff, not the old black scrim.
        let darkness = if self.frontend_page == FrontendPage::Main {
            0.0
        } else if image_expected {
            1.0 - self.frontend_stage.brightness(now, FRONTEND_STAGE_FADE_IN_SECS)
        } else {
            1.0 - handoff
        };
        let image_alpha = 1.0 - handoff;
        let rect = root.max_rect();
        let mut painted_image = false;
        if let Some(image) = &self.frontend_scene_texture {
            if image_alpha > 0.001 {
                // Cover/crop, not stretch: resizing the window should never
                // deform the levelshot, even when the cached aspect differs.
                let size = image.size_vec2();
                let cover = (rect.width() / size.x).max(rect.height() / size.y);
                let visible = egui::vec2(
                    rect.width() / (size.x * cover),
                    rect.height() / (size.y * cover),
                );
                let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), visible);
                root.painter().image(
                    image.id(), rect, uv,
                    crate::renderer::egui_alpha_tint(image_alpha),
                );
                painted_image = true;
            }
        }
        if darkness > 0.001 {
            root.painter().rect_filled(
                rect, egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha((darkness * 255.0).round() as u8),
            );
        }
        if darkness > 0.001 || painted_image && handoff > 0.0 && handoff < 1.0 {
            root.ctx().request_repaint();
        }
    }

    pub(in crate::app::egui_menu) fn egui_frontend_header(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("jka_frontend_header")
            .exact_size(theme::TOP_BAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(GUTTER as i8, 0)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    if self.frontend_page != FrontendPage::Profile {
                        theme::glow_label(ui, "DINURDOJK", 16.0, theme::TEXT);
                        ui.add_space(14.0);
                    }
                    let page = if self.egui_display_overlay() == OverlayMode::Video {
                        "SETTINGS"
                    } else {
                        match self.frontend_page {
                            FrontendPage::Main => "MAIN MENU",
                            FrontendPage::Play => "PLAY",
                            FrontendPage::ServerBrowser => "SERVERS",
                            FrontendPage::SoloGame => "SOLO GAME",
                            FrontendPage::PlayDemo => "PLAY DEMO",
                            FrontendPage::Profile => "PROFILE",
                            FrontendPage::Controls => "CONTROLS",
                            FrontendPage::ChatLogs => "CHAT LOGS",
                            FrontendPage::DeveloperTools => "DEVELOPER TOOLS",
                            FrontendPage::AssetViewer => "ASSET VIEWER",
                            FrontendPage::Screenshots => "SCREENSHOTS",
                            FrontendPage::MapViewer => "MAP VIEWER",
                        }
                    };
                    theme::glow_label(ui, page, 12.5, theme::TEXT_DIM);
                    if self.egui_display_overlay() == OverlayMode::Video
                        || self.frontend_page != FrontendPage::Main
                    {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Keep clear of the FPS counter pinned to the
                            // top-right corner of the window.
                            ui.add_space(96.0);
                            if theme::ghost_button(ui, "BACK").clicked() {
                                self.frontend_back();
                            }
                        });
                    }
                });
            });
    }

    pub(in crate::app::egui_menu) fn egui_frontend_footer(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("jka_frontend_footer")
            .exact_size(theme::FOOTER_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(0, 0)),
            )
            .show(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), 1.0)),
                    egui::CornerRadius::ZERO,
                    theme::LINE,
                );
                ui.horizontal_centered(|ui| {
                    ui.add_space(GUTTER);
                    let hint = match self.frontend_page {
                        FrontendPage::Main => "Select an option.  ` opens the console.",
                        FrontendPage::Play => "ESC returns to the main menu.",
                        FrontendPage::ServerBrowser
                        | FrontendPage::SoloGame
                        | FrontendPage::PlayDemo => "ESC returns to Play.",
                        FrontendPage::Profile | FrontendPage::Controls | FrontendPage::ChatLogs => {
                            "ESC returns to the main menu."
                        }
                        FrontendPage::DeveloperTools => "ESC returns to the main menu.",
                        FrontendPage::AssetViewer | FrontendPage::MapViewer => {
                            "ESC returns to Developer Tools."
                        }
                        FrontendPage::Screenshots => "ESC returns to the main menu.",
                    };
                    theme::label(ui, theme::plain(hint, 11.5, theme::TEXT_FAINT));
                });
            });
    }

    pub(in crate::app::egui_menu) fn egui_frontend_body(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(root, |ui| {
                let max_w = if matches!(
                    self.frontend_page,
                    FrontendPage::ServerBrowser | FrontendPage::SoloGame | FrontendPage::PlayDemo
                ) {
                    1120.0
                } else if matches!(
                    self.frontend_page,
                    FrontendPage::Profile
                        | FrontendPage::ChatLogs
                        | FrontendPage::AssetViewer
                        | FrontendPage::Screenshots
                        | FrontendPage::MapViewer
                ) {
                    ui.available_width()
                } else {
                    CONTENT_MAX_W
                };
                let content_w = ui.available_width().min(max_w);
                let full = ui.max_rect();
                let left = full.center().x - content_w * 0.5;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, full.top()),
                    egui::pos2(left + content_w, full.bottom()),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("jka_frontend_body")
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui| {
                        let frame = if self.frontend_page == FrontendPage::Profile {
                            // Match the in-game Profile layout: the right half is a
                            // renderer-owned 3D viewport, while the controls pane
                            // paints its own opaque surface.
                            egui::Frame::new().fill(egui::Color32::TRANSPARENT)
                        } else {
                            egui::Frame::new()
                                .fill(
                                    if matches!(
                                        self.frontend_page,
                                        FrontendPage::AssetViewer | FrontendPage::Screenshots
                                    ) {
                                        egui::Color32::TRANSPARENT
                                    } else {
                                        theme::PANEL_FILL
                                    },
                                )
                                .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        };
                        frame
                            .inner_margin(egui::Margin::symmetric(GUTTER as i8, 18))
                            .show(ui, |ui| {
                                ui.set_width(content_w - GUTTER * 2.0);
                                ui.set_min_height(ui.available_height());
                                self.egui_frontend_page(ui);
                            });
                    },
                );
            });
    }

    pub(in crate::app::egui_menu) fn egui_frontend_page(&mut self, ui: &mut egui::Ui) {
        match self.frontend_page {
            FrontendPage::Main => self.egui_frontend_main_page(ui),
            FrontendPage::Play => self.egui_frontend_play_page(ui),
            FrontendPage::ServerBrowser => self.egui_server_browser_page(ui),
            FrontendPage::SoloGame => self.egui_solo_game_page(ui),
            FrontendPage::PlayDemo => self.egui_play_demo_page(ui),
            FrontendPage::Profile => self.egui_profile_studio(ui),
            FrontendPage::Controls => self.egui_controls_page(ui),
            FrontendPage::ChatLogs => self.egui_chat_log_browser_page(ui),
            FrontendPage::DeveloperTools => self.egui_developer_tools_page(ui),
            FrontendPage::AssetViewer => self.egui_asset_viewer_page(ui),
            FrontendPage::Screenshots => self.egui_screenshot_browser_page(ui),
            FrontendPage::MapViewer => self.egui_map_viewer_page(ui),
        }
    }

    pub(in crate::app::egui_menu) fn egui_frontend_main_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "MAIN MENU", "Choose where you want to go.");
        theme::section(ui, "DINURDOJK", "");
        if menu_action(
            ui,
            "Play",
            "Join a server, start a solo map, or play a demo.",
            true,
        ) {
            self.frontend_page = FrontendPage::Play;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Profile",
            "Customize your character, Force, sabers, and appearance.",
            true,
        ) {
            self.open_profile_studio(None);
            self.frontend_page = FrontendPage::Profile;
            self.set_overlay(OverlayMode::Game);
            return;
        }
        if menu_action(ui, "Controls", "Keyboard and mouse bindings.", true) {
            self.frontend_page = FrontendPage::Controls;
            self.controls_waiting_for_key = false;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Settings",
            "Use the same Setup pages available in game.",
            true,
        ) {
            self.setup_selected = SETUP_TAB_VIDEO;
            self.set_overlay(OverlayMode::Video);
            return;
        }
        if menu_action(
            ui,
            "Chat Logs",
            "Browse saved server chat by mod, server, date, message type, or text.",
            true,
        ) {
            self.frontend_page = FrontendPage::ChatLogs;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Screenshots",
            "Browse screenshots from base and every mod, inspect capture metadata, or return to a saved spot.",
            true,
        ) {
            self.frontend_page = FrontendPage::Screenshots;
            self.screenshot_catalog_loaded = false;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Developer Tools",
            "Browse and inspect game assets and source maps.",
            true,
        ) {
            self.frontend_page = FrontendPage::DeveloperTools;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Quit",
            "Exit DinurdoJK and return to the desktop.",
            true,
        ) {
            self.request_quit();
        }
    }

    pub(in crate::app::egui_menu) fn egui_frontend_play_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "PLAY", "Choose a game source.");
        theme::section(ui, "PLAY", "");
        if menu_action(
            ui,
            "Join a game",
            "Browse Internet, LAN, favorites, and recent servers.",
            true,
        ) {
            // The front-end browser uses its own page-navigation path.
            // Default to recent visits only when entering; column clicks remain sticky.
            self.server_browser.sort = crate::server_browser::BrowserSort::LastVisited;
            self.server_browser.sort_ascending = false;
            self.frontend_page = FrontendPage::ServerBrowser;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Solo Game",
            "Browse every compiled map on the active JKA asset path.",
            true,
        ) {
            self.frontend_page = FrontendPage::SoloGame;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Play Demo",
            "Browse Jedi Academy demo recordings.",
            true,
        ) {
            self.frontend_page = FrontendPage::PlayDemo;
            self.egui_repaint_requested = true;
            return;
        }
    }

    pub(in crate::app::egui_menu) fn egui_developer_tools_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "DEVELOPER TOOLS",
            "Inspect the active JKA virtual filesystem without leaving the client.",
        );
        theme::section(ui, "TOOLS", "");
        if menu_action(
            ui,
            "Asset Viewer",
            "Browse MD3, GLM, EFX and .shader assets with search, folder typeahead, preview controls, and inspection stats.",
            true,
        ) {
            self.frontend_page = FrontendPage::AssetViewer;
            self.render_command(RenderCommand::SetAssetPreviewBackground(None));
            self.profile_background_requested=None;
            self.render_command(RenderCommand::SetAssetPreviewMode(true));
            self.render_command(RenderCommand::SetAssetPreviewViewport(None));
            self.asset_preview_viewport_key = None;
            self.clear_asset_preview_runtime();
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(
            ui,
            "Map Viewer",
            "Browse source maps (*.map) from loose files and mounted PK3s, then launch them directly.",
            true,
        ) {
            self.frontend_page = FrontendPage::MapViewer;
            self.egui_repaint_requested = true;
        }
    }
}
