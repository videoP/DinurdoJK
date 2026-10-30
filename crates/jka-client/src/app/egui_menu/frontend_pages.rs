//! Frontend.
use crate::app::egui_menu::{
    menu_action, theme, App, FrontendPage, OverlayMode, RenderCommand, CONTENT_MAX_W,
    GUTTER, SETUP_TAB_VIDEO,
};
use crate::app::frontend_scene_cache::{
    FRONTEND_CHROME_FADE_IN_SECS, FRONTEND_STAGE_FADE_IN_SECS,
};

impl App {
    pub(in crate::app::egui_menu) fn build_egui_frontend_menu(&mut self, ui: &mut egui::Ui) {
        self.egui_frontend_scene_fade(ui);
        // Main is drawn by the renderer (rainy café window); egui only routes
        // its input. Settings opens over it with the regular chrome.
        if self.frontend_page == FrontendPage::Main && self.egui_display_overlay() != OverlayMode::Video {
            self.draw_neon_main_menu(ui);
            self.egui_frontend_neon_footer(ui);
            self.egui_disconnect_notice(ui);
            self.egui_frontend_quit_confirmation(ui);
            return;
        }
        // Play and Developer Tools share Main's actual HDR tubes, ignition,
        // CRT TV content, perspective projection and bloom. No egui body/header.
        if self.egui_display_overlay() != OverlayMode::Video {
            let submenu = match self.frontend_page {
                FrontendPage::Play => Some(crate::neon_menu::MenuPage::Play),
                FrontendPage::DeveloperTools => Some(crate::neon_menu::MenuPage::DeveloperTools),
                _ => None,
            };
            if let Some(kind) = submenu {
                self.draw_neon_frontend_submenu(ui, kind);
                self.egui_frontend_neon_footer(ui);
                self.egui_disconnect_notice(ui);
                self.egui_frontend_quit_confirmation(ui);
                return;
            }
        }
        self.release_neon_main_menu();
        // Render Studio is a full-height editor, not a frontend menu page.
        // It owns its header and workspace: no reserved footer strip, and no
        // GPU neon footer over the preview or timeline. This also clears any
        // chrome left by the Developer Tools page on entry.
        if self.frontend_page == FrontendPage::RenderStudio
            && self.egui_display_overlay() != OverlayMode::Video {
            self.release_neon_chrome();
            self.egui_frontend_header(ui);
            self.egui_frontend_body(ui);
            self.egui_disconnect_notice(ui);
            self.egui_frontend_2d_fade(ui);
            return;
        }
        self.egui_frontend_header(ui);
        if self.egui_display_overlay() == OverlayMode::Video {
            self.egui_setup_tabs(ui);
            self.egui_footer(ui);
            self.egui_body(ui);
        } else {
            self.egui_frontend_footer(ui);
            self.egui_frontend_body(ui);
        }
        self.egui_frontend_neon_footer(ui);
        self.egui_disconnect_notice(ui);
        self.egui_frontend_2d_fade(ui);
        self.egui_frontend_quit_confirmation(ui);
    }

