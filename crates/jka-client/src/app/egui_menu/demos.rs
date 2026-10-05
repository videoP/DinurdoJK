//! Demos.
use crate::app::egui_menu::{
    frontend, rail_item, theme, thread, App, BTreeMap, DemoConsoleFilter, DemoMetadata,
    DemoMetadataResult, Path,
};

impl App {
    pub(in crate::app::egui_menu) fn poll_demo_metadata_jobs(&mut self) {
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

    pub(in crate::app::egui_menu) fn ensure_demo_metadata(&mut self, demo_name: &str) {
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
        let thread_name = format!(
            "demo-meta:{}",
            demo_name.chars().take(24).collect::<String>()
        );
        if let Err(error) = thread::Builder::new().name(thread_name).spawn(move || {
            let result = frontend::index_demo_metadata(&base, game.as_deref(), &worker_demo_name);
            let _ = tx.send(DemoMetadataResult {
                generation,
                demo_name: worker_demo_name,
                result,
            });
        }) {
            self.demo_metadata_inflight.remove(&demo_name);
            self.demo_metadata_errors
                .insert(demo_name, format!("metadata worker: {error}"));
        }
    }

    pub(in crate::app::egui_menu) fn ensure_demo_catalog(&mut self) {
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

    pub(in crate::app::egui_menu) fn format_demo_time(ms: i32) -> String {
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

    pub(in crate::app::egui_menu) fn demo_gametype_name(gametype: i32) -> &'static str {
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

    pub(in crate::app::egui_menu) fn demo_team_summary(teams: &[i32]) -> String {
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

    pub(in crate::app::egui_menu) fn render_demo_metadata(
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
                ui.label(
                    egui::RichText::new("DURATION")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(Self::format_demo_time(metadata.duration_ms));
                ui.label(
                    egui::RichText::new("SIZE")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(format!("{size_mb:.1} MB"));
                ui.end_row();

                ui.label(
                    egui::RichText::new("MOD")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(&metadata.fs_game);
                ui.label(
                    egui::RichText::new("PROTOCOL")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(metadata.protocol.to_string());
                ui.end_row();

                ui.label(
                    egui::RichText::new("GAME TYPE")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(Self::demo_gametype_name(metadata.gametype));
                ui.label(
                    egui::RichText::new("SERVER")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(metadata.server_name.as_deref().unwrap_or("unknown"));
                ui.end_row();

                ui.label(
                    egui::RichText::new("SNAPSHOTS")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(format!(
                    "{}  ({:.1}/s)",
                    metadata.snapshot_count, metadata.average_snapshot_rate
                ));
                ui.label(
                    egui::RichText::new("KILLS")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(metadata.kill_count.to_string());
                ui.end_row();

                ui.label(
                    egui::RichText::new("RECORDED POV")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
                ui.label(pov);
                ui.label(
                    egui::RichText::new("SOURCE")
                        .color(theme::TEXT_DISABLED)
                        .size(11.0),
                );
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
        theme::section(
            ui,
            "PLAYERS",
            &format!("{} identities · roster presence", metadata.players.len()),
        );
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
                            ui.label(
                                egui::RichText::new(detail)
                                    .color(theme::TEXT_DISABLED)
                                    .size(10.0),
                            );
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
                ui.label(
                    egui::RichText::new(format!("ROUND {}", round.index)).color(theme::ACCENT),
                );
                ui.monospace(format!(
                    "{} +{}",
                    Self::format_demo_time(round.start_ms),
                    Self::format_demo_time(round.duration_ms)
                ));
                ui.label(format!("{} kills · {} players", round.kills, round.players));
                if let Some([red, blue]) = round
                    .team_scores
                    .filter(|score| score[0] != 0 || score[1] != 0)
                {
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
            ui.label(
                egui::RichText::new("No duel rounds visible in this recording.")
                    .color(theme::TEXT_DISABLED),
            );
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
                    ui.label(
                        egui::RichText::new("OVERVIEW")
                            .color(theme::TEXT_DISABLED)
                            .size(10.0),
                    );
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
                    ui.label(
                        egui::RichText::new(format!("DUEL {}", duel.index)).color(theme::ACCENT),
                    );
                    ui.label(
                        egui::RichText::new(duel.kind.label())
                            .color(theme::TEXT_DISABLED)
                            .size(10.0),
                    );
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
                        let scores = duel
                            .scores
                            .iter()
                            .map(|(name, score)| format!("{name} {score}"))
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.label(
                            egui::RichText::new(scores)
                                .color(theme::TEXT_DISABLED)
                                .size(10.0),
                        );
                    }
                });
            }
            if metadata
                .duels
                .iter()
                .any(|duel| matches!(duel.kind, frontend::DemoDuelKind::PrivatePov))
            {
                ui.label(egui::RichText::new("PRIVATE (POV) duels are limited to duels exposed by the recorded player's playerState.")
                    .color(theme::TEXT_DISABLED).size(10.0));
            }
        }

        ui.add_space(10.0);
        theme::section(
            ui,
            "CONSOLE",
            &format!("{} messages", metadata.console.len()),
        );
        ui.horizontal(|ui| {
            for (label, filter) in [
                ("ALL", DemoConsoleFilter::All),
                ("PRINT", DemoConsoleFilter::Print),
                ("CHAT", DemoConsoleFilter::Chat),
                ("CENTER", DemoConsoleFilter::CenterPrint),
                ("EVENTS", DemoConsoleFilter::Event),
            ] {
                if ui
                    .selectable_label(*console_filter == filter, label)
                    .clicked()
                {
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
                for entry in metadata
                    .console
                    .iter()
                    .filter(|entry| (*console_filter).matches(&entry.kind))
                {
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
                    row.response
                        .on_hover_text("Double-click to play this demo from here");
                }
            });
    }

    pub(in crate::app::egui_menu) fn egui_play_demo_page(&mut self, ui: &mut egui::Ui) {
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
                            } else if let Some(error) =
                                self.demo_metadata_errors.get(&selected_name)
                            {
                                theme::banner(
                                    ui,
                                    &format!("Metadata scan failed: {error}"),
                                    theme::WARNING,
                                );
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
}
