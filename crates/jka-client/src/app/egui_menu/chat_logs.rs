//! Chat logs.
use crate::app::egui_menu::{
    chat_log_duration_label, jka_colored_text, refresh_icon, theme, App, BTreeMap,
    ChatLogRangeFilter,
};

impl App {
    pub(in crate::app::egui_menu) fn egui_chat_log_browser_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_chat_log_browser_catalog();

        if self.ui_catalog.pending.chat_logs && self.chat_log_browser_entries.is_empty() {
            theme::page_title(ui, "CHAT LOGS", "Browse saved live-server chat sessions.");
            theme::label(
                ui,
                theme::plain("Scanning chat logs...", 12.0, theme::TEXT_FAINT),
            );
            return;
        }
        if let Some(error) = &self.chat_log_browser_catalog_error {
            theme::page_title(ui, "CHAT LOGS", "Browse saved live-server chat sessions.");
            theme::banner(
                ui,
                &format!("Chat log scan failed: {error}"),
                theme::WARNING,
            );
            if theme::ghost_button(ui, "RETRY").clicked() {
                self.refresh_chat_log_browser_catalog();
            }
            return;
        }

        let mut mods = self
            .chat_log_browser_entries
            .iter()
            .map(|entry| entry.mod_name.clone())
            .collect::<Vec<_>>();
        mods.sort_by_key(|value| value.to_ascii_lowercase());
        mods.dedup_by(|a, b| a.as_str().eq_ignore_ascii_case(b.as_str()));