    /// Fade the 2D chrome in on the shared stage clock (see `FrontendStage`).
    /// The UI remains interactive as the scrim is just a paint layer, not a
    /// hit-testable Area or blocking loading overlay. Until the renderer has
    /// presented the first dark frame the scrim is fully opaque.
    fn egui_frontend_2d_fade(&self, ui: &egui::Ui) {
        // Background decoding/preparation stays asynchronous. A preferred-page
        // launch must never wait behind the scene's presentation clock, even
        // when navigating back to Main before the scene is ready.
        if self.frontend_skip_chrome_fade || matches!(self.frontend_page, FrontendPage::Main | FrontendPage::Play | FrontendPage::DeveloperTools) {
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
            FrontendPage::DeveloperTools | FrontendPage::Profile | FrontendPage::AssetViewer | FrontendPage::RenderStudio | FrontendPage::Screenshots
        ) {
            return;
        }
        let now = std::time::Instant::now();
        let handoff = self.frontend_scene_handoff(now);
        let image_expected =
            self.frontend_scene_texture.is_some() || self.frontend_stage.cache_pending;
        // Main displays the art-directed scene without artificial darkening.
        // Retain the cached still -> live-3D handoff, not the old black scrim.
        let darkness = if matches!(self.frontend_page, FrontendPage::Main | FrontendPage::Play | FrontendPage::DeveloperTools) {
            0.0
        } else if image_expected {
            1.0 - self.frontend_stage.brightness(now, FRONTEND_STAGE_FADE_IN_SECS)
        } else {
            1.0 - handoff
        };
        let image_alpha = 1.0 - handoff;
        let full = root.max_rect();
        // Scene-stage photographs and scrims must never cover the GPU footer,
        // which is drawn underneath egui in the shared UI composite.
        let rect = egui::Rect::from_min_max(full.min, egui::pos2(full.right(),
            (full.bottom()-crate::neon_menu::footer::HEIGHT).max(full.top())));
        let painter = root.painter().with_clip_rect(rect);
        let mut painted_image = false;
        // Before the WGPU compositor is ready, keep showing the existing egui
        // still as a safe fallback. Once ready, only WGPU blends the still
        // *under* the neon, rain, glass and CRT passes.
        // Settings over Main suspends the compositor, so egui keeps the still.
        let egui_owns_image = !matches!(self.frontend_page, FrontendPage::Main | FrontendPage::Play | FrontendPage::DeveloperTools)
            || !self.neon_menu.renderer_ready
            || self.egui_display_overlay() == OverlayMode::Video;
        if let Some(image) = &self.frontend_scene_texture {
            if egui_owns_image && image_alpha > 0.001 {
                // Cover/crop, not stretch: resizing the window should never
                // deform the levelshot, even when the cached aspect differs.
                let size = image.size_vec2();
                let cover = (rect.width() / size.x).max(rect.height() / size.y);
                let visible = egui::vec2(
                    rect.width() / (size.x * cover),
                    rect.height() / (size.y * cover),
                );
                let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), visible);
                painter.image(
                    image.id(), rect, uv,
                    crate::renderer::egui_alpha_tint(image_alpha),
                );
                painted_image = true;
            }
        }
        if darkness > 0.001 {
            painter.rect_filled(
                rect, egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha((darkness * 255.0).round() as u8),
            );
        }
        if darkness > 0.001 || painted_image && handoff > 0.0 && handoff < 1.0 {
            root.ctx().request_repaint();
        }
    }

    pub(in crate::app::egui_menu) fn egui_frontend_header(&mut self, root: &mut egui::Ui) {
        let shell = self.menu_shell(root.ctx());
        super::shell_layout::region(root, "jka_frontend_header", shell.header, |ui| {
                super::holo::header_backdrop(ui.painter(), ui.max_rect());
                ui.horizontal_centered(|ui| {
                    ui.add_space(GUTTER);
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
                            FrontendPage::Pk3Manager => "PK3 MANAGER",
                            FrontendPage::Screenshots => "SCREENSHOTS",
                            FrontendPage::MapViewer => "MAP VIEWER",
                            FrontendPage::RenderStudio => "RENDER STUDIO",
                        }
                    };
                    // The page name in neon tube lettering, like the
                    // Main page's signs it was reached from.
                    let cap = 22.0;
                    let (title, _) = ui.allocate_exact_size(
                        egui::vec2(super::holo::neon_width(page, cap) + 6.0, cap * 1.8),
                        egui::Sense::hover(),
                    );
                    super::holo::neon_text(
                        ui.painter(),
                        egui::pos2(title.left() + 3.0, title.center().y + cap * 0.5),
                        page,
                        cap,
                        theme::ACCENT,
                        1.0,
                    );
                    // Submenus use the persistent footer for BACK / QUIT.
                    // Render Studio's dedicated header tools stay here.
                    if self.frontend_page == FrontendPage::RenderStudio
                        && self.egui_display_overlay() != OverlayMode::Video {
                        ui.add_space(12.0);
                        self.egui_render_studio_header_controls(ui);
                    }
                    if self.egui_display_overlay() == OverlayMode::Video
                        || self.frontend_page != FrontendPage::Main
                    {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Keep clear of the FPS counter pinned to the
                            // top-right corner of the window.
                            ui.add_space(96.0);
                            if self.frontend_page == FrontendPage::RenderStudio
                                && self.egui_display_overlay() != OverlayMode::Video {
                                // Preserve the editor's original top-bar BACK,
                                // instead of relying on the frontend footer.
                                if theme::ghost_button(ui, "BACK").clicked() {
                                    self.frontend_back();
                                }
                                ui.add_space(10.0);
                                self.egui_render_studio_header_actions(ui);
                            }
                        });
                    }
                });
            });
    }

    /// GPU/Taffy footer shared by Main, Developer Tools, and all other
    /// frontend pages. egui provides hit testing only; no default buttons or
    /// frames are drawn in this footer.
    fn egui_frontend_neon_footer(&mut self, ui: &mut egui::Ui) {
        use crate::neon_menu::{ChromeFrame, ChromeLine, footer};
        let area = ui.max_rect();
        let (w,h) = (area.width(),area.height());
        let back = self.frontend_page != FrontendPage::Main
            || self.egui_display_overlay() == OverlayMode::Video;
        let pending_video_changes = if self.egui_display_overlay() == OverlayMode::Video
            && self.setup_selected == SETUP_TAB_VIDEO && self.video_restart_required() {
            self.pending_video_changes()
        } else { Vec::new() };
        let action = (!pending_video_changes.is_empty())
            .then(|| format!("APPLY ({})",pending_video_changes.len()));
        let Some(footer) = footer::compute_with_action(w,h,back,action.as_deref()) else { return; };
        let rect = |r: [f32;4]| egui::Rect::from_min_max(
            area.min + egui::vec2(r[0]*w,r[1]*h),
            area.min + egui::vec2(r[2]*w,r[3]*h));
        let mut back_hovered=false;
        let mut quit_hovered=false;
        let mut apply_hovered=false;
        let mut activate_back=false;
        let mut activate_quit=false;
        let mut activate_apply=false;
        if let Some((_,r))=footer.apply {
            let response=ui.interact(rect(r),ui.make_persistent_id("neon_footer_apply"),egui::Sense::click())
                .on_hover_ui(|ui| {
                    ui.set_max_width(420.0);
                    theme::glow_label(ui,&format!("{} pending video changes",pending_video_changes.len()),
                        12.5,theme::WARNING);
                    for change in &pending_video_changes {
                        theme::glow_label(ui,&format!("{} - {}",change.label,change.detail),
                            11.5,theme::TEXT_DIM);
                    }
                });
            apply_hovered=response.hovered(); activate_apply=response.clicked();
        }
        if let Some((_,r))=footer.back {
            let response=ui.interact(rect(r),ui.make_persistent_id("neon_footer_back"),egui::Sense::click());
            back_hovered=response.hovered(); activate_back=response.clicked();
        }
        let response=ui.interact(rect(footer.quit.1),ui.make_persistent_id("neon_footer_quit"),egui::Sense::click());
        quit_hovered=response.hovered(); activate_quit=response.clicked();
        if back_hovered || quit_hovered || apply_hovered { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
        let mut lines=Vec::new();
        let mut add=|text: &str, position: crate::neon_menu::layout::TextLine,
                     gas: [f32;3], power: f32, tube_scale: f32| {
            lines.push(ChromeLine {text:text.into(),position,gas,power,tube_scale});
        };
        // The persistent footer owns the original main-menu wordmark.
        // The exact saber geometry and matching colors are appended by WGPU.
        add(crate::neon_menu::TITLE,footer.brand,[0.80,0.88,1.0],0.85,1.15);
        if let Some((line,_))=footer.apply {
            add(action.as_deref().unwrap_or("APPLY"),line,[1.0,0.57,0.11],
                if apply_hovered {1.55} else {0.85},1.10);
        }
        if let Some((line,_))=footer.back {
            add("BACK",line,[0.12,0.78,1.0],if back_hovered {1.45} else {0.72},1.05);
        }
        add("QUIT",footer.quit.0,[0.94,0.22,0.37],if quit_hovered {1.45} else {0.72},1.05);
        let frame=ChromeFrame {lines,footer,back_hovered,quit_hovered,apply_hovered,
            dimmed:false};
        if self.neon_chrome_sent.as_ref()!=Some(&frame) {
            self.render_command(RenderCommand::SetNeonChrome(Some(frame.clone())));
            self.neon_chrome_sent=Some(frame);
        }
        if !self.frontend_quit_confirmation {
            if activate_apply { self.egui_apply_video_requested=true; }
            if activate_back { self.frontend_back(); }
            if activate_quit { self.frontend_quit_confirmation=true; }
        }
    }

    pub(in crate::app::egui_menu) fn release_neon_chrome(&mut self) {
        if self.neon_chrome_sent.take().is_some() {
            self.render_command(RenderCommand::SetNeonChrome(None));
        }
    }

    fn egui_frontend_quit_confirmation(&mut self, root: &mut egui::Ui) {
        if !self.frontend_quit_confirmation { return; }
        let mut decision=0;
        let response=egui::Modal::new(egui::Id::new("frontend_quit_confirmation"))
            .backdrop_color(egui::Color32::from_black_alpha(175))
            .frame(egui::Frame::new().fill(theme::CHROME)
                .stroke(egui::Stroke::new(1.0,theme::ACCENT_DEEP))
                .inner_margin(egui::Margin::same(22)))
            .show(root.ctx(), |ui| {
                ui.set_width(380.0);
                theme::glow_label(ui,"QUIT DINURDOJK?",22.0,theme::TEXT);
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Are you sure you want to exit to desktop?").color(theme::TEXT_DIM));
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui,"QUIT GAME").clicked() { decision=1; }
                    ui.add_space(12.0);
                    if theme::ghost_button(ui,"CANCEL").clicked() { decision=2; }
                });
            });
        if response.should_close() || decision==2 { self.frontend_quit_confirmation=false; }
        if decision==1 {
            self.frontend_quit_confirmation=false;
            self.request_quit();
        }
    }

    pub(in crate::app::egui_menu) fn egui_frontend_footer(&mut self, root: &mut egui::Ui) {
        // The shell reserves the footer strip; the footer itself is WGPU
        // signage (`egui_frontend_neon_footer`).
        let _ = root;
    }

    pub(in crate::app::egui_menu) fn egui_frontend_body(&mut self, root: &mut egui::Ui) {
        let body = self.menu_shell(root.ctx()).body;
        super::shell_layout::region(root, "jka_frontend_body_region", body, |ui| {
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
                        | FrontendPage::Pk3Manager
                        | FrontendPage::Screenshots
                        | FrontendPage::MapViewer
                        | FrontendPage::RenderStudio
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
                        let frame = if matches!(self.frontend_page, FrontendPage::Profile | FrontendPage::RenderStudio) {
                            // Match the in-game Profile layout: the right half is a
                            // renderer-owned 3D viewport, while the controls pane
                            // paints its own opaque surface.
                            egui::Frame::new().fill(egui::Color32::TRANSPARENT)
                        } else if matches!(self.frontend_page, FrontendPage::AssetViewer | FrontendPage::Screenshots) {
                            egui::Frame::new().stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        } else {
                            super::holo::glass()
                        };
                        let studio = self.frontend_page == FrontendPage::RenderStudio;
                        let margin = if studio { egui::Margin::ZERO }
                            else { egui::Margin::symmetric(GUTTER as i8, 18) };
                        frame.inner_margin(margin).show(ui, |ui| {
                            ui.set_width(if studio { content_w } else { content_w - GUTTER * 2.0 });
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
            FrontendPage::Play => self.draw_neon_frontend_submenu(ui, crate::neon_menu::MenuPage::Play),
            FrontendPage::ServerBrowser => self.egui_server_browser_page(ui),
            FrontendPage::SoloGame => self.egui_solo_game_page(ui),
            FrontendPage::PlayDemo => self.egui_play_demo_page(ui),
            FrontendPage::Profile => self.egui_profile_studio(ui),
            FrontendPage::Controls => self.egui_controls_page(ui),
            FrontendPage::ChatLogs => self.egui_chat_log_browser_page(ui),
            FrontendPage::DeveloperTools => self.draw_neon_frontend_submenu(ui, crate::neon_menu::MenuPage::DeveloperTools),
            FrontendPage::AssetViewer => self.egui_asset_viewer_page(ui),
            FrontendPage::Pk3Manager => self.egui_pk3_manager_page(ui),
            FrontendPage::Screenshots => self.egui_screenshot_browser_page(ui),
            FrontendPage::MapViewer => self.egui_map_viewer_page(ui),
            FrontendPage::RenderStudio => self.egui_render_studio_page(ui),
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
    }

    // Developer Tools is now a full-bleed Taffy/neon page, routed directly
    // from build_egui_frontend_menu rather than through the old body panel.

}
