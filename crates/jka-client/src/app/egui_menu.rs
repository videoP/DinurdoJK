//! The whole in-game menu, drawn with egui.
//!
//! Everything the player sees once ESC is pressed lives here: the global top
//! bar, the Setup tab strip, the Video section rail, the non-Video pages and
//! the modal video confirmation. Laying the chrome out with real egui panels
//! (rather than free-floating areas over renderer-drawn text) is what keeps
//! the tab strip clickable instead of buried under the settings panel.
//!
//! The renderer-drawn overlay still owns the HUD, chat, console, crosshair and
//! loading screen, which are in-world surfaces rather than menu chrome.

use super::egui_theme as theme;
use super::*;

const TOP_ITEMS: [&str; 7] = [
    "GAME", "SERVERS", "PROFILE", "CONTROLS", "SETUP", "VOTE", "MOD",
];
const SETUP_TABS: [&str; 6] = ["GAME", "VIDEO", "INPUT", "AUDIO", "NETWORK", "INTERFACE"];

const TOP_RESUME: usize = 0;
const TOP_CONTROLS: usize = 3;
const TOP_SETUP: usize = 4;

const SETUP_TAB_VIDEO: usize = 1;
const SETUP_TAB_INPUT: usize = 2;
const SETUP_TAB_AUDIO: usize = 3;
const SETUP_TAB_INTERFACE: usize = 5;

/// Widest the settings/page column is allowed to get. Sized to just fit a
/// label, a full-width meter and its readout: on the Video page the world
/// behind the menu is deliberately left un-blurred so the effect of a setting
/// is visible, and every extra point of panel width hides more of it.
const CONTENT_MAX_W: f32 = 720.0;
/// Left inset shared by the tab strip, the rail and the page column so all
/// three read off the same vertical line.
const GUTTER: f32 = 24.0;

/// Keep the renderer hidden until the frontend BSP is actually ready, then
/// reveal it gently behind the menu. This also replaces the renderer's empty
/// world/fog clear color with intentional black during startup.
const FRONTEND_SCENE_FADE_IN_SECS: f32 = 2.0;

/// One entry in the Video page's left rail. Splitting the old single scrolling
/// wall of settings into addressable sections is what makes the page skimmable;
/// the RENDERING/ENVIRONMENT split becomes a group heading in the rail instead
/// of a pair of buttons floating in the panel header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum VideoSection {
    Display,
    ImageQuality,
    Visibility,
    Models,
    Lighting,
    Shadows,
    Reflections,
    PostProcessing,
    Film,
    DebugTools,
    BakedAo,
    Physics,
    Sun,
    Fog,
    Clouds,
    Weather,
    Surface,
    Water,
}

impl VideoSection {
    const RENDERING: [(Self, &'static str); 12] = [
        (Self::Display, "Display"),
        (Self::ImageQuality, "Image quality"),
        (Self::Visibility, "Visibility"),
        (Self::Models, "Models"),
        (Self::Lighting, "Lighting"),
        (Self::Shadows, "Shadows"),
        (Self::Reflections, "Reflections"),
        (Self::PostProcessing, "Post processing"),
        (Self::Film, "Film emulation"),
        (Self::DebugTools, "Debug & tools"),
        (Self::BakedAo, "Baked AO"),
        (Self::Physics, "Physics"),
    ];
    const ENVIRONMENT: [(Self, &'static str); 6] = [
        (Self::Sun, "Sun"),
        (Self::Fog, "Fog"),
        (Self::Clouds, "Clouds"),
        (Self::Weather, "Weather"),
        (Self::Surface, "Surface"),
        (Self::Water, "Water"),
    ];

}

impl App {
    pub(super) fn reset_egui_for_window(&mut self, window: &Window) {
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
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
    }

    /// egui owns every menu overlay, not just the Video page.
    pub(super) fn egui_menu_active(&self) -> bool {
        matches!(self.overlay, OverlayMode::Game | OverlayMode::Video | OverlayMode::HudEdit)
            && self.video_confirmation.is_none()
            && self.loading.is_none()
            && !self.video.skip_ui
            && self.window.is_some()
            && self.render.is_some()
    }

    /// The post-apply countdown suspends the menu but still owns the pointer.
    fn egui_confirmation_active(&self) -> bool {
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
    pub(super) fn route_egui_window_event(&mut self, event: &WindowEvent) -> bool {
        let confirmation = self.egui_confirmation_active();
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

        let response = state.on_window_event(window, event);
        self.egui_repaint_requested |= response.repaint;

        if let WindowEvent::KeyboardInput { event, .. } = event {
            let PhysicalKey::Code(code) = event.physical_key else {
                return true;
            };
            if matches!(
                code,
                KeyCode::PrintScreen | KeyCode::Backquote | KeyCode::Escape
            ) {
                return false;
            }
            // Everything else belongs to egui while a menu is open.
            return true;
        }

        response.consumed
    }

    pub(super) fn tick_egui_menu(&mut self) {
        self.poll_server_browser_events();
        if self.tick_egui_confirmation() {
            return;
        }
        if !self.egui_menu_active() {
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

    fn run_egui_menu_frame(&mut self) {
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
        } = ctx.run_ui(raw_input, |ui| self.build_egui_menu(ui));
        state.handle_platform_output(&window, platform_output);
        self.egui_state = Some(state);

        // Applying display/backend changes recreates both the wgpu renderer and
        // egui state. Defer that until Context::run has fully returned so this
        // frame cannot overwrite the freshly-reset egui integration.
        if std::mem::take(&mut self.egui_apply_video_requested) {
            // The current egui context/renderer are about to be discarded, so
            // these deltas intentionally have no target. Clear them explicitly
            // before dropping to satisfy egui's integration contract.
            textures_delta.clear();
            self.restart_renderer();
            return;
        }

        let paint_jobs = ctx.tessellate(shapes, pixels_per_point);
        self.render_command(RenderCommand::SetEgui(Some(EguiRenderData {
            paint_jobs,
            textures_delta,
            pixels_per_point,
        })));
        self.egui_renderer_active = true;
        self.egui_repaint_requested = false;
        self.egui_last_frame = Instant::now();
    }

    // ------------------------------------------------------------- chrome --

    fn build_egui_menu(&mut self, ui: &mut egui::Ui) {
        if self.overlay == OverlayMode::HudEdit {
            self.egui_hud_editor(ui);
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

    fn egui_hud_editor(&mut self, root: &mut egui::Ui) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let pixels_per_point = root.ctx().pixels_per_point().max(0.001);
        let to_points = |value: f32| value / pixels_per_point;
        let painter = root.painter().clone();
        let full = egui::Rect::from_min_size(
            root.max_rect().min,
            egui::vec2(to_points(size.width as f32), to_points(size.height as f32)),
        );

        // The grid is visual only. Layout values remain framebuffer-pixel offsets
        // from their anchors and are snapped in that same unit, so the renderer
        // never needs to parse cvar text or know that edit mode exists.
        if self.hud_layout.snap_to_grid {
            let spacing_px = self.hud_layout.grid_size.clamp(1.0, 64.0);
            let spacing = to_points(spacing_px);
            let line = egui::Color32::from_rgba_premultiplied(128, 180, 220, 24);
            let major = egui::Color32::from_rgba_premultiplied(128, 180, 220, 42);
            let mut x = full.left();
            let mut column = 0usize;
            while x <= full.right() {
                painter.line_segment(
                    [egui::pos2(x, full.top()), egui::pos2(x, full.bottom())],
                    egui::Stroke::new(1.0, if column % 4 == 0 { major } else { line }),
                );
                x += spacing;
                column += 1;
            }
            let mut y = full.top();
            let mut row = 0usize;
            while y <= full.bottom() {
                painter.line_segment(
                    [egui::pos2(full.left(), y), egui::pos2(full.right(), y)],
                    egui::Stroke::new(1.0, if row % 4 == 0 { major } else { line }),
                );
                y += spacing;
                row += 1;
            }
        }

        let mut layout_changed = false;
        let mut drag_ended = false;
        for id in HudElementId::ALL {
            let layout = self.hud_layout.element(id);
            let hud_rect = ui::hud_element_rect(id, layout, size.width, size.height);
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    full.left() + to_points(hud_rect.x),
                    full.top() + to_points(hud_rect.y),
                ),
                egui::vec2(to_points(hud_rect.width), to_points(hud_rect.height)),
            );
            let response = root.interact(
                rect,
                egui::Id::new(("hud_edit_item", id)),
                egui::Sense::click_and_drag(),
            );
            if response.clicked() || response.drag_started() {
                self.hud_edit_selected = Some(id);
                self.hud_edit_drag_origin = Some(layout.offset);
                self.hud_edit_drag_delta = [0.0, 0.0];
                self.egui_repaint_requested = true;
            }
            if response.dragged() {
                let origin = self.hud_edit_drag_origin.unwrap_or(layout.offset);
                let delta = response.drag_delta() * pixels_per_point;
                self.hud_edit_drag_delta[0] += delta.x;
                self.hud_edit_drag_delta[1] += delta.y;
                let mut offset = [
                    origin[0] + self.hud_edit_drag_delta[0],
                    origin[1] + self.hud_edit_drag_delta[1],
                ];
                let alt_bypass = root.input(|input| input.modifiers.alt);
                if self.hud_layout.snap_to_grid && !alt_bypass {
                    let grid = self.hud_layout.grid_size.clamp(1.0, 64.0);
                    offset[0] = (offset[0] / grid).round() * grid;
                    offset[1] = (offset[1] / grid).round() * grid;
                }
                let target = self.hud_layout.element_mut(id);
                if target.offset != offset {
                    target.offset = offset;
                    layout_changed = true;
                }
            }
            if response.drag_stopped() {
                drag_ended = true;
            }

            let selected = self.hud_edit_selected == Some(id);
            let outline = if selected {
                theme::ACCENT
            } else if response.hovered() {
                theme::TEXT
            } else {
                egui::Color32::from_rgba_premultiplied(170, 205, 230, 150)
            };
            painter.rect_stroke(
                rect.expand(3.0),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(if selected { 2.0 } else { 1.0 }, outline),
                egui::StrokeKind::Outside,
            );
            painter.text(
                rect.left_top() + egui::vec2(0.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                id.label(),
                egui::FontId::proportional(11.0),
                outline,
            );
        }
        if drag_ended {
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
        }

        if let Some(selected) = self.hud_edit_selected {
            let (left, right, up, down, shift) = root.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowLeft),
                    input.key_pressed(egui::Key::ArrowRight),
                    input.key_pressed(egui::Key::ArrowUp),
                    input.key_pressed(egui::Key::ArrowDown),
                    input.modifiers.shift,
                )
            });
            let mut delta = [0.0_f32, 0.0_f32];
            let step = if shift {
                self.hud_layout.grid_size.clamp(1.0, 64.0)
            } else {
                1.0
            };
            if left { delta[0] -= step; }
            if right { delta[0] += step; }
            if up { delta[1] -= step; }
            if down { delta[1] += step; }
            if delta != [0.0, 0.0] {
                let target = self.hud_layout.element_mut(selected);
                target.offset[0] += delta[0];
                target.offset[1] += delta[1];
                layout_changed = true;
            }
        }

        let mut close_editor = false;
        let mut reset_selected = false;
        let mut reset_all = false;
        let mut snap = self.hud_layout.snap_to_grid;
        let mut grid = self.hud_layout.grid_size;
        let mut selected_scale = self
            .hud_edit_selected
            .map(|id| self.hud_layout.element(id).scale)
            .unwrap_or(1.0);
        egui::Area::new(egui::Id::new("jka_hud_edit_toolbar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 14.0))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(10, 15, 22, 236))
                    .stroke(egui::Stroke::new(1.0, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::glow_label(ui, "HUD EDIT", 13.0, theme::TEXT);
                            ui.separator();
                            if ui.checkbox(&mut snap, "Snap").changed() {
                                layout_changed = true;
                            }
                            ui.label("Grid");
                            if ui
                                .add(egui::DragValue::new(&mut grid).range(1.0..=64.0).speed(1.0))
                                .changed()
                            {
                                layout_changed = true;
                            }
                            ui.separator();
                            ui.add_enabled_ui(self.hud_edit_selected.is_some(), |ui| {
                                ui.label("Scale");
                                if ui
                                    .add(
                                        egui::Slider::new(&mut selected_scale, 0.5..=2.0)
                                            .show_value(true),
                                    )
                                    .changed()
                                {
                                    layout_changed = true;
                                }
                                if ui.button("RESET SELECTED").clicked() {
                                    reset_selected = true;
                                }
                            });
                            if ui.button("RESET ALL").clicked() {
                                reset_all = true;
                            }
                            if ui.button("DONE").clicked() {
                                close_editor = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            theme::label(
                                ui,
                                theme::plain(
                                    "Drag panels. ALT temporarily bypasses snapping. Arrow keys nudge 1 px; SHIFT+arrow uses the grid step.",
                                    10.5,
                                    theme::TEXT_FAINT,
                                ),
                            );
                        });
                    });
            });