        let mut server_map = BTreeMap::<String, String>::new();
        for entry in self.chat_log_browser_entries.iter().filter(|entry| {
            self.chat_log_browser_mod_filter.is_empty()
                || entry
                    .mod_name
                    .eq_ignore_ascii_case(&self.chat_log_browser_mod_filter)
        }) {
            server_map
                .entry(entry.server_key().to_owned())
                .or_insert_with(|| entry.server_label());
        }
        if !self.chat_log_browser_server_filter.is_empty()
            && !server_map.contains_key(&self.chat_log_browser_server_filter)
        {
            self.chat_log_browser_server_filter.clear();
        }
        let mut servers = server_map.into_iter().collect::<Vec<_>>();
        servers.sort_by(|a, b| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()));

        let search_terms = self
            .chat_log_browser_search
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect::<Vec<_>>();
        let now_ms = crate::chat_log::unix_ms_now();
        let relative_cutoff = match self.chat_log_browser_range {
            ChatLogRangeFilter::Hours24 => Some(now_ms.saturating_sub(24 * 60 * 60 * 1000)),
            ChatLogRangeFilter::Days7 => Some(now_ms.saturating_sub(7 * 24 * 60 * 60 * 1000)),
            ChatLogRangeFilter::Days30 => Some(now_ms.saturating_sub(30 * 24 * 60 * 60 * 1000)),
            ChatLogRangeFilter::All | ChatLogRangeFilter::Custom => None,
        };
        let custom_from = self.chat_log_browser_date_from.trim();
        let custom_to = self.chat_log_browser_date_to.trim();

        let filtered = self
            .chat_log_browser_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                self.chat_log_browser_mod_filter.is_empty()
                    || entry
                        .mod_name
                        .eq_ignore_ascii_case(&self.chat_log_browser_mod_filter)
            })
            .filter(|(_, entry)| {
                self.chat_log_browser_server_filter.is_empty()
                    || entry.server_key() == self.chat_log_browser_server_filter.as_str()
            })
            .filter(|(_, entry)| {
                if let Some(cutoff) = relative_cutoff {
                    return entry.ended_unix_ms >= cutoff;
                }
                if self.chat_log_browser_range != ChatLogRangeFilter::Custom {
                    return true;
                }
                // ISO local dates sort lexicographically. Treat a session as in
                // range when any part of it overlaps the requested date span.
                let after_start =
                    custom_from.is_empty() || entry.ended_date.as_str() >= custom_from;
                let before_end = custom_to.is_empty() || entry.started_date.as_str() <= custom_to;
                after_start && before_end
            })
            .filter(|(_, entry)| {
                search_terms
                    .iter()
                    .all(|term| entry.search_text.contains(term))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        if self
            .chat_log_browser_selected
            .as_ref()
            .is_some_and(|selected| {
                !filtered
                    .iter()
                    .any(|&index| self.chat_log_browser_entries[index].path == *selected)
            })
        {
            self.chat_log_browser_selected = None;
            self.chat_log_browser_detail_path = None;
            self.chat_log_browser_detail = None;
        }

        let body_height = ui.available_height().max(360.0);
        let total_width = ui.available_width().max(1.0);
        let gap = 10.0_f32;
        let browser_width = (total_width * 0.31)
            .clamp(320.0, 430.0)
            .min(total_width * 0.42);
        let detail_width = (total_width - browser_width - gap).max(1.0);
        let mut refresh_requested = false;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            ui.allocate_ui_with_layout(
                egui::vec2(browser_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::RAIL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::symmetric(10, 10))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (browser_width - 20.0).max(1.0),
                                (body_height - 20.0).max(1.0),
                            ));

                            ui.horizontal(|ui| {
                                theme::label(ui, theme::plain("SESSIONS", 11.0, theme::TEXT_DIM));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    refresh_requested |= refresh_icon(ui, "Rescan chat log folders");
                                    if theme::ghost_button(ui, "CLEAR").clicked() {
                                        self.chat_log_browser_mod_filter.clear();
                                        self.chat_log_browser_server_filter.clear();
                                        self.chat_log_browser_search.clear();
                                        self.chat_log_browser_range = ChatLogRangeFilter::All;
                                        self.chat_log_browser_date_from.clear();
                                        self.chat_log_browser_date_to.clear();
                                    }
                                });
                            });
                            ui.add_space(4.0);

                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("chat_log_mod_filter")
                                    .selected_text(if self.chat_log_browser_mod_filter.is_empty() {
                                        "All mods"
                                    } else {
                                        self.chat_log_browser_mod_filter.as_str()
                                    })
                                    .width((ui.available_width() * 0.43).max(100.0))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.chat_log_browser_mod_filter,
                                            String::new(),
                                            "All mods",
                                        );
                                        for mod_name in &mods {
                                            ui.selectable_value(
                                                &mut self.chat_log_browser_mod_filter,
                                                mod_name.clone(),
                                                mod_name,
                                            );
                                        }
                                    });
                                egui::ComboBox::from_id_salt("chat_log_range_filter")
                                    .selected_text(self.chat_log_browser_range.label())
                                    .width(ui.available_width().max(110.0))
                                    .show_ui(ui, |ui| {
                                        for range in [
                                            ChatLogRangeFilter::All,
                                            ChatLogRangeFilter::Hours24,
                                            ChatLogRangeFilter::Days7,
                                            ChatLogRangeFilter::Days30,
                                            ChatLogRangeFilter::Custom,
                                        ] {
                                            ui.selectable_value(
                                                &mut self.chat_log_browser_range,
                                                range,
                                                range.label(),
                                            );
                                        }
                                    });
                            });

                            egui::ComboBox::from_id_salt("chat_log_server_filter")
                                .selected_text(
                                    servers
                                        .iter()
                                        .find(|(key, _)| *key == self.chat_log_browser_server_filter)
                                        .map(|(_, label)| label.as_str())
                                        .unwrap_or("All servers"),
                                )
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.chat_log_browser_server_filter,
                                        String::new(),
                                        "All servers",
                                    );
                                    for (key, label) in &servers {
                                        ui.selectable_value(
                                            &mut self.chat_log_browser_server_filter,
                                            key.clone(),
                                            label,
                                        );
                                    }
                                });

                            if self.chat_log_browser_range == ChatLogRangeFilter::Custom {
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        [(ui.available_width() - 8.0) * 0.5, 24.0],
                                        egui::TextEdit::singleline(&mut self.chat_log_browser_date_from)
                                            .hint_text("From YYYY-MM-DD"),
                                    );
                                    ui.add_sized(
                                        [ui.available_width(), 24.0],
                                        egui::TextEdit::singleline(&mut self.chat_log_browser_date_to)
                                            .hint_text("To YYYY-MM-DD"),
                                    );
                                });
                            }

                            ui.add_sized(
                                [ui.available_width(), 26.0],
                                egui::TextEdit::singleline(&mut self.chat_log_browser_search)
                                    .hint_text("Search all chat messages..."),
                            );

                            ui.horizontal(|ui| {
                                theme::label(
                                    ui,
                                    theme::plain(
                                        &format!("{} / {} sessions", filtered.len(), self.chat_log_browser_entries.len()),
                                        9.5,
                                        theme::TEXT_FAINT,
                                    ),
                                );
                                if self.ui_catalog.pending.chat_logs {
                                    ui.spinner();
                                }
                            });
                            ui.separator();

                            let list_height = ui.available_height().max(1.0);
                            egui::ScrollArea::vertical()
                                .id_salt("chat_log_session_list")
                                .auto_shrink([false, false])
                                .max_height(list_height)
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    if filtered.is_empty() {
                                        ui.add_space(8.0);
                                        theme::label(
                                            ui,
                                            theme::plain("No chat sessions match these filters.", 11.0, theme::TEXT_FAINT),
                                        );
                                    }
                                    for &index in &filtered {
                                        let entry = self.chat_log_browser_entries[index].clone();
                                        let selected = self.chat_log_browser_selected.as_ref() == Some(&entry.path);
                                        let row_height = 58.0;
                                        let (rect, response) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), row_height),
                                            egui::Sense::click(),
                                        );
                                        let painter = ui.painter().with_clip_rect(rect);
                                        if selected {
                                            painter.rect_filled(rect, egui::CornerRadius::same(3), theme::CONTROL_SELECTED);
                                            painter.rect_filled(
                                                egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
                                                egui::CornerRadius::ZERO,
                                                theme::ACCENT,
                                            );
                                        } else if response.hovered() {
                                            painter.rect_filled(rect, egui::CornerRadius::same(3), theme::CONTROL);
                                        }

                                        let inset = rect.shrink2(egui::vec2(8.0, 5.0));
                                        let when = entry.started_label.get(..16).unwrap_or(entry.started_label.as_str());
                                        painter.text(
                                            inset.left_top(),
                                            egui::Align2::LEFT_TOP,
                                            when,
                                            egui::FontId::monospace(9.5),
                                            theme::TEXT_FAINT,
                                        );
                                        painter.text(
                                            egui::pos2(inset.right(), inset.top()),
                                            egui::Align2::RIGHT_TOP,
                                            &entry.mod_name,
                                            egui::FontId::monospace(9.0),
                                            if selected { theme::ACCENT } else { theme::TEXT_FAINT },
                                        );
                                        let server = if entry.server.is_empty() { &entry.address } else { &entry.server };
                                        painter.text(
                                            egui::pos2(inset.left(), inset.top() + 17.0),
                                            egui::Align2::LEFT_TOP,
                                            server,
                                            egui::FontId::proportional(12.0),
                                            if selected { theme::TEXT } else { theme::TEXT_DIM },
                                        );
                                        let bottom = format!(
                                            "{}{}{} msg{}{}",
                                            if entry.initial_map.is_empty() { "" } else { entry.initial_map.as_str() },
                                            if entry.initial_map.is_empty() { "" } else { "  ·  " },
                                            entry.message_count,
                                            if entry.message_count == 1 { "" } else { "s" },
                                            if entry.cleanly_closed { "" } else { "  ·  incomplete" },
                                        );
                                        painter.text(
                                            egui::pos2(inset.left(), inset.top() + 35.0),
                                            egui::Align2::LEFT_TOP,
                                            bottom,
                                            egui::FontId::monospace(9.0),
                                            theme::TEXT_FAINT,
                                        );

                                        let response = response.on_hover_text(entry.path.display().to_string());
                                        if response.clicked() && !selected {
                                            self.chat_log_browser_selected = Some(entry.path);
                                            self.chat_log_browser_detail_path = None;
                                            self.chat_log_browser_detail = None;
                                            self.egui_repaint_requested = true;
                                        }
                                    }
                                });
                        });
                },
            );

            self.ensure_chat_log_browser_detail();
            let detail = self
                .chat_log_browser_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().ok())
                .cloned();
            let detail_error = self
                .chat_log_browser_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().err())
                .cloned();

            ui.allocate_ui_with_layout(
                egui::vec2(detail_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::PANEL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(
                                (detail_width - 28.0).max(1.0),
                                (body_height - 24.0).max(1.0),
                            ));

                            let Some(detail) = detail else {
                                if self.ui_catalog.pending.chat_log_detail {
                                    ui.spinner();
                                    theme::label(ui, theme::plain("Loading transcript...", 11.0, theme::TEXT_FAINT));
                                } else if let Some(error) = detail_error {
                                    theme::banner(ui, &format!("Could not read chat log: {error}"), theme::WARNING);
                                } else {
                                    theme::page_title(ui, "CHAT LOGS", "Select a saved session to read its transcript.");
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            "Search on the left scans message contents across every mod and server.",
                                            11.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );
                                }
                                return;
                            };

                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    let title = if detail.server.is_empty() {
                                        detail.address.as_str()
                                    } else {
                                        detail.server.as_str()
                                    };
                                    theme::label(ui, theme::plain(title, 17.0, theme::TEXT));
                                    let mut subtitle = detail.started_label.clone();
                                    if !detail.address.is_empty() {
                                        subtitle.push_str("  ·  ");
                                        subtitle.push_str(&detail.address);
                                    }
                                    theme::label(ui, theme::plain(&subtitle, 10.5, theme::TEXT_FAINT));
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let state = if detail.cleanly_closed { "CLOSED" } else { "INCOMPLETE" };
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            state,
                                            9.0,
                                            if detail.cleanly_closed { theme::TEXT_FAINT } else { theme::WARNING },
                                        ),
                                    );
                                });
                            });
                            ui.add_space(5.0);
                            ui.horizontal_wrapped(|ui| {
                                for text in [
                                    format!("Mod: {}", detail.mod_name),
                                    format!("Player: {}", if detail.player.is_empty() { "—" } else { detail.player.as_str() }),
                                    format!("Initial map: {}", if detail.initial_map.is_empty() { "—" } else { detail.initial_map.as_str() }),
                                    format!("Duration: {}", chat_log_duration_label(detail.started_unix_ms, detail.ended_unix_ms)),
                                ] {
                                    egui::Frame::new()
                                        .fill(theme::CONTROL)
                                        .inner_margin(egui::Margin::symmetric(6, 3))
                                        .show(ui, |ui| {
                                            theme::label(ui, theme::plain(&text, 9.5, theme::TEXT_DIM));
                                        });
                                }
                            });
                            ui.add_space(7.0);
                            ui.separator();

                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(&mut self.chat_log_browser_show_global, "Global");
                                ui.checkbox(&mut self.chat_log_browser_show_team, "Team");
                                ui.checkbox(&mut self.chat_log_browser_show_located, "Located");
                                ui.checkbox(&mut self.chat_log_browser_show_voice, "Voice");
                                ui.checkbox(&mut self.chat_log_browser_show_session, "Session");
                                if !self.chat_log_browser_search.trim().is_empty() {
                                    theme::label(
                                        ui,
                                        theme::plain("search active", 9.0, theme::ACCENT),
                                    );
                                }
                            });
                            ui.separator();

                            let visible = detail
                                .entries
                                .iter()
                                .filter(|entry| match entry.kind {
                                    crate::chat_log::BrowserEntryKind::Say => self.chat_log_browser_show_global,
                                    crate::chat_log::BrowserEntryKind::Team => self.chat_log_browser_show_team,
                                    crate::chat_log::BrowserEntryKind::Located => self.chat_log_browser_show_located,
                                    crate::chat_log::BrowserEntryKind::Voice => self.chat_log_browser_show_voice,
                                    crate::chat_log::BrowserEntryKind::Session => self.chat_log_browser_show_session,
                                })
                                .filter(|entry| {
                                    search_terms
                                        .iter()
                                        .all(|term| entry.plain_lower.contains(term))
                                })
                                .collect::<Vec<_>>();

                            ui.horizontal(|ui| {
                                theme::label(
                                    ui,
                                    theme::plain(
                                        &format!("{} / {} entries", visible.len(), detail.entries.len()),
                                        9.5,
                                        theme::TEXT_FAINT,
                                    ),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    theme::label(
                                        ui,
                                        theme::plain(&detail.path.display().to_string(), 8.5, theme::TEXT_FAINT),
                                    );
                                });
                            });

                            let transcript_height = ui.available_height().max(1.0);
                            egui::ScrollArea::vertical()
                                .id_salt("chat_log_transcript")
                                .auto_shrink([false, false])
                                .max_height(transcript_height)
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    if visible.is_empty() {
                                        ui.add_space(10.0);
                                        theme::label(
                                            ui,
                                            theme::plain("No transcript entries match the current search/type filters.", 11.0, theme::TEXT_FAINT),
                                        );
                                    }
                                    for entry in visible {
                                        if entry.kind == crate::chat_log::BrowserEntryKind::Session {
                                            ui.add_space(3.0);
                                            ui.horizontal(|ui| {
                                                ui.add_sized(
                                                    [88.0, 18.0],
                                                    egui::Label::new(theme::plain(&entry.timestamp, 9.0, theme::TEXT_FAINT)),
                                                );
                                                theme::label(
                                                    ui,
                                                    theme::plain(&entry.text, 10.5, theme::TEXT_DIM),
                                                );
                                            });
                                            ui.add_space(3.0);
                                            continue;
                                        }

                                        let response = ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [88.0, 20.0],
                                                egui::Label::new(theme::plain(&entry.timestamp, 9.0, theme::TEXT_FAINT)),
                                            );
                                            let badge_color = match entry.kind {
                                                crate::chat_log::BrowserEntryKind::Team => egui::Color32::from_rgb(0x70, 0xD8, 0x78),
                                                crate::chat_log::BrowserEntryKind::Located => theme::ACCENT,
                                                crate::chat_log::BrowserEntryKind::Voice => theme::WARNING,
                                                crate::chat_log::BrowserEntryKind::Say => theme::TEXT_FAINT,
                                                crate::chat_log::BrowserEntryKind::Session => theme::TEXT_FAINT,
                                            };
                                            ui.add_sized(
                                                [62.0, 20.0],
                                                egui::Label::new(theme::plain(entry.kind.label(), 8.5, badge_color)),
                                            );
                                            ui.add(
                                                egui::Label::new(jka_colored_text(&entry.text, 11.5, theme::TEXT))
                                                    .wrap(),
                                            );
                                        });
                                        response.response.on_hover_text(format!("{} ms", entry.unix_ms));
                                        ui.add_space(2.0);
                                    }
                                });
                        });
                },
            );
        });

        if refresh_requested {
            self.refresh_chat_log_browser_catalog();
            self.egui_repaint_requested = true;
        }
    }
}
