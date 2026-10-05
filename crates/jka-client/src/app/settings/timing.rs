//! Settings timing.
use crate::app::{config, App, RenderCommand, RendererBackend, VsyncMode};

impl App {
    /// Refresh rate of the monitor the window is on, in Hz.
    pub(in crate::app) fn monitor_refresh_hz(&self) -> Option<f64> {
        let hz = self
            .window
            .as_ref()?
            .current_monitor()?
            .refresh_rate_millihertz()? as f64
            / 1000.0;
        (hz >= 1.0).then_some(hz)
    }

    /// Highest FPS the present path can actually reach, when something other than
    /// the GPU is the limit.
    ///
    /// Only DX12 with FAST vsync has one: wgpu presents Mailbox without the DXGI
    /// tearing flag, so presents retire at vblank and at most
    /// `r_maxFrameLatency` frames can retire per refresh. VSync OFF
    /// (Immediate) sets the tearing flag and is uncapped, ON/Adaptive are
    /// refresh-bound by definition on every backend, and Vulkan's Mailbox lets
    /// the app render ahead freely.
    pub(in crate::app) fn present_fps_ceiling(&self) -> Option<u32> {
        if self.applied_renderer_backend != RendererBackend::Dx12
            || self.video.vsync != VsyncMode::Fast
        {
            return None;
        }
        let refresh = self.monitor_refresh_hz()?;
        Some((refresh * f64::from(self.video.max_frame_latency)) as u32)
    }

    /// Highest cap that means anything here: the DX12 FAST present ceiling when
    /// there is one, otherwise the cvar's own maximum.
    pub(in crate::app) fn fps_cap_limit(&self) -> u32 {
        self.present_fps_ceiling().unwrap_or(config::FPS_CAP_MAX)
    }

    /// Resolve a requested cap to the number that will actually be honoured.
    ///
    /// "Unlimited" is not shown as 0 anywhere, because 0 tells the player nothing
    /// about the frame rate they will get. It resolves to whatever the real limit
    /// is, so the displayed cap is always the true one on every backend and vsync
    /// mode rather than only on the DX12 path that has a hard ceiling.
    pub(in crate::app) fn resolved_fps_cap(&self, requested: u32) -> u32 {
        let limit = self.fps_cap_limit();
        if requested == 0 || requested > limit {
            limit
        } else {
            requested
        }
    }

    pub(in crate::app) fn effective_fps_cap(&self) -> u32 {
        self.resolved_fps_cap(self.video.fps_cap)
    }

    pub(in crate::app) fn frontend_refresh_fps_cap(&self) -> Option<u32> {
        self.monitor_refresh_hz()
            .map(|hz| hz.round().clamp(1.0, u32::MAX as f64) as u32)
    }

    pub(in crate::app) fn active_render_fps_cap(&self) -> u32 {
        if self.front_end {
            // The menu follows the monitor, but never runs faster than com_maxfps.
            let cap = self.effective_fps_cap();
            self.frontend_refresh_fps_cap()
                .map_or(cap, |refresh| refresh.min(cap))
        } else {
            self.effective_fps_cap()
        }
    }

    pub(in crate::app) fn sync_render_fps_cap(&self) {
        self.render_command(RenderCommand::SetFpsCap(self.active_render_fps_cap()));
    }