        if self.hud_layout.snap_to_grid != snap {
            self.hud_layout.snap_to_grid = snap;
        }
        let clamped_grid = grid.clamp(1.0, 64.0);
        if (self.hud_layout.grid_size - clamped_grid).abs() > f32::EPSILON {
            self.hud_layout.grid_size = clamped_grid;
        }
        if let Some(selected) = self.hud_edit_selected {
            let scale = selected_scale.clamp(0.5, 2.0);
            let target = self.hud_layout.element_mut(selected);
            if (target.scale - scale).abs() > f32::EPSILON {
                target.scale = scale;
            }
        }
        if reset_selected {
            if let Some(selected) = self.hud_edit_selected {
                self.hud_layout.reset_element(selected);
                self.hud_edit_drag_origin = None;
                self.hud_edit_drag_delta = [0.0, 0.0];
                layout_changed = true;
            }
        }
        if reset_all {
            self.hud_layout = HudLayout::default();
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
            layout_changed = true;
        }
        if layout_changed {
            self.mark_config_dirty();
            self.publish_ui();
            self.egui_repaint_requested = true;
        }
        if close_editor {
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
            self.set_overlay(OverlayMode::None);
        }
    }

    fn build_egui_frontend_menu(&mut self, ui: &mut egui::Ui) {
        self.egui_frontend_scene_fade(ui);
        self.egui_frontend_header(ui);
        if self.overlay == OverlayMode::Video {
            self.egui_setup_tabs(ui);
            self.egui_footer(ui);
            self.egui_body(ui);
        } else {
            self.egui_frontend_footer(ui);
            self.egui_frontend_body(ui);
        }
    }

    /// Paint a black scrim *under* the menu chrome but over the 3D renderer.
    /// Before duel3 is available it is fully opaque, so startup is black rather
    /// than the renderer's no-world clear color. Once the cinematic is ready,
    /// the scrim eases away and the BSP appears to brighten from 0 -> 1.
    fn egui_frontend_scene_fade(&self, root: &mut egui::Ui) {
        let brightness = self.frontend_cinematic.map_or(0.0, |cinematic| {
            let t = cinematic.started.elapsed().as_secs_f32() / FRONTEND_SCENE_FADE_IN_SECS;
            let t = t.clamp(0.0, 1.0);
            // Smoothstep avoids a visible pop in slope at either end of the fade.
            t * t * (3.0 - 2.0 * t)
        });
        let blackout = 1.0 - brightness;
        if blackout <= 0.001 {
            return;
        }

        let alpha = (blackout * 255.0).round() as u8;
        root.painter().rect_filled(
            root.max_rect(),
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgba_premultiplied(0, 0, 0, alpha),
        );
    }

    fn egui_frontend_header(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("jka_frontend_header")
            .exact_size(theme::TOP_BAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(GUTTER as i8, 0)),
            )
            .show_inside(root, |ui| {
                ui.horizontal_centered(|ui| {
                    theme::glow_label(ui, "DINURDOJK", 16.0, theme::TEXT);
                    ui.add_space(14.0);
                    let page = if self.overlay == OverlayMode::Video {
                        "SETTINGS"
                    } else {
                        match self.frontend_page {
                            FrontendPage::Main => "MAIN MENU",
                            FrontendPage::Play => "PLAY",
                            FrontendPage::ServerBrowser => "SERVERS",
                            FrontendPage::SoloGame => "SOLO GAME",
                            FrontendPage::PlayDemo => "PLAY DEMO",
                            FrontendPage::Controls => "CONTROLS",
                        }
                    };
                    theme::glow_label(ui, page, 12.5, theme::TEXT_DIM);
                    if self.overlay == OverlayMode::Video
                        || self.frontend_page != FrontendPage::Main
                    {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if theme::ghost_button(ui, "BACK").clicked() {
                                    self.frontend_back();
                                }
                            },
                        );
                    }
                });
            });
    }

    fn egui_frontend_footer(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("jka_frontend_footer")
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
                    let hint = match self.frontend_page {
                        FrontendPage::Main => "Select an option.  ` opens the console.",
                        FrontendPage::Play => "ESC returns to the main menu.",
                        FrontendPage::ServerBrowser | FrontendPage::SoloGame | FrontendPage::PlayDemo => "ESC returns to Play.",
                        FrontendPage::Controls => "ESC returns to the main menu.",
                    };
                    theme::label(ui, theme::plain(hint, 11.5, theme::TEXT_FAINT));
                });
            });
    }

    fn egui_frontend_body(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show_inside(root, |ui| {
                let max_w = if matches!(
                    self.frontend_page,
                    FrontendPage::ServerBrowser | FrontendPage::SoloGame | FrontendPage::PlayDemo
                ) {
                    1120.0
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
                        egui::Frame::new()
                            .fill(theme::PANEL_FILL)
                            .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
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

    fn egui_frontend_page(&mut self, ui: &mut egui::Ui) {
        match self.frontend_page {
            FrontendPage::Main => self.egui_frontend_main_page(ui),
            FrontendPage::Play => self.egui_frontend_play_page(ui),
            FrontendPage::ServerBrowser => self.egui_server_browser_page(ui),
            FrontendPage::SoloGame => self.egui_solo_game_page(ui),
            FrontendPage::PlayDemo => self.egui_play_demo_page(ui),
            FrontendPage::Controls => self.egui_controls_page(ui),
        }
    }

    fn egui_frontend_main_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "MAIN MENU", "Choose where you want to go.");
        theme::section(ui, "DINURDOJK", "");
        if menu_action(ui, "Play", "Join a server, start a solo map, or play a demo.", true) {
            self.frontend_page = FrontendPage::Play;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(ui, "Controls", "Keyboard and mouse bindings.", true) {
            self.frontend_page = FrontendPage::Controls;
            self.controls_waiting_for_key = false;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(ui, "Settings", "Use the same Setup pages available in game.", true) {
            self.setup_selected = SETUP_TAB_VIDEO;
            self.set_overlay(OverlayMode::Video);
            return;
        }
        if menu_action(ui, "Quit", "Exit DinurdoJK and return to the desktop.", true) {
            self.request_quit();
        }
    }

    fn egui_frontend_play_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "PLAY", "Choose a game source.");
        theme::section(ui, "PLAY", "");
        if menu_action(ui, "Join a game", "Browse Internet, LAN, favorites, and recent servers.", true) {
            self.frontend_page = FrontendPage::ServerBrowser;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(ui, "Solo Game", "Browse every compiled map on the active JKA asset path.", true) {
            self.frontend_page = FrontendPage::SoloGame;
            self.egui_repaint_requested = true;
            return;
        }
        if menu_action(ui, "Play Demo", "Browse Jedi Academy demo recordings.", true) {
            self.frontend_page = FrontendPage::PlayDemo;
            self.egui_repaint_requested = true;
            return;
        }
    }

    fn poll_server_browser_events(&mut self) {
        let mut changed = false;
        loop {
            match self.server_browser_rx.try_recv() {
                Ok(event) => {
                    self.server_browser.handle_event(event);
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.server_browser.status_text = "Server browser worker stopped".to_owned();
                    changed = true;
                    break;
                }
            }
        }
        if changed {
            self.egui_repaint_requested = true;
        }
    }

    fn request_server_refresh(&mut self, source: ServerSource) {
        let command = match source {
            ServerSource::Internet => {
                self.server_browser.internet_requested_once = true;
                BrowserCommand::RefreshInternet {
                    masters: self.server_browser.master_servers.to_vec(),
                }
            }
            ServerSource::Lan => BrowserCommand::RefreshLan,
            ServerSource::Favorites | ServerSource::History => BrowserCommand::RefreshAddresses {
                source,
                addresses: self.server_browser.addresses_for_source(source),
            },
        };
        if self.server_browser_tx.send(command).is_err() {
            self.server_browser.status_text = "Server browser worker is unavailable".to_owned();
        } else {
            self.server_browser.refreshing.insert(source);
            self.server_browser.status_text = format!("Refreshing {}…", source.label());
        }
        self.egui_repaint_requested = true;
    }

    fn ensure_server_browser_started(&mut self) {
        if !self.server_browser.internet_requested_once {
            self.request_server_refresh(ServerSource::Internet);
        }
    }

    fn select_browser_server(&mut self, address: std::net::SocketAddr) {
        if self.server_browser.selected == Some(address) {
            return;
        }
        self.server_browser.selected = Some(address);
        self.server_browser.details = None;
        let _ = self.server_browser_tx.send(BrowserCommand::QueryStatus(address));
        self.egui_repaint_requested = true;
    }

    fn connect_browser_server(&mut self, address: std::net::SocketAddr) {
        self.connect_to_server(&address.to_string());
    }

    fn server_browser_sort_header(
        &mut self,
        ui: &mut egui::Ui,
        sort: server_browser::BrowserSort,
        width: f32,
    ) {
        let selected = self.server_browser.sort == sort;
        let label = if selected {
            format!(
                "{} {}",
                sort.label(),
                if self.server_browser.sort_ascending { "↑" } else { "↓" }
            )
        } else {
            sort.label().to_owned()
        };
        let color = if selected { theme::ACCENT } else { theme::TEXT_DIM };
        let response = ui.add_sized(
            [width, 20.0],
            egui::Label::new(theme::plain(&label, 10.5, color))
                .sense(egui::Sense::click()),
        );
        if response.clicked() {
            if selected {
                self.server_browser.sort_ascending = !self.server_browser.sort_ascending;
            } else {
                self.server_browser.sort = sort;
                self.server_browser.sort_ascending = matches!(
                    sort,
                    server_browser::BrowserSort::Ping
                        | server_browser::BrowserSort::Name
                        | server_browser::BrowserSort::Map
                        | server_browser::BrowserSort::Gametype
                );
            }
            self.egui_repaint_requested = true;
        }
        if response.hovered() {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(response.rect.left(), response.rect.bottom() - 1.0),
                    egui::vec2(response.rect.width(), 1.0),
                ),
                egui::CornerRadius::ZERO,
                if selected { theme::ACCENT } else { theme::LINE_STRONG },
            );
        }
        response.on_hover_text("Click to sort; click again to reverse the order.");
    }

    fn egui_server_browser_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_server_browser_started();

        theme::page_title(
            ui,
            "SERVER BROWSER",
            "Jedi Academy protocol 26 discovery with asynchronous Internet, LAN, favorites, and history queries.",
        );

        // Source selector + refresh.
        ui.horizontal(|ui| {
            for source in [
                ServerSource::Internet,
                ServerSource::Lan,
                ServerSource::Favorites,
                ServerSource::History,
            ] {
                if theme::chip(ui, source.label(), self.server_browser.source == source).clicked() {
                    self.server_browser.source = source;
                    self.server_browser.selected = None;
                    self.server_browser.details = None;
                    if self.server_browser.addresses_for_source(source).is_empty()
                        && matches!(source, ServerSource::Lan)
                    {
                        self.request_server_refresh(source);
                    } else if matches!(source, ServerSource::Favorites | ServerSource::History) {
                        self.request_server_refresh(source);
                    }
                    self.egui_repaint_requested = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refreshing = self.server_browser.refreshing.contains(&self.server_browser.source);
                let label = if refreshing { "REFRESHING…" } else { "REFRESH" };
                if ui.add_enabled(!refreshing, egui::Button::new(label)).clicked() {
                    self.request_server_refresh(self.server_browser.source);
                }
            });
        });
        ui.add_space(7.0);

        let enabled_masters = self
            .server_browser
            .master_servers
            .iter()
            .filter(|master| !master.trim().is_empty())
            .count();
        let mut pending_master_changes: Vec<(usize, String)> = Vec::new();
        egui::CollapsingHeader::new(format!(
            "MASTER SERVERS ({enabled_masters}/{})",
            server_browser::MAX_MASTER_SLOTS
        ))
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::label(
                    ui,
                    theme::plain(
                        "TaystJK sv_master1..sv_master5. Non-empty slots are queried together; port 29060 is assumed when omitted.",
                        10.5,
                        theme::TEXT_FAINT,
                    ),
                );
                if ui.small_button("RESTORE TAYSTJK DEFAULTS").clicked() {
                    pending_master_changes.clear();
                    for (slot, default) in server_browser::DEFAULT_MASTER_CVARS.iter().enumerate() {
                        self.server_browser.master_drafts[slot] = (*default).to_owned();
                        pending_master_changes.push((slot, (*default).to_owned()));
                    }
                }
            });
            ui.add_space(4.0);
            let slot_labels = ["RAVEN", "JKHUB", "OUNED", "CUSTOM", "CUSTOM"];
            for slot in 0..server_browser::MAX_MASTER_SLOTS {
                let current = self.server_browser.master_servers[slot].clone();
                let mut enabled = !current.trim().is_empty();
                let was_enabled = enabled;
                let mut draft = self.server_browser.master_drafts[slot].clone();
                let old_draft = draft.clone();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut enabled, "");
                    theme::label(
                        ui,
                        theme::plain(
                            &format!("sv_master{}", slot + 1),
                            10.5,
                            theme::TEXT_DIM,
                        ),
                    );
                    let edit = ui.add_sized(
                        [320.0, 22.0],
                        egui::TextEdit::singleline(&mut draft)
                            .hint_text(if slot < 3 {
                                server_browser::DEFAULT_MASTER_CVARS[slot]
                            } else {
                                "master.example.org[:port]"
                            }),
                    );
                    theme::label(
                        ui,
                        theme::plain(slot_labels[slot], 10.0, theme::TEXT_FAINT),
                    );
                    if edit.changed() {
                        self.egui_repaint_requested = true;
                    }
                });

                if draft != old_draft {
                    self.server_browser.master_drafts[slot] = draft.clone();
                    if enabled {
                        pending_master_changes.push((slot, draft.trim().to_owned()));
                    }
                }
                if enabled != was_enabled {
                    if enabled {
                        let restore = if draft.trim().is_empty() {
                            server_browser::DEFAULT_MASTER_CVARS[slot].to_owned()
                        } else {
                            draft.trim().to_owned()
                        };
                        if !restore.is_empty() {
                            self.server_browser.master_drafts[slot] = restore.clone();
                            pending_master_changes.push((slot, restore));
                        }
                    } else {
                        pending_master_changes.push((slot, String::new()));
                    }
                }
            }
        });
        for (slot, value) in pending_master_changes {
            let cvar = format!("sv_master{}", slot + 1);
            if let Err(error) = self.set_console_cvar(&cvar, &value) {
                self.server_browser.status_text = error;
            }
        }
        ui.add_space(7.0);

        // Search and practical filters are all local and instantaneous.
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("SEARCH", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [260.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.search)
                    .hint_text("name, map, mod, address"),
            );
            if response.changed() {
                self.egui_repaint_requested = true;
            }
            ui.add_space(10.0);
            if theme::chip(ui, "HIDE EMPTY", self.server_browser.hide_empty).clicked() {
                self.server_browser.hide_empty = !self.server_browser.hide_empty;
            }
            if theme::chip(ui, "HIDE FULL", self.server_browser.hide_full).clicked() {
                self.server_browser.hide_full = !self.server_browser.hide_full;
            }
            if theme::chip(ui, "HIDE BOTS", self.server_browser.hide_bots)
                .on_hover_text("Exclude bots from player counts and hide 0-ping rows in the selected server's player list.")
                .clicked()
            {
                self.server_browser.hide_bots = !self.server_browser.hide_bots;
            }
            ui.add_space(10.0);
            theme::label(ui, theme::plain("MAX PING", 11.5, theme::TEXT_FAINT));
            ui.add(
                egui::DragValue::new(&mut self.server_browser.max_ping)
                    .range(0..=999)
                    .suffix(" ms")
                    .speed(5),
            )
            .on_hover_text("0 disables the ping filter");
        });

        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("MODS", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [360.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.mod_filter)
                    .hint_text("comma-separated fs_game values, e.g. base,japro,japlus"),
            );
            if response.changed() {
                self.egui_repaint_requested = true;
            }
            response.on_hover_text("Case-insensitive exact matches. 'base' matches servers with no mod game directory.");
        });
        ui.add_space(7.0);

        let search = self.server_browser.search.trim().to_ascii_lowercase();
        let mod_filters: Vec<String> = self
            .server_browser
            .mod_filter
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        let mut servers: Vec<_> = self
            .server_browser
            .servers_for_current_source()
            .into_iter()
            .filter(|server| {
                let visible_clients = if self.server_browser.hide_bots {
                    server.humans
                } else {
                    server.clients
                };
                if self.server_browser.hide_empty && visible_clients == 0 {
                    return false;
                }
                if self.server_browser.hide_full
                    && server.max_clients > 0
                    && server.clients >= server.max_clients
                {
                    return false;
                }
                if self.server_browser.max_ping > 0
                    && server.ping_ms > self.server_browser.max_ping
                {
                    return false;
                }
                if !mod_filters.is_empty() {
                    let game = if server.game.trim().is_empty() {
                        "base".to_owned()
                    } else {
                        server.game.trim().to_ascii_lowercase()
                    };
                    if !mod_filters.iter().any(|filter| filter == &game) {
                        return false;
                    }
                }
                if search.is_empty() {
                    return true;
                }
                let hostname = crate::logging::strip_jka_colors(&server.hostname).to_ascii_lowercase();
                hostname.contains(&search)
                    || server.map.to_ascii_lowercase().contains(&search)
                    || server.game.to_ascii_lowercase().contains(&search)
                    || server.address.to_string().contains(&search)
                    || server.gametype_label().to_ascii_lowercase().contains(&search)
            })
            .collect();

        let sort = self.server_browser.sort;
        servers.sort_by(|a, b| {
            let order = match sort {
                server_browser::BrowserSort::Ping => {
                    let ap = if a.ping_ms == 0 { u32::MAX } else { a.ping_ms };
                    let bp = if b.ping_ms == 0 { u32::MAX } else { b.ping_ms };
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Players => {
                    let ap = if self.server_browser.hide_bots { a.humans } else { a.clients };
                    let bp = if self.server_browser.hide_bots { b.humans } else { b.clients };
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Name => crate::logging::strip_jka_colors(&a.hostname)
                    .to_ascii_lowercase()
                    .cmp(&crate::logging::strip_jka_colors(&b.hostname).to_ascii_lowercase()),
                server_browser::BrowserSort::Map => a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase()),
                server_browser::BrowserSort::Gametype => a.gametype.cmp(&b.gametype),
            };
            if self.server_browser.sort_ascending { order } else { order.reverse() }
        });

        let browser_height = (ui.available_height() - 78.0).max(220.0);
        let mut selected_after = None;
        let mut connect_after = None;
        ui.horizontal(|ui| {
            let list_width = (ui.available_width() * 0.67).max(560.0);
            ui.allocate_ui_with_layout(
                egui::vec2(list_width, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(
                        ui,
                        self.server_browser.source.label(),
                        &format!("{} visible — {}", servers.len(), self.server_browser.status_text),
                    );
                    ui.horizontal(|ui| {
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Name, 250.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Map, 118.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Gametype, 82.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Players, 55.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Ping, 44.0);
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("jka_server_browser_list")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for server in &servers {
                                let selected = self.server_browser.selected == Some(server.address);
                                let hostname = if server.hostname.is_empty() {
                                    server.address.to_string()
                                } else {
                                    crate::logging::strip_jka_colors(&server.hostname)
                                };
                                let response = ui
                                    .horizontal(|ui| {
                                        let label = if server.need_password {
                                            format!("🔒 {hostname}")
                                        } else {
                                            hostname
                                        };
                                        let row_text = theme::TEXT;
                                        let response = ui.add_sized(
                                            [250.0, 24.0],
                                            egui::Button::selectable(
                                                selected,
                                                theme::plain(&label, 11.5, theme::TEXT),
                                            ),
                                        );
                                        ui.add_sized([118.0, 24.0], egui::Label::new(theme::plain(&server.map, 11.5, row_text)));
                                        ui.add_sized([82.0, 24.0], egui::Label::new(theme::plain(server.gametype_label(), 10.8, row_text)));
                                        let visible_clients = if self.server_browser.hide_bots { server.humans } else { server.clients };
                                        ui.add_sized([55.0, 24.0], egui::Label::new(theme::plain(&format!("{}/{}", visible_clients, server.max_clients), 11.5, row_text)));
                                        let ping = if server.ping_ms == 0 { "—".to_owned() } else { server.ping_ms.to_string() };
                                        ui.add_sized([44.0, 24.0], egui::Label::new(theme::plain(&ping, 11.5, row_text)));
                                        response
                                    })
                                    .inner;
                                if response.clicked() {
                                    selected_after = Some(server.address);
                                }
                                if response.double_clicked() {
                                    connect_after = Some(server.address);
                                }
                            }
                        });
                },
            );

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SERVER DETAILS", "Select a server to inspect it.");
                    let Some(address) = self.server_browser.selected else {
                        theme::label(ui, theme::plain("No server selected.", 12.5, theme::TEXT_FAINT));
                        return;
                    };
                    let server = self.server_browser.servers.get(&address).cloned()
                        .unwrap_or_else(|| server_browser::ServerEntry::placeholder(address));
                    let hostname = if server.hostname.is_empty() {
                        address.to_string()
                    } else {
                        crate::logging::strip_jka_colors(&server.hostname)
                    };
                    theme::glow_label(ui, &hostname, 17.0, theme::TEXT);
                    theme::label(ui, theme::plain(&address.to_string(), 11.5, theme::TEXT_FAINT));
                    ui.add_space(8.0);
                    theme::label(ui, theme::plain(&format!("Map: {}", if server.map.is_empty() { "—" } else { &server.map }), 12.0, theme::TEXT_DIM));
                    theme::label(ui, theme::plain(&format!("Mode: {}", server.gametype_label()), 12.0, theme::TEXT_DIM));
                    let visible_clients = if self.server_browser.hide_bots { server.humans } else { server.clients };
                    let composition = if self.server_browser.hide_bots && server.bots > 0 {
                        format!("  •  {} bots hidden", server.bots)
                    } else if server.bots > 0 {
                        format!("  •  {} humans + {} bots", server.humans, server.bots)
                    } else {
                        String::new()
                    };
                    theme::label(ui, theme::plain(&format!("Players: {}/{}{}  •  Ping: {} ms", visible_clients, server.max_clients, composition, server.ping_ms), 12.0, theme::TEXT_DIM));
                    theme::label(ui, theme::plain(&format!("Game: {}  •  Protocol: {}", if server.game.is_empty() { "base" } else { &server.game }, server.protocol), 12.0, theme::TEXT_DIM));
                    if server.need_password {
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            theme::label(ui, theme::plain("PASSWORD", 11.0, theme::TEXT_FAINT));
                            ui.add_sized(
                                [170.0, 23.0],
                                egui::TextEdit::singleline(&mut self.network.password)
                                    .password(true)
                                    .hint_text("server password"),
                            );
                        });
                    }
                    ui.add_space(9.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "CONNECT").clicked() {
                            connect_after = Some(address);
                        }
                        let favorite = self.server_browser.is_favorite(address);
                        let favorite_label = if favorite { "REMOVE FAVORITE" } else { "ADD FAVORITE" };
                        if theme::ghost_button(ui, favorite_label).clicked() {
                            match self.server_browser.toggle_favorite(address) {
                                Ok(true) => self.server_browser.status_text = "Added favorite".to_owned(),
                                Ok(false) => self.server_browser.status_text = "Removed favorite".to_owned(),
                                Err(error) => self.server_browser.status_text = error,
                            }
                        }
                    });
                    ui.add_space(12.0);
                    theme::section(ui, "PLAYERS", "Live getstatus result for the selected server.");
                    if let Some(status) = self.server_browser.details.as_ref().filter(|status| status.address == address) {
                        if let Some((_, version)) = status.fields.iter().find(|(key, _)| key.eq_ignore_ascii_case("version")) {
                            theme::label(ui, theme::plain(&format!("Server: {version}"), 11.2, theme::TEXT_FAINT));
                            ui.add_space(4.0);
                        }
                        let visible_players: Vec<_> = status
                            .players
                            .iter()
                            .filter(|player| !self.server_browser.hide_bots || player.ping != 0)
                            .collect();
                        if visible_players.is_empty() {
                            let text = if self.server_browser.hide_bots && !status.players.is_empty() {
                                "No human player rows returned (0-ping bot rows hidden)."
                            } else {
                                "No player rows returned."
                            };
                            theme::label(ui, theme::plain(text, 11.5, theme::TEXT_FAINT));
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("jka_server_player_list")
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    for player in visible_players {
                                        let name = crate::logging::strip_jka_colors(&player.name);
                                        theme::label(ui, theme::plain(&format!("{:>4} ms   {:>4}   {}", player.ping, player.score, name), 11.5, theme::TEXT));
                                    }
                                });
                        }
                    } else {
                        theme::label(ui, theme::plain("Querying status…", 11.5, theme::TEXT_FAINT));
                    }
                },
            );
        });

        if let Some(address) = selected_after {
            self.select_browser_server(address);
        }
        if let Some(address) = connect_after {
            self.connect_browser_server(address);
            return;
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("DIRECT CONNECT", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [300.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.direct_connect)
                    .hint_text("hostname or address[:port]"),
            );
            let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if (theme::primary_button(ui, "CONNECT").clicked() || enter)
                && !self.server_browser.direct_connect.trim().is_empty()
            {
                let target = self.server_browser.direct_connect.trim().to_owned();
                self.connect_to_server(&target);
            }
        });
    }

    fn ensure_solo_map_catalog(&mut self) {
        if self.solo_catalog_loaded {
            return;
        }

        self.solo_catalog_loaded = true;
        self.solo_maps.clear();
        self.solo_catalog_error = None;
        match frontend::scan_solo_maps(&self.base, self.game.as_deref()) {
            Ok(maps) => self.solo_maps = maps,
            Err(error) => {
                eprintln!("Could not build Solo Game map catalog: {error}");
                self.solo_catalog_error = Some(error);
            }
        }
        self.solo_map_selected = self
            .solo_map_selected
            .min(self.solo_maps.len().saturating_sub(1));
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
    }

    fn ensure_solo_levelshot_texture(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.solo_maps.get(self.solo_map_selected) else {
            self.solo_levelshot_texture = None;
            self.solo_levelshot_texture_map = None;
            return;
        };
        let map_name = entry.map_name.clone();
        if self.solo_levelshot_texture_map.as_deref() == Some(map_name.as_str()) {
            return;
        }

        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = Some(map_name.clone());
        let Some(bytes) = self
            .solo_maps
            .get(self.solo_map_selected)
            .and_then(|entry| entry.levelshot.as_ref())
        else {
            return;
        };
        match image::load_from_memory_with_format(&bytes.bytes, bytes.format) {
            Ok(image) => {
                let rgba = image.into_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.solo_levelshot_texture = Some(ctx.load_texture(
                    format!("levelshot:{map_name}"),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
            Err(error) => {
                eprintln!("Levelshot {map_name} decode failed: {error}");
            }
        }
    }

    fn egui_solo_game_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "SOLO GAME",
            "Compiled BSPs discovered from loose base/maps files and mounted PK3s.",
        );

        self.ensure_solo_map_catalog();

        if let Some(error) = &self.solo_catalog_error {
            theme::banner(ui, &format!("Map scan failed: {error}"), theme::WARNING);
            return;
        }
        if self.solo_maps.is_empty() {
            theme::banner(ui, "No maps/*.bsp assets were found.", theme::WARNING);
            return;
        }

        self.solo_map_selected = self.solo_map_selected.min(self.solo_maps.len() - 1);
        self.ensure_solo_levelshot_texture(ui.ctx());
        let mut selected = self.solo_map_selected;
        let selected_name = self.solo_maps[selected].map_name.clone();

        let browser_height = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(330.0, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                ui.set_min_size(egui::vec2(330.0, browser_height));
                theme::section(ui, "MAPS", &format!("{} found", self.solo_maps.len()));
                let list_height = ui.available_height().max(1.0);
                egui::ScrollArea::vertical()
                    .id_salt("jka_solo_map_list")
                    .auto_shrink([false, false])
                    .max_height(list_height)
                    .show(ui, |ui| {
                        for (index, entry) in self.solo_maps.iter().enumerate() {
                            if rail_item(ui, &entry.map_name, index == selected).clicked() {
                                selected = index;
                            }
                        }
                    });
                },
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                theme::section(ui, "SELECTED MAP", "");
                theme::glow_label(ui, &selected_name, 18.0, theme::TEXT);
                ui.add_space(10.0);

                let preview_w = ui.available_width().max(1.0);
                let preview_box = egui::vec2(preview_w, 360.0);
                if let Some(texture) = &self.solo_levelshot_texture {
                    let source = texture.size_vec2();
                    let scale = (preview_box.x / source.x.max(1.0))
                        .min(preview_box.y / source.y.max(1.0));
                    let preview_size = source * scale;
                    ui.add(egui::Image::new(texture).fit_to_exact_size(preview_size));
                } else {
                    let placeholder = egui::vec2(preview_w, (preview_w * 0.5).min(360.0));
                    let (rect, _) = ui.allocate_exact_size(placeholder, egui::Sense::hover());
                    ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL);
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "NO LEVELSHOT",
                        egui::FontId::proportional(13.0),
                        theme::TEXT_DISABLED,
                    );
                }
                ui.add_space(14.0);
                if theme::primary_button(ui, "LOAD MAP").clicked() {
                    self.request_map(scene::MapSource::Bsp(selected_name.clone()));
                }
                },
            );
        });

        if selected != self.solo_map_selected {
            self.solo_map_selected = selected;
            self.solo_levelshot_texture = None;
            self.solo_levelshot_texture_map = None;
            self.egui_repaint_requested = true;
        }
    }

    fn ensure_demo_catalog(&mut self) {
        if self.demo_catalog_loaded {
            return;
        }

        self.demo_catalog_loaded = true;
        self.demo_entries.clear();
        self.demo_catalog_error = None;
        match frontend::scan_demos(&self.base, self.game.as_deref()) {
            Ok(demos) => self.demo_entries = demos,
            Err(error) => {
                eprintln!("Could not build Play Demo catalog: {error}");
                self.demo_catalog_error = Some(error);
            }
        }
        self.demo_selected = self
            .demo_selected
            .min(self.demo_entries.len().saturating_sub(1));
    }

    fn egui_play_demo_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "PLAY DEMO",
            "Jedi Academy .dm_26 demos discovered from loose base/demos files and mounted PK3s.",
        );

        self.ensure_demo_catalog();

        if let Some(error) = &self.demo_catalog_error {
            theme::banner(ui, &format!("Demo scan failed: {error}"), theme::WARNING);
            return;
        }
        if self.demo_entries.is_empty() {
            theme::banner(ui, "No demos/*.dm_26 assets were found.", theme::WARNING);
            return;
        }

        self.demo_selected = self.demo_selected.min(self.demo_entries.len() - 1);
        let mut selected = self.demo_selected;
        let selected_name = self.demo_entries[selected].demo_name.clone();

        let browser_height = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(330.0, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::vec2(330.0, browser_height));
                    theme::section(ui, "DEMOS", &format!("{} found", self.demo_entries.len()));
                    let list_height = ui.available_height().max(1.0);
                    egui::ScrollArea::vertical()
                        .id_salt("jka_demo_list")
                        .auto_shrink([false, false])
                        .max_height(list_height)
                        .show(ui, |ui| {
                            for (index, entry) in self.demo_entries.iter().enumerate() {
                                if rail_item(ui, &entry.demo_name, index == selected).clicked() {
                                    selected = index;
                                }
                            }
                        });
                },
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SELECTED DEMO", "");
                    theme::glow_label(ui, &selected_name, 18.0, theme::TEXT);
                    ui.add_space(10.0);

                    let placeholder_height = (ui.available_height() - 56.0)
                        .clamp(180.0, 360.0);
                    let placeholder_size = egui::vec2(
                        ui.available_width().max(1.0),
                        placeholder_height,
                    );
                    let (rect, _) = ui.allocate_exact_size(placeholder_size, egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::ZERO,
                        theme::CONTROL,
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "[PLACEHOLDER]",
                        egui::FontId::proportional(16.0),
                        theme::TEXT_DISABLED,
                    );

                    ui.add_space(14.0);
                    if theme::primary_button(ui, "PLAY DEMO").clicked() {
                        self.play_selected_demo();
                    }
                },
            );
        });

        if selected != self.demo_selected {
            self.demo_selected = selected;
            self.egui_repaint_requested = true;
        }
    }

    fn egui_top_bar(&mut self, root: &mut egui::Ui) {
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

    fn select_top_menu(&mut self, index: usize) {
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
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    fn egui_setup_tabs(&mut self, root: &mut egui::Ui) {
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

    fn egui_footer(&mut self, root: &mut egui::Ui) {
        let on_video = self.overlay == OverlayMode::Video && self.setup_selected == SETUP_TAB_VIDEO;
        let restart = on_video && self.video_restart_required();
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
                    let hint = if restart {
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
                            if theme::primary_button(ui, "APPLY VIDEO SETTINGS").clicked() {
                                self.egui_apply_video_requested = true;
                            }
                        });
                    }
                });
            });
    }

    fn egui_body(&mut self, root: &mut egui::Ui) {
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
                // The server browser is data-dense and uses a two-pane table/detail
                // layout, so it owns the full in-game body width. Other pages keep
                // the compact 720-point column so the live world remains visible.
                let server_browser_full_width =
                    self.overlay == OverlayMode::Game && self.menu_selected == 1;
                let content_w = if server_browser_full_width {
                    ui.available_width()
                } else {
                    ui.available_width().min(CONTENT_MAX_W)
                };
                // Compact pages hang off the left edge, while the Servers page
                // intentionally spans the complete central area.
                let left = ui.max_rect().left();
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, ui.max_rect().top()),
                    egui::pos2(left + content_w, ui.max_rect().bottom()),
                );
                // The Video page draws straight over the live scene -- its
                // captions carry their own halo instead (see `theme::glow_text`).
                // Every other page sits on the renderer's blurred backdrop,
                // where a solid panel reads better.
                let frame = if on_video {
                    egui::Frame::new().fill(theme::VIDEO_SCRIM)
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

    fn egui_page(&mut self, ui: &mut egui::Ui) {
        if self.overlay == OverlayMode::Video {
            match self.setup_selected {
                0 => self.egui_game_settings(ui),
                SETUP_TAB_VIDEO => self.egui_video_page(ui),
                SETUP_TAB_INPUT => self.egui_input_page(ui),
                SETUP_TAB_AUDIO => self.egui_audio_page(ui),
                4 => placeholder(
                    ui,
                    "NETWORK",
                    "Connection, download and snapshot options.",
                    &[
                        "Rate and snapshot frequency",
                        "Automatic downloads",
                        "Packet duplication and lag compensation",
                    ],
                ),
                SETUP_TAB_INTERFACE => self.egui_interface_page(ui),
                _ => unreachable!("setup tab index is bounded by SETUP_TABS"),
            }
            return;
        }

        match self.menu_selected {
            TOP_RESUME => self.egui_resume_page(ui),
            1 => self.egui_server_browser_page(ui),
            2 => placeholder(
                ui,
                "PROFILE",
                "Player identity and appearance.",
                &[
                    "Name and colours",
                    "Player model and skin",
                    "Saber hilt and blade colour",
                ],
            ),
            TOP_CONTROLS => self.egui_controls_page(ui),
            5 => placeholder(
                ui,
                "VOTE",
                "Server and gametype vote actions.",
                &["Call a map or gametype vote", "Kick and mute votes"],
            ),
            6 => placeholder(
                ui,
                "MOD",
                "Mod-specific menu slot.",
                &["Populated by the active mod"],
            ),
            _ => {}
        }
    }

    // -------------------------------------------------------------- pages --

    fn egui_resume_page(&mut self, ui: &mut egui::Ui) {
        let demo_rate = self
            .game_session
            .as_ref()
            .and_then(GameSession::playback_rate);
        let mode = match self.local_player.as_ref().map(|player| player.mode) {
            Some(JoinMode::Player) => "Playing",
            _ => "Spectating",
        };
        if let Some(rate) = demo_rate {
            theme::page_title(ui, "DEMO", if rate == 0.0 { "Playback paused." } else { "Demo playback controls." });
            theme::section(ui, "PLAYBACK", "Pause keeps the current demo time fixed; Play resumes at the previous non-zero demo speed.");
            let label = if rate == 0.0 { "PLAY" } else { "PAUSE" };
            if theme::primary_button(ui, label).clicked() {
                self.console_status = match self.game_session.as_mut() {
                    Some(playback) => match playback.toggle_pause(std::time::Instant::now()) {
                        Ok(0.0) => "DEMO PAUSED".into(),
                        Ok(rate) => format!("DEMO PLAYING AT {rate}X"),
                        Err(error) => error,
                    },
                    None => "NO DEMO IS PLAYING".into(),
                };
                self.egui_repaint_requested = true;
            }
            ui.add_space(8.0);
        } else {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
        }

        let can_join = self.live_connected()
            || self
                .local_player
                .as_ref()
                .is_some_and(LocalPlayer::can_join);

        theme::section(ui, "SESSION", "");
        let return_label = if demo_rate.is_some() { "Return to demo" } else { "Return to game" };
        if menu_action(ui, return_label, "Close the menu and return to the current view.", true) {
            self.set_overlay(OverlayMode::None);
            return;
        }
        if menu_action(
            ui,
            "Join game",
            if self.live_connected() {
                "Ask the server to leave spectator and join play."
            } else if can_join {
                "Spawn in at the next available spawn point."
            } else {
                "No spawn point is available on this map."
            },
            can_join,
        ) {
            self.join_as(JoinMode::Player);
            return;
        }
        if menu_action(
            ui,
            "Join spectate",
            if self.live_connected() {
                "Ask the server to join spectators."
            } else {
                "Watch the map with a free-flying camera."
            },
            true,
        ) {
            self.join_as(JoinMode::Spectator);
            return;
        }

        ui.add_space(12.0);
        theme::section(ui, "LEAVE", "Leave the current session or exit DinurdoJK.");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::primary_button(ui, "QUIT TO MAIN MENU").clicked() {
                self.disconnect_to_main_menu();
                return;
            }
            ui.add_space(10.0);
            if theme::ghost_button(ui, "QUIT TO DESKTOP").clicked() {
                self.request_quit();
            }
        });
    }

    fn join_as(&mut self, mode: JoinMode) {
        if self.live_connected() {
            let command = match mode {
                JoinMode::Player => "team free",
                JoinMode::Spectator => "team spectator",
            };
            self.forward_command_to_server(command);
            self.console_status = match mode {
                JoinMode::Player => "JOIN GAME REQUEST SENT".into(),
                JoinMode::Spectator => "JOIN SPECTATE REQUEST SENT".into(),
            };
            self.set_overlay(OverlayMode::None);
            return;
        }

        let Some(spawn) = self.spawns.get(self.spawn_index).copied() else {
            return;
        };
        let Some(player) = &mut self.local_player else {
            return;
        };
        match player.join(mode, spawn) {
            Ok(()) => {
                self.third_person_camera.reset();
                self.update_solo_player_view_and_presentation();
                self.set_overlay(OverlayMode::None);
            }
            Err(error) => {
                self.console_status = error;
                self.publish_ui();
            }
        }
    }

    fn egui_controls_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "CONTROLS",
            "Click a binding to rebind it.  Right-click clears it.",
        );

        if self.controls_waiting_for_key {
            theme::banner(
                ui,
                "Press any key, mouse button or wheel direction…  ESC cancels.",
                theme::WARNING,
            );
            ui.add_space(6.0);
        }

        let mut rebind = None;
        let mut clear = None;
        egui::ScrollArea::vertical()
            .id_salt("jka_controls_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut group = "";
                for (index, action) in keybinds::CONTROL_ACTIONS.iter().enumerate() {
                    if action.group != group {
                        group = action.group;
                        theme::section(ui, &group.to_uppercase(), "");
                    }
                    let waiting = self.controls_waiting_for_key && self.controls_selected == index;
                    let binding = self.bindings.display_for_command(action.command);
                    let tip = format!("Bound to the \"{}\" command.", action.command);
                    theme::row(ui, &title_case(action.label), &tip, theme::Reset::None, |ui| {
                        let (text, color) = if waiting {
                            ("PRESS A KEY…".to_owned(), theme::WARNING)
                        } else if binding == "UNBOUND" {
                            ("Unbound".to_owned(), theme::TEXT_DISABLED)
                        } else {
                            (binding.clone(), theme::ACCENT)
                        };
                        let response = binding_slot(ui, &text, color, waiting);
                        if response.clicked() {
                            rebind = Some(index);
                        }
                        if response.secondary_clicked() {
                            clear = Some(index);
                        }
                    });
                }
            });

        if let Some(index) = rebind {
            self.controls_selected = index;
            self.controls_waiting_for_key = true;
            self.publish_ui();
        } else if let Some(index) = clear {
            self.controls_selected = index;
            self.unbind_selected_control();
        }
    }

    fn egui_input_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "INPUT", "Mouse and client input timing.");

        theme::section(ui, "MOUSE", "");
        macro_rules! mouse_number {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal, $speed:expr, $decimals:expr) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.mouse_input.$field;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .speed($speed)
                                .max_decimals($decimals)
                                .update_while_editing(false),
                        )
                        .changed()
                    {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        mouse_number!(
            "Sensitivity",
            "sensitivity. TaystJK mouse sensitivity multiplier; stock default is 5. Raw input does not bypass this scaling.",
            sensitivity,
            "sensitivity",
            0.1,
            4
        );
        mouse_number!(
            "Yaw scale",
            "m_yaw. Horizontal mouse scale applied after sensitivity/acceleration; TaystJK default is 0.022.",
            yaw,
            "m_yaw",
            0.001,
            6
        );
        mouse_number!(
            "Pitch scale",
            "m_pitch. Vertical mouse scale applied after sensitivity/acceleration; TaystJK default is 0.022. A negative value inverts vertical look.",
            pitch,
            "m_pitch",
            0.001,
            6
        );
        mouse_number!(
            "Mouse acceleration",
            "cl_mouseAccel. TaystJK legacy style-0 acceleration; 0 disables it. With subframe input, DinurdoJK uses the raw-event interval as the acceleration timebase rather than waiting for a client frame.",
            accel,
            "cl_mouseAccel",
            0.01,
            4
        );
        theme::row(
            ui,
            "Subframe input",
            "cl_input_subframe. Applies mouse-look on each raw mouse event instead of waiting for the next client tick, reducing input latency.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.input_subframe) {
                    self.set_input_subframe(enabled);
                    self.egui_repaint_requested = true;
                }
            },
        );
    }

    fn egui_audio_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "AUDIO", "OpenJK mixer levels, Steam Audio spatial acoustics and native output diagnostics.");

        theme::section(ui, "MIX", "Changes are live and archived to jka-rust.cfg.");
        macro_rules! audio_slider {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.audio.$field;
                    let readout = format!("{:.0}%", value * 100.0);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        audio_slider!(
            "Effects / game",
            "s_volume. OpenJK game/effects volume; stock default is 0.5.",
            effects_volume,
            "s_volume"
        );
        audio_slider!(
            "Voice",
            "s_volumeVoice. OpenJK voice-channel level; stock default is 1.0.",
            voice_volume,
            "s_volumeVoice"
        );
        audio_slider!(
            "Music",
            "s_musicvolume. Archived now; background music playback is not connected yet.",
            music_volume,
            "s_musicvolume"
        );
        theme::row(
            ui,
            "Mute when unfocused",
            "s_muteWhenUnfocused. Mutes game and voice audio while another window has focus.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.mute_when_unfocused) {
                    let _ = self.set_console_cvar(
                        "s_muteWhenUnfocused",
                        if enabled { "1" } else { "0" },
                    );
                }
            },
        );

        theme::section(
            ui,
            "SPATIAL AUDIO",
            "Valve Steam Audio is optional. When disabled, DinurdoJK keeps the legacy OpenJK-compatible spatial mixer and skips acoustic BSP/bake preparation entirely.",
        );
        theme::row(
            ui,
            "Steam Audio",
            "s_steamAudio. Master gate for Steam Audio HRTF/occlusion/reflections/pathing. Turning this off prevents the Steam Audio runtime from being used and prevents acoustic geometry/bake work on subsequent map loads. Enabling it while a map is already loaded takes full environmental acoustics on the next map load.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio) {
                    let _ = self.set_console_cvar("s_steamAudio", if enabled { "1" } else { "0" });
                }
            },
        );

        theme::section(ui, "OUTPUT", "The active device opens with the CGame/demo sound presenter.");
        let info = self
            .game_session
            .as_ref()
            .and_then(|playback| playback.sound_presenter.as_ref())
            .and_then(|sound| sound.info());
        if let Some(info) = info {
            theme::row(
                ui,
                "Device format",
                "Actual CPAL output format. Source WAV/MP3 sample rates are retained and Rodio resamples them to this device rate.",
                theme::Reset::None,
                |ui| {
                    theme::hint(
                        ui,
                        &format!("{}ch · {} Hz · {}", info.channels, info.sample_rate, info.sample_format),
                        theme::TEXT,
                    );
                },
            );
            theme::row(
                ui,
                "Buffer",
                "CPAL/Rodio output buffer selected for the current device.",
                theme::Reset::None,
                |ui| theme::hint(ui, &info.buffer_size, theme::TEXT_DIM),
            );
            let overload = info.overload_samples > 0;
            theme::row(
                ui,
                "Mix headroom",
                "Peak is measured before the final safety limiter. Values above 1.0 mean the floating-point game mix would clip without output protection.",
                theme::Reset::None,
                |ui| {
                    theme::hint(
                        ui,
                        &format!(
                            "peak {:.2} · {} overload sample{} · {} voices",
                            info.peak_before_limiter,
                            info.overload_samples,
                            if info.overload_samples == 1 { "" } else { "s" },
                            info.active_voices
                        ),
                        if overload { theme::WARNING } else { theme::TEXT_DIM },
                    );
                },
            );
            theme::row(
                ui,
                "Steam Audio state",
                "Master feature gate plus the currently attached BSP acoustic geometry. Geometry ready is only stage one; the rows below separately report offline bake readiness and audible DSP activation.",
                theme::Reset::None,
                |ui| {
                    let state = if !info.steam_audio_enabled {
                        "Disabled".to_string()
                    } else if info.steam_audio_scene_triangles == 0 {
                        "Enabled · waiting for next RBSP map load".to_string()
                    } else {
                        format!("Geometry ready · {} acoustic tris", info.steam_audio_scene_triangles)
                    };
                    theme::hint(
                        ui,
                        &state,
                        if info.steam_audio_enabled { theme::TEXT } else { theme::TEXT_FAINT },
                    );
                },
            );
            theme::row(
                ui,
                "Offline acoustic bake",
                "Steam Audio floor probes plus baked reflections/reverb and pathing. Cache hits attach immediately; first-time bakes run in the background after CPU map preparation.",
                theme::Reset::None,
                |ui| {
                    let (state, color) = if !info.steam_audio_enabled {
                        ("Disabled".to_string(), theme::TEXT_FAINT)
                    } else if let Some(progress) = self.steam_audio_bake_progress {
                        (format!("Baking in background · {:.0}%", progress * 100.0), theme::TEXT)
                    } else if let Some(error) = &self.steam_audio_bake_error {
                        (format!("Bake failed · {error}"), theme::WARNING)
                    } else if info.steam_audio_bake_ready {
                        (
                            format!(
                                "Ready · {} probes · {:.2} MiB · {}{}",
                                info.steam_audio_probe_count,
                                info.steam_audio_bake_bytes as f64 / (1024.0 * 1024.0),
                                if info.steam_audio_runtime_validated { "validated" } else { "not validated" },
                                if info.steam_audio_bake_cache_hit { " · cache hit" } else { " · freshly baked" },
                            ),
                            theme::TEXT,
                        )
                    } else if info.steam_audio_scene_triangles > 0 {
                        ("Geometry ready · bake queued/not yet attached".to_string(), theme::TEXT_DIM)
                    } else {
                        ("Waiting for map load".to_string(), theme::TEXT_DIM)
                    };
                    theme::hint(ui, &state, color);
                },
            );
            theme::row(
                ui,
                "Steam Audio DSP",
                "Audible HRTF/direct occlusion/transmission/reflection/path processing. This is intentionally reported separately from the feature gate and bake so 'On' never implies that Steam Audio is already changing the mix.",
                theme::Reset::None,
                |ui| {
                    theme::hint(
                        ui,
                        if info.steam_audio_dsp_active {
                            "Active"
                        } else if info.steam_audio_bake_ready {
                            "Pending · baked environment ready; legacy mixer still audible"
                        } else {
                            "Pending · legacy mixer currently audible"
                        },
                        if info.steam_audio_dsp_active { theme::TEXT } else { theme::TEXT_DIM },
                    );
                },
            );
        } else {
            theme::row(
                ui,
                "Device format",
                "Audio output has not been opened by the active CGame/demo presenter yet.",
                theme::Reset::None,
                |ui| theme::hint(ui, "Not active", theme::TEXT_FAINT),
            );
        }
    }

    // ---------------------------------------------------------- video rail --

    fn egui_video_rail(&mut self, root: &mut egui::Ui) {
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
    fn tick_egui_confirmation(&mut self) -> bool {
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
        if !self.egui_repaint_requested && self.egui_last_frame.elapsed() < Duration::from_millis(33)
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

// ------------------------------------------------------------- free widgets --

/// The legacy control table stores SHOUTED labels; the menu reads them back in
/// sentence case so it matches every other page.
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start_of_word = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            start_of_word = true;
            out.push(ch);
        } else if start_of_word {
            start_of_word = false;
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

/// Group heading in the rail. Deliberately unlike a rail item: larger, bold,
/// bright, and closed off with a rule, because at the same weight as the
/// entries beneath it people read it as one more thing to click.
fn rail_group(ui: &mut egui::Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 30.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter().clone();
    theme::glow_text(
        &painter,
        egui::pos2(rect.left() + GUTTER - 12.0, rect.center().y + 2.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(14.0),
        theme::TEXT,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + GUTTER - 12.0, rect.bottom() - 1.0),
            egui::vec2(rect.width() - (GUTTER - 12.0) - 12.0, 1.0),
        ),
        egui::CornerRadius::ZERO,
        theme::LINE,
    );
    ui.add_space(4.0);
}

fn rail_item(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 27.0),
        egui::Sense::click(),
    );
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL_SELECTED);
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            egui::CornerRadius::ZERO,
            theme::ACCENT,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL);
    }
    painter.text(
        egui::pos2(rect.left() + GUTTER, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        if selected {
            theme::TEXT
        } else if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response
}

/// A wide, flat action row used by the Resume page.
fn menu_action(ui: &mut egui::Ui, label: &str, detail: &str, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 48.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let painter = ui.painter().clone();
    let hovered = enabled && response.hovered();
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        if hovered {
            theme::CONTROL_HOVER
        } else {
            theme::CONTROL
        },
    );
    if hovered {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            egui::CornerRadius::ZERO,
            theme::ACCENT,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 16.0, rect.top() + 15.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        if enabled { theme::TEXT } else { theme::TEXT_DISABLED },
    );
    painter.text(
        egui::pos2(rect.left() + 16.0, rect.top() + 33.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::proportional(11.5),
        theme::TEXT_FAINT,
    );
    ui.add_space(6.0);
    enabled && response.clicked()
}

fn binding_slot(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    waiting: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(190.0, 24.0), egui::Sense::click_and_drag());
    let painter = ui.painter().clone();
    let fill = if waiting {
        theme::CONTROL_SELECTED
    } else if response.hovered() {
        theme::CONTROL_HOVER
    } else {
        theme::CONTROL
    };
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, if waiting { theme::WARNING } else { theme::LINE }),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.5),
        color,
    );
    response
}

