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
use super::map_editor::MapEditTool;
use super::ui_catalog::CatalogPayload;
use super::*;

const TOP_ITEMS: [&str; 7] = [
    "GAME", "SERVERS", "PROFILE", "CONTROLS", "SETUP", "VOTE", "MOD",
];
const SETUP_TABS: [&str; 6] = ["GAME", "CAMERA", "VIDEO", "AUDIO", "NETWORK", "INTERFACE"];

const TOP_RESUME: usize = 0;
const TOP_PROFILE: usize = 2;
const TOP_CONTROLS: usize = 3;
pub(super) const TOP_SETUP: usize = 4;

const SETUP_TAB_GAME: usize = 0;
pub(super) const SETUP_TAB_CAMERA: usize = 1;
const SETUP_TAB_VIDEO: usize = 2;
const SETUP_TAB_AUDIO: usize = 3;
const SETUP_TAB_NETWORK: usize = 4;
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
const FRONTEND_SCENE_FADE_IN_SECS: f32 = 4.0;

const PROFILE_SECTIONS: [&str; 5] = ["IDENTITY", "MODEL", "FORCE", "SABER", "COSMETICS"];
const PROFILE_IDENTITY: usize = 0;
const PROFILE_MODEL: usize = 1;
const PROFILE_FORCE: usize = 2;
const PROFILE_SABER: usize = 3;
const PROFILE_COSMETICS: usize = 4;

// OpenJK bg_misc.c / TaystJK ui_force.c. Keep Profile force editing on the
// protocol-26 rank-side-18digits representation rather than inventing a new
// loadout format.
const FORCE_MASTERY_POINTS: [i32; 8] = [0, 5, 10, 20, 30, 50, 75, 100];
const FORCE_MASTERY_NAMES: [&str; 8] = [
    "Uninitiated", "Initiate", "Padawan", "Jedi", "Jedi Adept", "Jedi Guardian",
    "Jedi Knight", "Jedi Master",
];
const FORCE_COSTS: [[i32; 4]; 18] = [
    [0, 2, 4, 6], [0, 0, 2, 6], [0, 2, 4, 6], [0, 1, 3, 6], [0, 1, 3, 6],
    [0, 4, 6, 8], [0, 1, 3, 6], [0, 2, 5, 8], [0, 4, 6, 8], [0, 2, 5, 8],
    [0, 1, 3, 6], [0, 1, 3, 6], [0, 1, 3, 6], [0, 2, 4, 6], [0, 2, 5, 8],
    [0, 1, 5, 8], [0, 1, 5, 8], [0, 4, 6, 8],
];
const FORCE_SIDES: [i32; 18] = [1, 0, 0, 0, 0, 1, 2, 2, 2, 1, 1, 1, 2, 2, 0, 0, 0, 0];
const FORCE_NAMES: [&str; 18] = [
    "Heal", "Jump", "Speed", "Push", "Pull", "Mind Trick", "Grip", "Lightning",
    "Rage", "Protect", "Absorb", "Team Heal", "Team Force", "Drain", "Seeing",
    "Saber Attack", "Saber Defense", "Saber Throw",
];
// Keep neutral powers visually stable at the top, then append only the chosen
// alignment. This mirrors the stock allocation rules while making the modern
// editor much easier to scan.
const FORCE_NEUTRAL_ORDER: [usize; 8] = [1, 2, 3, 4, 14, 15, 16, 17];
const FORCE_LIGHT_ORDER: [usize; 5] = [0, 5, 9, 10, 11];
const FORCE_DARK_ORDER: [usize; 5] = [6, 7, 8, 13, 12];

#[derive(Clone, Copy)]
struct ProfileForceConfig {
    rank: u8,
    side: u8,
    powers: [u8; 18],
}

impl ProfileForceConfig {
    fn parse(value: &str) -> Self {
        let mut out = Self { rank: 7, side: 1, powers: [0; 18] };
        let mut parts = value.trim().splitn(3, '-');
        out.rank = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(7).min(7);
        out.side = match parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(1) { 2 => 2, _ => 1 };
        if let Some(powers) = parts.next() {
            for (index, byte) in powers.bytes().take(18).enumerate() {
                if byte.is_ascii_digit() {
                    out.powers[index] = (byte - b'0').min(3);
                }
            }
        }
        // ui_force.c always gives the player the free first Jump level.
        out.powers[1] = out.powers[1].max(1);
        out
    }

    fn serialize(self) -> String {
        let powers = self.powers.iter().map(|level| char::from(b'0' + (*level).min(3))).collect::<String>();
        format!("{}-{}-{powers}", self.rank, self.side)
    }

    fn budget(self) -> i32 {
        FORCE_MASTERY_POINTS[self.rank.min(7) as usize]
    }

    fn used(self, free_saber: bool) -> i32 {
        let mut total = 0;
        for (power, &level) in self.powers.iter().enumerate() {
            for rank in 1..=level.min(3) as usize {
                if (power == 1 && rank == 1)
                    || (free_saber && (power == 15 || power == 16) && rank == 1)
                {
                    continue;
                }
                total += FORCE_COSTS[power][rank];
            }
        }
        total
    }

