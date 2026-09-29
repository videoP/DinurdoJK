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
use super::*;

const TOP_ITEMS: [&str; 7] = [
    "GAME", "SERVERS", "PROFILE", "CONTROLS", "SETUP", "VOTE", "MOD",
];
const SETUP_TABS: [&str; 5] = ["GAME", "VIDEO", "AUDIO", "NETWORK", "INTERFACE"];

const TOP_RESUME: usize = 0;
const TOP_PROFILE: usize = 2;
const TOP_CONTROLS: usize = 3;
const TOP_SETUP: usize = 4;

const SETUP_TAB_VIDEO: usize = 1;
const SETUP_TAB_AUDIO: usize = 2;
const SETUP_TAB_NETWORK: usize = 3;
const SETUP_TAB_INTERFACE: usize = 4;

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

const PROFILE_SECTIONS: [&str; 4] = ["IDENTITY", "MODEL", "FORCE", "SABER"];
const PROFILE_IDENTITY: usize = 0;
const PROFILE_MODEL: usize = 1;
const PROFILE_FORCE: usize = 2;
const PROFILE_SABER: usize = 3;

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
    Film,
    DebugTools,
    BakedAo,
    Physics,
    Sun,
    Clouds,
    Weather,
    Surface,
    Water,
}

impl VideoSection {
    const RENDERING: [(Self, &'static str); 13] = [
        (Self::Display, "Display"),
        (Self::ImageQuality, "Image quality"),
        (Self::Visibility, "Visibility"),
        (Self::Models, "Models"),
        (Self::Lighting, "Lighting"),
        (Self::Effects, "Effects"),
        (Self::Shadows, "Shadows"),
        (Self::Reflections, "Reflections"),
        (Self::PostProcessing, "Post processing"),
        (Self::Film, "Film emulation"),
        (Self::DebugTools, "Debug & tools"),
        (Self::BakedAo, "Baked AO"),
        (Self::Physics, "Physics"),
    ];
    const ENVIRONMENT: [(Self, &'static str); 5] = [
        (Self::Sun, "Sun"),
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
            || matches!(self.overlay, OverlayMode::Game | OverlayMode::Video | OverlayMode::Vgs | OverlayMode::HudEdit | OverlayMode::MapEdit))
            && self.video_confirmation.is_none()
            && (blocking_download_ui || self.loading.is_none())
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
        if self.overlay == OverlayMode::MapEdit {
            self.egui_map_editor(ui);
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
                egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, outline),
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
                    self.push_console_line(format!("^2{}", self.console_status));
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
            "Browse MD3, GLM, EFX and .shader files in a grid with search, folder filtering and A-Z navigation.",
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
        match frontend::scan_asset_viewer_assets(&self.base, self.game.as_deref()) {
            Ok(entries) => self.asset_viewer_entries = entries,
            Err(error) => {
                eprintln!("Could not build Asset Viewer catalog: {error}");
                self.asset_viewer_catalog_error = Some(error);
            }
        }
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

        egui::Frame::new()
            .fill(theme::RAIL_FILL)
            .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::glow_label(ui, "ASSET VIEWER", 16.0, theme::TEXT);
                    ui.add_space(14.0);
                    theme::label(ui, theme::plain("Type", 11.0, theme::TEXT_FAINT));
                    egui::ComboBox::from_id_salt("asset_viewer_type")
                        .selected_text(self.asset_viewer_filter.label())
                        .width(118.0)
                        .show_ui(ui, |ui| {
                            for filter in [
                                AssetFilter::All,
                                AssetFilter::Models,
                                AssetFilter::Md3,
                                AssetFilter::Glm,
                                AssetFilter::Efx,
                                AssetFilter::Shader,
                            ] {
                                ui.selectable_value(&mut self.asset_viewer_filter, filter, filter.label());
                            }
                        });
                    ui.add_space(8.0);
                    theme::label(ui, theme::plain("Search", 11.0, theme::TEXT_FAINT));
                    ui.add_sized(
                        [210.0, 24.0],
                        egui::TextEdit::singleline(&mut self.asset_viewer_search).hint_text("name or path"),
                    );
                    ui.add_space(8.0);
                    theme::label(ui, theme::plain("Folder", 11.0, theme::TEXT_FAINT));
                    ui.add_sized(
                        [180.0, 24.0],
                        egui::TextEdit::singleline(&mut self.asset_viewer_folder).hint_text("e.g. models/players"),
                    );
                    if self.asset_viewer_filter == AssetFilter::Shader {
                        ui.add_space(8.0);
                        theme::label(ui, theme::plain("Shader file", 11.0, theme::TEXT_FAINT));
                        let selected_file = if self.asset_viewer_shader_file.is_empty() {
                            "All files".to_owned()
                        } else {
                            self.asset_viewer_shader_file
                                .rsplit('/')
                                .next()
                                .unwrap_or(self.asset_viewer_shader_file.as_str())
                                .to_owned()
                        };
                        egui::ComboBox::from_id_salt("asset_viewer_shader_file")
                            .selected_text(selected_file)
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.asset_viewer_shader_file,
                                    String::new(),
                                    "All files",
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
                    if theme::ghost_button(ui, "CLEAR").clicked() {
                        self.asset_viewer_filter = AssetFilter::All;
                        self.asset_viewer_search.clear();
                        self.asset_viewer_folder.clear();
                        self.asset_viewer_shader_file.clear();
                        self.asset_viewer_letter = None;
                        self.asset_viewer_jump_letter = Some('A');
                    }
                });
            });

        ui.add_space(8.0);
        let search = self.asset_viewer_search.trim().to_ascii_lowercase();
        let folder = self.asset_viewer_folder.trim().replace('\\', "/").to_ascii_lowercase();
        let shader_file = self.asset_viewer_shader_file.as_str();
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
            .filter(|(_, entry)| folder.is_empty() || entry.folder.to_ascii_lowercase().contains(&folder))
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
        self.ensure_asset_viewer_detail();

        let body_height = ui.available_height().max(260.0);
        let total_width = ui.available_width().max(1.0);
        let gap = 8.0_f32;
        // Give the selected asset more room than V3 while keeping enough grid
        // width for several compact name cards.
        let right_width = if total_width >= 900.0 {
            (total_width * 0.40).clamp(440.0, 760.0)
        } else {
            (total_width * 0.42).max(280.0)
        };
        let left_width = (total_width - right_width - gap).max(280.0);
        let alphabet_width = 22.0_f32;
        let jump_letter = self.asset_viewer_jump_letter.take();
        let jump_target = jump_letter.and_then(|letter| {
            filtered
                .iter()
                .copied()
                .find(|&index| asset_entry_letter(&self.asset_viewer_entries[index]) == Some(letter))
        });
        let mut center_letter = None;
        let mut center_letter_distance = f32::MAX;
        let mut selection_changed = false;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            ui.allocate_ui_with_layout(
                egui::vec2(left_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::RAIL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::symmetric(8, 8))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (left_width - 16.0).max(1.0),
                                (body_height - 16.0).max(1.0),
                            ));
                            theme::section(
                                ui,
                                "ASSETS",
                                &format!("{} shown / {} total", filtered.len(), self.asset_viewer_entries.len()),
                            );
                            ui.add_space(4.0);

                            let browser_height = ui.available_height().max(1.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;

                                // A-Z lives inside the grid's left edge. It is a
                                // thin navigator, not its own panel.
                                ui.allocate_ui_with_layout(
                                    egui::vec2(alphabet_width, browser_height),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        let letter_height = (browser_height / 26.0).max(1.0);
                                        for byte in b'A'..=b'Z' {
                                            let ch = byte as char;
                                            let active = self.asset_viewer_letter == Some(ch);
                                            let (rect, response) = ui.allocate_exact_size(
                                                egui::vec2(alphabet_width, letter_height),
                                                egui::Sense::click(),
                                            );
                                            ui.painter().text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                ch.to_string(),
                                                egui::FontId::monospace(if active { 11.0 } else { 9.5 }),
                                                if active {
                                                    theme::ACCENT
                                                } else if response.hovered() {
                                                    theme::TEXT_DIM
                                                } else {
                                                    theme::TEXT_FAINT
                                                },
                                            );
                                            if active {
                                                ui.painter().rect_filled(
                                                    egui::Rect::from_min_size(
                                                        egui::pos2(rect.left(), rect.center().y - 7.0),
                                                        egui::vec2(2.0, 14.0),
                                                    ),
                                                    egui::CornerRadius::ZERO,
                                                    theme::ACCENT,
                                                );
                                            }
                                            if response.clicked() {
                                                self.asset_viewer_letter = Some(ch);
                                                self.asset_viewer_jump_letter = Some(ch);
                                                self.egui_repaint_requested = true;
                                            }
                                        }
                                    },
                                );

                                let grid_width = ui.available_width().max(1.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(grid_width, browser_height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt("asset_viewer_grid")
                                            .auto_shrink([false, false])
                                            .max_height(browser_height)
                                            .show(ui, |ui| {
                                                ui.set_min_width(grid_width);
                                                let available = ui.available_width().max(1.0);
                                                let card_gap = 6.0_f32;
                                                let desired_card = 150.0_f32;
                                                let columns = (((available + card_gap)
                                                    / (desired_card + card_gap))
                                                    .floor() as usize)
                                                    .max(1);
                                                let card_width = ((available
                                                    - card_gap * (columns.saturating_sub(1) as f32))
                                                    / columns as f32)
                                                    .max(105.0);
                                                let card_height = 38.0_f32;
                                                let clip = ui.clip_rect();
                                                let clip_center = clip.center();

                                                for row in filtered.chunks(columns) {
                                                    ui.horizontal(|ui| {
                                                        ui.spacing_mut().item_spacing.x = card_gap;
                                                        for &index in row {
                                                            let entry = self.asset_viewer_entries[index].clone();
                                                            let selected = self.asset_viewer_selected.as_deref()
                                                                == Some(entry.id.as_str());
                                                            let (rect, response) = ui.allocate_exact_size(
                                                                egui::vec2(card_width, card_height),
                                                                egui::Sense::click(),
                                                            );
                                                            let fill = if selected {
                                                                theme::CONTROL_SELECTED
                                                            } else if response.hovered() {
                                                                theme::CONTROL
                                                            } else {
                                                                theme::PANEL_FILL
                                                            };
                                                            let card_painter =
                                                                ui.painter().with_clip_rect(rect.shrink(1.0));
                                                            card_painter.rect_filled(
                                                                rect,
                                                                egui::CornerRadius::same(2),
                                                                fill,
                                                            );
                                                            card_painter.rect_stroke(
                                                                rect,
                                                                egui::CornerRadius::same(2),
                                                                egui::Stroke::new(
                                                                    1.0_f32,
                                                                    if selected { theme::ACCENT } else { theme::LINE },
                                                                ),
                                                                egui::StrokeKind::Inside,
                                                            );
                                                            card_painter.text(
                                                                rect.left_center() + egui::vec2(8.0, -2.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                &entry.display_name,
                                                                egui::FontId::monospace(10.5),
                                                                theme::TEXT,
                                                            );
                                                            card_painter.text(
                                                                rect.right_bottom() + egui::vec2(-6.0, -4.0),
                                                                egui::Align2::RIGHT_BOTTOM,
                                                                entry.kind.label(),
                                                                egui::FontId::monospace(7.5),
                                                                if selected { theme::ACCENT } else { theme::TEXT_FAINT },
                                                            );

                                                            let hover = entry.shader_name.as_ref().map_or_else(
                                                                || entry.qpath.clone(),
                                                                |shader| format!("{shader}\n{}", entry.qpath),
                                                            );
                                                            let response = response.on_hover_text(hover);

                                                            // Track the asset nearest the physical center
                                                            // of the visible grid, not the first visible row.
                                                            if rect.intersects(clip) {
                                                                if let Some(letter) = asset_entry_letter(&entry) {
                                                                    let delta = rect.center() - clip_center;
                                                                    let distance = delta.x * delta.x + delta.y * delta.y;
                                                                    if distance < center_letter_distance {
                                                                        center_letter_distance = distance;
                                                                        center_letter = Some(letter);
                                                                    }
                                                                }
                                                            }
                                                            if jump_target == Some(index) {
                                                                ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                                            }
                                                            if response.clicked() && !selected {
                                                                self.asset_viewer_selected = Some(entry.id.clone());
                                                                self.asset_viewer_detail_path = None;
                                                                self.asset_viewer_shader_name = None;
                                                                self.clear_asset_preview_runtime();
                                                                selection_changed = true;
                                                                self.egui_repaint_requested = true;
                                                            }
                                                        }
                                                    });
                                                    ui.add_space(card_gap);
                                                }
                                            });
                                    },
                                );
                            });
                        });
                },
            );

            if jump_target.is_none() {
                if let Some(letter) = center_letter {
                    self.asset_viewer_letter = Some(letter);
                }
            } else if let Some(letter) = jump_letter {
                self.asset_viewer_letter = Some(letter);
            }

            if selection_changed {
                self.ensure_asset_viewer_detail();
            }

            ui.add_space(gap);
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

            ui.vertical(|ui| {
                ui.set_width(right_width);
                ui.set_height(body_height);
                let preview_height = (body_height * 0.62).clamp(260.0, 650.0);
                let (preview_rect, preview_response) = ui.allocate_exact_size(
                    egui::vec2(right_width, preview_height),
                    egui::Sense::click_and_drag(),
                );
                // Deliberately no filled egui panel here: the WGPU preview is
                // rendered only inside this physical viewport, so it is never
                // darkened by translucent menu chrome.
                ui.painter().rect_stroke(
                    preview_rect,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(1.0_f32, theme::LINE),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    preview_rect.left_top() + egui::vec2(10.0, 9.0),
                    egui::Align2::LEFT_TOP,
                    match detail_snapshot.as_ref().map(|detail| detail.kind) {
                        Some(AssetKind::Md3) | Some(AssetKind::Glm) => "MODEL PREVIEW",
                        Some(AssetKind::Efx) => "EFX PREVIEW · LOOPING",
                        Some(AssetKind::Shader) => "SHADER PREVIEW",
                        None => "PREVIEW",
                    },
                    egui::FontId::proportional(10.5),
                    theme::TEXT_FAINT,
                );
                self.set_asset_preview_viewport(preview_rect.shrink(1.0), ui.ctx().pixels_per_point());

                let mut preview_changed = false;
                if detail_snapshot.as_ref().is_some_and(|detail| detail.kind.is_model()) && preview_response.dragged() {
                    let delta = ui.input(|input| input.pointer.delta());
                    if delta.x.abs() > f32::EPSILON {
                        self.asset_viewer_model_yaw =
                            (self.asset_viewer_model_yaw + delta.x * 0.45).rem_euclid(360.0);
                        preview_changed = true;
                    }
                }
                if preview_response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    if scroll.abs() > f32::EPSILON {
                        self.asset_viewer_model_zoom =
                            (self.asset_viewer_model_zoom * (-scroll * 0.0015).exp()).clamp(0.35, 5.0);
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

                ui.add_space(8.0);
                let metadata_height = (body_height - preview_height - 8.0).max(100.0);
                egui::Frame::new()
                    .fill(theme::RAIL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::symmetric(10, 9))
                    .show(ui, |ui| {
                        ui.set_width((right_width - 20.0).max(1.0));
                        ui.set_height((metadata_height - 18.0).max(1.0));
                        egui::ScrollArea::vertical()
                            .id_salt("asset_viewer_metadata")
                            .auto_shrink([false, false])
                            .max_height((metadata_height - 18.0).max(1.0))
                            .show(ui, |ui| {
                                match detail_snapshot.as_ref() {
                                    Some(detail) => {
                                        let detail_title = if detail.kind == AssetKind::Shader {
                                            self.asset_viewer_shader_name
                                                .as_deref()
                                                .unwrap_or(detail.qpath.as_str())
                                        } else {
                                            detail.qpath.as_str()
                                        };
                                        theme::glow_label(ui, detail_title, 13.0, theme::TEXT);
                                        if detail.kind == AssetKind::Shader {
                                            theme::label(
                                                ui,
                                                theme::plain(&format!("File: {}", detail.qpath), 9.5, theme::TEXT_FAINT),
                                            );
                                        }
                                        ui.add_space(4.0);
                                        theme::label(
                                            ui,
                                            theme::plain(
                                                &format!("{}  •  {}", detail.kind.label(), asset_size_label(detail.size_bytes)),
                                                10.5,
                                                theme::TEXT_DIM,
                                            ),
                                        );
                                        theme::label(
                                            ui,
                                            theme::plain(&format!("Source: {}", detail.source), 9.5, theme::TEXT_FAINT),
                                        );
                                        ui.add_space(8.0);
                                        for (name, value) in &detail.stats {
                                            ui.horizontal(|ui| {
                                                theme::label(ui, theme::plain(name, 10.5, theme::TEXT_FAINT));
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    theme::label(ui, theme::plain(value, 10.5, theme::TEXT));
                                                });
                                            });
                                        }

                                        if detail.kind.is_model() {
                                            ui.add_space(7.0);
                                            ui.horizontal(|ui| {
                                                theme::label(ui, theme::plain("Yaw", 10.5, theme::TEXT_FAINT));
                                                if ui
                                                    .add(egui::Slider::new(&mut self.asset_viewer_model_yaw, 0.0..=360.0).show_value(false))
                                                    .changed()
                                                {
                                                    self.asset_preview_model_key = None;
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                theme::label(ui, theme::plain("Zoom", 10.5, theme::TEXT_FAINT));
                                                if ui
                                                    .add(
                                                        egui::Slider::new(&mut self.asset_viewer_model_zoom, 0.35..=5.0)
                                                            .logarithmic(true)
                                                            .show_value(false),
                                                    )
                                                    .changed()
                                                {
                                                    self.asset_preview_model_key = None;
                                                }
                                            });
                                            self.update_asset_preview_content();
                                        } else {
                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                theme::label(ui, theme::plain("Zoom", 10.5, theme::TEXT_FAINT));
                                                if ui
                                                    .add(
                                                        egui::Slider::new(&mut self.asset_viewer_model_zoom, 0.35..=5.0)
                                                            .logarithmic(true)
                                                            .show_value(false),
                                                    )
                                                    .changed()
                                                {
                                                    self.asset_preview_shader_key = None;
                                                    self.asset_preview_fx = None;
                                                    self.update_asset_preview_content();
                                                }
                                            });
                                        }

                                        ui.add_space(8.0);
                                        theme::section(
                                            ui,
                                            match detail.kind {
                                                AssetKind::Shader => "STAGES",
                                                kind if kind.is_model() => "SURFACES",
                                                _ => "PRIMITIVES",
                                            },
                                            &format!("{}", detail.items.len()),
                                        );
                                        for item in &detail.items {
                                            ui.label(
                                                egui::RichText::new(item)
                                                    .monospace()
                                                    .size(9.5)
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
                                    }
                                    None if detail_error.is_some() => {
                                        theme::banner(
                                            ui,
                                            detail_error.as_deref().unwrap_or("Asset inspection failed"),
                                            theme::WARNING,
                                        );
                                    }
                                    None => {
                                        theme::label(ui, theme::plain("No asset selected.", 11.5, theme::TEXT_FAINT));
                                    }
                                }
                            });
                    });
            });
        });

        // EFX needs to advance every frontend frame even when no UI control is
        // moving; model/shader paths cheaply return when their cache key matches.
        self.update_asset_preview_content();
    }

    fn ensure_source_map_catalog(&mut self) {
        if self.source_map_catalog_loaded {
            return;
        }
        self.source_map_catalog_loaded = true;
        self.source_maps.clear();
        self.source_map_catalog_error = None;
        match frontend::scan_source_maps(&self.base, self.game.as_deref()) {
            Ok(maps) => self.source_maps = maps,
            Err(error) => {
                eprintln!("Could not build Map Viewer catalog: {error}");
                self.source_map_catalog_error = Some(error);
            }
        }
        self.source_map_selected = self.source_map_selected.min(self.source_maps.len().saturating_sub(1));
        self.source_map_levelshot_texture = None;
        self.source_map_levelshot_texture_map = None;
    }

    fn ensure_source_map_levelshot_texture(&mut self, ctx: &egui::Context) {
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
        let Some(bytes) = self.source_maps.get(self.source_map_selected).and_then(|entry| entry.levelshot.as_ref()) else {
            return;
        };
        match image::load_from_memory_with_format(&bytes.bytes, bytes.format) {
            Ok(image) => {
                let rgba = image.into_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                self.source_map_levelshot_texture = Some(ctx.load_texture(
                    format!("source-levelshot:{map_name}"),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
            Err(error) => eprintln!("Source map levelshot {map_name} decode failed: {error}"),
        }
    }

    fn egui_map_viewer_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "MAP VIEWER", "Source .map files discovered from the active VFS; launch uses the existing direct .map path.");
        self.ensure_source_map_catalog();
        if let Some(error) = &self.source_map_catalog_error {
            theme::banner(ui, &format!("Source map scan failed: {error}"), theme::WARNING);
            return;
        }
        if self.source_maps.is_empty() {
            theme::banner(ui, "No maps/*.map assets were found.", theme::WARNING);
            return;
        }
        self.source_map_selected = self.source_map_selected.min(self.source_maps.len() - 1);
        self.ensure_source_map_levelshot_texture(ui.ctx());
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
                        ui.label(&entry.text);
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

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SELECTED DEMO", "");
                    theme::glow_label(ui, &selected_name, 18.0, theme::TEXT);
                    ui.add_space(10.0);

                    let inspector_height = (ui.available_height() - 56.0).max(180.0);
                    egui::Frame::new()
                        .fill(theme::CONTROL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
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
                let full_width_page = self.overlay == OverlayMode::Game
                    && matches!(self.menu_selected, 1 | TOP_PROFILE);
                let content_w = if full_width_page {
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
                0 => self.egui_game_settings(ui),
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
            5 => placeholder(
                ui,
                "VOTE",
                "Server and gametype vote actions.",
                &["Call a map or gametype vote", "Kick and mute votes"],
            ),
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
        ctx: &egui::Context,
        entry: &ProfileModelEntry,
    ) -> Option<egui::TextureHandle> {
        let key = entry.value.to_ascii_lowercase();
        if let Some(texture) = self.profile_model_icon_textures.get(&key) {
            return Some(texture.clone());
        }
        let asset = entry.icon.as_ref()?;
        let image = match image::load_from_memory_with_format(&asset.bytes, asset.format) {
            Ok(image) => image.into_rgba8(),
            Err(error) => {
                eprintln!("Profile icon {} decode failed: {error}", entry.value);
                return None;
            }
        };
        let size = [image.width() as usize, image.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        let texture = ctx.load_texture(
            format!("profile-model-icon:{}", entry.value),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.profile_model_icon_textures.insert(key, texture.clone());
        Some(texture)
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

        ui.add_space(4.0);
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
                                    _ => {}
                                });
                        });
                },
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let saber = self.profile_selected_section == PROFILE_SABER;
                    theme::section(
                        ui,
                        if saber { "SABER PREVIEW" } else { "PLAYER PREVIEW" },
                        "Drag to rotate · wheel to zoom",
                    );
                    let preview_h = ui.available_height().max(220.0);
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), preview_h),
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
                    let caption = if saber {
                        &self.network.saber1
                    } else {
                        &self.solo_client_info.model_name
                    };
                    ui.painter().text(
                        rect.left_bottom() + egui::vec2(10.0, -10.0),
                        egui::Align2::LEFT_BOTTOM,
                        caption,
                        egui::FontId::proportional(11.5),
                        theme::TEXT_FAINT,
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
        let edit = egui::TextEdit::singleline(&mut self.profile_name_input)
            .desired_width(ui.available_width())
            .hint_text("Player name");
        let response = ui.add(edit);
        let apply_enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        ui.add_space(8.0);
        if (theme::primary_button(ui, "APPLY NAME").clicked() || apply_enter)
            && !self.profile_name_input.trim().is_empty()
        {
            let value = self.profile_name_input.trim().to_owned();
            if let Err(error) = self.set_console_cvar("name", &value) {
                self.console_status = error;
            }
        }

        theme::section(ui, "CURRENT LOADOUT", "Uses the same cvars as the stock client");
        Self::profile_readout(ui, "MODEL", &self.solo_client_info.model_cvar());
        Self::profile_readout(ui, "PRIMARY SABER", &self.network.saber1);
        Self::profile_readout(ui, "SECONDARY SABER", &self.network.saber2);
        Self::profile_readout(ui, "FORCE", &self.network.forcepowers);
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
        let ctx = ui.ctx().clone();
        let tile = egui::vec2(92.0, 116.0);
        let columns = ((ui.available_width() + 8.0) / (tile.x + 8.0)).floor().max(1.0) as usize;
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
                    if let Some(texture) = self.profile_model_icon_texture(&ctx, &representative) {
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
                                if let Some(texture) = self.profile_model_icon_texture(&ctx, entry) {
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
                if let Err(error) = self.set_console_cvar("model", &value) {
                    self.console_status = error;
                } else {
                    self.profile_preview_key = None;
                }
            }
        }

        if self.profile_models.is_empty() {
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
            if let Err(error) = self.set_console_cvar("forcepowers", &after) {
                self.console_status = error;
            } else if self.live_connected() {
                self.forward_command_to_server("forcechanged");
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
            if let Err(error) = self.set_console_cvar("saber1", &primary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }
        if !desired_secondary.eq_ignore_ascii_case(&current_secondary) {
            if let Err(error) = self.set_console_cvar("saber2", desired_secondary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }

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
        } else {
            theme::page_title(ui, "GAME", &format!("Currently {}.", mode.to_lowercase()));
        }

        let can_join = self.live_connected()
            || self
                .local_server
                .as_ref()
                .is_some_and(|server| server.can_join());

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

        let Some(spawn) = self.spawns.get(self.spawn_index).copied() else {
            return;
        };
        let result = match self.local_server.as_mut() {
            Some(server) => server.join(mode, spawn),
            None => return,
        };
        match result {
            Ok(()) => {
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

                self.egui_mouse_controls(ui);
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

    fn egui_network_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "NETWORK", "Connection, packet pacing and map/package autodownload policy.");

        theme::section(
            ui,
            "AUTOMATIC DOWNLOADS",
            "When a server's map is missing, DinurdoJK asks before downloading. HTTP takes priority when the server advertises it; legacy JKA download is the fallback.",
        );
        theme::row(
            ui,
            "HTTP downloads",
            "cl_allowHttpDownload. Allow PK3 downloads from TaystJK-compatible mvhttp/mvhttpurl endpoints. Preferred over legacy UDP when available.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.network.allow_http_downloads) {
                    let _ = self.set_console_cvar("cl_allowHttpDownload", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Legacy server downloads",
            "cl_allowDownload. Allow the stock Jedi Academy svc_download / nextdl transport. Used when HTTP is unavailable or an HTTP transfer fails.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.network.allow_legacy_downloads) {
                    let _ = self.set_console_cvar("cl_allowDownload", if enabled { "1" } else { "0" });
                }
            },
        );

        theme::section(ui, "PACKET SETTINGS", "Existing JKA-compatible client networking controls.");
        theme::row(ui, "Rate", "rate. Maximum bytes per second requested from the server.", theme::Reset::None, |ui| {
            ui.label(format!("{} B/s", self.network.rate));
        });
        theme::row(ui, "Snapshots", "snaps. Snapshot frequency requested from the server.", theme::Reset::None, |ui| {
            ui.label(format!("{} Hz", self.network.snaps));
        });
        theme::row(ui, "Max packets", "cl_maxpackets. Maximum client packets per second.", theme::Reset::None, |ui| {
            ui.label(format!("{} Hz", self.network.max_packets));
        });
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

fn asset_entry_letter(entry: &AssetEntry) -> Option<char> {
    entry
        .display_name
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .and_then(|c| {
            let c = c.to_ascii_uppercase();
            c.is_ascii_alphabetic().then_some(c)
        })
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