fn placeholder(ui: &mut egui::Ui, title: &str, detail: &str, planned: &[&str]) {
    theme::page_title(ui, title, detail);
    theme::section(ui, "NOT IMPLEMENTED YET", "Planned for this page:");
    for item in planned {
        theme::row(ui, item, "", theme::Reset::None, |ui| {
            theme::glow_label(ui, "—", 12.5, theme::TEXT_DISABLED);
        });
    }
}

fn confirmation_dialog(ctx: &egui::Context, seconds: u32) -> Option<bool> {
    let mut result = None;
    let screen = ctx.content_rect();
    // Painted straight onto the background layer: an `Area` clips to its own
    // (zero-sized) content rect, so a full-screen scrim drawn inside one never
    // shows up.
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("jka_video_confirmation_dim"),
    ))
    .rect_filled(
        screen,
        egui::CornerRadius::ZERO,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 170),
    );
    egui::Area::new(egui::Id::new("jka_video_confirmation"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(
            screen.center().x - 250.0,
            screen.center().y - 90.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                .inner_margin(egui::Margin::same(22))
                .show(ui, |ui| {
                    ui.set_width(456.0);
                    theme::label(
                        ui,
                        theme::plain("KEEP THESE DISPLAY SETTINGS?", 17.0, theme::TEXT).strong(),
                    );
                    ui.add_space(8.0);
                    theme::label(
                        ui,
                        theme::plain(
                            "The previous display mode is restored automatically if you do nothing.",
                            12.5,
                            theme::TEXT_DIM,
                        ),
                    );
                    ui.add_space(12.0);
                    theme::banner(
                        ui,
                        &format!("Reverting in {seconds} second{}", if seconds == 1 { "" } else { "s" }),
                        theme::WARNING,
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "KEEP  (ENTER)").clicked() {
                            result = Some(true);
                        }
                        ui.add_space(10.0);
                        if theme::ghost_button(ui, "REVERT  (ESC)").clicked() {
                            result = Some(false);
                        }
                    });
                });
        });
    result
}
