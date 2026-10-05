//! Screenshots.
use crate::app::egui_menu::{theme, App, PathBuf};

impl App {
    pub(in crate::app::egui_menu) fn egui_screenshot_browser_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_screenshot_catalog();
        if self.ui_catalog.pending.screenshots && self.screenshot_entries.is_empty() {
            theme::label(
                ui,
                theme::plain("Scanning screenshot folders...", 12.0, theme::TEXT_FAINT),
            );
            return;
        }
        if let Some(error) = &self.screenshot_catalog_error {
            theme::banner(
                ui,
                &format!("Screenshot scan failed: {error}"),
                theme::WARNING,
            );
            return;
        }

        let mut games = self
            .screenshot_entries
            .iter()
            .map(|entry| entry.game.clone())
            .collect::<Vec<_>>();
        games.sort_by_key(|game| (game != "base", game.to_ascii_lowercase()));
        games.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let search = self.screenshot_search.trim().to_ascii_lowercase();
        let game_filter = self.screenshot_game_filter.trim().to_ascii_lowercase();
        let filtered =
            self.screenshot_entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    game_filter.is_empty() || entry.game.eq_ignore_ascii_case(&game_filter)
                })
                .filter(|(_, entry)| {
                    if search.is_empty() {
                        return true;
                    }
                    entry.file_name.to_ascii_lowercase().contains(&search)
                        || entry.relative_name.to_ascii_lowercase().contains(&search)
                        || entry.game.to_ascii_lowercase().contains(&search)
                        || entry.metadata.as_ref().is_some_and(|metadata| {
                            metadata
                                .map_name
                                .as_deref()
                                .is_some_and(|map| map.to_ascii_lowercase().contains(&search))
                                || metadata
                                    .server_name
                                    .as_deref()
                                    .is_some_and(|name| name.to_ascii_lowercase().contains(&search))
                                || metadata.server_address.as_deref().is_some_and(|address| {
                                    address.to_ascii_lowercase().contains(&search)
                                })
                                || metadata.player_name.to_ascii_lowercase().contains(&search)
                                || metadata.players.iter().any(|player| {
                                    player.name.to_ascii_lowercase().contains(&search)
                                })
                        })
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();

        if self.screenshot_selected.as_ref().is_some_and(|selected| {
            !filtered
                .iter()
                .any(|&index| self.screenshot_entries[index].path == *selected)
        }) {
            self.screenshot_selected = filtered
                .first()
                .map(|&index| self.screenshot_entries[index].path.clone());
            self.screenshot_preview_texture = None;
            self.screenshot_preview_path = None;
            self.screenshot_preview_pending = None;
        } else if self.screenshot_selected.is_none() {
            self.screenshot_selected = filtered
                .first()
                .map(|&index| self.screenshot_entries[index].path.clone());
        }

        if let Some(path) = self.screenshot_selected.clone() {
            self.request_screenshot_preview(path);
        }

        let selected_entry = self.screenshot_selected.as_ref().and_then(|path| {
            self.screenshot_entries
                .iter()
                .find(|entry| &entry.path == path)
                .cloned()
        });
        let body_height = ui.available_height().max(320.0);
        let total_width = ui.available_width().max(1.0);
        let (browser_width, preview_width, inspector_width) =
            Self::browser_workspace_widths(total_width);
        let gap = 8.0_f32;
        let mut select_path = None::<PathBuf>;
        let mut refresh_requested = false;
        let mut jump_metadata = None::<crate::screenshot::ScreenshotMetadata>;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

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
                                egui::ComboBox::from_id_salt("screenshot_game_filter")
                                    .selected_text(if self.screenshot_game_filter.is_empty() {
                                        "All mods"
                                    } else {
                                        self.screenshot_game_filter.as_str()
                                    })
                                    .width((ui.available_width() - 74.0).max(90.0))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.screenshot_game_filter,
                                            String::new(),
                                            "All mods",
                                        );
                                        for game in &games {
                                            ui.selectable_value(
                                                &mut self.screenshot_game_filter,
                                                game.clone(),
                                                game.as_str(),
                                            );
                                        }
                                    });
                                if theme::ghost_button(ui, "REFRESH").clicked() {
                                    refresh_requested = true;
                                }
                            });
                            ui.add(
                                egui::TextEdit::singleline(&mut self.screenshot_search)
                                    .hint_text("Search screenshots, maps, servers, players...")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                theme::label(
                                    ui,
                                    theme::plain(
                                        &format!(
                                            "{} / {}",
                                            filtered.len(),
                                            self.screenshot_entries.len()
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
                                            theme::plain(
                                                "base + mod screenshot folders",
                                                8.5,
                                                theme::TEXT_FAINT,
                                            ),
                                        );
                                    },
                                );
                            });
                            ui.separator();

                            egui::ScrollArea::vertical()
                                .id_salt("screenshot_browser_list")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for &index in &filtered {
                                        let entry = &self.screenshot_entries[index];
                                        let selected = self
                                            .screenshot_selected
                                            .as_ref()
                                            .is_some_and(|path| path == &entry.path);
                                        let label = if let Some(metadata) = &entry.metadata {
                                            let map = metadata.map_name.as_deref().unwrap_or("menu");
                                            format!("{}\n{}  ·  {}", entry.file_name, entry.game, map)
                                        } else {
                                            format!("{}\n{}  ·  legacy", entry.file_name, entry.game)
                                        };
                                        let response = ui.selectable_label(selected, label);
                                        let response = response.on_hover_text(entry.path.display().to_string());
                                        if response.clicked() {
                                            select_path = Some(entry.path.clone());
                                        }
                                    }
                                    if filtered.is_empty() {
                                        theme::label(
                                            ui,
                                            theme::plain(
                                                "No screenshots match this filter.",
                                                10.0,
                                                theme::TEXT_FAINT,
                                            ),
                                        );
                                    }
                                });
                        });
                },
            );

            ui.allocate_ui_with_layout(
                egui::vec2(preview_width, body_height),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::PANEL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (preview_width - 20.0).max(1.0),
                                (body_height - 20.0).max(1.0),
                            ));
                            if let Some(entry) = &selected_entry {
                                theme::label(
                                    ui,
                                    theme::plain(&entry.file_name, 11.0, theme::TEXT_DIM),
                                );
                                ui.separator();
                            }
                            let available = ui.available_size();
                            if self.screenshot_preview_pending.is_some() {
                                ui.centered_and_justified(|ui| {
                                    theme::label(
                                        ui,
                                        theme::plain("Decoding screenshot...", 11.0, theme::TEXT_FAINT),
                                    );
                                });
                            } else if let Some(error) = &self.screenshot_preview_error {
                                ui.centered_and_justified(|ui| {
                                    theme::label(ui, theme::plain(error, 10.5, theme::WARNING));
                                });
                            } else if let Some(texture) = &self.screenshot_preview_texture {
                                let source = texture.size_vec2();
                                let scale = (available.x / source.x)
                                    .min(available.y / source.y)
                                    .min(1.0)
                                    .max(0.01);
                                ui.centered_and_justified(|ui| {
                                    ui.add(
                                        egui::Image::new(texture)
                                            .fit_to_exact_size(source * scale),
                                    );
                                });
                            } else {
                                ui.centered_and_justified(|ui| {
                                    theme::label(
                                        ui,
                                        theme::plain("Select a screenshot.", 11.0, theme::TEXT_FAINT),
                                    );
                                });
                            }
                        });
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
                            theme::section(ui, "CAPTURE", "");
                            let Some(entry) = &selected_entry else {
                                theme::label(
                                    ui,
                                    theme::plain("No screenshot selected.", 10.0, theme::TEXT_FAINT),
                                );
                                return;
                            };
                            theme::label(
                                ui,
                                theme::plain(
                                    &format!("MOD  {}", entry.game),
                                    10.0,
                                    theme::TEXT_DIM,
                                ),
                            );
                            theme::label(
                                ui,
                                theme::plain(
                                    &entry.relative_name,
                                    9.0,
                                    theme::TEXT_FAINT,
                                ),
                            );
                            ui.add_space(5.0);

                            if let Some(metadata) = &entry.metadata {
                                let can_jump = metadata.can_go_to_spot();
                                let clicked = ui
                                    .add_enabled_ui(can_jump, |ui| {
                                        theme::primary_button(ui, "GO TO SPOT").clicked()
                                    })
                                    .inner;
                                if clicked {
                                    jump_metadata = Some(metadata.clone());
                                }
                                if can_jump {
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            "Loads the saved map/mod and restores the saved position + view in the local free camera.",
                                            8.5,
                                            theme::TEXT_FAINT,
                                        ),
                                    );
                                }
                                ui.separator();
                                egui::ScrollArea::vertical()
                                    .id_salt("screenshot_metadata_inspector")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        Self::screenshot_metadata_row(ui, "TIME", metadata.captured_at.clone());
                                        Self::screenshot_metadata_row(
                                            ui,
                                            "MAP",
                                            metadata
                                                .map_name
                                                .clone()
                                                .unwrap_or_else(|| "—".into()),
                                        );
                                        Self::screenshot_metadata_row(
                                            ui,
                                            "MAP TIME",
                                            metadata
                                                .map_time_ms
                                                .map(Self::format_demo_time)
                                                .unwrap_or_else(|| "—".into()),
                                        );
                                        Self::screenshot_metadata_row(
                                            ui,
                                            "SERVER",
                                            metadata
                                                .server_name
                                                .clone()
                                                .unwrap_or_else(|| "local / unknown".into()),
                                        );
                                        Self::screenshot_metadata_row(
                                            ui,
                                            "ADDRESS",
                                            metadata
                                                .server_address
                                                .clone()
                                                .unwrap_or_else(|| "—".into()),
                                        );
                                        Self::screenshot_metadata_row(ui, "PLAYER", metadata.player_name.clone());
                                        Self::screenshot_metadata_row(ui, "FOV", format!("{:.1}", metadata.fov));
                                        Self::screenshot_metadata_row(
                                            ui,
                                            "VIEW",
                                            if metadata.third_person {
                                                "third person"
                                            } else {
                                                "first person"
                                            },
                                        );
                                        if let Some(origin) = metadata.player_origin {
                                            Self::screenshot_metadata_row(
                                            ui,
                                                "POSITION",
                                                format!(
                                                    "{:.1}  {:.1}  {:.1}",
                                                    origin[0], origin[1], origin[2]
                                                ),
                                            );
                                        } else if let Some(origin) = metadata.camera_origin {
                                            Self::screenshot_metadata_row(
                                            ui,
                                                "CAMERA",
                                                format!(
                                                    "{:.1}  {:.1}  {:.1}",
                                                    origin[0], origin[1], origin[2]
                                                ),
                                            );
                                        }
                                        if let Some(angles) = metadata.view_angles {
                                            Self::screenshot_metadata_row(
                                            ui,
                                                "ANGLES",
                                                format!(
                                                    "{:.1}  {:.1}  {:.1}",
                                                    angles[0], angles[1], angles[2]
                                                ),
                                            );
                                        }
                                        if let Some(crosshair) = &metadata.crosshair {
                                            ui.add_space(7.0);
                                            theme::section(ui, "CROSSHAIR HIT", "");
                                            Self::screenshot_metadata_row(ui, "TYPE", crosshair.kind.clone());
                                            if !crosshair.title.is_empty() {
                                                Self::screenshot_metadata_row(ui, "HIT", crosshair.title.clone());
                                            }
                                            if let Some(material) = &crosshair.material {
                                                Self::screenshot_metadata_row(ui, "SHADER", material.clone());
                                            }
                                            if let Some(distance) = &crosshair.distance {
                                                Self::screenshot_metadata_row(ui, "DIST", distance.clone());
                                            }
                                            if let Some(entity_num) = crosshair.entity_num {
                                                Self::screenshot_metadata_row(ui, "ENTITY", entity_num.to_string());
                                            }
                                        }
                                        ui.add_space(7.0);
                                        theme::section(
                                            ui,
                                            "PLAYERS",
                                            &format!("{} captured", metadata.players.len()),
                                        );
                                        for player in &metadata.players {
                                            let score = player
                                                .score
                                                .map(|score| format!("  score {score}"))
                                                .unwrap_or_default();
                                            let ping = player
                                                .ping
                                                .map(|ping| format!("  {ping} ms"))
                                                .unwrap_or_default();
                                            theme::label(
                                                ui,
                                                theme::plain(
                                                    &format!("{}{}{}", player.name, score, ping),
                                                    9.0,
                                                    theme::TEXT_DIM,
                                                ),
                                            );
                                        }
                                    });
                            } else {
                                theme::banner(
                                    ui,
                                    "Legacy/plain screenshot: no DinurdoJK capture metadata is embedded. Preview is still available.",
                                    theme::TEXT_FAINT,
                                );
                                if let Some(error) = &entry.metadata_error {
                                    ui.add_space(5.0);
                                    theme::label(
                                        ui,
                                        theme::plain(error, 9.0, theme::WARNING),
                                    );
                                }
                            }
                        });
                },
            );
        });

        if let Some(path) = select_path {
            self.screenshot_selected = Some(path.clone());
            self.screenshot_preview_texture = None;
            self.screenshot_preview_path = None;
            self.screenshot_preview_pending = None;
            self.screenshot_preview_error = None;
            self.request_screenshot_preview(path);
            self.egui_repaint_requested = true;
        }
        if refresh_requested {
            self.screenshot_catalog_loaded = false;
            self.screenshot_catalog_error = None;
            self.screenshot_preview_pending = None;
            self.ensure_screenshot_catalog();
            self.egui_repaint_requested = true;
        }
        if let Some(metadata) = jump_metadata {
            self.launch_screenshot_spot(metadata);
        }
    }
}
