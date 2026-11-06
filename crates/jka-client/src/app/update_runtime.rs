//! The update worker reports through winit UserEvent; no disk or HTTP on the game thread.
use super::{App, UserEvent};
use crate::updater::Event;

impl App {
    pub(in crate::app) fn check_for_updates(&mut self, manual: bool) {
        if self.updater.checking || self.updater.downloading { return; }
        if manual { self.updater.manual_result = None; self.updater.error = None; }
        self.updater.started = true;
        self.updater.checking = true;
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("github-update-check".into()).spawn(move || {
            let result = crate::updater::check();
            let _ = proxy.send_event(UserEvent::Update(Event::Checked(result)));
        });
        if let Err(e) = spawned { self.updater.checking = false; self.updater.error = Some(e.to_string()); }
    }

    pub(in crate::app) fn download_update(&mut self) {
        if self.updater.downloading || self.updater.staged.is_some() { return; }
        let Some(release) = self.updater.release.clone() else { return; };
        self.updater.error = None;
        self.updater.downloading = true;
        self.updater.downloaded = 0;
        self.updater.total = 0; // stage() calculates changed-file bytes after local SHA-256 checks
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("github-update-download".into()).spawn(move || {
            let mut last_report = 0;
            let result = crate::updater::stage(&release, |received, total| {
                // Throttle event-loop messages, even for gigabyte-scale assets.
                if received == 0 || received == total || received.saturating_sub(last_report) >= 1024 * 1024 {
                    let _ = proxy.send_event(UserEvent::Update(Event::Progress { received, total }));
                    last_report = received;
                }
            }, |line| {
                let _ = proxy.send_event(UserEvent::Update(Event::FileDecision(line)));
            });
            let _ = proxy.send_event(UserEvent::Update(Event::Staged(result)));
        });
        if let Err(e) = spawned { self.updater.downloading = false; self.updater.error = Some(e.to_string()); }
    }

    /// Validating the stage and starting the restart helper touches the disk,
    /// so it runs on a worker; the game quits only after the helper is running.
    pub(in crate::app) fn apply_update_now(&mut self) {
        if self.updater.applying { return; }
        let Some(stage) = self.updater.staged.clone() else { return; };
        self.updater.error = None;
        self.updater.applying = true;
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("github-update-apply".into()).spawn(move || {
            let result = crate::updater::restart_with_update(&stage);
            let _ = proxy.send_event(UserEvent::Update(Event::Applied(result)));
        });
        if let Err(e) = spawned { self.updater.applying = false; self.updater.error = Some(e.to_string()); }
    }

    pub(in crate::app) fn handle_update_event(&mut self, event: Event) {
        let mut quit = false;
        match event {
            Event::Checked(Ok(Some(release))) => {
                self.updater.checking = false;
                self.updater.manual_result = Some(format!("Update {} is available", release.tag));
                self.updater.release = Some(release);
                self.updater.notice = true;
            }
            Event::Checked(Ok(None)) => {
                self.updater.checking = false;
                self.updater.manual_result = Some("Up to date (or no published GitHub release yet).".into());
                self.updater.release = None;
                self.updater.notice = false;
            }
            Event::Checked(Err(error)) => {
                self.updater.checking = false;
                // Automatic checks should fail quietly: no release/configured
                // archive or temporary connectivity must not pop a modal.
                self.push_console_line_no_notify(format!("[UPDATE] Check failed: {error}"),
                    crate::logging::ConsoleLineKind::Normal);
                self.updater.error = Some(error);
            }
            Event::Progress { received, total } => {
                self.updater.downloaded = received;
                self.updater.total = total;
            }
            Event::Applied(result) => {
                self.updater.applying = false;
                match result {
                    Ok(()) => quit = true,
                    Err(e) => {
                        self.push_console_error(format!("[UPDATE] Restart failed: {e}"));
                        self.updater.error = Some(e);
                    },
                }
            }
            Event::FileDecision(line) => {
                self.push_console_line_no_notify(line, crate::logging::ConsoleLineKind::Normal);
            }
            Event::Staged(result) => {
                self.updater.downloading = false;
                match result {
                    Ok(stage) => { self.updater.staged = Some(stage); self.updater.manual_result = Some("Update ready. Restart to apply.".into()); }
                    Err(e) => {
                        self.push_console_error(format!("[UPDATE] Download/staging failed: {e}"));
                        self.updater.error = Some(e);
                    },
                }
            }
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
        if quit { self.request_quit(); }
    }

    /// Only the actual frontend Main page shows this nonmodal native notice.
    pub(in crate::app) fn native_update_notice_ui(&self) -> Option<crate::ui::update_notice::UpdateNoticeUi> {
        if !self.front_end || self.frontend_page != super::FrontendPage::Main
            || self.overlay == crate::ui::OverlayMode::Console || !self.updater.notice {
            return None;
        }
        let release = self.updater.release.as_ref()?;
        Some(crate::ui::update_notice::UpdateNoticeUi {
            version: release.tag.clone(),
            downloading: self.updater.downloading,
            applying: self.updater.applying,
            staged: self.updater.staged.is_some(),
            received: self.updater.downloaded,
            total: self.updater.total,
            error: self.updater.error.clone(),
            hovered: self.updater.notice_hovered,
        })
    }

    /// Input and paint share the Taffy layout. No egui Area/widgets are used.
    pub(in crate::app) fn native_update_notice_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{WindowEvent, ElementState, MouseButton};
        let Some(notice) = self.native_update_notice_ui() else { return false; };
        let Some(window) = self.window.as_ref() else { return false; };
        let size = window.inner_size();
        let layout = crate::ui::update_notice::layout(&notice, size.width, size.height);
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let p = [position.x as f32, position.y as f32];
                self.cursor_position = (position.x, position.y);
                let hovered = layout.hit_test(p, &notice);
                if self.updater.notice_hovered != hovered {
                    self.updater.notice_hovered = hovered;
                    self.publish_ui();
                }
                layout.contains(p)
            }
            WindowEvent::CursorLeft { .. } => {
                self.updater.notice_hovered = None;
                self.publish_ui();
                false
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let p = [self.cursor_position.0 as f32, self.cursor_position.1 as f32];
                if !layout.contains(p) { return false; }
                match layout.hit_test(p, &notice) {
                    Some(crate::ui::update_notice::Action::Download) => self.download_update(),
                    Some(crate::ui::update_notice::Action::Apply) => self.apply_update_now(),
                    Some(crate::ui::update_notice::Action::Dismiss) => {
                        self.updater.notice = false;
                        self.updater.notice_hovered = None;
                    }
                    None => {},
                }
                self.publish_ui();
                true
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {
                layout.contains([self.cursor_position.0 as f32, self.cursor_position.1 as f32])
            }
            _ => false,
        }
    }
}
