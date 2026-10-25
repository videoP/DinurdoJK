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
        self.updater.total = release.archive_size;
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("github-update-download".into()).spawn(move || {
            let mut last_report = 0;
            let result = crate::updater::stage(&release, |received, total| {
                // Throttle event-loop messages, even for gigabyte-scale assets.
                if received == total || received.saturating_sub(last_report) >= 1024 * 1024 {
                    let _ = proxy.send_event(UserEvent::Update(Event::Progress { received, total }));
                    last_report = received;
                }
            });
            let _ = proxy.send_event(UserEvent::Update(Event::Staged(result)));
        });
        if let Err(e) = spawned { self.updater.downloading = false; self.updater.error = Some(e.to_string()); }
    }

    pub(in crate::app) fn handle_update_event(&mut self, event: Event) {
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
                self.updater.error = Some(error);
            }
            Event::Progress { received, total } => {
                self.updater.downloaded = received;
                self.updater.total = total;
            }
            Event::Staged(result) => {
                self.updater.downloading = false;
                match result {
                    Ok(stage) => { self.updater.staged = Some(stage); self.updater.manual_result = Some("Update ready. Restart to apply.".into()); }
                    Err(e) => self.updater.error = Some(e),
                }
            }
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app) fn egui_update_notice(&mut self, root: &mut egui::Ui) {
        if !self.updater.notice { return; }
        // Launch notifications only appear in the main menu. Never interrupt
        // multiplayer/gameplay or steal keyboard focus while playing.
        if !self.front_end || self.frontend_page != super::FrontendPage::Main { return; }
        let Some(release) = self.updater.release.clone() else { return; };
        egui::Area::new(egui::Id::new("djk_update_notice"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-22.0, 90.0))
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(24, 29, 39))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(95)))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(350.0);
                        ui.heading(format!("DinurdoJK {} available", release.tag));
                        ui.small("Download is manual. Gameplay will not be interrupted.");
                        if !release.notes.is_empty() {
                            egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| { ui.label(&release.notes); });
                        }
                        if self.updater.downloading {
                            let fraction = if self.updater.total == 0 { 0.0 } else {
                                self.updater.downloaded as f32 / self.updater.total as f32
                            };
                            ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
                        } else if let Some(stage) = self.updater.staged.clone() {
                            ui.label("Verified and ready to install when the game restarts.");
                            if ui.button("Restart & Apply Update").clicked() {
                                match crate::updater::restart_with_update(&stage) {
                                    Ok(()) => self.request_quit(),
                                    Err(e) => self.updater.error = Some(e),
                                }
                            }
                        } else if ui.button("Download & Install").clicked() {
                            self.download_update();
                        }
                        if let Some(error) = &self.updater.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                        if ui.button("Later / Dismiss").clicked() { self.updater.notice = false; }
                    });
            });
    }
}
