//! Event-driven transitions between shader, texture and display brightness.
use crate::{
    app::{App, RenderCommand, UserEvent},
    display_gamma::{Controller, Status, Target},
    gamma::GammaMethod,
};
use std::sync::Arc;

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Preparing {
        renderer: bool,
        desktop: bool,
    },
    Applying,
}
#[derive(Default)]
pub(super) struct GammaRuntime {
    controller: Controller,
    generation: u64,
    phase: Phase,
    hardware_active: bool,
    pub(super) message: Option<String>,
    monitor: Option<String>,
}
impl App {
    fn gamma_notify(&self) -> Arc<dyn Fn(Status) + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move |status| {
            let _ = proxy.send_event(UserEvent::HardwareGamma(status));
        })
    }
    pub(super) fn sync_gamma_method(&mut self) {
        if self.render.is_none() || self.quit_requested {
            return;
        }
        self.gamma_runtime.generation = self.gamma_runtime.generation.wrapping_add(1);
        let generation = self.gamma_runtime.generation;
        if self.video.gamma_method != GammaMethod::Hardware
            && !self.gamma_runtime.hardware_active
            && matches!(self.gamma_runtime.phase, Phase::Idle)
        {
            self.gamma_runtime.message = None;
            self.render_command(RenderCommand::SetGammaMethod(self.video.gamma_method));
            return;
        }
        self.gamma_runtime.phase = Phase::Preparing {
            renderer: false,
            desktop: false,
        };
        self.gamma_runtime.message = Some("Applying brightness method...".into());
        // Neutral output first; baked originals and desktop calibration must both
        // be restored before the next method can start. Generation rejects late events.
        self.render_command(RenderCommand::PrepareGammaMethod(generation));
        let notify = self.gamma_notify();
        if let Err(error) = self
            .gamma_runtime
            .controller
            .request(generation, None, notify)
        {
            self.on_hardware_gamma(Status {
                generation,
                active: false,
                restored: false,
                result: Err(error),
            });
        }
    }
    pub(super) fn gamma_slider_changed(&mut self) {
        if self.video.gamma_method != GammaMethod::Hardware {
            return;
        }
        if self.gamma_runtime.hardware_active
            && self.window_focused
            && matches!(self.gamma_runtime.phase, Phase::Idle | Phase::Applying)
        {
            self.gamma_runtime.generation = self.gamma_runtime.generation.wrapping_add(1);
            self.apply_hardware_gamma();
        } else {
            self.sync_gamma_method();
        }
    }
    pub(super) fn on_gamma_prepared(&mut self, generation: u64) {
        if generation != self.gamma_runtime.generation {
            return;
        }
        if let Phase::Preparing { renderer, .. } = &mut self.gamma_runtime.phase {
            *renderer = true;
        }
        self.finish_gamma_preparation();
    }
    fn finish_gamma_preparation(&mut self) {
        if !matches!(
            self.gamma_runtime.phase,
            Phase::Preparing {
                renderer: true,
                desktop: true
            }
        ) {
            return;
        }
        self.gamma_runtime.hardware_active = false;
        if self.video.gamma_method == GammaMethod::Hardware {
            if self.window_focused && self.pending_renderer_restart.is_none() {
                self.apply_hardware_gamma();
            } else {
                self.gamma_runtime.phase = Phase::Idle;
                self.gamma_runtime.message =
                    Some("Hardware brightness paused while the game is unfocused.".into());
                self.render_command(RenderCommand::SetGammaMethod(GammaMethod::Hardware));
            }
        } else {
            self.gamma_runtime.phase = Phase::Idle;
            self.gamma_runtime.message = None;
            self.render_command(RenderCommand::SetGammaMethod(self.video.gamma_method));
        }
        self.publish_ui();
    }
    fn hardware_target(&self) -> Result<Target, String> {
        let window = self.window.as_ref().ok_or("game window is not ready")?;
        #[cfg(windows)]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(handle) =
                window.window_handle().map_err(|e| e.to_string())?.as_raw()
            else {
                return Err("hardware brightness requires a Windows window".into());
            };
            Ok(Target {
                hwnd: handle.hwnd.get() as usize,
                gamma: self.video.gamma,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            Err("hardware brightness is available on Windows only".into())
        }
    }
    fn apply_hardware_gamma(&mut self) {
        self.gamma_runtime.phase = Phase::Applying;
        let generation = self.gamma_runtime.generation;
        let target = match self.hardware_target() {
            Ok(target) => target,
            Err(error) => {
                self.on_hardware_gamma(Status {
                    generation,
                    active: false,
                    restored: true,
                    result: Err(error),
                });
                return;
            }
        };
        let notify = self.gamma_notify();
        if let Err(error) = self
            .gamma_runtime
            .controller
            .request(generation, Some(target), notify)
        {
            self.on_hardware_gamma(Status {
                generation,
                active: false,
                restored: false,
                result: Err(error),
            });
        }
    }
    pub(super) fn on_hardware_gamma(&mut self, status: Status) {
        if status.generation != self.gamma_runtime.generation {
            return;
        }
        if let Err(error) = status.result {
            self.gamma_runtime.phase = Phase::Idle;
            self.gamma_runtime.hardware_active = !status.restored;
            self.gamma_runtime.message = Some(if status.restored {
                format!("Hardware brightness unavailable. Using Shader: {error}")
            } else {
                format!("Display brightness restoration is pending: {error}")
            });
            self.console_status = self.gamma_runtime.message.clone().unwrap();
            self.push_console_line(format!("^3{}", self.console_status));
            // A shader curve is safe only after desktop calibration is confirmed.
            self.render_command(RenderCommand::SetGammaMethod(if status.restored {
                GammaMethod::Shader
            } else {
                GammaMethod::Hardware
            }));
            self.publish_ui();
            return;
        }
        match &mut self.gamma_runtime.phase {
            Phase::Preparing { desktop, .. } => {
                *desktop = true;
                self.gamma_runtime.hardware_active = false;
                self.finish_gamma_preparation();
            }
            Phase::Applying => {
                self.gamma_runtime.hardware_active = status.active;
                self.gamma_runtime.phase = Phase::Idle;
                self.gamma_runtime.message = None;
                self.render_command(RenderCommand::SetGammaMethod(GammaMethod::Hardware));
                self.gamma_runtime.monitor = self
                    .hardware_target()
                    .ok()
                    .and_then(|target| crate::display_gamma::monitor_key(target.hwnd).ok());
                self.publish_ui();
            }
            Phase::Idle => {}
        }
    }
    pub(super) fn sync_gamma_focus(&mut self) {
        if self.video.gamma_method == GammaMethod::Hardware || self.gamma_runtime.hardware_active {
            self.sync_gamma_method();
        }
    }
    pub(super) fn sync_gamma_monitor(&mut self) {
        if self.video.gamma_method != GammaMethod::Hardware || !self.window_focused {
            return;
        }
        let monitor = self
            .hardware_target()
            .ok()
            .and_then(|target| crate::display_gamma::monitor_key(target.hwnd).ok());
        if monitor != self.gamma_runtime.monitor {
            self.gamma_runtime.monitor = monitor;
            self.sync_gamma_method();
        }
    }
    pub(super) fn restore_gamma_before_restart(&mut self) {
        if !self.gamma_runtime.controller.was_started() {
            return;
        }
        self.gamma_runtime.generation = self.gamma_runtime.generation.wrapping_add(1);
        self.gamma_runtime.phase = Phase::Idle;
        let restore = self.gamma_runtime.controller.restore_before_exit();
        self.gamma_runtime.hardware_active = restore.is_err();
        if let Err(error) = restore {
            eprintln!("Display gamma restart restoration: {error}");
        }
    }
    pub(super) fn restore_gamma_before_exit(&mut self) {
        if let Err(error) = self.gamma_runtime.controller.restore_before_exit() {
            eprintln!("Display gamma exit restoration pending: {error}");
        }
    }
}
