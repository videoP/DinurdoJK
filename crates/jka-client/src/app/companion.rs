use super::*;
use super::egui_theme as theme;
use crate::renderer::{CompanionId, CompanionSceneView};

pub(super) const PRIMARY_COMPANION_ID: CompanionId = CompanionId(1);

/// Window-independent identity for a tool surface. Phase 1 lays these out as a
/// passive dashboard; later docking/dragging moves this ID rather than migrating
/// widget state or renderer ownership between native windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ToolPanelId {
    Console,
    Scoreboard,
    ServerInfo,
}

impl ToolPanelId {
    const PASSIVE_DASHBOARD: [Self; 3] = [Self::ServerInfo, Self::Scoreboard, Self::Console];
}

pub(super) struct CompanionWindowState {
    pub(super) id: CompanionId,
    pub(super) window: Arc<Window>,
    pub(super) egui_ctx: egui::Context,
    pub(super) egui_state: egui_winit::State,
    pub(super) repaint_requested: bool,
    pub(super) last_frame: Instant,
}

impl CompanionWindowState {
    fn new(window: Arc<Window>) -> Self {
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        Self {
            id: PRIMARY_COMPANION_ID,
            window,
            egui_ctx,
            egui_state,
            repaint_requested: true,
            last_frame: Instant::now() - Duration::from_secs(1),
        }
    }
}

impl App {
    pub(super) fn companion_enabled(&self) -> bool {
        self.companion_target_enabled
    }

    pub(super) fn set_companion_enabled(&mut self, enabled: bool) {
        if self.companion_target_enabled == enabled {
            return;
        }
        self.companion_target_enabled = enabled;
        if !enabled {
            self.companion_scene_target = None;
        }
        self.egui_repaint_requested = true;
        if enabled {
            self.console_status = "COMPANION WINDOW: OPENING".into();
        } else {
            self.console_status = "COMPANION WINDOW: CLOSED".into();
        }
    }