    fn normalize(&mut self, max_rank: u8, disabled: u32, gametype: i32, free_saber: bool) {
        self.rank = self.rank.min(max_rank.min(7));
        if self.side != 1 && self.side != 2 {
            self.side = 2;
        }

        for power in 0..18 {
            self.powers[power] = self.powers[power].min(3);
            if self.powers[power] != 0
                && FORCE_SIDES[power] != 0
                && FORCE_SIDES[power] != self.side as i32
            {
                self.powers[power] = 0;
            }
            if self.powers[power] != 0 && disabled & (1u32 << power) != 0 {
                self.powers[power] = 0;
            }
        }
        if gametype < 6 {
            self.powers[11] = 0;
            self.powers[12] = 0;
        }

        // Port BG_LegalizedForcePowers' over-budget reduction order. It drains
        // lower-ranked powers first, preserving higher investments, with the
        // saber attack/defense/throw dependency handled exactly like OpenJK.
        let allowed = self.budget();
        let mut used = self.used(free_saber);
        if used > allowed {
            let min_saber = u8::from(free_saber);
            let mut attempted_cycles = 0;
            let mut power_cycle = 2u8;
            while used > allowed {
                for power in 0..18 {
                    if used <= allowed {
                        break;
                    }
                    if self.powers[power] != 0 && self.powers[power] < power_cycle {
                        if power == 15
                            && (self.powers[16] > min_saber || self.powers[17] > 0)
                        {
                            let which = if self.powers[17] != 0 { 17 } else { 16 };
                            while self.powers[which] > 0 && used > allowed {
                                let level = self.powers[which] as usize;
                                if self.powers[which] > 1
                                    || ((which != 15 || !free_saber)
                                        && (which != 16 || !free_saber))
                                {
                                    used -= FORCE_COSTS[which][level];
                                    self.powers[which] -= 1;
                                } else {
                                    break;
                                }
                            }
                        } else {
                            while self.powers[power] > 0 && used > allowed {
                                let level = self.powers[power] as usize;
                                if self.powers[power] > 1
                                    || (power != 1
                                        && (power != 15 || !free_saber)
                                        && (power != 16 || !free_saber))
                                {
                                    used -= FORCE_COSTS[power][level];
                                    self.powers[power] -= 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
                power_cycle = power_cycle.saturating_add(1);
                attempted_cycles += 1;
                if attempted_cycles > 18 {
                    break;
                }
            }
            if used > allowed {
                self.powers = [0; 18];
            }
        }

        if free_saber {
            self.powers[15] = self.powers[15].max(1);
            self.powers[16] = self.powers[16].max(1);
        }
        self.powers[1] = self.powers[1].max(1);

        // BG_LegalizedForcePowers has deliberate special handling for these
        // three disabled powers (all-force-disabled servers depend on it).
        if disabled & (1u32 << 1) != 0 {
            self.powers[1] = 1;
        }
        if disabled & (1u32 << 15) != 0 {
            self.powers[15] = 3;
        }
        if disabled & (1u32 << 16) != 0 {
            self.powers[16] = 3;
        }
        if self.powers[15] == 0 {
            self.powers[16] = 0;
            self.powers[17] = 0;
        }
    }
}


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
    Effects,
    Shadows,
    Reflections,
    PostProcessing,
    DebugTools,
    Physics,
    Sun,
    Clouds,
    Weather,
    Surface,
    Water,
}

impl VideoSection {
    const RENDERING: [(Self, &'static str); 10] = [
        (Self::Display, "Display"),
        (Self::ImageQuality, "Image quality"),
        (Self::Visibility, "Visibility"),
        (Self::Models, "Models"),
        (Self::Lighting, "Lighting"),
        (Self::Effects, "Effects"),
        (Self::Shadows, "Shadows"),
        (Self::Reflections, "Reflections"),
        (Self::PostProcessing, "Post processing"),
        (Self::Physics, "Physics"),
    ];
    const ENVIRONMENT: [(Self, &'static str); 5] = [
        (Self::Sun, "Sun"),
        (Self::Clouds, "Clouds"),
        (Self::Weather, "Weather"),
        (Self::Surface, "Surface"),
        (Self::Water, "Water"),
    ];
    const TOOLS: [(Self, &'static str); 1] = [(Self::DebugTools, "Debug & tools")];

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
        self.map_edit_toolbar_rect = None;
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
    }

    /// egui owns every menu overlay, not just the Video page.
    pub(super) fn egui_menu_active(&self) -> bool {
        let blocking_download_ui = self.missing_map_prompt.is_some()
            || self.demo_missing_map_prompt.is_some()
            || self.live_download.is_some()
            || self.live_join_ui.is_some();
        let asset_viewer_behind_console = self.front_end
            && self.frontend_page == FrontendPage::AssetViewer
            && self.overlay == OverlayMode::Console;
        (blocking_download_ui
            || asset_viewer_behind_console
            || matches!(self.overlay, OverlayMode::Game | OverlayMode::Video | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::CameraEdit | OverlayMode::MapEdit | OverlayMode::EntityGraph | OverlayMode::Trace))
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
    pub(super) fn egui_paint_needed(&self) -> bool {
        self.egui_menu_active()
            || (self.video.draw_entities
                && self.video_confirmation.is_none()
                && self.loading.is_none()
                && !self.video.skip_ui
                && self.window.is_some()
                && self.render.is_some())
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
                && (self.missing_map_prompt.is_some()
                    || self.demo_missing_map_prompt.is_some()
                    || self.live_download.is_some()
                    || self.live_join_ui.is_some())
            {
                // The missing-map/download flow is modal. Do not let the
                // ordinary in-game Escape handler obscure it.
                return true;
            }
            if matches!(code, KeyCode::PrintScreen | KeyCode::Backquote | KeyCode::Escape) {
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

    pub(super) fn tick_egui_menu(&mut self) {
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

    fn egui_missing_map_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.missing_map_prompt.clone() else { return };
        let can_auto = self.network.allow_http_downloads || self.network.allow_legacy_downloads;
        #[derive(Clone, Copy)]
        enum Choice { Abort, Connect, Download }
        let mut choice = None;
        egui::Area::new(egui::Id::new("missing_map_prompt"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        theme::page_title(ui, "MAP NOT FOUND", "The server uses a map that is not installed locally.");
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&prompt.map_name).size(22.0).strong().color(theme::TEXT));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(&prompt.reason).color(theme::TEXT_DIM));
                        ui.add_space(16.0);
                        ui.label("Choose how to continue:");
                        ui.add_space(10.0);
                        if theme::primary_button(ui, "AUTO-DOWNLOAD & CONNECT").clicked() {
                            if can_auto { choice = Some(Choice::Download); }
                        }
                        if !can_auto {
                            ui.label(egui::RichText::new("Autodownload is disabled in Setup > Network.").color(theme::WARNING));
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if theme::ghost_button(ui, "CONNECT ANYWAY").clicked() {
                                choice = Some(Choice::Connect);
                            }
                            ui.add_space(8.0);
                            if theme::ghost_button(ui, "ABORT").clicked() {
                                choice = Some(Choice::Abort);
                            }
                        });
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(
                            "HTTP is preferred when advertised by the server; legacy JKA download is the fallback when enabled."
                        ).small().color(theme::TEXT_DIM));
                    });
            });
        match choice {
            Some(Choice::Abort) => self.choose_missing_map_abort(),
            Some(Choice::Connect) => self.choose_missing_map_connect_anyway(),
            Some(Choice::Download) => self.choose_missing_map_autodownload(),
            None => {}
        }
    }

    fn egui_demo_missing_map_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.demo_missing_map_prompt.clone() else { return };
        #[derive(Clone, Copy)]
        enum Choice { Continue, Exit }
        let mut choice = None;
        egui::Area::new(egui::Id::new("demo_missing_map_prompt"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        theme::page_title(
                            ui,
                            "DEMO MAP NOT FOUND",
                            "This demo was recorded on a map that is not installed locally.",
                        );
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&prompt.map_name).size(22.0).strong().color(theme::TEXT));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(&prompt.reason).color(theme::TEXT_DIM));
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(
                            "You can still play the recorded snapshots, but world geometry and map-dependent effects may be missing."
                        ).color(theme::TEXT));
                        ui.add_space(12.0);
                        if theme::primary_button(ui, "CONTINUE ANYWAY").clicked() {
                            choice = Some(Choice::Continue);
                        }
                        ui.add_space(8.0);
                        if theme::ghost_button(ui, "EXIT").clicked() {
                            choice = Some(Choice::Exit);
                        }
                    });
            });
        match choice {
            Some(Choice::Continue) => self.choose_demo_missing_map_continue(),
            Some(Choice::Exit) => self.choose_demo_missing_map_exit(),
            None => {}
        }
    }

    fn egui_live_join_pipeline(&mut self, root: &mut egui::Ui) {
        let Some(join) = self.live_join_ui.clone() else { return };

        // Own the whole frame for the duration of an autodownload-assisted join.
        // The renderer may still have the frontend scene or the normal map-load
        // splash underneath us; an opaque backdrop prevents either from flashing
        // between connection, download, filesystem refresh and map preparation.
        root.painter().rect_filled(root.max_rect(), 0.0, egui::Color32::BLACK);

        let (phase, detail, overall, secondary, package_line) =
            if let Some(download) = self.live_download.as_ref() {
                let count = download.files.len().max(1);
                let current_fraction = download
                    .total
                    .filter(|total| *total > 0)
                    .map(|total| (download.received as f32 / total as f32).clamp(0.0, 1.0));
                let package_fraction = ((download.index as f32 + current_fraction.unwrap_or(0.0))
                    / count as f32)
                    .clamp(0.0, 1.0);
                let package_line = download.current().map(|spec| {
                    format!(
                        "PACKAGE {} / {}  ·  {}",
                        (download.index + 1).min(download.files.len()),
                        download.files.len(),
                        spec.remote_name
                    )
                });
                let transfer_text = if let Some(total) = download.total {
                    format!(
                        "{:.1} / {:.1} MiB",
                        download.received as f64 / 1_048_576.0,
                        total as f64 / 1_048_576.0
                    )
                } else {
                    format!("{:.1} MiB", download.received as f64 / 1_048_576.0)
                };
                (
                    "DOWNLOADING CONTENT",
                    download.status.clone(),
                    0.10 + 0.55 * package_fraction,
                    Some((current_fraction.unwrap_or(0.0), transfer_text, current_fraction.is_none())),
                    package_line,
                )
            } else if let Some(loading) = self.loading.as_ref() {
                let fraction = loading.progress_fraction();
                let (phase, detail) = if loading.preparation_finished {
                    (
                        "ENTERING GAME",
                        "Map preparation is complete; uploading the world to the renderer...".to_owned(),
                    )
                } else {
                    (
                        "LOADING MAP",
                        "Preparing BSP, collision, shaders and map assets...".to_owned(),
                    )
                };
                (
                    phase,
                    detail,
                    0.75 + 0.23 * fraction,
                    Some((fraction, format!("Preparing {}", loading.name), false)),
                    None,
                )
            } else {
                match join.phase {
                    LiveJoinUiPhase::CheckingContent => (
                        "CHECKING CONTENT",
                        join.detail.clone(),
                        0.08,
                        None,
                        None,
                    ),
                    LiveJoinUiPhase::Synchronizing => (
                        "SYNCHRONIZING",
                        join.detail.clone(),
                        0.70,
                        None,
                        None,
                    ),
                }
            };

        egui::Area::new(egui::Id::new("live_join_pipeline"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(28))
                    .show(ui, |ui| {
                        ui.set_width(620.0);
                        theme::page_title(ui, "JOINING SERVER", &format!("Preparing {}", join.map_name));
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(phase).size(21.0).strong().color(theme::TEXT));
                        ui.label(egui::RichText::new(detail).color(theme::TEXT_DIM));
                        ui.add_space(16.0);

                        let overall_text = format!("Overall  {:.0}%", overall.clamp(0.0, 0.99) * 100.0);
                        ui.add(egui::ProgressBar::new(overall.clamp(0.0, 0.99)).text(overall_text));

                        if let Some(line) = package_line {
                            ui.add_space(14.0);
                            ui.label(egui::RichText::new(line).strong().color(theme::TEXT));
                        } else {
                            ui.add_space(14.0);
                        }
                        if let Some((fraction, text, animate)) = secondary {
                            let mut bar = egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).text(text);
                            if animate {
                                bar = bar.animate(true);
                            }
                            ui.add(bar);
                        } else {
                            ui.add(egui::ProgressBar::new(0.0).animate(true).text("Working..."));
                        }

                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            for (index, label) in ["CONTENT", "SYNC", "MAP", "ENTER"].iter().enumerate() {
                                if index > 0 {
                                    ui.label(egui::RichText::new("  ›  ").color(theme::TEXT_DIM));
                                }
                                let active = match phase {
                                    "CHECKING CONTENT" | "DOWNLOADING CONTENT" => index == 0,
                                    "SYNCHRONIZING" => index == 1,
                                    "LOADING MAP" => index == 2,
                                    "ENTERING GAME" => index == 3,
                                    _ => false,
                                };
                                let text = egui::RichText::new(*label).small();
                                ui.label(if active {
                                    text.strong().color(theme::TEXT)
                                } else {
                                    text.color(theme::TEXT_DIM)
                                });
                            }
                        });
                    });
            });
    }

    fn egui_download_dialog(&mut self, root: &mut egui::Ui) {
        let Some(state) = self.live_download.as_ref() else { return };
        let map_name = state.map_name.clone();
        let status = state.status.clone();
        let file_name = state.current().map(|spec| {
            format!("{} · {}", spec.referenced_name, spec.remote_name)
        }).unwrap_or_default();
        let file_index = (state.index + 1).min(state.files.len());
        let file_count = state.files.len();
        let received = state.received;
        let total = state.total;
        let fraction = total.filter(|total| *total > 0).map(|total| (received as f32 / total as f32).clamp(0.0, 1.0));
        egui::Area::new(egui::Id::new("map_autodownload_progress"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        theme::page_title(ui, "DOWNLOADING MAP", &format!("Preparing {map_name} before joining."));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(format!("Package {file_index} / {file_count}")).strong());
                        ui.label(&file_name);
                        ui.label(egui::RichText::new(status).small().color(theme::TEXT_DIM));
                        ui.add_space(8.0);
                        let text = if let Some(total) = total {
                            format!("{:.1} / {:.1} MiB", received as f64 / 1_048_576.0, total as f64 / 1_048_576.0)
                        } else {
                            format!("{:.1} MiB", received as f64 / 1_048_576.0)
                        };
                        let mut bar = egui::ProgressBar::new(fraction.unwrap_or(0.0)).text(text).show_percentage();
                        if fraction.is_none() { bar = bar.animate(true); }
                        ui.add(bar);
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Downloaded PK3s are installed as dl_*.pk3 and the client requests a fresh gamestate before loading the world.").small().color(theme::TEXT_DIM));
                    });
            });
    }

    // ------------------------------------------------------------- chrome --

    fn build_egui_menu(&mut self, ui: &mut egui::Ui) {
        if self.video.draw_entities {
            self.egui_entity_labels(ui);
        }
        if !self.egui_menu_active() {
            // Only the label painter wanted this frame; no menu to open.
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

    fn egui_vgs_menu(&mut self, root: &mut egui::Ui) {
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
                            if menu == crate::vgs::Menu::Main { "" } else { menu.title() },
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
                    egui::Stroke::new(1.0_f32, if column % 4 == 0 { major } else { line }),
                );
                x += spacing;
                column += 1;
            }
            let mut y = full.top();
            let mut row = 0usize;
            while y <= full.bottom() {
                painter.line_segment(
                    [egui::pos2(full.left(), y), egui::pos2(full.right(), y)],
                    egui::Stroke::new(1.0_f32, if row % 4 == 0 { major } else { line }),
                );
                y += spacing;
                row += 1;
            }
        }

        let rect_ctx = ui::HudRectContext::new(
            self.movement_keys_hud,
            &self.video,
            &self.perf,
            self.threads.len(),
            &self.japro_cg,
        );
        // The profiler panel is huge; register it first so the elements it
        // overlaps stay on top and remain grabbable.
        let mut draw_order = HudElementId::ALL.to_vec();
        draw_order.sort_by_key(|id| *id != HudElementId::Fps);

        let mut layout_changed = false;
        let mut drag_ended = false;
        for id in draw_order {
            let layout = self.hud_layout.element(id);
            let hud_rect = ui::hud_element_rect(id, layout, &rect_ctx, size.width, size.height);
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
                egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, outline),
                egui::StrokeKind::Outside,
            );
            let disabled = match id {
                HudElementId::MovementKeys => self.movement_keys_hud.mode == 0,
                HudElementId::Fps => self.video.draw_fps == 0,
                _ => false,
            };
            let label = if disabled {
                format!("{} (off)", id.label())
            } else {
                id.label().to_owned()
            };
            // Keep the tag on screen for elements hugging the top edge.
            let (label_pos, label_align) = if rect.top() < 24.0 {
                (rect.left_bottom() + egui::vec2(0.0, 8.0), egui::Align2::LEFT_TOP)
            } else {
                (rect.left_top() + egui::vec2(0.0, -8.0), egui::Align2::LEFT_BOTTOM)
            };
            painter.text(
                label_pos,
                label_align,
                label,
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
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
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

    /// `r_drawEntities` classname labels: one line of text above each
    /// positioned map entity, color-matched to its category and the box drawn
    /// by `App::rebuild_entity_markers`/`DebugVolumeRenderer`. Called first in
    /// `build_egui_menu`, before any menu/overlay content, and painted onto
    /// `ui`'s own layer (not a separate one — a same-order sibling layer
    /// created mid-frame paints on top of the frame's base layer regardless of
    /// call order, which is what put labels over the menu) so immediate-mode
    /// draw order does what it looks like: labels first, so anything the menu
    /// draws afterward on the same layer covers them.
    fn egui_entity_labels(&mut self, ui: &mut egui::Ui) {
        const MAX_LABEL_DISTANCE: f32 = 4096.0;
        let Some(graph) = self.entity_graph.as_deref() else { return };
        let Some(window) = self.window.as_ref() else { return };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let pixels_per_point = ui.ctx().pixels_per_point().max(0.001);
        let painter = ui.painter().clone();
        let eye = self.camera.position;
        let font = egui::FontId::proportional(12.0);
        for entity in &graph.entities {
            if !entity.positioned || entity.classname.is_empty() {
                continue;
            }
            let origin = self.live_entity_origin(entity);
            let render_point = glam::Vec3::from_array(scene::render_position(origin));
            if eye.distance(render_point) > MAX_LABEL_DISTANCE {
                continue;
            }
            let Some(screen) = self.camera.project_to_screen(size.width, size.height, render_point) else {
                continue;
            };
            let point = egui::pos2(screen.x / pixels_per_point, screen.y / pixels_per_point);
            let [r, g, b, _] = entity.category.color();
            theme::glow_text(
                &painter,
                point,
                egui::Align2::CENTER_BOTTOM,
                &entity.classname,
                font.clone(),
                egui::Color32::from_rgb(r, g, b),
            );
        }
    }

    fn egui_map_editor(&mut self, root: &mut egui::Ui) {
        self.map_edit_toolbar_rect = None;
        let Some(window) = self.window.as_ref() else { return };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 { return; }

        let pixels_per_point = root.ctx().pixels_per_point().max(0.001);
        let viewport_points = egui::vec2(
            size.width as f32 / pixels_per_point,
            size.height as f32 / pixels_per_point,
        );
        let view_proj = self.camera.view_projection(size.width, size.height);
        let project = |point_jka: [f64; 3]| -> Option<egui::Pos2> {
            let render = scene::render_position(point_jka.map(|v| v as f32));
            let clip = view_proj * glam::Vec3::from_array(render).extend(1.0);
            if clip.w <= 0.001 { return None; }
            let ndc = clip.truncate() / clip.w;
            if ndc.z < -0.1 || ndc.z > 1.1 { return None; }
            Some(egui::pos2(
                (ndc.x * 0.5 + 0.5) * viewport_points.x,
                (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_points.y,
            ))
        };

        if let Some(editor) = self.map_editor.as_ref() {
            let painter = root.painter().clone();
            let changed = editor.changed_geometries();
            let selected = editor.selected_geometry();
            let selected_face = editor.selected.map(|selection| selection.face);

            for geometry in changed {
                for face in geometry.faces {
                    if face.vertices.len() < 2 { continue; }
                    for edge in 0..face.vertices.len() {
                        let a = face.vertices[edge];
                        let b = face.vertices[(edge + 1) % face.vertices.len()];
                        if let (Some(a), Some(b)) = (project(a), project(b)) {
                            painter.line_segment(
                                [a, b],
                                egui::Stroke::new(0.75_f32, egui::Color32::from_rgba_unmultiplied(255, 166, 64, 90)),
                            );
                        }
                    }
                }
            }
            if let Some(geometry) = selected {
                for face in geometry.faces {
                    if face.vertices.len() < 2 { continue; }
                    let selected_face_line = selected_face == Some(face.face_index);
                    for edge in 0..face.vertices.len() {
                        let a = face.vertices[edge];
                        let b = face.vertices[(edge + 1) % face.vertices.len()];
                        if let (Some(a), Some(b)) = (project(a), project(b)) {
                            painter.line_segment(
                                [a, b],
                                egui::Stroke::new(
                                    if selected_face_line { 1.75_f32 } else { 1.0_f32 },
                                    if selected_face_line {
                                        egui::Color32::from_rgba_unmultiplied(255, 214, 92, 210)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 220, 255, 150)
                                    },
                                ),
                            );
                        }
                    }
                }
            }
        }

        let mut save_requested = false;
        let mut revert_requested = false;
        let mut close_requested = false;
        let mut snapshot_refresh_requested = false;
        let toolbar = egui::Area::new(egui::Id::new("jka_map_edit_toolbar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(8, 13, 20, 242))
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        let Some(editor) = self.map_editor.as_mut() else {
                            ui.horizontal(|ui| {
                                theme::glow_label(ui, "MAP EDIT", 13.0, theme::TEXT);
                                ui.label(egui::RichText::new("This source map is not a writable loose .map file.").color(theme::WARNING));
                                if ui.button("DONE").clicked() { close_requested = true; }
                            });
                            return;
                        };

                        ui.horizontal(|ui| {
                            theme::glow_label(ui, "MAP EDIT", 13.0, theme::TEXT);
                            ui.separator();
                            if ui.selectable_label(editor.tool == MapEditTool::MoveBrush, "BRUSH MOVE").clicked() {
                                editor.cancel_drag();
                                snapshot_refresh_requested = true;
                                editor.tool = MapEditTool::MoveBrush;
                                editor.status = "Brush Move: click a brush and drag in the camera plane.".into();
                            }
                            if ui.selectable_label(editor.tool == MapEditTool::MoveFace, "FACE RESIZE").clicked() {
                                editor.cancel_drag();
                                snapshot_refresh_requested = true;
                                editor.tool = MapEditTool::MoveFace;
                                editor.status = "Face Resize: click a face and drag along its normal.".into();
                            }
                            ui.separator();
                            ui.label("Grid");
                            ui.add(egui::DragValue::new(&mut editor.grid).range(0.125..=256.0).speed(1.0));
                            ui.separator();
                            let pending = editor.pending_changes;
                            ui.label(egui::RichText::new(format!("{pending} pending change{}", if pending == 1 { "" } else { "s" }))
                                .color(if pending == 0 { theme::TEXT_DIM } else { theme::WARNING }));
                            if ui.add_enabled(pending != 0, egui::Button::new("SAVE")).clicked() {
                                save_requested = true;
                            }
                            if ui.add_enabled(pending != 0, egui::Button::new("REVERT")).clicked() {
                                revert_requested = true;
                            }
                            if ui.button("DONE").clicked() { close_requested = true; }
                        });
                        ui.horizontal(|ui| {
                            let selection = editor.selected.map_or_else(
                                || "No brush selected".to_owned(),
                                |selection| format!(
                                    "entity {} / brush {} / face {}  {}",
                                    selection.entity,
                                    selection.brush,
                                    selection.face,
                                    editor.selected_shader().unwrap_or("<no shader>")
                                ),
                            );
                            theme::label(ui, theme::plain(
                                &format!("{}  —  {}  —  LMB selects/drags. ESC exits edit mode without discarding pending edits.", selection, editor.status),
                                10.5,
                                theme::TEXT_FAINT,
                            ));
                        });
                    });
            });
        self.map_edit_toolbar_rect = Some(toolbar.response.rect);
        if snapshot_refresh_requested {
            self.publish_snapshot();
        }

        if revert_requested {
            if let Some(editor) = self.map_editor.as_mut() {
                editor.revert_all();
            }
            self.queue_map_edit_preview();
            self.publish_snapshot();
            self.egui_repaint_requested = true;
        }
        if save_requested {
            let result = self.map_editor.as_mut().map(|editor| {
                editor.save()?;
                Ok::<_, String>(editor.source_path.clone())
            });
            match result {
                Some(Ok(path)) => {
                    self.console_status = format!("MAP EDIT SAVED: {}", path.display());
                    self.push_console_path_line(
                        format!("^2{}", self.console_status),
                        path,
                    );
                    // Save may happen while a live textured drag preview is still
                    // building. Queue the newest reparsed document instead of
                    // superseding it with another full rebuild immediately.
                    self.queue_map_edit_preview();
                    self.publish_snapshot();
                }
                Some(Err(error)) => {
                    self.console_status = format!("MAP EDIT SAVE FAILED: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                    if let Some(editor) = self.map_editor.as_mut() { editor.status = error; }
                }
                None => {}
            }
            self.egui_repaint_requested = true;
        }
        if close_requested {
            if let Some(editor) = self.map_editor.as_mut() { editor.cancel_drag(); }
            self.set_overlay(OverlayMode::None);
            self.publish_snapshot();
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
        self.egui_disconnect_notice(ui);
    }

    /// jaPRO/stock JKA show a kick/ban/drop reason as a popup over the main menu
    /// (`com_errorMessage` -> `error_popmenu`) instead of only printing it to a
    /// console the player may not have open. This is the equivalent here.
    fn egui_disconnect_notice(&mut self, root: &mut egui::Ui) {
        let Some(reason) = self.disconnect_notice.clone() else { return };
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

    /// Paint a black scrim *under* the menu chrome but over the 3D renderer.
    /// Before duel3 is available it is fully opaque, so startup is black rather
    /// than the renderer's no-world clear color. Once the cinematic is ready,
    /// the scrim eases away and the BSP appears to brighten from 0 -> 1.
    fn egui_frontend_scene_fade(&self, root: &mut egui::Ui) {
        if self.frontend_page == FrontendPage::AssetViewer {
            return;
        }
        let brightness = self
            .frontend_cinematic
            .and_then(|cinematic| cinematic.fade_started)
            .map_or(0.0, |started| {
                let t = started.elapsed().as_secs_f32() / FRONTEND_SCENE_FADE_IN_SECS;
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
                            FrontendPage::DeveloperTools => "DEVELOPER TOOLS",
                            FrontendPage::AssetViewer => "ASSET VIEWER",
                            FrontendPage::MapViewer => "MAP VIEWER",
                        }
                    };
                    theme::glow_label(ui, page, 12.5, theme::TEXT_DIM);
                    if self.overlay == OverlayMode::Video
                        || self.frontend_page != FrontendPage::Main
                    {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                // Keep clear of the FPS counter pinned to the
                                // top-right corner of the window.
                                ui.add_space(96.0);
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
                        FrontendPage::DeveloperTools => "ESC returns to the main menu.",
                        FrontendPage::AssetViewer | FrontendPage::MapViewer => "ESC returns to Developer Tools.",
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
                } else if matches!(self.frontend_page, FrontendPage::AssetViewer | FrontendPage::MapViewer) {
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
                        egui::Frame::new()
                            .fill(if self.frontend_page == FrontendPage::AssetViewer {
                                egui::Color32::TRANSPARENT
                            } else {
                                theme::PANEL_FILL
                            })
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
            FrontendPage::DeveloperTools => self.egui_developer_tools_page(ui),
            FrontendPage::AssetViewer => self.egui_asset_viewer_page(ui),
            FrontendPage::MapViewer => self.egui_map_viewer_page(ui),
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
        if menu_action(ui, "Developer Tools", "Browse and inspect game assets and source maps.", true) {
            self.frontend_page = FrontendPage::DeveloperTools;
            self.egui_repaint_requested = true;
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

    fn egui_developer_tools_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "DEVELOPER TOOLS", "Inspect the active JKA virtual filesystem without leaving the client.");
        theme::section(ui, "TOOLS", "");
        if menu_action(
            ui,
            "Asset Viewer",
            "Browse MD3, GLM, EFX and .shader assets with search, folder typeahead, preview controls, and inspection stats.",
            true,
        ) {
            self.frontend_page = FrontendPage::AssetViewer;
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

    fn ensure_asset_viewer_catalog(&mut self) {
        if self.asset_viewer_catalog_loaded {
            return;
        }
        self.asset_viewer_catalog_loaded = true;
        self.asset_viewer_entries.clear();
        self.asset_viewer_catalog_error = None;
        self.ui_catalog.pending.asset_viewer = true;
        self.ui_catalog
            .request(&self.base, self.game.as_deref(), ui_catalog::CatalogRequest::AssetViewer);
    }

    /// Install finished background catalog scans and image decodes.
    fn poll_ui_catalog(&mut self) {
        let results = self.ui_catalog.drain();
        if results.is_empty() {
            return;
        }
        let ctx = self.egui_ctx.clone();
        for result in results {
            match result.payload {
                CatalogPayload::AssetViewer(result) => {
                    self.ui_catalog.pending.asset_viewer = false;
                    match result {
                        Ok(entries) => self.asset_viewer_entries = entries,
                        Err(error) => {
                            eprintln!("Could not build Asset Viewer catalog: {error}");
                            self.asset_viewer_catalog_error = Some(error);
                        }
                    }
                }
                CatalogPayload::Profile(result) => {
                    self.ui_catalog.pending.profile = false;
                    self.apply_profile_catalog(result);
                }
                CatalogPayload::SoloMaps(result) => {
                    self.ui_catalog.pending.solo_maps = false;
                    match result {
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
                CatalogPayload::SourceMaps(result) => {
                    self.ui_catalog.pending.source_maps = false;
                    match result {
                        Ok(maps) => self.source_maps = maps,
                        Err(error) => {
                            eprintln!("Could not build Map Viewer catalog: {error}");
                            self.source_map_catalog_error = Some(error);
                        }
                    }
                    self.source_map_selected = self
                        .source_map_selected
                        .min(self.source_maps.len().saturating_sub(1));
                    self.source_map_levelshot_texture = None;
                    self.source_map_levelshot_texture_map = None;
                }
                CatalogPayload::Levelshot { source, map_name, image } => {
                    let (wanted, slot, prefix) = if source {
                        (
                            &self.source_map_levelshot_texture_map,
                            &mut self.source_map_levelshot_texture,
                            "source-levelshot",
                        )
                    } else {
                        (&self.solo_levelshot_texture_map, &mut self.solo_levelshot_texture, "levelshot")
                    };
                    // The selection may have moved on while this was decoding.
                    if wanted.as_deref() == Some(map_name.as_str()) {
                        *slot = image.map(|image| {
                            ctx.load_texture(
                                format!("{prefix}:{map_name}"),
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    }
                }
                CatalogPayload::ProfileIcon { key, image } => {
                    self.ui_catalog.icons_inflight.remove(&key);
                    match image {
                        Some(image) => {
                            let texture = ctx.load_texture(
                                format!("profile-model-icon:{key}"),
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            );
                            self.profile_model_icon_textures.insert(key, texture);
                        }
                        None => {
                            self.ui_catalog.icons_missing.insert(key);
                        }
                    }
                }
                CatalogPayload::CrosshairImage { index, image } => {
                    let key = Self::crosshair_image_key(index);
                    self.ui_catalog.icons_inflight.remove(&key);
                    match image {
                        Some(image) => {
                            let texture = ctx.load_texture(
                                key,
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            );
                            self.crosshair_image_textures.insert(index, texture);
                        }
                        None => {
                            self.ui_catalog.icons_missing.insert(key);
                        }
                    }
                }
            }
        }
        self.egui_repaint_requested = true;
    }

    fn ensure_asset_viewer_detail(&mut self) {
        let Some(selected_id) = self.asset_viewer_selected.clone() else {
            self.asset_viewer_detail_path = None;
            self.asset_viewer_detail = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
            return;
        };
        if self.asset_viewer_detail_path.as_deref() == Some(selected_id.as_str()) {
            self.update_asset_preview_content();
            return;
        }
        let Some(entry) = self
            .asset_viewer_entries
            .iter()
            .find(|entry| entry.id == selected_id)
            .cloned()
        else {
            self.asset_viewer_detail_path = None;
            self.asset_viewer_detail = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
            return;
        };
        self.asset_viewer_detail_path = Some(selected_id);
        let selected_shader = entry.shader_name.clone();
        let detail = frontend::inspect_asset(&self.base, self.game.as_deref(), &entry);
        self.asset_viewer_shader_name = selected_shader.or_else(|| {
            detail
                .as_ref()
                .ok()
                .and_then(|detail| detail.shader_names.first().cloned())
        });
        self.asset_viewer_detail = Some(detail);
        self.clear_asset_preview_runtime();
        self.update_asset_preview_content();
    }

    fn egui_asset_viewer_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_asset_viewer_catalog();
        if self.ui_catalog.pending.asset_viewer {
            theme::label(ui, theme::plain("Scanning assets...", 12.0, theme::TEXT_FAINT));
            return;
        }
        if let Some(error) = &self.asset_viewer_catalog_error {
            theme::banner(ui, &format!("Asset scan failed: {error}"), theme::WARNING);
            return;
        }

        let mut shader_files = self
            .asset_viewer_entries
            .iter()
            .filter(|entry| entry.kind == AssetKind::Shader)
            .map(|entry| entry.qpath.clone())
            .collect::<Vec<_>>();
        shader_files.sort();
        shader_files.dedup();

        let search = self.asset_viewer_search.trim().to_ascii_lowercase();
        let folder = self
            .asset_viewer_folder
            .trim()
            .replace('\\', "/")
            .trim_matches('/')
            .to_ascii_lowercase();
        let shader_file = self.asset_viewer_shader_file.as_str();

        // Build the folder picker from the catalog we already scanned. Every
        // ancestor is included, so typing `ships` can offer both `effects/ships`
        // and deeper folders even when no asset lives directly in the parent.
        // Counts are for the current asset type / shader-file scope and include
        // descendants, which makes the suggestions useful rather than decorative.
        let mut folder_counts = BTreeMap::<String, usize>::new();
        for entry in self.asset_viewer_entries.iter().filter(|entry| {
            self.asset_viewer_filter.matches(entry.kind)
                && (self.asset_viewer_filter != AssetFilter::Shader
                    || shader_file.is_empty()
                    || entry.qpath == shader_file)
        }) {
            let mut ancestor = String::new();
            for part in entry.folder.replace('\\', "/").split('/').filter(|part| !part.is_empty()) {
                if !ancestor.is_empty() {
                    ancestor.push('/');
                }
                ancestor.push_str(part);
                *folder_counts.entry(ancestor.clone()).or_default() += 1;
            }
        }
        let folder_is_exact = !folder.is_empty()
            && folder_counts
                .keys()
                .any(|candidate| candidate.eq_ignore_ascii_case(&folder));

        let filtered = self
            .asset_viewer_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.asset_viewer_filter.matches(entry.kind))
            .filter(|(_, entry)| {
                search.is_empty()
                    || entry.qpath.to_ascii_lowercase().contains(&search)
                    || entry.leaf.to_ascii_lowercase().contains(&search)
                    || entry.display_name.to_ascii_lowercase().contains(&search)
                    || entry
                        .shader_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&search))
            })
            .filter(|(_, entry)| {
                if folder.is_empty() {
                    return true;
                }
                let entry_folder = entry.folder.replace('\\', "/").to_ascii_lowercase();
                if folder_is_exact {
                    entry_folder == folder
                        || entry_folder
                            .strip_prefix(&folder)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                } else {
                    // While the user is still typing, preserve the old useful
                    // live-filter behavior. Once a real folder is selected or
                    // typed exactly, switch to boundary-safe folder semantics.
                    entry_folder.contains(&folder)
                }
            })
            .filter(|(_, entry)| {
                self.asset_viewer_filter != AssetFilter::Shader
                    || shader_file.is_empty()
                    || entry.qpath == shader_file
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        if self.asset_viewer_selected.as_ref().is_some_and(|selected| {
            !filtered
                .iter()
                .any(|&index| self.asset_viewer_entries[index].id == *selected)
        }) {
            self.asset_viewer_selected = None;
            self.asset_viewer_detail_path = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
        }

        let body_height = ui.available_height().max(320.0);
        let total_width = ui.available_width().max(1.0);
        let gap = 8.0_f32;
        let browser_width = (total_width * 0.21)
            .clamp(250.0, 320.0)
            .min(total_width * 0.30);
        let inspector_width = (total_width * 0.25)
            .clamp(280.0, 380.0)
            .min(total_width * 0.32);
        let preview_width = (total_width - browser_width - inspector_width - gap * 2.0).max(1.0);
        let mut keyboard_selection = None::<String>;
        let mut refresh_requested = false;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            // Compact single-column browser. Search/filter controls belong to
            // this pane because they only affect this list; they should not
            // steal a full-width toolbar from the actual preview workspace.
            ui.allocate_ui_with_layout(
                egui::vec2(browser_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::RAIL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::symmetric(9, 9))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (browser_width - 18.0).max(1.0),
                                (body_height - 18.0).max(1.0),
                            ));

                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("asset_viewer_type")
                                    .selected_text(self.asset_viewer_filter.label())
                                    .width((ui.available_width() - 112.0).max(88.0))
                                    .show_ui(ui, |ui| {
                                        for filter in [
                                            AssetFilter::All,
                                            AssetFilter::Models,
                                            AssetFilter::Md3,
                                            AssetFilter::Glm,
                                            AssetFilter::Efx,
                                            AssetFilter::Shader,
                                        ] {
                                            ui.selectable_value(
                                                &mut self.asset_viewer_filter,
                                                filter,
                                                filter.label(),
                                            );
                                        }
                                    });
                                if theme::ghost_button(ui, "CLEAR").clicked() {
                                    self.asset_viewer_filter = AssetFilter::All;
                                    self.asset_viewer_search.clear();
                                    self.asset_viewer_folder.clear();
                                    self.asset_viewer_folder_suggestion = 0;
                                    self.asset_viewer_shader_file.clear();
                                }
                                refresh_requested |= filesystem_refresh_icon(ui);
                            });

                            let search_response = ui.add_sized(
                                [ui.available_width(), 25.0],
                                egui::TextEdit::singleline(&mut self.asset_viewer_search)
                                    .hint_text("Search assets..."),
                            );

                            // Folder is a discoverable typeahead, not a magic
                            // substring box. The field still filters live while
                            // typing, but real catalog folders appear directly
                            // underneath and can be selected with mouse or keys.
                            let folder_response = ui
                                .horizontal(|ui| {
                                    let clear_width = if self.asset_viewer_folder.is_empty() {
                                        0.0
                                    } else {
                                        48.0
                                    };
                                    let response = ui.add_sized(
                                        [(ui.available_width() - clear_width).max(60.0), 23.0],
                                        egui::TextEdit::singleline(&mut self.asset_viewer_folder)
                                            .hint_text("Filter folder..."),
                                    );
                                    if !self.asset_viewer_folder.is_empty()
                                        && theme::ghost_button(ui, "×")
                                            .on_hover_text("Clear folder filter")
                                            .clicked()
                                    {
                                        self.asset_viewer_folder.clear();
                                        self.asset_viewer_folder_suggestion = 0;
                                        self.egui_repaint_requested = true;
                                    }
                                    response
                                })
                                .inner;

                            if folder_response.changed() {
                                self.asset_viewer_folder_suggestion = 0;
                            }

                            let folder_query = self
                                .asset_viewer_folder
                                .trim()
                                .replace('\\', "/")
                                .trim_matches('/')
                                .to_ascii_lowercase();
                            let mut folder_suggestions = folder_counts
                                .iter()
                                .filter(|(path, _)| {
                                    if folder_query.is_empty() {
                                        !path.contains('/')
                                    } else {
                                        path.to_ascii_lowercase().contains(&folder_query)
                                    }
                                })
                                .map(|(path, count)| (path.clone(), *count))
                                .collect::<Vec<_>>();
                            folder_suggestions.sort_by(|(a, _), (b, _)| {
                                if folder_query.is_empty() {
                                    return a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
                                }
                                let a_lower = a.to_ascii_lowercase();
                                let b_lower = b.to_ascii_lowercase();
                                let a_pos = a_lower.find(&folder_query).unwrap_or(usize::MAX);
                                let b_pos = b_lower.find(&folder_query).unwrap_or(usize::MAX);
                                a_pos
                                    .cmp(&b_pos)
                                    .then_with(|| a_lower.len().cmp(&b_lower.len()))
                                    .then_with(|| a_lower.cmp(&b_lower))
                            });
                            folder_suggestions.truncate(8);

                            let mut picked_folder = None::<String>;
                            let (folder_up, folder_down, folder_enter) = ui.input(|input| {
                                (
                                    input.key_pressed(egui::Key::ArrowUp),
                                    input.key_pressed(egui::Key::ArrowDown),
                                    input.key_pressed(egui::Key::Enter),
                                )
                            });
                            let folder_typeahead_active = folder_response.has_focus()
                                || (folder_response.lost_focus() && folder_enter);
                            if folder_typeahead_active {
                                if !folder_suggestions.is_empty() {
                                    self.asset_viewer_folder_suggestion = self
                                        .asset_viewer_folder_suggestion
                                        .min(folder_suggestions.len().saturating_sub(1));
                                    let up = folder_up;
                                    let down = folder_down;
                                    let enter = folder_enter;
                                    if down {
                                        self.asset_viewer_folder_suggestion =
                                            (self.asset_viewer_folder_suggestion + 1)
                                                % folder_suggestions.len();
                                    } else if up {
                                        self.asset_viewer_folder_suggestion =
                                            (self.asset_viewer_folder_suggestion
                                                + folder_suggestions.len()
                                                - 1)
                                                % folder_suggestions.len();
                                    }
                                    if enter {
                                        picked_folder = Some(
                                            folder_suggestions[self.asset_viewer_folder_suggestion]
                                                .0
                                                .clone(),
                                        );
                                    }

                                    ui.horizontal(|ui| {
                                        theme::label(
                                            ui,
                                            theme::plain("FOLDERS", 8.5, theme::TEXT_FAINT),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                theme::label(
                                                    ui,
                                                    theme::plain(
                                                        "↑ ↓ · ENTER",
                                                        8.0,
                                                        theme::TEXT_FAINT,
                                                    ),
                                                );
                                            },
                                        );
                                    });
                                    egui::Frame::new()
                                        .fill(theme::CONTROL)
                                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                                        .inner_margin(egui::Margin::same(2))
                                        .show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            for (index, (path, count)) in
                                                folder_suggestions.iter().enumerate()
                                            {
                                                let keyboard_selected = index
                                                    == self.asset_viewer_folder_suggestion;
                                                let (rect, response) = ui.allocate_exact_size(
                                                    egui::vec2(ui.available_width(), 21.0),
                                                    egui::Sense::click(),
                                                );
                                                let hovered = response.hovered();
                                                if keyboard_selected || hovered {
                                                    ui.painter().rect_filled(
                                                        rect,
                                                        egui::CornerRadius::same(2),
                                                        if keyboard_selected {
                                                            theme::CONTROL_SELECTED
                                                        } else {
                                                            theme::CONTROL_HOVER
                                                        },
                                                    );
                                                }
                                                let path_rect = egui::Rect::from_min_max(
                                                    rect.min,
                                                    egui::pos2(rect.right() - 42.0, rect.bottom()),
                                                );
                                                ui.painter()
                                                    .with_clip_rect(path_rect)
                                                    .text(
                                                        rect.left_center()
                                                            + egui::vec2(6.0, 0.0),
                                                        egui::Align2::LEFT_CENTER,
                                                        path,
                                                        egui::FontId::monospace(9.5),
                                                        if keyboard_selected {
                                                            theme::TEXT
                                                        } else {
                                                            theme::TEXT_DIM
                                                        },
                                                    );
                                                ui.painter().text(
                                                    rect.right_center() - egui::vec2(6.0, 0.0),
                                                    egui::Align2::RIGHT_CENTER,
                                                    count.to_string(),
                                                    egui::FontId::monospace(8.5),
                                                    theme::TEXT_FAINT,
                                                );
                                                let response = response.on_hover_text(path);
                                                if response.clicked() {
                                                    picked_folder = Some(path.clone());
                                                }
                                            }
                                        });
                                } else if !folder_query.is_empty() {
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            "No matching folders",
                                            9.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );
                                }
                            }

                            if let Some(path) = picked_folder {
                                self.asset_viewer_folder = path;
                                self.asset_viewer_folder_suggestion = 0;
                                ui.memory_mut(|memory| memory.surrender_focus(folder_response.id));
                                self.egui_repaint_requested = true;
                            }

                            if self.asset_viewer_filter == AssetFilter::Shader {
                                let selected_file = if self.asset_viewer_shader_file.is_empty() {
                                    "All shader files".to_owned()
                                } else {
                                    self.asset_viewer_shader_file
                                        .rsplit('/')
                                        .next()
                                        .unwrap_or(self.asset_viewer_shader_file.as_str())
                                        .to_owned()
                                };
                                egui::ComboBox::from_id_salt("asset_viewer_shader_file")
                                    .selected_text(selected_file)
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.asset_viewer_shader_file,
                                            String::new(),
                                            "All shader files",
                                        );
                                        for qpath in &shader_files {
                                            let label = qpath.rsplit('/').next().unwrap_or(qpath);
                                            ui.selectable_value(
                                                &mut self.asset_viewer_shader_file,
                                                qpath.clone(),
                                                label,
                                            )
                                            .on_hover_text(qpath);
                                        }
                                    });
                            }

                            ui.horizontal(|ui| {
                                theme::label(
                                    ui,
                                    theme::plain(
                                        &format!(
                                            "{} / {}",
                                            filtered.len(),
                                            self.asset_viewer_entries.len()
                                        ),
                                        9.5,
                                        theme::TEXT_FAINT,
                                    ),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        theme::label(
                                            ui,
                                            theme::plain("↑ ↓ browse", 9.0, theme::TEXT_FAINT),
                                        );
                                    },
                                );
                            });
                            ui.separator();

                            // Arrow navigation intentionally stays dormant while
                            // either text field owns focus so cursor movement in
                            // search/filter boxes remains standard text editing.
                            if !search_response.has_focus()
                                && !folder_response.has_focus()
                                && !filtered.is_empty()
                            {
                                let nav = ui.input(|input| {
                                    if input.key_pressed(egui::Key::ArrowDown) {
                                        Some(1_i32)
                                    } else if input.key_pressed(egui::Key::ArrowUp) {
                                        Some(-1_i32)
                                    } else {
                                        None
                                    }
                                });
                                if let Some(direction) = nav {
                                    let current = self
                                        .asset_viewer_selected
                                        .as_deref()
                                        .and_then(|selected| {
                                            filtered.iter().position(|&index| {
                                                self.asset_viewer_entries[index].id == selected
                                            })
                                        });
                                    let next = match (current, direction) {
                                        (Some(position), 1) => (position + 1).min(filtered.len() - 1),
                                        (Some(position), -1) => position.saturating_sub(1),
                                        (Some(position), _) => position,
                                        (None, 1) => 0,
                                        (None, -1) => filtered.len() - 1,
                                        (None, _) => 0,
                                    };
                                    let entry_id = self.asset_viewer_entries[filtered[next]].id.clone();
                                    if self.asset_viewer_selected.as_deref() != Some(entry_id.as_str()) {
                                        self.asset_viewer_selected = Some(entry_id.clone());
                                        self.asset_viewer_detail_path = None;
                                        self.asset_viewer_shader_name = None;
                                        self.clear_asset_preview_runtime();
                                        keyboard_selection = Some(entry_id);
                                        self.egui_repaint_requested = true;
                                    }
                                }
                            }

                            let list_height = ui.available_height().max(1.0);
                            egui::ScrollArea::vertical()
                                .id_salt("asset_viewer_list")
                                .auto_shrink([false, false])
                                .max_height(list_height)
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    for &index in &filtered {
                                        let entry = self.asset_viewer_entries[index].clone();
                                        let selected = self.asset_viewer_selected.as_deref()
                                            == Some(entry.id.as_str());
                                        let (rect, response) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 24.0),
                                            egui::Sense::click(),
                                        );
                                        let painter = ui.painter().with_clip_rect(rect);
                                        if selected {
                                            painter.rect_filled(
                                                rect,
                                                egui::CornerRadius::same(2),
                                                theme::CONTROL_SELECTED,
                                            );
                                            painter.rect_filled(
                                                egui::Rect::from_min_size(
                                                    rect.left_top(),
                                                    egui::vec2(2.0, rect.height()),
                                                ),
                                                egui::CornerRadius::ZERO,
                                                theme::ACCENT,
                                            );
                                        } else if response.hovered() {
                                            painter.rect_filled(
                                                rect,
                                                egui::CornerRadius::same(2),
                                                theme::CONTROL,
                                            );
                                        }
                                        painter.text(
                                            rect.left_center() + egui::vec2(7.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            &entry.display_name,
                                            egui::FontId::monospace(10.5),
                                            if selected { theme::TEXT } else { theme::TEXT_DIM },
                                        );
                                        painter.text(
                                            rect.right_center() - egui::vec2(5.0, 0.0),
                                            egui::Align2::RIGHT_CENTER,
                                            entry.kind.label(),
                                            egui::FontId::monospace(8.0),
                                            if selected { theme::ACCENT } else { theme::TEXT_FAINT },
                                        );

                                        let hover = entry.shader_name.as_ref().map_or_else(
                                            || entry.qpath.clone(),
                                            |shader| format!("{shader}\n{}", entry.qpath),
                                        );
                                        let response = response.on_hover_text(hover);
                                        if response.clicked() && !selected {
                                            self.asset_viewer_selected = Some(entry.id.clone());
                                            self.asset_viewer_detail_path = None;
                                            self.asset_viewer_shader_name = None;
                                            self.clear_asset_preview_runtime();
                                                self.egui_repaint_requested = true;
                                        }
                                        if selected
                                            && keyboard_selection.as_deref() == Some(entry.id.as_str())
                                        {
                                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                        }
                                    }
                                });
                        });
                },
            );

            self.ensure_asset_viewer_detail();

            let detail_snapshot = self
                .asset_viewer_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().ok())
                .cloned();
            let detail_error = self
                .asset_viewer_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().err())
                .cloned();

            // The preview gets the largest share of the workspace. Controls are
            // deliberately direct-manipulation: drag in both axes for models,
            // wheel for zoom, and a compact reset control in the inspector.
            ui.allocate_ui_with_layout(
                egui::vec2(preview_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let (preview_rect, preview_response) = ui.allocate_exact_size(
                        egui::vec2(preview_width, body_height),
                        egui::Sense::click_and_drag(),
                    );
                    ui.painter().rect_stroke(
                        preview_rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    let preview_label = match detail_snapshot.as_ref().map(|detail| detail.kind) {
                        Some(AssetKind::Md3) | Some(AssetKind::Glm) => {
                            "MODEL · drag rotate · wheel zoom"
                        }
                        Some(AssetKind::Efx) => "EFX · looping · wheel zoom",
                        Some(AssetKind::Shader) => "SHADER · wheel zoom",
                        None => "PREVIEW",
                    };
                    ui.painter().text(
                        preview_rect.left_top() + egui::vec2(10.0, 9.0),
                        egui::Align2::LEFT_TOP,
                        preview_label,
                        egui::FontId::proportional(10.5),
                        theme::TEXT_FAINT,
                    );
                    self.set_asset_preview_viewport(
                        preview_rect.shrink(1.0),
                        ui.ctx().pixels_per_point(),
                    );

                    let kind = detail_snapshot.as_ref().map(|detail| detail.kind);
                    let zoom_max = match kind {
                        Some(AssetKind::Efx) => 64.0,
                        Some(AssetKind::Shader) => 20.0,
                        Some(AssetKind::Md3) | Some(AssetKind::Glm) => 12.0,
                        None => 12.0,
                    };
                    let mut preview_changed = false;
                    if kind.is_some_and(|kind| kind.is_model()) && preview_response.dragged() {
                        let delta = ui.input(|input| input.pointer.delta());
                        if delta.x.abs() > f32::EPSILON {
                            self.asset_viewer_model_yaw =
                                (self.asset_viewer_model_yaw + delta.x * 0.45).rem_euclid(360.0);
                            preview_changed = true;
                        }
                        if delta.y.abs() > f32::EPSILON {
                            self.asset_viewer_model_pitch =
                                (self.asset_viewer_model_pitch - delta.y * 0.35).clamp(-89.0, 89.0);
                            preview_changed = true;
                        }
                    }
                    if preview_response.hovered() {
                        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                        if scroll.abs() > f32::EPSILON {
                            self.asset_viewer_model_zoom = (self.asset_viewer_model_zoom
                                * (-scroll * 0.0015).exp())
                            .clamp(0.20, zoom_max);
                            preview_changed = true;
                        }
                    }
                    if preview_changed {
                        self.asset_preview_model_key = None;
                        self.asset_preview_shader_key = None;
                        self.asset_preview_fx = None;
                        self.update_asset_preview_content();
                        self.egui_repaint_requested = true;
                    }
                },
            );

            ui.allocate_ui_with_layout(
                egui::vec2(inspector_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::RAIL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::symmetric(10, 9))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (inspector_width - 20.0).max(1.0),
                                (body_height - 18.0).max(1.0),
                            ));

                            match detail_snapshot.as_ref() {
                                Some(detail) => {
                                    let detail_title = if detail.kind == AssetKind::Shader {
                                        self.asset_viewer_shader_name
                                            .as_deref()
                                            .unwrap_or(detail.qpath.as_str())
                                    } else {
                                        detail.qpath.as_str()
                                    };
                                    theme::glow_label(ui, detail_title, 12.5, theme::TEXT);
                                    if detail.kind == AssetKind::Shader {
                                        theme::label(
                                            ui,
                                            theme::plain(
                                                &format!("File: {}", detail.qpath),
                                                9.0,
                                                theme::TEXT_FAINT,
                                            ),
                                        );
                                    }
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            &format!(
                                                "{}  •  {}",
                                                detail.kind.label(),
                                                asset_size_label(detail.size_bytes)
                                            ),
                                            10.0,
                                            theme::TEXT_DIM,
                                        ),
                                    );
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            &format!("Source: {}", detail.source),
                                            9.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );

                                    ui.add_space(6.0);
                                    theme::section(ui, "VIEW", "");
                                    let zoom_max = match detail.kind {
                                        AssetKind::Efx => 64.0,
                                        AssetKind::Shader => 20.0,
                                        AssetKind::Md3 | AssetKind::Glm => 12.0,
                                    };
                                    let mut view_changed = false;
                                    if detail.kind.is_model() {
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain("Yaw", 10.0, theme::TEXT_FAINT),
                                            );
                                            view_changed |= ui
                                                .add(
                                                    egui::Slider::new(
                                                        &mut self.asset_viewer_model_yaw,
                                                        0.0..=360.0,
                                                    )
                                                    .show_value(false),
                                                )
                                                .changed();
                                        });
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain("Pitch", 10.0, theme::TEXT_FAINT),
                                            );
                                            view_changed |= ui
                                                .add(
                                                    egui::Slider::new(
                                                        &mut self.asset_viewer_model_pitch,
                                                        -89.0..=89.0,
                                                    )
                                                    .show_value(false),
                                                )
                                                .changed();
                                        });
                                    }
                                    ui.horizontal(|ui| {
                                        theme::label(
                                            ui,
                                            theme::plain("Zoom", 10.0, theme::TEXT_FAINT),
                                        );
                                        view_changed |= ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut self.asset_viewer_model_zoom,
                                                    0.20..=zoom_max,
                                                )
                                                .logarithmic(true)
                                                .show_value(false),
                                            )
                                            .changed();
                                    });
                                    if theme::ghost_button(ui, "RESET VIEW").clicked() {
                                        self.asset_viewer_model_yaw = 180.0;
                                        self.asset_viewer_model_pitch = 0.0;
                                        self.asset_viewer_model_zoom = 1.0;
                                        view_changed = true;
                                    }
                                    if view_changed {
                                        self.asset_preview_model_key = None;
                                        self.asset_preview_shader_key = None;
                                        self.asset_preview_fx = None;
                                        self.update_asset_preview_content();
                                        self.egui_repaint_requested = true;
                                    }

                                    ui.add_space(7.0);
                                    theme::section(ui, "STATS", &format!("{}", detail.stats.len()));
                                    for (name, value) in &detail.stats {
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain(name, 10.0, theme::TEXT_FAINT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    theme::label(
                                                        ui,
                                                        theme::plain(value, 10.0, theme::TEXT),
                                                    );
                                                },
                                            );
                                        });
                                    }

                                    // Stats stay outside the scroll area so the
                                    // useful summary remains visible. Long
                                    // surface/stage/source dumps get the remaining
                                    // vertical space instead.
                                    ui.add_space(7.0);
                                    let list_title = match detail.kind {
                                        AssetKind::Shader => "STAGES",
                                        kind if kind.is_model() => "SURFACES",
                                        _ => "PRIMITIVES",
                                    };
                                    theme::section(ui, list_title, &format!("{}", detail.items.len()));
                                    let details_height = ui.available_height().max(1.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("asset_viewer_inspector_details")
                                        .auto_shrink([false, false])
                                        .max_height(details_height)
                                        .show(ui, |ui| {
                                            for item in &detail.items {
                                                ui.label(
                                                    egui::RichText::new(item)
                                                        .monospace()
                                                        .size(9.25)
                                                        .color(theme::TEXT_DIM),
                                                );
                                            }
                                            if let Some(text) = &detail.raw_text {
                                                ui.add_space(8.0);
                                                ui.collapsing("SOURCE", |ui| {
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .monospace()
                                                            .size(9.0)
                                                            .color(theme::TEXT_DIM),
                                                    );
                                                });
                                            }
                                        });
                                }
                                None if detail_error.is_some() => {
                                    theme::banner(
                                        ui,
                                        detail_error
                                            .as_deref()
                                            .unwrap_or("Asset inspection failed"),
                                        theme::WARNING,
                                    );
                                }
                                None => {
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            "Select an asset from the browser.",
                                            11.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );
                                }
                            }
                        });
                },
            );
        });

        // EFX needs to advance every frontend frame even when no UI control is
        // moving; model/shader paths cheaply return when their cache key matches.
        self.update_asset_preview_content();

        // refresh_filesystem() invalidates this catalog. Defer it until every
        // list index/reference from the current frame has been consumed.
        if refresh_requested {
            self.refresh_filesystem();
            self.egui_repaint_requested = true;
        }
    }

    fn ensure_source_map_catalog(&mut self) {
        if self.source_map_catalog_loaded {
            return;
        }
        self.source_map_catalog_loaded = true;
        self.source_maps.clear();
        self.source_map_catalog_error = None;
        self.ui_catalog.pending.source_maps = true;
        self.ui_catalog
            .request(&self.base, self.game.as_deref(), ui_catalog::CatalogRequest::SourceMaps);
    }

    fn ensure_source_map_levelshot_texture(&mut self) {
        let Some(entry) = self.source_maps.get(self.source_map_selected) else {
            self.source_map_levelshot_texture = None;
            self.source_map_levelshot_texture_map = None;
            return;
        };
        let map_name = entry.map_name.clone();
        if self.source_map_levelshot_texture_map.as_deref() == Some(map_name.as_str()) {
            return;
        }
        self.source_map_levelshot_texture = None;
        self.source_map_levelshot_texture_map = Some(map_name.clone());
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::Levelshot { source: true, map_name },
        );
    }

    fn egui_map_viewer_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "MAP VIEWER", "Source .map files discovered from the active VFS; launch uses the existing direct .map path.");
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 28.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if filesystem_refresh_icon(ui) {
                    self.refresh_filesystem();
                    self.egui_repaint_requested = true;
                }
            },
        );
        self.ensure_source_map_catalog();
        if self.ui_catalog.pending.source_maps {
            theme::label(ui, theme::plain("Scanning maps...", 12.0, theme::TEXT_FAINT));
            return;
        }
        if let Some(error) = &self.source_map_catalog_error {
            theme::banner(ui, &format!("Source map scan failed: {error}"), theme::WARNING);
            return;
        }
        if self.source_maps.is_empty() {
            theme::banner(ui, "No maps/*.map assets were found.", theme::WARNING);
            return;
        }
        self.source_map_selected = self.source_map_selected.min(self.source_maps.len() - 1);
        self.ensure_source_map_levelshot_texture();
        let mut selected = self.source_map_selected;
        let selected_name = self.source_maps[selected].map_name.clone();
        let browser_height = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(390.0, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SOURCE MAPS", &format!("{} found", self.source_maps.len()));
                    egui::ScrollArea::vertical()
                        .id_salt("jka_source_map_list")
                        .auto_shrink([false, false])
                        .max_height(ui.available_height().max(1.0))
                        .show(ui, |ui| {
                            for (index, entry) in self.source_maps.iter().enumerate() {
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
                    theme::section(ui, "SELECTED SOURCE MAP", "");
                    theme::glow_label(ui, &format!("maps/{selected_name}.map"), 18.0, theme::TEXT);
                    ui.add_space(10.0);
                    let preview_w = ui.available_width().max(1.0);
                    let preview_box = egui::vec2(preview_w, 360.0);
                    if let Some(texture) = &self.source_map_levelshot_texture {
                        let source = texture.size_vec2();
                        let scale = (preview_box.x / source.x.max(1.0)).min(preview_box.y / source.y.max(1.0));
                        ui.add(egui::Image::new(texture).fit_to_exact_size(source * scale));
                    } else {
                        let placeholder = egui::vec2(preview_w, (preview_w * 0.5).min(360.0));
                        let (rect, _) = ui.allocate_exact_size(placeholder, egui::Sense::hover());
                        ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL);
                        ui.painter().rect_stroke(rect, egui::CornerRadius::ZERO, egui::Stroke::new(1.0_f32, theme::LINE), egui::StrokeKind::Inside);
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "NO LEVELSHOT", egui::FontId::proportional(13.0), theme::TEXT_DISABLED);
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "LOAD SOURCE MAP").clicked() {
                            self.source_map_edit_on_load = false;
                            self.render_command(RenderCommand::SetAssetPreviewMode(false));
                            self.request_map(scene::MapSource::Map(selected_name.clone()));
                        }
                        if theme::ghost_button(ui, "EDIT SOURCE MAP").clicked() {
                            self.source_map_edit_on_load = true;
                            self.render_command(RenderCommand::SetAssetPreviewMode(false));
                            self.request_map(scene::MapSource::Map(selected_name.clone()));
                        }
                    });
                },
            );
        });
        if selected != self.source_map_selected {
            self.source_map_selected = selected;
            self.source_map_levelshot_texture = None;
            self.source_map_levelshot_texture_map = None;
            self.egui_repaint_requested = true;
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

        // Autojoin is intentionally driven from the menu tick rather than the
        // server-browser page itself, so it keeps waiting even if the player
        // closes the browser or switches to another menu page. A full server is
        // polled cheaply in-place; once getinfo reports a free client slot we
        // stop polling and use the normal connect path.
        if let Some((source, address)) = self.server_browser.autojoin {
            let slot_open = self
                .server_browser
                .servers
                .get(&address)
                .is_some_and(|server| {
                    server.max_clients > 0 && server.clients < server.max_clients
                });
            if slot_open {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
                let text = format!("Slot opened on {address}; connecting…");
                self.server_browser.source_status.insert(source, text.clone());
                self.server_browser.status_text = text;
                self.connect_browser_server(address);
                changed = true;
            } else {
                let due = self
                    .server_browser
                    .autojoin_last_query
                    .is_none_or(|last| last.elapsed() >= Duration::from_millis(1500));
                if due {
                    if self
                        .server_browser_tx
                        .send(BrowserCommand::RefreshServer {
                            source,
                            address,
                            quiet: true,
                        })
                        .is_ok()
                    {
                        self.server_browser.autojoin_last_query = Some(Instant::now());
                        let text = format!("Autojoin: waiting for a slot on {address}…");
                        self.server_browser.source_status.insert(source, text.clone());
                        self.server_browser.status_text = text;
                    } else {
                        self.server_browser.autojoin = None;
                        self.server_browser.autojoin_last_query = None;
                        self.server_browser.status_text =
                            "Server browser worker is unavailable".to_owned();
                    }
                    changed = true;
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
            let text = "Server browser worker is unavailable".to_owned();
            self.server_browser.source_status.insert(source, text.clone());
            self.server_browser.status_text = text;
        } else {
            self.server_browser.refreshing.insert(source);
            let text = format!("Refreshing {}…", source.label());
            self.server_browser.source_status.insert(source, text.clone());
            self.server_browser.status_text = text;
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
        self.server_browser.details = self.server_browser.status_cache.get(&address).cloned();
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
        let label = sort.label();
        let color = if selected { theme::ACCENT } else { theme::TEXT_DIM };
        let response = ui.add_sized(
            [width, 20.0],
            egui::Label::new(theme::plain(label, 10.5, color))
                .sense(egui::Sense::click()),
        );
        if selected {
            // Do not rely on a font glyph for the sort arrow. The bundled UI
            // font does not contain U+2191/U+2193 on some installs, which made
            // the old arrow render as the familiar missing-glyph square.
            let center = egui::pos2(response.rect.right() - 7.0, response.rect.center().y);
            let points = if self.server_browser.sort_ascending {
                vec![
                    center + egui::vec2(-3.5, 2.0),
                    center + egui::vec2(3.5, 2.0),
                    center + egui::vec2(0.0, -2.5),
                ]
            } else {
                vec![
                    center + egui::vec2(-3.5, -2.0),
                    center + egui::vec2(3.5, -2.0),
                    center + egui::vec2(0.0, 2.5),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                theme::ACCENT,
                egui::Stroke::NONE,
            ));
        }
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
                    self.server_browser.player_search_last_query = None;
                    self.server_browser.status_text = self
                        .server_browser
                        .source_status
                        .get(&source)
                        .cloned()
                        .unwrap_or_else(|| "Ready".to_owned());
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
                let clicked = ui
                    .add_enabled_ui(!refreshing, |ui| {
                        refresh_icon(
                            ui,
                            if refreshing {
                                "Refreshing server list…"
                            } else {
                                "Refresh server list"
                            },
                        )
                    })
                    .inner;
                if clicked {
                    self.request_server_refresh(self.server_browser.source);
                }
                if refreshing {
                    theme::label(ui, theme::plain("REFRESHING…", 10.5, theme::TEXT_FAINT));
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
                if ui.small_button("RESTORE DEFAULTS").clicked() {
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
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("PLAYERS", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [360.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.player_search)
                    .hint_text("names/substrings, comma-separated"),
            );
            if response.changed() {
                // Keep cached status rows for instant filtering, but force a
                // fresh getstatus batch immediately so joins/leaves are picked
                // up without the user having to Refresh the whole browser.
                self.server_browser.player_search_last_query = None;
                self.egui_repaint_requested = true;
            }
            response.on_hover_text(
                "Comma-separated player names. Matches any term, ignoring JKA color codes and case.",
            );
            if theme::chip(
                ui,
                "EXACT MATCH",
                self.server_browser.player_exact_match,
            )
            .on_hover_text("Match the whole color-stripped player name instead of a substring.")
            .clicked()
            {
                self.server_browser.player_exact_match = !self.server_browser.player_exact_match;
            }
        });
        ui.add_space(7.0);

        let search = self.server_browser.search.trim().to_ascii_lowercase();
        let player_terms: Vec<String> = self
            .server_browser
            .player_search
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
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

        if !player_terms.is_empty() {
            let missing_status = servers
                .iter()
                .any(|server| !self.server_browser.status_cache.contains_key(&server.address));
            let due = (missing_status
                || self
                    .server_browser
                    .player_search_last_query
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(4)))
                && self.server_browser.status_pending.is_empty();
            if due {
                let addresses: Vec<_> = servers.iter().map(|server| server.address).collect();
                if !addresses.is_empty() {
                    if self
                        .server_browser_tx
                        .send(BrowserCommand::QueryStatusBatch(addresses.clone()))
                        .is_ok()
                    {
                        self.server_browser.status_pending.extend(addresses);
                        self.server_browser.player_search_last_query = Some(Instant::now());
                    } else {
                        self.server_browser.status_text =
                            "Server browser worker is unavailable".to_owned();
                    }
                }
            }

            let exact = self.server_browser.player_exact_match;
            let hide_bots = self.server_browser.hide_bots;
            servers.retain(|server| {
                let Some(status) = self.server_browser.status_cache.get(&server.address) else {
                    // Optimistically keep rows until the first status batch has
                    // had a chance to answer; the status-batch completion caches
                    // an empty result for non-responders.
                    return true;
                };
                status.players.iter().any(|player| {
                    if hide_bots && player.ping == 0 {
                        return false;
                    }
                    let name = crate::logging::strip_jka_colors(&player.name).to_ascii_lowercase();
                    player_terms.iter().any(|term| {
                        if exact { name == *term } else { name.contains(term) }
                    })
                })
            });
        }

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
        let mut refresh_one_after = None;
        let mut favorite_after = None;
        let mut autojoin_after = None;
        ui.horizontal(|ui| {
            let list_width = (ui.available_width() * 0.67).max(560.0);
            // The name column absorbs whatever the fixed columns leave, so the
            // table always spans its pane instead of hugging the left edge.
            let name_w = (list_width - 299.0 - 32.0 - 24.0).max(250.0);
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
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Name, name_w);
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
                                    server.hostname.clone()
                                };
                                let favorite = self.server_browser.is_favorite(server.address);
                                let autojoining = self
                                    .server_browser
                                    .autojoin
                                    .is_some_and(|(_, address)| address == server.address);
                                let full = server.max_clients > 0 && server.clients >= server.max_clients;
                                let response = ui
                                    .horizontal(|ui| {
                                        let label = if server.need_password {
                                            format!("🔒 {hostname}")
                                        } else {
                                            hostname
                                        };
                                        let row_text = theme::TEXT;
                                        let mut response = ui.add_sized(
                                            [name_w, 24.0],
                                            egui::Button::selectable(
                                                selected,
                                                jka_colored_text(&label, 11.5, theme::TEXT),
                                            ),
                                        );
                                        response |= ui.add_sized(
                                            [118.0, 24.0],
                                            egui::Label::new(theme::plain(&server.map, 11.5, row_text))
                                                .sense(egui::Sense::click()),
                                        );
                                        response |= ui.add_sized(
                                            [82.0, 24.0],
                                            egui::Label::new(theme::plain(server.gametype_label(), 10.8, row_text))
                                                .sense(egui::Sense::click()),
                                        );
                                        let visible_clients = if self.server_browser.hide_bots { server.humans } else { server.clients };
                                        let player_color = if full { theme::WARNING } else { row_text };
                                        response |= ui.add_sized(
                                            [55.0, 24.0],
                                            egui::Label::new(theme::plain(&format!("{}/{}", visible_clients, server.max_clients), 11.5, player_color))
                                                .sense(egui::Sense::click()),
                                        );
                                        let ping = if server.ping_ms == 0 { "—".to_owned() } else { server.ping_ms.to_string() };
                                        let ping_color = match server.ping_ms {
                                            0 => theme::TEXT_FAINT,
                                            1..=79 => egui::Color32::from_rgb(0x83, 0xD6, 0x8A),
                                            80..=149 => theme::TEXT,
                                            150..=249 => theme::WARNING,
                                            _ => theme::DANGER,
                                        };
                                        response |= ui.add_sized(
                                            [44.0, 24.0],
                                            egui::Label::new(theme::plain(&ping, 11.5, ping_color))
                                                .sense(egui::Sense::click()),
                                        );
                                        response
                                    })
                                    .inner;
                                if response.clicked() {
                                    selected_after = Some(server.address);
                                }
                                if response.double_clicked() {
                                    connect_after = Some(server.address);
                                }
                                if response.secondary_clicked() {
                                    selected_after = Some(server.address);
                                }
                                response.context_menu(|ui| {
                                    ui.set_min_width(190.0);
                                    if ui.button("Connect").clicked() {
                                        connect_after = Some(server.address);
                                        ui.close();
                                    }
                                    if full {
                                        let label = if autojoining {
                                            "Cancel autojoin"
                                        } else {
                                            "Autojoin when slot opens"
                                        };
                                        if ui.button(label).clicked() {
                                            autojoin_after = Some(server.address);
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if ui
                                        .button(if favorite { "Remove favorite" } else { "Add favorite" })
                                        .clicked()
                                    {
                                        favorite_after = Some(server.address);
                                        ui.close();
                                    }
                                    if ui.button("Refresh this server").clicked() {
                                        refresh_one_after = Some(server.address);
                                        ui.close();
                                    }
                                    if ui.button("Copy IP").clicked() {
                                        ui.ctx().copy_text(server.address.ip().to_string());
                                        ui.close();
                                    }
                                    if ui.button("Copy address").clicked() {
                                        ui.ctx().copy_text(server.address.to_string());
                                        ui.close();
                                    }
                                });
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
                        server.hostname.clone()
                    };
                    ui.add(egui::Label::new(jka_colored_text(&hostname, 17.0, theme::TEXT)));
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
                        let full = server.max_clients > 0 && server.clients >= server.max_clients;
                        if full {
                            let autojoining = self
                                .server_browser
                                .autojoin
                                .is_some_and(|(_, autojoin_address)| autojoin_address == address);
                            let label = if autojoining {
                                "CANCEL AUTOJOIN"
                            } else {
                                "AUTOJOIN"
                            };
                            if theme::ghost_button(ui, label)
                                .on_hover_text(if autojoining {
                                    "Stop waiting for a free slot on this server."
                                } else {
                                    "Poll this full server and connect automatically when a slot opens."
                                })
                                .clicked()
                            {
                                autojoin_after = Some(address);
                            }
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
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [72.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4} ms", player.ping),
                                                    11.5,
                                                    theme::TEXT_DIM,
                                                )),
                                            );
                                            ui.add_sized(
                                                [42.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4}", player.score),
                                                    11.5,
                                                    theme::TEXT_DIM,
                                                )),
                                            );
                                            ui.add(egui::Label::new(jka_colored_text(
                                                &player.name,
                                                11.5,
                                                theme::TEXT,
                                            )));
                                        });
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
        if let Some(address) = refresh_one_after {
            if self
                .server_browser_tx
                .send(BrowserCommand::RefreshServer {
                    source: self.server_browser.source,
                    address,
                    quiet: false,
                })
                .is_ok()
            {
                let text = format!("Refreshing {address}…");
                self.server_browser
                    .source_status
                    .insert(self.server_browser.source, text.clone());
                self.server_browser.status_text = text;
            } else {
                self.server_browser.status_text =
                    "Server browser worker is unavailable".to_owned();
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = favorite_after {
            match self.server_browser.toggle_favorite(address) {
                Ok(true) => self.server_browser.status_text = "Added favorite".to_owned(),
                Ok(false) => self.server_browser.status_text = "Removed favorite".to_owned(),
                Err(error) => self.server_browser.status_text = error,
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = autojoin_after {
            if self
                .server_browser
                .autojoin
                .is_some_and(|(_, autojoin_address)| autojoin_address == address)
            {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
                self.server_browser.status_text = "Autojoin cancelled".to_owned();
                self.server_browser.source_status.insert(
                    self.server_browser.source,
                    self.server_browser.status_text.clone(),
                );
            } else {
                self.server_browser.autojoin = Some((self.server_browser.source, address));
                self.server_browser.autojoin_last_query = None;
                self.server_browser.status_text =
                    format!("Autojoin: waiting for a slot on {address}…");
                self.server_browser.source_status.insert(
                    self.server_browser.source,
                    self.server_browser.status_text.clone(),
                );
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = connect_after {
            if self
                .server_browser
                .autojoin
                .is_some_and(|(_, autojoin_address)| autojoin_address == address)
            {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
            }
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

    pub(super) fn ensure_solo_map_catalog(&mut self) {
        if self.solo_catalog_loaded {
            return;
        }

        self.solo_catalog_loaded = true;
        self.solo_maps.clear();
        self.solo_catalog_error = None;
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
        self.ui_catalog.pending.solo_maps = true;
        self.ui_catalog
            .request(&self.base, self.game.as_deref(), ui_catalog::CatalogRequest::SoloMaps);
    }

    fn ensure_solo_levelshot_texture(&mut self) {
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
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::Levelshot { source: false, map_name },
        );
    }

    fn egui_solo_game_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "SOLO GAME",
            "Compiled BSPs discovered from loose base/maps files and mounted PK3s.",
        );
        // Fixed-height row: a bare `with_layout` would hand the icon the whole
        // remaining page height and center it vertically, collapsing the list.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 28.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if filesystem_refresh_icon(ui) {
                    self.refresh_filesystem();
                    self.egui_repaint_requested = true;
                }
            },
        );

        self.ensure_solo_map_catalog();

        if self.ui_catalog.pending.solo_maps {
            theme::label(ui, theme::plain("Scanning maps...", 12.0, theme::TEXT_FAINT));
            return;
        }
        if let Some(error) = &self.solo_catalog_error {
            theme::banner(ui, &format!("Map scan failed: {error}"), theme::WARNING);
            return;
        }
        if self.solo_maps.is_empty() {
            theme::banner(ui, "No maps/*.bsp assets were found.", theme::WARNING);
            return;
        }

        self.solo_map_selected = self.solo_map_selected.min(self.solo_maps.len() - 1);
        self.ensure_solo_levelshot_texture();
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
                    self.enter_local_game_dir();
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

    fn poll_demo_metadata_jobs(&mut self) {
        while let Ok(done) = self.demo_metadata_rx.try_recv() {
            if done.generation != self.demo_metadata_generation {
                continue;
            }
            self.demo_metadata_inflight.remove(&done.demo_name);
            match done.result {
                Ok(metadata) => {
                    self.demo_metadata_errors.remove(&done.demo_name);
                    self.demo_metadata_cache.insert(done.demo_name, metadata);
                }
                Err(error) => {
                    self.demo_metadata_errors.insert(done.demo_name, error);
                }
            }
            self.egui_repaint_requested = true;
        }
    }

    fn ensure_demo_metadata(&mut self, demo_name: &str) {
        self.poll_demo_metadata_jobs();
        if self.demo_metadata_cache.contains_key(demo_name)
            || self.demo_metadata_errors.contains_key(demo_name)
            || self.demo_metadata_inflight.contains(demo_name)
        {
            return;
        }

        let demo_name = demo_name.to_owned();
        self.demo_metadata_inflight.insert(demo_name.clone());
        let base = self.base.clone();
        let game = self.game.clone();
        let tx = self.demo_metadata_tx.clone();
        let generation = self.demo_metadata_generation;
        let worker_demo_name = demo_name.clone();
        let thread_name = format!("demo-meta:{}", demo_name.chars().take(24).collect::<String>());
        if let Err(error) = thread::Builder::new().name(thread_name).spawn(move || {
            let result = frontend::index_demo_metadata(&base, game.as_deref(), &worker_demo_name);
            let _ = tx.send(DemoMetadataResult { generation, demo_name: worker_demo_name, result });
        }) {
            self.demo_metadata_inflight.remove(&demo_name);
            self.demo_metadata_errors.insert(demo_name, format!("metadata worker: {error}"));
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

    fn format_demo_time(ms: i32) -> String {
        let total_seconds = ms.max(0) / 1000;
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    fn demo_gametype_name(gametype: i32) -> &'static str {
        match gametype {
            0 => "FFA",
            1 => "Holocron",
            2 => "Jedi Master",
            3 => "Duel",
            4 => "Power Duel",
            5 => "Single Player",
            6 => "Team FFA",
            7 => "Siege",
            8 => "CTF",
            9 => "CTY",
            _ => "Unknown",
        }
    }

    fn demo_team_summary(teams: &[i32]) -> String {
        teams
            .iter()
            .map(|team| match team {
                1 => "red",
                2 => "blue",
                3 => "spectator",
                _ => "free",
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    fn render_demo_metadata(
        ui: &mut egui::Ui,
        metadata: &DemoMetadata,
        console_filter: &mut DemoConsoleFilter,
        launch_seek_ms: &mut Option<i32>,
    ) {
        let size_mb = metadata.size_bytes as f64 / (1024.0 * 1024.0);
        let source = Path::new(&metadata.source)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&metadata.source);
        let pov = match (&metadata.recorded_player_name, metadata.recorded_client_num) {
            (Some(name), Some(client)) => format!("{name}  (client {client})"),
            (Some(name), None) => name.clone(),
            (None, Some(client)) => format!("client {client}"),
            (None, None) => "unknown".to_owned(),
        };

        egui::Grid::new("demo_metadata_summary")
            .num_columns(4)
            .spacing(egui::vec2(16.0, 4.0))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("DURATION").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(Self::format_demo_time(metadata.duration_ms));
                ui.label(egui::RichText::new("SIZE").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(format!("{size_mb:.1} MB"));
                ui.end_row();

                ui.label(egui::RichText::new("MOD").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(&metadata.fs_game);
                ui.label(egui::RichText::new("PROTOCOL").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(metadata.protocol.to_string());
                ui.end_row();

                ui.label(egui::RichText::new("GAME TYPE").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(Self::demo_gametype_name(metadata.gametype));
                ui.label(egui::RichText::new("SERVER").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(metadata.server_name.as_deref().unwrap_or("unknown"));
                ui.end_row();

                ui.label(egui::RichText::new("SNAPSHOTS").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(format!("{}  ({:.1}/s)", metadata.snapshot_count, metadata.average_snapshot_rate));
                ui.label(egui::RichText::new("KILLS").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(metadata.kill_count.to_string());
                ui.end_row();

                ui.label(egui::RichText::new("RECORDED POV").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(pov);
                ui.label(egui::RichText::new("SOURCE").color(theme::TEXT_DISABLED).size(11.0));
                ui.label(source);
                ui.end_row();
            });

        ui.add_space(12.0);
        theme::section(ui, "MAPS", &format!("{} used", metadata.maps.len()));
        for map in &metadata.maps {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&map.map_name).color(theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(Self::format_demo_time(map.duration_ms));
                });
            });
        }

        ui.add_space(10.0);
        theme::section(ui, "PLAYERS", &format!("{} identities · roster presence", metadata.players.len()));
        egui::ScrollArea::vertical()
            .id_salt("demo_metadata_players")
            .max_height(150.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for player in &metadata.players {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&player.name).color(theme::TEXT));
                            let teams = Self::demo_team_summary(&player.teams);
                            let detail = if player.model.is_empty() {
                                teams
                            } else if teams.is_empty() {
                                player.model.clone()
                            } else {
                                format!("{} · {}", player.model, teams)
                            };
                            ui.label(egui::RichText::new(detail).color(theme::TEXT_DISABLED).size(10.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(Self::format_demo_time(player.duration_ms));
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}–{}",
                                    Self::format_demo_time(player.first_seen_ms),
                                    Self::format_demo_time(player.last_seen_ms)
                                ))
                                .color(theme::TEXT_DISABLED)
                                .size(10.0),
                            );
                        });
                    });
                    ui.separator();
                }
            });

        ui.add_space(10.0);
        theme::section(ui, "ROUNDS", &format!("{} detected", metadata.rounds.len()));
        for round in &metadata.rounds {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("ROUND {}", round.index)).color(theme::ACCENT));
                ui.monospace(format!(
                    "{} +{}",
                    Self::format_demo_time(round.start_ms),
                    Self::format_demo_time(round.duration_ms)
                ));
                ui.label(format!("{} kills · {} players", round.kills, round.players));
                if let Some([red, blue]) = round.team_scores.filter(|score| score[0] != 0 || score[1] != 0) {
                    ui.label(format!("red {red} – blue {blue}"));
                } else if let Some((name, score)) = &round.leader {
                    ui.label(format!("leader {name} {score}"));
                }
                if metadata.maps.len() > 1 {
                    ui.label(egui::RichText::new(&round.map_name).color(theme::TEXT_DISABLED));
                }
            });
        }

        ui.add_space(10.0);
        theme::section(ui, "DUELS", &format!("{} detected", metadata.duels.len()));
        if metadata.duels.is_empty() {
            ui.label(egui::RichText::new("No duel rounds visible in this recording.").color(theme::TEXT_DISABLED));
        } else {
            let mut records = BTreeMap::<String, [usize; 3]>::new(); // W / L / unknown-draw
            for duel in &metadata.duels {
                for player in &duel.players {
                    let record = records.entry(player.clone()).or_default();
                    match duel.winner.as_deref() {
                        Some(winner) if winner == player => record[0] += 1,
                        Some(_) => record[1] += 1,
                        None => record[2] += 1,
                    }
                }
            }
            if !records.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("OVERVIEW").color(theme::TEXT_DISABLED).size(10.0));
                    for (player, [wins, losses, unknown]) in records {
                        let suffix = if unknown == 0 {
                            format!("{wins}-{losses}")
                        } else {
                            format!("{wins}-{losses} (+{unknown} unresolved)")
                        };
                        ui.label(format!("{player} {suffix}"));
                    }
                });
            }
            for duel in &metadata.duels {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(format!("DUEL {}", duel.index)).color(theme::ACCENT));
                    ui.label(egui::RichText::new(duel.kind.label()).color(theme::TEXT_DISABLED).size(10.0));
                    ui.monospace(format!(
                        "{} +{}",
                        Self::format_demo_time(duel.start_ms),
                        Self::format_demo_time(duel.duration_ms)
                    ));
                    if !duel.players.is_empty() {
                        ui.label(duel.players.join(" vs "));
                    }
                    if let Some(winner) = &duel.winner {
                        ui.label(format!("winner {winner}"));
                    }
                    if !duel.scores.is_empty() {
                        let scores = duel.scores.iter()
                            .map(|(name, score)| format!("{name} {score}"))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.label(egui::RichText::new(scores).color(theme::TEXT_DISABLED).size(10.0));
                    }
                });
            }
            if metadata.duels.iter().any(|duel| matches!(duel.kind, frontend::DemoDuelKind::PrivatePov)) {
                ui.label(egui::RichText::new("PRIVATE (POV) duels are limited to duels exposed by the recorded player's playerState.")
                    .color(theme::TEXT_DISABLED).size(10.0));
            }
        }

        ui.add_space(10.0);
        theme::section(ui, "CONSOLE", &format!("{} messages", metadata.console.len()));
        ui.horizontal(|ui| {
            for (label, filter) in [
                ("ALL", DemoConsoleFilter::All),
                ("PRINT", DemoConsoleFilter::Print),
                ("CHAT", DemoConsoleFilter::Chat),
                ("CENTER", DemoConsoleFilter::CenterPrint),
                ("EVENTS", DemoConsoleFilter::Event),
            ] {
                if ui.selectable_label(*console_filter == filter, label).clicked() {
                    *console_filter = filter;
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("demo_metadata_console")
            .max_height(190.0)
            .stick_to_bottom(false)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for entry in metadata.console.iter().filter(|entry| (*console_filter).matches(&entry.kind)) {
                    let row = ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(Self::format_demo_time(entry.elapsed_ms))
                                .color(theme::TEXT_DISABLED)
                                .monospace(),
                        );
                        ui.label(
                            egui::RichText::new(entry.kind.label())
                                .color(theme::ACCENT)
                                .size(10.0),
                        );
                        // The theme's default is `Extend`; one long chat line
                        // would otherwise widen the whole page frame.
                        ui.add(egui::Label::new(&entry.text).wrap());
                    });
                    if row.response.double_clicked() {
                        *launch_seek_ms = Some(entry.elapsed_ms);
                    }
                    row.response.on_hover_text("Double-click to play this demo from here");
                }
            });
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
        self.ensure_demo_metadata(&selected_name);

        let browser_height = ui.available_height().max(1.0);
        let mut console_seek_request = None;
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

            let pane_w = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(pane_w, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    // Without a hard cap the inspector's scroll area sizes
                    // itself from the widest line it has ever laid out and
                    // drags the whole page frame wider than its 1120pt column.
                    ui.set_max_width(pane_w);
                    theme::section(ui, "SELECTED DEMO", "");
                    theme::glow_label(ui, &selected_name, 18.0, theme::TEXT);
                    ui.add_space(10.0);

                    let inspector_height = (ui.available_height() - 56.0).max(180.0);
                    let inspector_w = ui.available_width();
                    egui::Frame::new()
                        .fill(theme::CONTROL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_width((inspector_w - 26.0).max(1.0));
                            ui.set_min_height(inspector_height);
                            if let Some(metadata) = self.demo_metadata_cache.get(&selected_name) {
                                let mut filter = self.demo_console_filter;
                                egui::ScrollArea::vertical()
                                    .id_salt("demo_metadata_inspector")
                                    .max_height(inspector_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        Self::render_demo_metadata(
                                            ui,
                                            metadata,
                                            &mut filter,
                                            &mut console_seek_request,
                                        );
                                    });
                                self.demo_console_filter = filter;
                            } else if let Some(error) = self.demo_metadata_errors.get(&selected_name) {
                                theme::banner(ui, &format!("Metadata scan failed: {error}"), theme::WARNING);
                            } else {
                                ui.vertical_centered(|ui| {
                                    ui.add_space((inspector_height * 0.35).max(20.0));
                                    ui.spinner();
                                    ui.label(
                                        egui::RichText::new("INDEXING SELECTED DEMO…")
                                            .color(theme::TEXT_DISABLED),
                                    );
                                });
                            }
                        });

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
        if let Some(elapsed_ms) = console_seek_request {
            self.play_selected_demo_at(elapsed_ms);
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
        self.sync_profile_preview_mode(
            !self.front_end && self.overlay == OverlayMode::Game && self.menu_selected == TOP_PROFILE,
        );
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    /// `uipage <name> [tab]`: jump straight to a menu page. A scripting aid
    /// for screenshotting every page (`+uipage setup audio +screenshot`).
    pub(super) fn open_ui_page(&mut self, args: &[&str]) {
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
                Some("controls") => FrontendPage::Controls,
                Some("devtools") => FrontendPage::DeveloperTools,
                Some("assets") => FrontendPage::AssetViewer,
                Some("maps") => FrontendPage::MapViewer,
                Some("setup") => {
                    self.setup_selected = Self::setup_tab_index(tab.as_deref());
                    self.set_overlay(OverlayMode::Video);
                    return;
                }
                _ => {
                    self.console_status =
                        "USAGE: uipage main|play|servers|solo|demo|controls|devtools|assets|maps|setup [game|camera|video|audio|network|interface]".into();
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

    fn setup_tab_index(tab: Option<&str>) -> usize {
        tab.and_then(|tab| SETUP_TABS.iter().position(|name| name.eq_ignore_ascii_case(tab)))
            .unwrap_or(SETUP_TAB_VIDEO)
    }

    pub(super) fn remembered_in_game_menu_overlay(&mut self) -> OverlayMode {
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

    fn egui_page(&mut self, ui: &mut egui::Ui) {
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

    // -------------------------------------------------------------- pages --

    fn profile_readout(ui: &mut egui::Ui, label: &str, value: &str) {
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            theme::label(ui, theme::plain(label, 11.5, theme::TEXT_FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::label(ui, theme::plain(value, 11.5, theme::TEXT));
            });
        });
    }

    fn profile_model_icon_texture(
        &mut self,
        entry: &ProfileModelEntry,
    ) -> Option<egui::TextureHandle> {
        let key = entry.value.to_ascii_lowercase();
        if let Some(texture) = self.profile_model_icon_textures.get(&key) {
            return Some(texture.clone());
        }
        if self.ui_catalog.icons_missing.contains(&key) || self.ui_catalog.icons_inflight.contains(&key) {
            return None;
        }
        self.ui_catalog.icons_inflight.insert(key.clone());
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ProfileIcon {
                key,
                model: entry.model_name.clone(),
                skin: entry.skin_name.clone(),
            },
        );
        None
    }

    fn profile_model_matches_team_filter(entry: &ProfileModelEntry, filter: u8) -> bool {
        if filter == 0 {
            return true;
        }
        let skin = entry.skin_name.to_ascii_lowercase();
        let wanted = if filter == 1 { "red" } else { "blue" };
        skin == wanted
            || skin.starts_with(&format!("{wanted}_"))
            || skin.starts_with(&format!("{wanted}-"))
            || skin.starts_with(wanted)
            || skin.ends_with(&format!("_{wanted}"))
            || skin.ends_with(&format!("-{wanted}"))
    }

    fn profile_saber_label(entry: &ProfileSaberEntry) -> &str {
        // Stock saber files may use @MENUS_* localization tokens. Until the
        // string-package browser is wired into egui, the block identifier is a
        // cleaner fallback than exposing the raw token.
        if entry.display_name.is_empty() || entry.display_name.starts_with('@') {
            &entry.name
        } else {
            &entry.display_name
        }
    }

    fn profile_saber_is_single(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_SINGLE")
    }

    fn profile_saber_is_staff(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_STAFF")
    }

    fn profile_server_force_limits(&self) -> (u8, u32, i32, bool, Option<u8>) {
        let info = if let Some(net) = self.net.as_ref() {
            crate::net::mod_support::server_info(&net.session().decoder().configstrings)
        } else if let Some(server) = self.local_server.as_ref() {
            server
                .configstrings()
                .get(&crate::cgame::CS_SERVERINFO)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            &[]
        };
        let value = |key: &[u8], fallback: i32| {
            jka_protocol::commands::info_value(info, key)
                .map(jka_protocol::commands::atoi)
                .unwrap_or(fallback)
        };
        let max_rank = value(b"g_maxForceRank", 7).clamp(0, 7) as u8;
        let disabled = value(b"g_forcePowerDisable", 0) as u32;
        let gametype = value(b"g_gametype", 0);

        // TaystJK UI_HasSetSaberOnly, ported verbatim in behavior. Jedi Master
        // never grants the free saber; duel modes use g_duelWeaponDisable and
        // other modes use g_weaponDisable. WP_NONE=0, WP_SABER=3, WP_NUM=19.
        let free_saber = if gametype == 2 {
            false
        } else {
            let weapon_disable = if matches!(gametype, 3 | 4) {
                value(b"g_duelWeaponDisable", 0)
            } else {
                value(b"g_weaponDisable", 0)
            } as u32;
            (0..19).all(|weapon| {
                weapon == 0 || weapon == 3 || (weapon_disable & (1u32 << weapon)) != 0
            })
        };

        // UI_DrawForceSide: force-based teams pin Red to Dark and Blue to Light.
        let forced_side = if value(b"g_forceBasedTeams", 0) != 0 {
            self.game_session.as_ref().and_then(|session| {
                let client_num = session
                    .current_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                    .and_then(|client| usize::try_from(client).ok())?;
                let team = session
                    .client_game
                    .client_info(client_num, &session.siege_classes)?
                    .team;
                match team {
                    1 => Some(2), // TEAM_RED -> FORCE_DARKSIDE
                    2 => Some(1), // TEAM_BLUE -> FORCE_LIGHTSIDE
                    _ => None,
                }
            })
        } else {
            None
        };
        (max_rank, disabled, gametype, free_saber, forced_side)
    }

    fn egui_profile_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_profile_catalog();
        theme::page_title(
            ui,
            "PROFILE",
            "Protocol-26 player identity, appearance, Force loadout and saber selection.",
        );

        // jaPRO's player menu gates everything on APPLY (setForce -> forcechanged
        // + userinfo). It lives above the section tabs so it is always reachable.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let pending = self.profile_has_pending_changes();
            let response = if pending {
                theme::primary_button(ui, "APPLY")
            } else {
                theme::ghost_button(ui, "APPLY")
            };
            if response.clicked() {
                self.apply_profile_changes();
            }
            theme::label(
                ui,
                if pending {
                    theme::plain("Changes are local until applied.", 11.0, theme::WARNING)
                } else {
                    theme::plain("All changes applied.", 11.0, theme::TEXT_FAINT)
                },
            );
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (index, label) in PROFILE_SECTIONS.iter().enumerate() {
                if theme::chip(ui, label, self.profile_selected_section == index).clicked() {
                    self.profile_selected_section = index;
                    self.profile_preview_key = None;
                }
            }
        });
        ui.add_space(10.0);

        let height = ui.available_height().max(280.0);
        ui.horizontal(|ui| {
            let controls_w = (ui.available_width() * 0.46).clamp(390.0, 610.0);
            ui.allocate_ui_with_layout(
                egui::vec2(controls_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::PANEL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_min_width((controls_w - 24.0).max(1.0));
                            ui.set_min_height((height - 24.0).max(1.0));
                            egui::ScrollArea::vertical()
                                .id_salt("profile_controls")
                                .auto_shrink([false, false])
                                .show(ui, |ui| match self.profile_selected_section {
                                    PROFILE_IDENTITY => self.egui_profile_identity(ui),
                                    PROFILE_MODEL => self.egui_profile_model(ui),
                                    PROFILE_FORCE => self.egui_profile_force(ui),
                                    PROFILE_SABER => self.egui_profile_saber(ui),
                                    PROFILE_COSMETICS => self.egui_profile_cosmetics(ui),
                                    _ => {}
                                });
                        });
                },
            );

            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let saber = self.profile_selected_section == PROFILE_SABER;
                    // The preview frame starts on the same line as the controls
                    // frame and shares its height; its heading is painted
                    // inside the frame so the two panes read as a matched pair.
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::drag(),
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    if response.dragged() {
                        let delta = ui.input(|input| input.pointer.delta());
                        if delta.x.abs() > f32::EPSILON {
                            self.profile_preview_yaw =
                                (self.profile_preview_yaw + delta.x * 0.45).rem_euclid(360.0);
                            self.profile_preview_key = None;
                        }
                    }
                    if response.hovered() {
                        let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
                        if scroll.abs() > f32::EPSILON {
                            self.profile_preview_zoom = (self.profile_preview_zoom
                                * (-scroll * 0.0015).exp())
                                .clamp(0.45, 2.5);
                            self.profile_preview_key = None;
                        }
                    }
                    self.set_asset_preview_viewport(rect, ui.ctx().pixels_per_point());
                    let painter = ui.painter().clone();
                    theme::glow_text(
                        &painter,
                        rect.left_top() + egui::vec2(14.0, 14.0),
                        egui::Align2::LEFT_TOP,
                        if saber { "SABER PREVIEW" } else { "PLAYER PREVIEW" },
                        egui::FontId::proportional(12.5),
                        theme::ACCENT,
                    );
                    theme::glow_text(
                        &painter,
                        rect.left_top() + egui::vec2(14.0, 34.0),
                        egui::Align2::LEFT_TOP,
                        "Drag to rotate · wheel to zoom",
                        egui::FontId::proportional(11.5),
                        theme::TEXT_FAINT,
                    );
                    let caption = if saber {
                        &self.network.saber1
                    } else {
                        &self.solo_client_info.model_name
                    };
                    theme::glow_text(
                        &painter,
                        rect.left_bottom() + egui::vec2(14.0, -12.0),
                        egui::Align2::LEFT_BOTTOM,
                        caption,
                        egui::FontId::proportional(11.5),
                        theme::TEXT_DIM,
                    );
                },
            );
        });
        // The Profile idle is intentionally capped to ~30 Hz rather than
        // forcing egui to repaint at the uncapped game/render rate.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        self.update_profile_preview();
    }

    fn egui_profile_identity(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "IDENTITY", "Userinfo sent to protocol-26 servers");
        theme::label(ui, theme::plain("NAME", 11.5, theme::TEXT_FAINT));
        ui.add_space(4.0);
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = profile_name_layout(buffer.as_str(), 14.0, theme::TEXT);
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let edit = egui::TextEdit::singleline(&mut self.profile_name_input)
            .desired_width(ui.available_width())
            .hint_text("Player name")
            .layouter(&mut layouter);
        let response = ui.add(edit);
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.apply_profile_changes();
        }
        ui.add_space(8.0);

        theme::section(ui, "CURRENT LOADOUT", "Uses the same cvars as the stock client");
        Self::profile_readout(ui, "MODEL", &self.solo_client_info.model_cvar());
        Self::profile_readout(ui, "PRIMARY SABER", &self.network.saber1);
        Self::profile_readout(ui, "SECONDARY SABER", &self.network.saber2);
        Self::profile_readout(ui, "FORCE", &self.network.forcepowers);
        ui.add_space(8.0);
        self.egui_profile_skin_tint(ui);
    }

    /// `char_color_red/green/blue`: the tint servers relay as `customRGBA`. Only
    /// the parts of a model whose shader reads the entity colour follow it (for
    /// example the armour plates of jedi_zf), so 255/255/255 leaves every model as authored.
    fn egui_profile_skin_tint(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "SKIN TINT",
            "Tints the entity-coloured parts of models that support it. 255 / 255 / 255 is untinted.",
        );
        let mut channels = self.network.char_color.map(f32::from);
        let mut picked = None;
        for (channel, (cvar, label)) in [("char_color_red", "RED"), ("char_color_green", "GREEN"), ("char_color_blue", "BLUE")]
            .into_iter()
            .enumerate()
        {
            theme::row(ui, label, "Player tint channel, 0-255.", theme::Reset::None, |ui| {
                let readout = format!("{:.0}", channels[channel]);
                if theme::slider(ui, &mut channels[channel], 0.0..=255.0, &readout) {
                    picked = Some((cvar, channels[channel].round() as u8));
                }
            });
        }
        let swatch = egui::Color32::from_rgb(channels[0] as u8, channels[1] as u8, channels[2] as u8);
        let mut reset = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(46.0, 16.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, swatch);
            theme::label(ui, theme::plain(&format!("#{:02X}{:02X}{:02X}", swatch.r(), swatch.g(), swatch.b()), 11.5, theme::TEXT_DIM));
            reset = theme::chip(ui, "RESET", false).clicked();
        });
        // Applied to the preview at once; the userinfo goes out with APPLY.
        if reset {
            for cvar in ["char_color_red", "char_color_green", "char_color_blue"] {
                let _ = self.profile_set_cvar(cvar, "255");
            }
        } else if let Some((cvar, value)) = picked {
            if let Err(error) = self.profile_set_cvar(cvar, &value.to_string()) {
                self.console_status = error;
            }
        }
    }

    fn egui_profile_model(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "PLAYER MODEL",
            "One tile per humanoid model · hover a tile to choose its skin variants",
        );

        ui.horizontal(|ui| {
            let filters_width = 154.0;
            ui.add_sized(
                [(ui.available_width() - filters_width).max(120.0), 24.0],
                egui::TextEdit::singleline(&mut self.profile_model_search)
                    .hint_text("Search model or skin…"),
            );
            if theme::chip(ui, "ALL", self.profile_model_team_filter == 0).clicked() {
                self.profile_model_team_filter = 0;
            }
            if theme::chip(ui, "RED", self.profile_model_team_filter == 1).clicked() {
                self.profile_model_team_filter = 1;
            }
            if theme::chip(ui, "BLUE", self.profile_model_team_filter == 2).clicked() {
                self.profile_model_team_filter = 2;
            }
        });

        if let Some(error) = &self.profile_catalog_error {
            ui.add_space(8.0);
            theme::banner(ui, error, theme::WARNING);
        }
        ui.add_space(10.0);

        let needle = self.profile_model_search.trim().to_ascii_lowercase();
        let mut models = BTreeMap::<String, Vec<ProfileModelEntry>>::new();
        for entry in self.profile_models.iter().filter(|entry| {
            Self::profile_model_matches_team_filter(entry, self.profile_model_team_filter)
        }) {
            models.entry(entry.model_name.clone()).or_default().push(entry.clone());
        }
        models.retain(|model_name, variants| {
            if needle.is_empty() || model_name.to_ascii_lowercase().contains(&needle) {
                true
            } else {
                variants.iter().any(|entry| {
                    entry.skin_name.to_ascii_lowercase().contains(&needle)
                        || entry.value.to_ascii_lowercase().contains(&needle)
                })
            }
        });

        let current = self.solo_client_info.model_cvar();
        let current_model = self.solo_client_info.model_name.clone();
        let columns = ((ui.available_width() + 8.0) / (92.0 + 8.0)).floor().max(1.0) as usize;
        // Tiles grow (up to 20%) to absorb the slack, so the grid spans the
        // pane instead of leaving a ragged strip down its right side.
        let tile_w = ((ui.available_width() + 8.0) / columns as f32 - 8.0).clamp(92.0, 110.0);
        let tile = egui::vec2(tile_w, 116.0 * tile_w / 92.0);
        let mut pending_model: Option<String> = None;
        let mut visible_count = 0usize;

        egui::Grid::new("profile-model-grid")
            .num_columns(columns)
            .spacing(egui::vec2(8.0, 10.0))
            .show(ui, |ui| {
                for (model_name, mut variants) in models {
                    variants.sort_by(|a, b| a.skin_name.cmp(&b.skin_name));
                    let representative = variants
                        .iter()
                        .find(|entry| entry.value.eq_ignore_ascii_case(&current))
                        .or_else(|| variants.iter().find(|entry| entry.skin_name.eq_ignore_ascii_case("default")))
                        .unwrap_or(&variants[0])
                        .clone();
                    let selected_model = model_name.eq_ignore_ascii_case(&current_model);
                    let (rect, response) = ui.allocate_exact_size(tile, egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(3),
                        if selected_model { theme::PANEL_FILL } else { egui::Color32::TRANSPARENT },
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(3),
                        egui::Stroke::new(
                            if selected_model { 2.0_f32 } else { 1.0_f32 },
                            if selected_model { theme::ACCENT } else { theme::LINE },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    let image_rect = egui::Rect::from_min_max(
                        rect.min + egui::vec2(4.0, 4.0),
                        egui::pos2(rect.max.x - 4.0, rect.max.y - 25.0),
                    );
                    if let Some(texture) = self.profile_model_icon_texture(&representative) {
                        ui.painter().image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        ui.painter().rect_filled(image_rect, egui::CornerRadius::same(2), theme::PANEL_FILL);
                        ui.painter().text(
                            image_rect.center(), egui::Align2::CENTER_CENTER, "NO ICON",
                            egui::FontId::proportional(9.0), theme::TEXT_FAINT,
                        );
                    }
                    ui.painter().text(
                        egui::pos2(rect.center().x, rect.max.y - 12.0),
                        egui::Align2::CENTER_CENTER,
                        &model_name,
                        egui::FontId::proportional(10.0),
                        if selected_model { theme::TEXT } else { theme::TEXT_DIM },
                    );

                    let response = response.on_hover_ui(|ui| {
                        ui.set_min_width(220.0);
                        theme::label(ui, theme::plain(&model_name.to_ascii_uppercase(), 10.5, theme::TEXT_FAINT));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for entry in &variants {
                                let variant_size = egui::vec2(60.0, 80.0);
                                let (vrect, vresponse) = ui.allocate_exact_size(variant_size, egui::Sense::click());
                                let selected = entry.value.eq_ignore_ascii_case(&current);
                                ui.painter().rect_stroke(
                                    vrect,
                                    egui::CornerRadius::same(2),
                                    egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, if selected { theme::ACCENT } else { theme::LINE }),
                                    egui::StrokeKind::Inside,
                                );
                                let vimage = egui::Rect::from_min_max(
                                    vrect.min + egui::vec2(3.0, 3.0),
                                    egui::pos2(vrect.max.x - 3.0, vrect.max.y - 20.0),
                                );
                                if let Some(texture) = self.profile_model_icon_texture(entry) {
                                    ui.painter().image(
                                        texture.id(), vimage,
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        egui::Color32::WHITE,
                                    );
                                }
                                ui.painter().text(
                                    egui::pos2(vrect.center().x, vrect.max.y - 9.0),
                                    egui::Align2::CENTER_CENTER,
                                    &entry.skin_name,
                                    egui::FontId::proportional(8.5),
                                    theme::TEXT_DIM,
                                );
                                if vresponse.clicked() {
                                    pending_model = Some(entry.value.clone());
                                }
                            }
                        });
                    });
                    if response.clicked() {
                        pending_model = Some(representative.value.clone());
                    }

                    visible_count += 1;
                    if visible_count % columns == 0 {
                        ui.end_row();
                    }
                }
            });

        if let Some(value) = pending_model {
            if !value.eq_ignore_ascii_case(&current) {
                if let Err(error) = self.profile_set_cvar("model", &value) {
                    self.console_status = error;
                } else {
                    self.profile_preview_key = None;
                }
            }
        }

        if self.ui_catalog.pending.profile {
            theme::label(ui, theme::plain("Scanning player models...", 11.0, theme::TEXT_FAINT));
        } else if self.profile_models.is_empty() {
            theme::banner(ui, "No humanoid models/players/*/model.glm assets were found.", theme::WARNING);
        } else if visible_count == 0 {
            theme::label(ui, theme::plain("No models match this filter.", 11.0, theme::TEXT_FAINT));
        }

        theme::section(
            ui,
            "FORCED PLAYER MODELS",
            "Client-side only · preserves your own selected model",
        );
        let parsed = crate::cgame::ForcedPlayerModels::parse(&self.force_model)
            .ok()
            .flatten();
        let mut mode = match &parsed {
            None => 0u8,
            Some(models) if models.split => 2,
            Some(_) => 1,
        };
        let own = self.solo_client_info.model_cvar();
        let mut ally = parsed.as_ref().map(|models| models.ally.clone()).unwrap_or_else(|| own.clone());
        let mut enemy = parsed.as_ref().map(|models| models.enemy.clone()).unwrap_or_else(|| own.clone());

        ui.horizontal(|ui| {
            if theme::chip(ui, "OFF", mode == 0).clicked() {
                mode = 0;
            }
            if theme::chip(ui, "ALL", mode == 1).clicked() {
                if mode == 0 {
                    ally = own.clone();
                    enemy = own.clone();
                } else if mode == 2 {
                    enemy = ally.clone();
                }
                mode = 1;
            }
            if theme::chip(ui, "ALLY / ENEMY", mode == 2).clicked() {
                if mode == 0 {
                    ally = own.clone();
                    enemy = own.clone();
                }
                mode = 2;
            }
        });

        let mut choices = self.profile_models.iter().map(|entry| entry.value.clone()).collect::<Vec<_>>();
        choices.sort_by_key(|value| value.to_ascii_lowercase());
        choices.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let model_combo = |ui: &mut egui::Ui, id: &str, label: &str, value: &mut String| {
            ui.horizontal(|ui| {
                theme::label(ui, theme::plain(label, 10.5, theme::TEXT_FAINT));
                egui::ComboBox::from_id_salt(id)
                    .selected_text(value.as_str())
                    .width((ui.available_width() - 4.0).max(150.0))
                    .show_ui(ui, |ui| {
                        for candidate in &choices {
                            ui.selectable_value(value, candidate.clone(), candidate.as_str());
                        }
                    });
            });
        };
        match mode {
            1 => model_combo(ui, "profile-force-model-all", "MODEL", &mut ally),
            2 => {
                model_combo(ui, "profile-force-model-ally", "ALLY", &mut ally);
                model_combo(ui, "profile-force-model-enemy", "ENEMY", &mut enemy);
            }
            _ => {
                theme::label(
                    ui,
                    theme::plain(
                        "Players use their own advertised models.",
                        10.5,
                        theme::TEXT_FAINT,
                    ),
                );
            }
        }

        let desired = match mode {
            0 => "0".to_owned(),
            1 => ally.clone(),
            _ => format!("{},{}", ally, enemy),
        };
        if !desired.eq_ignore_ascii_case(&self.force_model) {
            if let Err(error) = self.set_console_cvar("cg_forceModel", &desired) {
                self.console_status = error;
            }
        }
        Self::profile_readout(ui, "CVAR", &self.force_model);
    }

    fn egui_profile_force(&mut self, ui: &mut egui::Ui) {
        let (max_rank, disabled, gametype, free_saber, forced_side) =
            self.profile_server_force_limits();
        let mut force = ProfileForceConfig::parse(&self.network.forcepowers);
        force.rank = max_rank;
        if let Some(side) = forced_side {
            force.side = side;
        }
        force.normalize(max_rank, disabled, gametype, free_saber);
        let before = force.serialize();

        theme::section(ui, "ALIGNMENT", "Light and Dark powers follow stock JKA restrictions");
        ui.horizontal(|ui| {
            let light = theme::chip(ui, "LIGHT", force.side == 1);
            let dark = theme::chip(ui, "DARK", force.side == 2);
            if forced_side.is_none() {
                if light.clicked() { force.side = 1; }
                if dark.clicked() { force.side = 2; }
            }
        });
        if forced_side.is_some() {
            theme::label(ui, theme::plain("Side is fixed by g_forceBasedTeams for your current team.", 10.5, theme::TEXT_FAINT));
        }

        theme::section(ui, "SERVER FORCE BUDGET", "g_maxForceRank controls the available points");
        force.normalize(max_rank, disabled, gametype, free_saber);
        let used = force.used(free_saber);
        let budget = force.budget();
        Self::profile_readout(
            ui,
            "MASTERY",
            &format!("{} ({max_rank})", FORCE_MASTERY_NAMES[max_rank as usize]),
        );
        Self::profile_readout(ui, "POINTS", &format!("{used} / {budget}"));

        theme::section(ui, "FORCE POWERS", "Neutral powers first, then the selected alignment");
        let aligned = if force.side == 2 { &FORCE_DARK_ORDER[..] } else { &FORCE_LIGHT_ORDER[..] };
        let order = FORCE_NEUTRAL_ORDER.iter().copied().chain(aligned.iter().copied());
        let mut clicked_power = None;
        for power in order {
            let server_disabled = disabled & (1u32 << power) != 0 && !matches!(power, 1 | 15 | 16);
            let team_disabled = gametype < 6 && matches!(power, 11 | 12);
            let saber_dependency = matches!(power, 16 | 17) && force.powers[15] == 0;
            let locked = server_disabled || team_disabled || saber_dependency;
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let color = if locked { theme::TEXT_DISABLED } else { theme::TEXT };
                theme::label(ui, theme::plain(FORCE_NAMES[power], 11.5, color));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let min_level = if power == 1 || (free_saber && matches!(power, 15 | 16)) { 1 } else { 0 };
                    for level in (min_level..=3).rev() {
                        let response = theme::chip(ui, &level.to_string(), force.powers[power] == level as u8);
                        if response.clicked() && !locked && force.powers[power] != level as u8 {
                            force.powers[power] = level as u8;
                            clicked_power = Some(power);
                        }
                    }
                });
            });
            ui.add_space(3.0);
        }

        if let Some(power) = clicked_power {
            // Delay the animation until allocation clicks settle. Repeated level
            // clicks for one power therefore restart a single preview, not a
            // full Ghoul2 animation setup on every UI event.
            self.profile_force_preview_pending = Some((power, Instant::now()));
        }

        force.normalize(max_rank, disabled, gametype, free_saber);
        let after = force.serialize();
        if after != before {
            if let Err(error) = self.profile_set_cvar("forcepowers", &after) {
                self.console_status = error;
            } else {
                self.profile_force_pending = true;
            }
        }
    }

    fn egui_profile_saber(&mut self, ui: &mut egui::Ui) {
        let singles = self.profile_sabers
            .iter()
            .filter(|entry| Self::profile_saber_is_single(entry))
            .cloned()
            .collect::<Vec<_>>();
        let staffs = self.profile_sabers
            .iter()
            .filter(|entry| Self::profile_saber_is_staff(entry))
            .cloned()
            .collect::<Vec<_>>();

        let current_primary = self.network.saber1.clone();
        let current_secondary = self.network.saber2.clone();
        let secondary_active = !current_secondary.is_empty()
            && !current_secondary.eq_ignore_ascii_case("none")
            && !current_secondary.eq_ignore_ascii_case("remove");
        let primary_is_staff = staffs.iter().any(|entry| entry.name.eq_ignore_ascii_case(&current_primary));
        let mut mode = if secondary_active { 1u8 } else if primary_is_staff { 2u8 } else { 0u8 };
        let mut primary = current_primary.clone();
        let mut secondary = current_secondary.clone();

        let default_single = singles
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case("single_1"))
            .or_else(|| singles.first())
            .map(|entry| entry.name.clone());
        let default_staff = staffs
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case("dual_1"))
            .or_else(|| staffs.first())
            .map(|entry| entry.name.clone());

        theme::section(ui, "SABER CONFIGURATION", "Stock saber types, presented as a modern compact selector");
        ui.horizontal(|ui| {
            if theme::chip(ui, "SINGLE", mode == 0).clicked() && mode != 0 {
                mode = 0;
                if !singles.iter().any(|entry| entry.name.eq_ignore_ascii_case(&primary)) {
                    if let Some(name) = &default_single { primary = name.clone(); }
                }
                secondary = "none".to_owned();
            }
            if theme::chip(ui, "DUAL", mode == 1).clicked() && mode != 1 {
                mode = 1;
                if !singles.iter().any(|entry| entry.name.eq_ignore_ascii_case(&primary)) {
                    if let Some(name) = &default_single { primary = name.clone(); }
                }
                if !singles.iter().any(|entry| entry.name.eq_ignore_ascii_case(&secondary)) {
                    secondary = primary.clone();
                }
            }
            if theme::chip(ui, "STAFF", mode == 2).clicked() && mode != 2 {
                mode = 2;
                if !staffs.iter().any(|entry| entry.name.eq_ignore_ascii_case(&primary)) {
                    if let Some(name) = &default_staff { primary = name.clone(); }
                }
                secondary = "none".to_owned();
            }
        });

        ui.add_space(8.0);
        let combo = |ui: &mut egui::Ui, id: &str, label: &str, selected: &mut String, entries: &[ProfileSaberEntry]| {
            ui.horizontal(|ui| {
                theme::label(ui, theme::plain(label, 11.0, theme::TEXT_FAINT));
                let selected_text = entries
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(selected))
                    .map(Self::profile_saber_label)
                    .unwrap_or(selected.as_str())
                    .to_owned();
                egui::ComboBox::from_id_salt(id)
                    .selected_text(selected_text)
                    .width((ui.available_width() - 4.0).max(150.0))
                    .show_ui(ui, |ui| {
                        for entry in entries {
                            ui.selectable_value(selected, entry.name.clone(), Self::profile_saber_label(entry));
                        }
                    });
            });
        };

        match mode {
            1 => {
                combo(ui, "profile-saber-right", "RIGHT HAND", &mut primary, &singles);
                combo(ui, "profile-saber-left", "LEFT HAND", &mut secondary, &singles);
            }
            2 => combo(ui, "profile-saber-staff", "STAFF HILT", &mut primary, &staffs),
            _ => combo(ui, "profile-saber-single", "HILT", &mut primary, &singles),
        }

        let desired_secondary = if mode == 1 { secondary.as_str() } else { "none" };
        if primary != current_primary {
            if let Err(error) = self.profile_set_cvar("saber1", &primary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }
        if !desired_secondary.eq_ignore_ascii_case(&current_secondary) {
            if let Err(error) = self.profile_set_cvar("saber2", desired_secondary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }

        self.egui_profile_saber_colors(ui, mode == 1);

        if let Some(entry) = self.profile_sabers.iter().find(|entry| entry.name.eq_ignore_ascii_case(&primary)) {
            theme::section(ui, "SELECTED", "Parsed directly from ext_data/sabers");
            Self::profile_readout(ui, "NAME", Self::profile_saber_label(entry));
            Self::profile_readout(ui, "TYPE", &entry.saber_type);
            Self::profile_readout(ui, "MODEL", &entry.model);
            Self::profile_readout(ui, "SKIN", entry.custom_skin.as_deref().unwrap_or("default"));
            Self::profile_readout(ui, "BLADES", &entry.num_blades.to_string());
        }

        if self.profile_sabers.is_empty() {
            theme::banner(ui, "No saber definitions were found.", theme::WARNING);
        } else if singles.is_empty() {
            theme::banner(ui, "No SABER_SINGLE definitions were found.", theme::WARNING);
        } else if mode == 2 && staffs.is_empty() {
            theme::banner(ui, "No SABER_STAFF definitions were found.", theme::WARNING);
        }
    }

    /// jaPRO UI_UpdateSaberCvars: `color1`/`color2` pick a stock blade colour, or
    /// `SABER_RGB` with the colour in `cp_sbRGB1`/`cp_sbRGB2`. Only jaPRO and JA+
    /// servers draw RGB; others get the closest stock colour (see userinfo_for_mod).
    fn egui_profile_saber_colors(&mut self, ui: &mut egui::Ui, dual: bool) {
        use crate::net::{base_saber_rgb_packed, SABER_RGB};
        const COLORS: [&str; 6] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple"];

        theme::section(
            ui,
            "BLADE COLOUR",
            "Stock colours work on every server. RGB is drawn on jaPRO and JA+ servers; elsewhere the closest stock colour is sent.",
        );
        let hands: &[(usize, &str)] = if dual { &[(0, "RIGHT HAND"), (1, "LEFT HAND")] } else { &[(0, "BLADE")] };
        for &(hand, label) in hands {
            let (color_cvar, rgb_cvar) = if hand == 0 { ("color1", "cp_sbRGB1") } else { ("color2", "cp_sbRGB2") };
            let color = if hand == 0 { self.network.color1 } else { self.network.color2 };
            let packed = if hand == 0 { self.network.sb_rgb1 } else { self.network.sb_rgb2 };
            let current = i32::from(color);

            theme::label(ui, theme::plain(label, 11.0, theme::TEXT_FAINT));
            let mut picked = None;
            ui.horizontal_wrapped(|ui| {
                for (index, name) in COLORS.iter().enumerate() {
                    if theme::chip(ui, name, current == index as i32).clicked() {
                        picked = Some(index as u8);
                    }
                    ui.add_space(3.0);
                }
                if theme::chip(ui, "RGB", current == SABER_RGB).clicked() {
                    picked = Some(SABER_RGB as u8);
                }
            });
            if let Some(index) = picked.filter(|&index| index != color) {
                // Start the RGB picker from the colour being replaced.
                if i32::from(index) == SABER_RGB && packed == 0 {
                    let _ = self.profile_set_cvar(rgb_cvar, &base_saber_rgb_packed(color).to_string());
                }
                if let Err(error) = self.profile_set_cvar(color_cvar, &index.to_string()) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            }

            if current == SABER_RGB {
                // jaPRO reads an unset (0) colour as pure red. Keep its packed
                // r | g << 8 | b << 16 storage, but present the same egui RGB
                // picker used by the Sunlight Override instead of three sliders.
                let packed = if packed == 0 { 255 } else { packed };
                let mut rgb = [
                    (packed & 255) as f32 / 255.0,
                    ((packed >> 8) & 255) as f32 / 255.0,
                    ((packed >> 16) & 255) as f32 / 255.0,
                ];
                let mut changed = false;
                theme::row(
                    ui,
                    "Color",
                    "Custom saber blade color. Stored as jaPRO cp_sbRGB packed RGB.",
                    theme::Reset::None,
                    |ui| {
                        changed = ui.color_edit_button_rgb(&mut rgb).changed();
                        ui.add_space(10.0);
                        let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                        theme::glow_label(
                            ui,
                            &format!(
                                "{}  {}  {}",
                                to_u8(rgb[0]),
                                to_u8(rgb[1]),
                                to_u8(rgb[2])
                            ),
                            12.5,
                            theme::TEXT_FAINT,
                        );
                    },
                );
                if changed {
                    let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
                    let value = to_u8(rgb[0]) | (to_u8(rgb[1]) << 8) | (to_u8(rgb[2]) << 16);
                    // Picking updates the preview and config at once; userinfo
                    // is sent by APPLY.
                    match self.network.set_cvar(rgb_cvar, &value.to_string()).unwrap_or(Ok(false)) {
                        Ok(true) => self.profile_userinfo_pending = true,
                        Ok(false) => {}
                        Err(error) => self.console_status = error,
                    }
                    self.mark_config_dirty();
                    self.profile_preview_key = None;
                }
            }
            ui.add_space(6.0);
        }
    }

    fn egui_resume_page(&mut self, ui: &mut egui::Ui) {
        if self.map_editor.is_some() {
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "EDIT SOURCE MAP").clicked() {
                    self.set_overlay(OverlayMode::MapEdit);
                }
                theme::label(ui, theme::plain("Loose .map brush editing is available for this world.", 10.5, theme::TEXT_FAINT));
            });
            ui.add_space(12.0);
        }
        let demo_rate = self
            .game_session
            .as_ref()
            .and_then(GameSession::playback_rate);
        let mode = match self.local_server.as_ref().map(|server| server.mode()) {
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
        } else if self.live_connected() {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
            if let Some(hostname) = self.current_server_hostname() {
                ui.horizontal(|ui| {
                    theme::glow_label(ui, "On", 12.0, theme::TEXT_FAINT);
                    ui.add(egui::Label::new(jka_colored_text(&hostname, 13.0, theme::TEXT)));
                });
                ui.add_space(8.0);
            }
        } else {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
        }

        let can_join = self.live_connected()
            || self
                .local_server
                .as_ref()
                .is_some_and(|server| server.can_join());

        theme::section(ui, "SESSION", "");
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
        self.egui_spectator_actions(ui);

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

    pub(super) fn join_as(&mut self, mode: JoinMode) {
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

        if mode == JoinMode::Player && !self.solo_initial_spawn_pending {
            // ClientSpawn -> SelectSpawnPoint(ps.origin): random among the
            // furthest half of the spots, so a rejoin does not reuse this one.
            if let Some(index) = crate::scene::select_spawn_index(
                &self.spawns,
                self.camera.position.to_array(),
                false,
                super::random_unit(),
            ) {
                self.spawn_index = index;
            }
        }
        let Some(spawn) = self.spawns.get(self.spawn_index).copied() else {
            return;
        };
        let result = match self.local_server.as_mut() {
            Some(server) => server.join(mode, spawn),
            None => return,
        };
        match result {
            Ok(()) => {
                if mode == JoinMode::Player {
                    self.solo_initial_spawn_pending = false;
                }
                self.push_local_snapshot(true);
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
            "Base JKA bindings and mouse input. Click a binding to rebind it; right-click clears it.",
        );

        if self.controls_waiting_for_key {
            theme::banner(
                ui,
                "Press any key, mouse button or wheel direction…  ESC cancels.",
                theme::WARNING,
            );
            ui.add_space(6.0);
        }

        // One tab per action group, then Mouse, styled like the Setup tab strip.
        let mut groups: Vec<&str> = Vec::new();
        for action in keybinds::CONTROL_ACTIONS {
            if !groups.contains(&action.group) {
                groups.push(action.group);
            }
        }
        let mouse_tab = groups.len();
        self.controls_section = self.controls_section.min(mouse_tab);
        let mut picked_tab = None;
        egui::Frame::new().fill(theme::SURFACE_ALT).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (index, label) in groups.iter().copied().chain(["Mouse"]).enumerate() {
                    let label = label.to_uppercase();
                    let width = 24.0 + label.len() as f32 * 9.0;
                    if theme::nav_item(ui, &label, 12.5, width, theme::TAB_BAR_H, index == self.controls_section)
                        .clicked()
                    {
                        picked_tab = Some(index);
                    }
                }
            });
        });
        if let Some(index) = picked_tab {
            if index != self.controls_section {
                self.controls_section = index;
                self.controls_waiting_for_key = false;
                self.publish_ui();
            }
        }
        ui.add_space(4.0);

        let mut rebind = None;
        let mut clear = None;
        let mut restore_defaults = false;
        let section = self.controls_section;
        egui::ScrollArea::vertical()
            .id_salt(("jka_controls_scroll", section))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if section == mouse_tab {
                    self.egui_mouse_controls(ui);
                    return;
                }
                let group = groups[section];
                theme::section(ui, &group.to_uppercase(), "");
                for (index, action) in keybinds::CONTROL_ACTIONS.iter().enumerate() {
                    if action.group != group {
                        continue;
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

                theme::section(ui, "DEFAULTS", "");
                theme::row(
                    ui,
                    "Default bindings",
                    "Replaces every binding, including ones you added from the console, with the stock jaPRO/JKA multiplayer layout.",
                    theme::Reset::None,
                    |ui| {
                        let armed_id = ui.id().with("restore_default_binds");
                        let armed = ui.ctx().data(|data| data.get_temp::<bool>(armed_id)).unwrap_or(false);
                        let label = if armed { "CLICK AGAIN TO CONFIRM" } else { "RESTORE DEFAULTS" };
                        let response = theme::ghost_button(ui, label);
                        if response.clicked() {
                            ui.ctx().data_mut(|data| data.insert_temp(armed_id, !armed));
                            restore_defaults = armed;
                        } else if armed && !response.hovered() {
                            // Moving away cancels, so a later single click can't confirm.
                            ui.ctx().data_mut(|data| data.insert_temp(armed_id, false));
                        }
                    },
                );
            });

        if restore_defaults {
            self.bindings = keybinds::Bindings::default();
            self.controls_waiting_for_key = false;
            self.refresh_bound_state();
            self.mark_config_dirty();
            self.publish_ui();
        } else if let Some(index) = rebind {
            self.controls_selected = index;
            self.controls_waiting_for_key = true;
            self.publish_ui();
        } else if let Some(index) = clear {
            self.controls_selected = index;
            self.unbind_selected_control();
        }
    }

    fn egui_mouse_controls(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "MOUSE",
            "Mouse look, raw-input scaling and client-side input latency controls.",
        );
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
        theme::row(
            ui,
            "Late-latched view",
            "cl_input_latelatch. Experimental A/B switch. Requires subframe input; the render thread resamples the newest real view orientation at the latest point that is still coherent with camera-dependent work. No mouse prediction or extra physics ticks.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.input_latelatch) {
                    self.set_input_latelatch(enabled);
                    self.egui_repaint_requested = true;
                }
            },
        );

        theme::section(
            ui,
            "MOUSE - ADVANCED LATENCY (A/B)",
            "Windows scheduling experiments. These do not alter JKA movement or networking.",
        );
        theme::row(
            ui,
            "1 ms Windows timer",
            "cl_timerResolution1ms. Windows only: requests timeBeginPeriod(1) while enabled, then pairs it with timeEndPeriod(1) when disabled/shutting down. This can improve timeout/sleep wake precision (for example capped frame pacing), but raw mouse input already wakes the event loop immediately, so it is not expected to reduce raw mouse-event wake latency. May increase power use.",
            theme::Reset::None,
            |ui| {
                if !cfg!(windows) {
                    ui.add_enabled(false, egui::Label::new("Windows only"));
                    return;
                }
                if let Some(enabled) = theme::switch(ui, self.video.timer_resolution_1ms) {
                    if let Err(error) = self.set_timer_resolution_1ms(enabled) {
                        self.console_status = format!("1 MS WINDOWS TIMER FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                        self.publish_ui();
                    }
                    self.egui_repaint_requested = true;
                }
            },
        );
    }

    fn audio_choice_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        cvar: &str,
        current: u8,
        options: &[(u8, &str)],
    ) {
        let mut picked = None;
        theme::row(ui, label, tip, theme::Reset::None, |ui| {
            for &(value, text) in options {
                if theme::chip(ui, text, value == current).clicked() && value != current {
                    picked = Some(value);
                }
                ui.add_space(3.0);
            }
        });
        if let Some(value) = picked {
            let _ = self.set_console_cvar(cvar, &value.to_string());
        }
    }

    fn egui_audio_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "AUDIO", "OpenJK mixer levels, Steam Audio spatial acoustics and native output diagnostics.");

        theme::section(ui, "MIX", "Changes are live and archived to DinurdoJK.cfg.");
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
            "s_musicvolume. Level music (the map's music track) and the duel track.",
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
            "GAMEPLAY SOUNDS",
            "jaPRO / TaystJK voice and feedback options. They apply on every server and are archived to DinurdoJK.cfg.",
        );
        const WHO: [(u8, &str); 4] = [(0, "Off"), (1, "Everyone"), (2, "Others"), (3, "Only me")];
        self.audio_choice_row(
            ui,
            "Jump voice",
            "cg_jumpSounds. Voice line on jumps: off, everyone, other players only, or only your own. jaPRO itself defaults to off; DinurdoJK keeps the stock JKA behaviour (everyone).",
            "cg_jumpSounds",
            self.audio.game.jump,
            &WHO,
        );
        self.audio_choice_row(
            ui,
            "Roll voice",
            "cg_rollSounds. The model's roll voice (falls back to its jump voice) when rolling: off, everyone, other players only, or only you.",
            "cg_rollSounds",
            self.audio.game.roll,
            &WHO,
        );
        theme::row(
            ui,
            "Silence taunts",
            "cg_noTaunt. Mutes taunt voice lines.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.no_taunt) {
                    let _ = self.set_console_cvar("cg_noTaunt", if enabled { "1" } else { "0" });
                }
            },
        );
        self.audio_choice_row(
            ui,
            "Footsteps",
            "cg_footsteps. The surface's footstep sound for each step of a player's walk and run animation. Footprints are a visual setting (Environment > Surface).",
            "cg_footsteps",
            u8::from(self.audio.game.footsteps > 0) * 3,
            &[(0, "Off"), (3, "On")],
        );
        self.audio_choice_row(
            ui,
            "Chat beep",
            "cg_chatSounds. Off by default. Legacy: the classic talk beep on every chat line. Distinct: separate beeps for private messages and team chat (jaPRO).",
            "cg_chatSounds",
            self.audio.game.chat_sounds,
            &[(0, "Off"), (1, "Legacy"), (2, "Distinct")],
        );
        theme::row(
            ui,
            "Race start sound",
            "cg_raceSounds (bit 1). jaPRO race mode: the sound the start trigger plays when your run begins.",
            theme::Reset::None,
            |ui| {
                let on = self.audio.game.race_sounds & 1 != 0;
                if let Some(enabled) = theme::switch(ui, on) {
                    let mask = if enabled { self.audio.game.race_sounds | 1 } else { self.audio.game.race_sounds & !1 };
                    let _ = self.set_console_cvar("cg_raceSounds", &mask.to_string());
                }
            },
        );
        theme::row(
            ui,
            "Level ambience",
            "cg_ambientSounds. The map's ambient sound sets: the worldspawn soundSet playing around you and local ambient emitters.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.ambient) {
                    let _ = self.set_console_cvar("cg_ambientSounds", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Duel music",
            "cg_duelMusic. Plays the duel track while you are in a duel, then returns to the level music.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.duel_music) {
                    let _ = self.set_console_cvar("cg_duelMusic", if enabled { "1" } else { "0" });
                }
            },
        );
        self.audio_choice_row(
            ui,
            "Duel start",
            "cg_duelSounds. The countdown sound and BEGIN DUEL text when your duel starts.",
            "cg_duelSounds",
            self.audio.game.duel,
            &[(0, "Off"), (1, "Sound + text"), (2, "Sound only"), (3, "Text only")],
        );
        self.audio_choice_row(
            ui,
            "Kill sound",
            "cg_killSounds. Frag sound when you kill someone; 'Mid-air' also plays a special sound for rocket, conc, bowcaster, alt repeater and saber kills on airborne targets. Needs the jaPRO sound/frag assets.",
            "cg_killSounds",
            self.audio.game.kill,
            &[(0, "Off"), (1, "Frag"), (2, "Frag + mid-air")],
        );
        self.audio_choice_row(
            ui,
            "Kill message",
            "cg_killMessage. TaystJK center-screen kill confirmation. Normal includes your FFA place/score; Kill only suppresses that footer; High moves the message upward.",
            "cg_killMessage",
            self.audio.game.kill_message,
            &[(0, "Off"), (1, "Normal + score"), (2, "Kill only"), (3, "High")],
        );
        self.audio_choice_row(
            ui,
            "Awards",
            "cg_drawRewards. TaystJK/JKA reward medals and announcer. Quake 3 swaps the supported Excellent, Impressive, Humiliation and Denied presentation to the Q3 variants.",
            "cg_drawRewards",
            self.audio.game.draw_rewards,
            &[(0, "Off"), (1, "JKA"), (2, "Quake 3")],
        );
        self.audio_choice_row(
            ui,
            "Hit sound",
            "cg_hitsounds. Feedback when you damage an enemy (a team sound plays for teammates). Sets 1-4 need the jaPRO sound/effects/hitsound assets; 5 uses only the plain saber-hit sound, 6 any saber-hit variant.",
            "cg_hitsounds",
            self.audio.game.hit,
            &[(0, "Off"), (1, "Set 1"), (2, "Set 2"), (3, "Set 3"), (4, "Set 4"), (5, "Saber plain"), (6, "Saber any")],
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
        theme::row(
            ui,
            "Binaural HRTF",
            "s_steamAudioBinaural. Uses Steam Audio HRTF rendering for positional sounds. This is a live setting and does not require a map reload.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio_binaural) {
                    let _ = self.set_console_cvar("s_steamAudioBinaural", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Environmental Acoustics",
            "s_steamAudioEnvironmental. Live Steam Audio direct-path acoustics for positional world sounds: wall occlusion plus frequency-dependent material transmission. Simulation runs on a dedicated worker at a bounded rate; the audio callback only applies the latest result. No map reload or rebake is required once the map's Steam Audio scene is ready.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio_environmental) {
                    let _ = self.set_console_cvar(
                        "s_steamAudioEnvironmental",
                        if enabled { "1" } else { "0" },
                    );
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
                "Direct environmental DSP",
                "Live Steam Audio raycast occlusion and material transmission. Scene queries run on a dedicated worker at up to 30 Hz; the fixed-block audio DSP applies the latest coefficients.",
                theme::Reset::None,
                |ui| {
                    let (state, color) = if !info.steam_audio_enabled {
                        ("Disabled by Steam Audio master gate".to_owned(), theme::TEXT_FAINT)
                    } else if !info.steam_audio_environmental_enabled {
                        ("Off · direct sound unchanged".to_owned(), theme::TEXT_DIM)
                    } else if let Some(error) = &info.steam_audio_environmental_error {
                        (format!("Unavailable · {error}"), theme::WARNING)
                    } else if info.steam_audio_environmental_active {
                        (format!("Active · {} sources · {} updates · last {:.2} ms", info.steam_audio_environment_voice_count, info.steam_audio_environment_updates, info.steam_audio_environment_last_ms), theme::TEXT)
                    } else if info.steam_audio_scene_triangles > 0 {
                        ("Initializing direct simulator…".to_owned(), theme::TEXT_DIM)
                    } else {
                        ("Waiting for map acoustic scene".to_owned(), theme::TEXT_DIM)
                    };
                    theme::hint(ui, &state, color);
                },
            );
            theme::row(
                ui,
                "Baked reflections / pathing DSP",
                "Probe-baked reflections, reverb, and pathing are reported separately from live direct occlusion and transmission.",
                theme::Reset::None,
                |ui| {
                    theme::hint(ui, if info.steam_audio_bake_ready { "Pending · baked probe data ready" } else { "Pending · waiting for acoustic bake" }, theme::TEXT_DIM);
                },
            );
            theme::row(
                ui,
                "Binaural HRTF",
                "Live Steam Audio headphone positioning for positional sound sources.",
                theme::Reset::None,
                |ui| {
                    let state = if let Some(error) = &info.steam_audio_hrtf_error {
                        format!("Unavailable · {error}")
                    } else if info.steam_audio_binaural_active {
                        format!("Active · {} positional voices", info.steam_audio_hrtf_voice_count)
                    } else if info.steam_audio_binaural_enabled {
                        "Requested · waiting for Steam Audio master gate".to_owned()
                    } else {
                        "Disabled".to_owned()
                    };
                    theme::hint(ui, &state, if info.steam_audio_binaural_active { theme::TEXT } else { theme::TEXT_DIM });
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
pub(super) fn title_case(text: &str) -> String {
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

fn refresh_icon(ui: &mut egui::Ui, tooltip: &str) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let painter = ui.painter().clone();
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        if response.hovered() {
            theme::CONTROL_HOVER
        } else {
            theme::CONTROL
        },
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(
            1.0_f32,
            if response.hovered() { theme::ACCENT } else { theme::LINE_STRONG },
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "↻",
        egui::FontId::proportional(16.0),
        if response.hovered() { theme::TEXT } else { theme::TEXT_DIM },
    );
    response.on_hover_text(tooltip).clicked()
}

/// Compact affordance for the existing `fs_refresh` path. Asset Viewer,
/// source .map browsing, and Solo Game all use the same VFS invalidation and
/// retry behavior rather than maintaining separate partial refresh routines.
fn filesystem_refresh_icon(ui: &mut egui::Ui) -> bool {
    refresh_icon(ui, "Refresh filesystem (fs_refresh)")
}

fn jka_ui_color(code: char, fallback: egui::Color32) -> egui::Color32 {
    match code {
        '0' => egui::Color32::from_rgb(0x24, 0x28, 0x2E),
        '1' => egui::Color32::from_rgb(0xFF, 0x5C, 0x5C),
        '2' => egui::Color32::from_rgb(0x70, 0xD8, 0x78),
        '3' => egui::Color32::from_rgb(0xF2, 0xD5, 0x62),
        '4' => egui::Color32::from_rgb(0x6E, 0x9C, 0xFF),
        '5' => egui::Color32::from_rgb(0x63, 0xD7, 0xE8),
        '6' => egui::Color32::from_rgb(0xD9, 0x78, 0xE8),
        '7' => theme::TEXT,
        '8' => egui::Color32::from_rgb(0xFF, 0x9A, 0x45),
        '9' => egui::Color32::from_rgb(0xB8, 0xBE, 0xC8),
        _ => fallback,
    }
}

/// Render JKA `^0`..`^9` color escapes directly in egui. Server/player names
/// keep their authored colors while searching and sorting continue to use the
/// color-stripped text.
pub(super) fn jka_colored_text(text: &str, size: f32, fallback: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut color = fallback;
    let mut run = String::new();
    let flush = |job: &mut egui::text::LayoutJob, run: &mut String, color: egui::Color32| {
        if run.is_empty() {
            return;
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
        run.clear();
    };

    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    flush(&mut job, &mut run, color);
                    chars.next();
                    color = jka_ui_color(next, fallback);
                    continue;
                }
            }
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, color);
    job
}

/// Build an editor galley with the same character count as the stored name.
/// Color escapes become zero-width placeholders, preserving cursor movement
/// and backspace semantics while the surrounding runs keep their JKA colors.
fn profile_name_layout(
    raw: &str,
    size: f32,
    fallback: egui::Color32,
) -> egui::text::LayoutJob {
    const HIDDEN_CHAR: char = '\u{2060}';

    let mut job = egui::text::LayoutJob::default();
    let mut color = fallback;
    let mut run = String::new();
    let flush = |job: &mut egui::text::LayoutJob,
                 run: &mut String,
                 color: egui::Color32| {
        if run.is_empty() {
            return;
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
        run.clear();
    };

    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    flush(&mut job, &mut run, color);
                    chars.next();
                    let hidden = format!("{HIDDEN_CHAR}{HIDDEN_CHAR}");
                    job.append(&hidden, 0.0, egui::TextFormat {
                        font_id: egui::FontId::proportional(size),
                        color: egui::Color32::TRANSPARENT,
                        ..Default::default()
                    });
                    color = jka_ui_color(next, fallback);
                    continue;
                }
            }
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, color);
    job
}

fn asset_size_label(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.2} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as usize)
    }
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

pub(super) fn binding_slot(
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
