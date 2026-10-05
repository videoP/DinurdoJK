//! Dialogs.
use crate::app::egui_menu::{theme, App, LiveJoinUiPhase};

impl App {
    pub(in crate::app::egui_menu) fn egui_server_password_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.server_password_prompt.clone() else {
            return;
        };
        #[derive(Clone, Copy)]
        enum Choice {
            Connect,
            Cancel,
        }
        let mut choice = None;
        egui::Area::new(egui::Id::new("server_password_prompt"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(460.0);
                        theme::page_title(ui, "SERVER PASSWORD", "Enter the password to connect.");
                        ui.add_space(8.0);
                        theme::label(ui, theme::plain(&prompt.target, 11.5, theme::TEXT_DIM));
                        if let Some(message) = prompt.message.as_deref() {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(message).color(theme::WARNING));
                        }
                        ui.add_space(14.0);
                        let response = ui.add_sized(
                            [ui.available_width(), 28.0],
                            egui::TextEdit::singleline(&mut self.network.password)
                                .password(true)
                                .hint_text("server password")
                                .desired_width(f32::INFINITY),
                        );
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        {
                            choice = Some(Choice::Connect);
                        }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if theme::primary_button(ui, "CONNECT").clicked() {
                                choice = Some(Choice::Connect);
                            }
                            ui.add_space(8.0);
                            if theme::ghost_button(ui, "CANCEL").clicked() {
                                choice = Some(Choice::Cancel);
                            }
                        });
                    });
            });
        match choice {
            Some(Choice::Connect) => self.submit_server_password_prompt(),
            Some(Choice::Cancel) => self.cancel_server_password_prompt(),
            None => {}
        }
    }

    pub(in crate::app::egui_menu) fn egui_missing_map_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.missing_map_prompt.clone() else {
            return;
        };
        let can_auto = self.network.allow_http_downloads || self.network.allow_legacy_downloads;
        #[derive(Clone, Copy)]
        enum Choice {
            Abort,
            Connect,
            Download,
        }
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

    pub(in crate::app::egui_menu) fn egui_demo_missing_map_dialog(&mut self, root: &mut egui::Ui) {
        let Some(prompt) = self.demo_missing_map_prompt.clone() else {
            return;
        };
        #[derive(Clone, Copy)]
        enum Choice {
            Continue,
            Exit,
        }
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

    pub(in crate::app::egui_menu) fn egui_live_join_pipeline(&mut self, root: &mut egui::Ui) {
        let Some(join) = self.live_join_ui.clone() else {
            return;
        };

        // Own the whole frame for the duration of an autodownload-assisted join.
        // The renderer may still have the frontend scene or the normal map-load
        // splash underneath us; an opaque backdrop prevents either from flashing
        // between connection, download, filesystem refresh and map preparation.
        root.painter()
            .rect_filled(root.max_rect(), 0.0, egui::Color32::BLACK);

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
                    Some((
                        current_fraction.unwrap_or(0.0),
                        transfer_text,
                        current_fraction.is_none(),
                    )),
                    package_line,
                )
            } else if let Some(loading) = self.loading.as_ref() {
                let fraction = loading.progress_fraction();
                let (phase, detail) = if loading.preparation_finished {
                    (
                        "ENTERING GAME",
                        "Map preparation is complete; uploading the world to the renderer..."
                            .to_owned(),
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
                    LiveJoinUiPhase::CheckingContent => {
                        ("CHECKING CONTENT", join.detail.clone(), 0.08, None, None)
                    }
                    LiveJoinUiPhase::Synchronizing => {
                        ("SYNCHRONIZING", join.detail.clone(), 0.70, None, None)
                    }
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
                        theme::page_title(
                            ui,
                            "JOINING SERVER",
                            &format!("Preparing {}", join.map_name),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(phase)
                                .size(21.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(egui::RichText::new(detail).color(theme::TEXT_DIM));
                        ui.add_space(16.0);

                        let overall_text =
                            format!("Overall  {:.0}%", overall.clamp(0.0, 0.99) * 100.0);
                        ui.add(egui::ProgressBar::new(overall.clamp(0.0, 0.99)).text(overall_text));

                        if let Some(line) = package_line {
                            ui.add_space(14.0);
                            ui.label(egui::RichText::new(line).strong().color(theme::TEXT));
                        } else {
                            ui.add_space(14.0);
                        }
                        if let Some((fraction, text, animate)) = secondary {
                            let mut bar =
                                egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).text(text);
                            if animate {
                                bar = bar.animate(true);
                            }
                            ui.add(bar);
                        } else {
                            ui.add(egui::ProgressBar::new(0.0).animate(true).text("Working..."));
                        }

                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            for (index, label) in
                                ["CONTENT", "SYNC", "MAP", "ENTER"].iter().enumerate()
                            {
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

    pub(in crate::app::egui_menu) fn egui_download_dialog(&mut self, root: &mut egui::Ui) {
        let Some(state) = self.live_download.as_ref() else {
            return;
        };
        let map_name = state.map_name.clone();
        let status = state.status.clone();
        let file_name = state
            .current()
            .map(|spec| format!("{} · {}", spec.referenced_name, spec.remote_name))
            .unwrap_or_default();
        let file_index = (state.index + 1).min(state.files.len());
        let file_count = state.files.len();
        let received = state.received;
        let total = state.total;
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| (received as f32 / total as f32).clamp(0.0, 1.0));
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
}