    /// Current-snapshot spectator candidates exposed in the *primary* UI. The
    /// list deliberately comes from `spectate_info()` instead of all clientinfo
    /// slots, so selecting a second POV cannot request a player that is absent
    /// from the authoritative snapshot/PVS we already received.
    pub(super) fn companion_spectator_targets(&self) -> Vec<(i32, String)> {
        let Some(session) = self.game_session.as_ref() else {
            return Vec::new();
        };
        let Some(info) = session.spectate_info() else {
            return Vec::new();
        };
        let mut out = info
            .others
            .into_iter()
            .map(|(client, _)| {
                let name = usize::try_from(client)
                    .ok()
                    .and_then(|index| session.client_game.client_name(index))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("CLIENT {client}"));
                (client, name)
            })
            .collect::<Vec<_>>();
        out.sort_by_key(|(client, _)| *client);
        out
    }

    pub(super) fn companion_scene_target_name(&self, client: i32) -> String {
        self.game_session
            .as_ref()
            .and_then(|session| usize::try_from(client).ok().and_then(|index| session.client_game.client_name(index)))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("CLIENT {client}"))
    }

    pub(super) fn set_companion_scene_target(&mut self, target: Option<i32>) {
        if self.companion_scene_target == target {
            return;
        }
        self.companion_scene_target = target;
        if target.is_some() {
            // Choosing a secondary POV is itself an explicit request to use the
            // companion, so do not make the user enable it separately first.
            self.companion_target_enabled = true;
            self.console_status = "COMPANION WINDOW: SECONDARY SPECTATOR POV".into();
        } else if self.companion_target_enabled {
            self.console_status = "COMPANION WINDOW: PASSIVE DASHBOARD".into();
        }
        self.mark_companion_dirty();
        self.egui_repaint_requested = true;
        self.publish_snapshot();
    }

    fn companion_scene_target_available(&self, client: i32) -> bool {
        self.game_session
            .as_ref()
            .and_then(GameSession::spectate_info)
            .is_some_and(|info| info.others.iter().any(|(candidate, _)| *candidate == client))
    }

    /// Build the render-thread request for the selected second spectator. This
    /// is intentionally an approximation of a remote first-person camera: JKA
    /// snapshots do not contain another client's exact viewheight/playerState,
    /// so we use the same standing-eye + entity-angle convention as demo fake
    /// view. Rendering remains restricted to the primary spectator's PVS.
    pub(super) fn companion_scene_view(&self) -> Option<CompanionSceneView> {
        let client = self.companion_scene_target?;
        if !self.companion_scene_target_available(client) {
            return None;
        }
        let session = self.game_session.as_ref()?;
        let entity = session.presented_entities.iter().find(|entity| {
            entity.entity_type == ET_PLAYER && i32::from(entity.number) == client
        })?;
        let mut eye = entity.origin;
        eye[2] += 26.0;
        let mut camera = Camera::new_with_fov(
            scene::render_position(eye),
            entity.angles[1].to_radians(),
            self.japro_effective_fov(Instant::now()),
        );
        camera.pitch = -entity.angles[0].to_radians();
        camera.shake = Default::default();
        Some(CompanionSceneView {
            id: PRIMARY_COMPANION_ID,
            target_entity: entity.number,
            camera,
        })
    }

    pub(super) fn mark_companion_dirty(&mut self) {
        if let Some(companion) = self.companion.as_mut() {
            companion.repaint_requested = true;
        }
    }

    fn companion_panel_visible(&self, panel: ToolPanelId) -> bool {
        self.companion.is_some()
            && self.companion_scene_target.is_none()
            && ToolPanelId::PASSIVE_DASHBOARD.contains(&panel)
    }

    pub(super) fn mark_companion_console_dirty(&mut self) {
        if !self.companion_panel_visible(ToolPanelId::Console) {
            return;
        }
        if let Some(companion) = self.companion.as_mut() {
            companion.repaint_requested = true;
        }
    }

    pub(super) fn companion_scoreboard_visible(&self) -> bool {
        self.companion_panel_visible(ToolPanelId::Scoreboard)
    }

    /// Create/destroy the native companion HWND on the Winit thread. Its WGPU
    /// Surface is also created here (through `RenderThread::create_companion`)
    /// before the already-created surface is handed to the sleeping render worker.
    pub(super) fn sync_companion_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.companion_target_enabled && self.companion.is_none() {
            let mut attributes = Window::default_attributes()
                .with_title("DinurdoJK Companion")
                .with_inner_size(PhysicalSize::new(1100, 760));

            // Prefer a monitor other than the one that owns the game window. This
            // is only a placement hint; users can move the companion anywhere.
            let primary_pos = self
                .window
                .as_ref()
                .and_then(|window| window.current_monitor())
                .map(|monitor| monitor.position());
            if let Some(monitor) = event_loop
                .available_monitors()
                .find(|monitor| Some(monitor.position()) != primary_pos)
            {
                let origin = monitor.position();
                let size = monitor.size();
                let width = 1100u32.min(size.width.saturating_sub(96).max(480));
                let height = 760u32.min(size.height.saturating_sub(96).max(360));
                attributes = attributes
                    .with_inner_size(PhysicalSize::new(width, height))
                    .with_position(PhysicalPosition::new(origin.x + 48, origin.y + 48));
            }

            match event_loop.create_window(attributes) {
                Ok(window) => {
                    let window = Arc::new(window);
                    let companion = CompanionWindowState::new(window.clone());
                    let attach_result = self
                        .render
                        .as_ref()
                        .map(|render| render.create_companion(companion.id, window.clone()));
                    self.companion = Some(companion);
                    match attach_result {
                        Some(Ok(())) => {
                            self.console_status =
                                "COMPANION WINDOW: READYING GPU SURFACE".into();
                        }
                        Some(Err(error)) => {
                            self.companion_target_enabled = false;
                            self.companion_scene_target = None;
                            self.companion = None;
                            self.console_status = format!("COMPANION WINDOW FAILED: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                        None => {
                            // A renderer restart may briefly leave App without a render
                            // thread. Keep the native/UI state; the new renderer will
                            // attach it from `attach_companion_to_renderer`.
                            self.console_status =
                                "COMPANION WINDOW: WAITING FOR RENDERER".into();
                        }
                    }
                }
                Err(error) => {
                    self.companion_target_enabled = false;
                    self.companion_scene_target = None;
                    self.console_status = format!("COMPANION WINDOW FAILED: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                }
            }
        } else if !self.companion_target_enabled {
            self.companion_scene_target = None;
            if let Some(companion) = self.companion.take() {
                self.render_command(RenderCommand::DestroyCompanion { id: companion.id });
            }
        }
    }

    /// Reattach surviving native companion windows after `vid_restart` creates a
    /// fresh WGPU Instance/Device. Layout/UI state lives in App and survives.
    pub(super) fn attach_companion_to_renderer(&mut self, render: &RenderThread) {
        let Some((id, window)) = self
            .companion
            .as_ref()
            .map(|companion| (companion.id, companion.window.clone()))
        else {
            return;
        };
        if let Err(error) = render.create_companion(id, window) {
            self.companion_target_enabled = false;
            self.companion_scene_target = None;
            self.console_status = format!("COMPANION WINDOW FAILED: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
        }
    }

    pub(super) fn handle_companion_window_event(
        &mut self,
        window_id: WindowId,
        event: WindowEvent,
    ) -> bool {
        let Some(mut companion) = self.companion.take() else {
            return false;
        };
        if companion.window.id() != window_id {
            self.companion = Some(companion);
            return false;
        }

        match &event {
            WindowEvent::CloseRequested => {
                self.companion_target_enabled = false;
                self.companion_scene_target = None;
                self.render_command(RenderCommand::DestroyCompanion { id: companion.id });
                self.console_status = "COMPANION WINDOW: CLOSED".into();
                return true;
            }
            WindowEvent::Resized(size) => {
                self.render_command(RenderCommand::ResizeCompanion {
                    id: companion.id,
                    size: *size,
                });
                companion.repaint_requested = true;
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(_) => {
                companion.repaint_requested = true;
            }
            _ => {}
        }

        let response = companion
            .egui_state
            .on_window_event(companion.window.as_ref(), &event);
        companion.repaint_requested |= response.repaint;
        self.companion = Some(companion);
        true
    }

    pub(super) fn tick_companion_ui(&mut self) {
        if let Some(client) = self.companion_scene_target {
            let still_spectating = self
                .game_session
                .as_ref()
                .and_then(GameSession::spectate_info)
                .is_some();
            if !still_spectating {
                // Leaving spectator mode returns monitor 2 to its passive
                // dashboard instead of leaving a dead POV selection behind.
                self.companion_scene_target = None;
                self.publish_snapshot();
                self.mark_companion_dirty();
            } else if !self.companion_scene_target_available(client) {
                // Keep the selection while the target is temporarily absent from
                // our authoritative snapshot/PVS, but stop 3D rendering and let
                // the companion explain why the view is unavailable.
                self.publish_snapshot();
                self.mark_companion_dirty();
            }
        }

        let Some(companion) = self.companion.as_ref() else {
            return;
        };

        // Dashboard clocks need only 1 Hz. A scene view gets a sparse 4 Hz egui
        // overlay refresh for target-name/availability changes; its actual 3D
        // frames are driven independently by the render snapshot at up to ~60 Hz.
        let elapsed = companion.last_frame.elapsed();
        let periodic_refresh = if self.companion_scene_target.is_some() {
            elapsed >= Duration::from_millis(250)
        } else {
            elapsed >= Duration::from_secs(1)
        };
        if !companion.repaint_requested && !periodic_refresh {
            return;
        }
        // Coalesce log/input bursts on the app thread as well as on the render
        // worker. The companion is a utility surface, so rebuilding it faster
        // than ~60 Hz would only steal CPU from the latency-sensitive game path.
        if companion.repaint_requested && elapsed < Duration::from_millis(16) {
            return;
        }
        self.run_companion_egui_frame();
    }

    fn run_companion_egui_frame(&mut self) {
        let Some(mut companion) = self.companion.take() else {
            return;
        };

        let size = companion.window.inner_size();
        if size.width == 0 || size.height == 0 {
            companion.repaint_requested = false;
            self.companion = Some(companion);
            return;
        }

        let ctx = companion.egui_ctx.clone();
        let raw_input = companion.egui_state.take_egui_input(companion.window.as_ref());
        // Consume the dirty bit for this build. Interactions that happen while
        // constructing the frame may set it again to request one follow-up frame.
        companion.repaint_requested = false;
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = ctx.run_ui(raw_input, |ui| self.build_companion_ui(ui, &mut companion));
        companion
            .egui_state
            .handle_platform_output(companion.window.as_ref(), platform_output);
        let paint_jobs = ctx.tessellate(shapes, pixels_per_point);
        self.render_command(RenderCommand::SetCompanionEgui {
            id: companion.id,
            frame: EguiRenderData {
                paint_jobs,
                textures_delta,
                pixels_per_point,
            },
        });
        companion.last_frame = Instant::now();
        self.companion = Some(companion);
    }

    fn build_companion_ui(&mut self, root: &mut egui::Ui, _companion: &mut CompanionWindowState) {
        let available = root.available_size();
        root.set_min_size(available);

        if let Some(client) = self.companion_scene_target {
            if self.companion_scene_target_available(client) {
                // Transparent overlay: the companion worker clears/renders the 3D
                // scene first and egui loads it, so only this small status badge is
                // composited over the secondary POV.
                let name = crate::logging::strip_jka_colors(&self.companion_scene_target_name(client));
                egui::Area::new(egui::Id::new("companion_scene_badge"))
                    .fixed_pos(egui::pos2(12.0, 12.0))
                    .show(root.ctx(), |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_black_alpha(165))
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .corner_radius(egui::CornerRadius::same(5))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!("SECONDARY POV · {name}"))
                                        .strong()
                                        .color(theme::TEXT),
                                );
                            });
                    });
                return;
            }

            egui::Frame::new()
                .fill(theme::PANEL_FILL)
                .inner_margin(egui::Margin::same(12))
                .show(root, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("SECONDARY VIEW UNAVAILABLE");
                            ui.label(
                                egui::RichText::new(
                                    "That player is not present in the current spectator snapshot/PVS.",
                                )
                                .color(theme::TEXT_DIM),
                            );
                        });
                    });
                });
            return;
        }

        let frame = egui::Frame::new()
            .fill(theme::PANEL_FILL)
            .inner_margin(egui::Margin::same(12));
        frame.show(root, |ui| {
            ui.label(
                egui::RichText::new("DINURDOJK COMPANION")
                    .strong()
                    .size(15.0)
                    .color(theme::TEXT),
            );
            ui.add_space(4.0);

            // Passive dashboard: no tabs, focus changes, or companion interaction
            // are required to see the useful state. Keep the compact server/status
            // strip at the top, scoreboard above, and give the console the remaining
            // space so all three surfaces stay visible while the game owns focus.
            egui::Frame::new()
                .fill(theme::PANEL_FILL)
                .inner_margin(egui::Margin::symmetric(4, 4))
                .show(ui, |ui| self.draw_companion_server_info(ui));

            ui.separator();
            ui.label(
                egui::RichText::new("SCOREBOARD")
                    .strong()
                    .size(13.0)
                    .color(theme::TEXT_DIM),
            );
            let scoreboard_height = (ui.available_height() * 0.36).clamp(150.0, 300.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), scoreboard_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.draw_companion_scoreboard(ui),
            );

            ui.separator();
            ui.label(
                egui::RichText::new("CONSOLE")
                    .strong()
                    .size(13.0)
                    .color(theme::TEXT_DIM),
            );
            self.draw_companion_console(ui);
        });
    }

    fn draw_companion_console(&mut self, ui: &mut egui::Ui) {
        let total = self.console_lines.len();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show_rows(ui, 15.0, total, |ui, range| {
                ui.style_mut().spacing.item_spacing.y = 1.0;
                for index in range {
                    let Some(line) = self.console_lines.get(index) else {
                        continue;
                    };
                    ui.label(
                        egui::RichText::new(crate::logging::strip_jka_colors(line))
                            .monospace()
                            .size(12.0)
                            .color(theme::TEXT),
                    );
                }
            });
    }

    fn draw_companion_scoreboard(&mut self, ui: &mut egui::Ui) {
        let Some(board) = self.scoreboard.as_ref() else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("Waiting for scoreboard data…").color(theme::TEXT_DIM));
            });
            return;
        };

        if board.team_game {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("RED {}", board.team_scores[0])).strong());
                ui.separator();
                ui.label(egui::RichText::new(format!("BLUE {}", board.team_scores[1])).strong());
            });
            ui.add_space(8.0);
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("companion_scoreboard_grid")
                    .striped(true)
                    .min_col_width(70.0)
                    .show(ui, |ui| {
                        ui.strong("PLAYER");
                        ui.strong("SCORE");
                        ui.strong("PING");
                        ui.strong("TIME");
                        ui.strong("TEAM");
                        ui.end_row();
                        for entry in &board.entries {
                            ui.label(
                                egui::RichText::new(crate::logging::strip_jka_colors(&entry.name))
                                    .color(theme::TEXT),
                            );
                            ui.label(entry.score.to_string());
                            let ping = if entry.ping < 0 {
                                "-".to_owned()
                            } else {
                                entry.ping.to_string()
                            };
                            ui.label(ping);
                            ui.label(entry.time.to_string());
                            ui.label(match entry.team {
                                1 => "Red",
                                2 => "Blue",
                                3 => "Spectator",
                                _ => "Free",
                            });
                            ui.end_row();
                        }
                    });
            });
    }

    fn draw_companion_server_info(&mut self, ui: &mut egui::Ui) {
        let (server, state, server_time) = if let Some(net) = self.net.as_ref() {
            (
                net.server_name.as_str(),
                format!("{:?}", net.state()),
                Some(net.session().server_time()),
            )
        } else if self.demo_playback_active() {
            ("Demo playback", "Demo".to_owned(), None)
        } else if self.local_server.is_some() {
            ("Local / Solo", "Local".to_owned(), None)
        } else {
            ("Not connected", "Disconnected".to_owned(), None)
        };

        // Keep this deliberately compact: it is persistent context, not a panel
        // that should consume a large fraction of the companion display.
        ui.horizontal_wrapped(|ui| {
            let field = |ui: &mut egui::Ui, label: &str, value: String| {
                ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
                ui.label(egui::RichText::new(value).color(theme::TEXT));
                ui.add_space(12.0);
            };
            field(ui, "SERVER", server.to_owned());
            field(ui, "STATE", state);
            field(ui, "MAP", self.initial_source.label().to_owned());
            field(
                ui,
                "TIME",
                server_time.map_or_else(|| "-".into(), |time| time.to_string()),
            );
            field(ui, "FPS", format!("{:.0}", self.perf.fps));
        });
        ui.label(
            egui::RichText::new(&self.console_status)
                .small()
                .color(theme::TEXT_DIM),
        );
    }

}