    pub(in crate::app) fn set_fps_cap(&mut self, fps_cap: u32) {
        let requested = fps_cap;
        let fps_cap = self.resolved_fps_cap(requested);
        // Only the DX12 present ceiling is worth explaining. Rounding 0 or an
        // over-range number down to the cvar maximum is not a surprise.
        if let Some(ceiling) = self.present_fps_ceiling() {
            if requested > ceiling {
                self.console_status = format!(
                    "DX12 + FAST VSYNC TOPS OUT AT {ceiling} FPS ({} FRAMES x {:.0} HZ); \
                     CAP SNAPPED BACK. USE VSYNC OFF FOR UNCAPPED.",
                    self.video.max_frame_latency,
                    self.monitor_refresh_hz().unwrap_or_default()
                );
                self.push_console_line(format!("^3{}", self.console_status));
            }
        }
        self.video.fps_cap = fps_cap;
        if !self.fps_cap_editing {
            self.fps_cap_input = fps_cap.to_string();
        }
        self.sync_render_fps_cap();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_max_frame_latency(&mut self, latency: u32) {
        let latency = latency.clamp(1, 3);
        if self.video.max_frame_latency == latency {
            return;
        }
        self.video.max_frame_latency = latency;
        self.render_command(RenderCommand::SetMaxFrameLatency(latency));
        self.mark_config_dirty();

        // DX12 + FAST has a real present-rate ceiling proportional to the
        // configured frame queue. Update the active cap without rewriting the
        // user's requested com_maxfps so A/B switching remains reversible.
        self.sync_render_fps_cap();
        self.console_status = format!(
            "MAX FRAME LATENCY: {latency} ({})",
            match latency {
                1 => "LOWEST LATENCY",
                2 => "BALANCED",
                _ => "MAXIMUM THROUGHPUT",
            }
        );
        self.publish_ui();
    }

    pub(in crate::app) fn begin_fps_cap_edit(&mut self) {
        if !self.fps_cap_editing {
            self.fps_cap_editing = true;
            self.fps_cap_input = self.video.fps_cap.to_string();
            self.fps_cap_replace_on_type = true;
        }
        self.publish_ui();
    }

    /// Push the FX sampling rate/scope into the active presentation session.
    pub(in crate::app) fn apply_fx_fps_settings(&mut self) {
        if let Some(session) = self.game_session.as_mut() {
            session.weapon_fx.set_continuous_fx_fps(self.video.fx_fps);
            session.weapon_fx.set_fx_fps_scope(self.video.fx_fps_scope);
        }
    }

    /// Push `fx_physics` into the live session's FX worker.
    pub(in crate::app) fn apply_fx_physics(&mut self) {
        if let Some(session) = self.game_session.as_mut() {
            session.weapon_fx.set_fx_physics(self.video.fx_physics);
            session.apply_client_options(
                self.screen_shake,
                self.audio.game,
                self.video.footprints,
                self.japro_cg,
                self.network.plugin_disable,
            );
        }
    }

    /// Push `fx_lod` / `fx_countScale` into the live session's FX worker.
    pub(in crate::app) fn apply_fx_lod(&mut self) {
        if let Some(session) = self.game_session.as_mut() {
            session.weapon_fx.set_fx_lod(
                self.video.fx_lod,
                self.video.fx_count_scale,
                self.video.fx_lod_scale,
            );
        }
    }

    pub(in crate::app) fn set_physics_msec(&mut self, physics_msec: u32) {
        self.video.physics_msec = physics_msec;
        if !self.physics_fps_editing {
            self.physics_fps_input = config::physics_fps_from_msec(physics_msec).to_string();
        }
        if let Some(player) = &mut self.local_server {
            if let Err(error) = player.set_physics_tick_msec(physics_msec) {
                self.console_status = format!("PHYSICS FPS ERROR: {error}");
            }
        }
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn begin_physics_fps_edit(&mut self) {
        if !self.physics_fps_editing {
            self.physics_fps_editing = true;
            self.physics_fps_input =
                config::physics_fps_from_msec(self.video.physics_msec).to_string();
            self.physics_fps_replace_on_type = true;
        }
        self.publish_ui();
    }

    pub(in crate::app) fn set_timer_resolution_1ms(&mut self, enabled: bool) -> Result<(), String> {
        if self.video.timer_resolution_1ms == enabled
            && self.windows_timer_resolution.is_active() == enabled
        {
            return Ok(());
        }

        self.windows_timer_resolution.set_enabled(enabled)?;
        self.video.timer_resolution_1ms = enabled;
        self.mark_config_dirty();
        self.console_status = if enabled {
            "WINDOWS TIMER RESOLUTION: 1 MS (A/B TEST ENABLED)".into()
        } else {
            "WINDOWS TIMER RESOLUTION: DEFAULT".into()
        };
        self.publish_ui();
        Ok(())
    }

    pub(in crate::app) fn set_input_latelatch(&mut self, enabled: bool) {
        if enabled == self.video.input_latelatch {
            return;
        }
        self.video.input_latelatch = enabled;
        self.render_command(RenderCommand::SetInputLateLatch(enabled));
        self.mark_config_dirty();
        // Seed the tiny render-thread view mailbox immediately when enabling.
        // It is otherwise refreshed naturally by subsequent snapshots/events.
        if enabled {
            self.publish_snapshot();
        }
        self.publish_ui();
    }
}
