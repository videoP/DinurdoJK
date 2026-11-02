//! Servers.
use crate::app::egui_menu::{
    server_browser_layout, jka_colored_text, mpsc, refresh_icon, server_browser, theme, App, BrowserCommand, BrowserEvent,
    Duration, Instant, ServerSource,
};

/// EgUI remains responsible for widgets; Taffy supplies every browser region.
/// This avoids egui's implicit panel width / item-spacing calculations.
fn browser_region<R>(ui: &mut egui::Ui, rect: egui::Rect, id: &'static str,
    add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope_builder(
        egui::UiBuilder::new().id_salt(id).max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(rect));
            add(ui)
        },
    ).inner
}

/// Place a vertically centered, clipped label in the exact Taffy cell.
/// The row owns the click target; the cell only handles hover/tooltip.
fn server_cell(ui: &mut egui::Ui, rect: egui::Rect, text: &str,
    content: impl Into<egui::WidgetText>, size: f32, tooltip: &str) {
    let content_rect = rect.shrink2(egui::vec2(7.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(content_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(content_rect));
            let response = ui.add_sized(
                [content_rect.width(), content_rect.height()],
                egui::Label::new(content).truncate(),
            );
            hover_if_truncated(ui, response, text, tooltip, size, 0.0);
        },
    );
}

/// Paint a filter icon as geometry, not a Unicode glyph. The client ships a
/// limited font atlas and the old "▾" disappeared on some setups. Keep this
/// inside the egui click target, so the entire 24px target opens the popup.
fn paint_column_filter_icon(ui: &egui::Ui, response: &egui::Response, active: bool) {
    let rect = response.rect;
    let color = if active { theme::ACCENT() } else if response.hovered() {
        theme::TEXT()
    } else {
        theme::TEXT_DIM()
    };
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect.shrink(1.0),
            egui::CornerRadius::ZERO,
            if active {
                egui::Color32::from_rgba_unmultiplied(67, 152, 179, 60)
            } else {
                egui::Color32::from_rgba_unmultiplied(180, 190, 205, 25)
            },
        );
    }
    let c = rect.center();
    let painter = ui.painter().with_clip_rect(rect);
    let points = vec![
        c + egui::vec2(-6.0, -4.2),
        c + egui::vec2(6.0, -4.2),
        c + egui::vec2(1.9, 0.3),
        c + egui::vec2(1.9, 5.0),
        c + egui::vec2(-1.9, 5.0),
        c + egui::vec2(-1.9, 0.3),
        c + egui::vec2(-6.0, -4.2),
    ];
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.4, color)));
    if active {
        painter.circle_filled(
            egui::pos2(rect.right() - 3.0, rect.top() + 3.0),
            2.1, theme::ACCENT(),
        );
    }
}

/// Add a full-value tooltip only when egui had to truncate the displayed cell.
///
/// `Label::truncate()` does not expose a truncation bit on `Response`, so compare
/// the untruncated galley width against the actual content width we gave the
/// widget. This keeps ordinary server-browser rows quiet while still making an
/// ellipsized value discoverable on hover. JKA color escapes are stripped for
/// measurement because they do not occupy glyph width.
fn hover_if_truncated(
    ui: &egui::Ui,
    response: egui::Response,
    displayed_text: &str,
    hover_text: impl Into<egui::WidgetText>,
    font_size: f32,
    horizontal_padding: f32,
) -> egui::Response {
    let measured = crate::logging::strip_jka_colors(displayed_text);
    let full_width = ui
        .painter()
        .layout_no_wrap(
            measured,
            egui::FontId::proportional(font_size),
            egui::Color32::WHITE,
        )
        .size()
        .x;
    let available_width = (response.rect.width() - horizontal_padding).max(0.0);
    if full_width > available_width + 0.5 {
        response.on_hover_text(hover_text)
    } else {
        response
    }
}

/// Syntax coloring for the editable MODS filter. Highlight comma-separated
/// *exclusions* red and *inclusions* green without altering what the user types.
/// Keep the comma neutral, and match the TextEdit's own Body font so editing
/// and caret positioning use exactly the same glyph metrics as the layout.
fn mod_filter_layout(text: &str, font_id: egui::FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.keep_trailing_whitespace = true;
    job.wrap.max_width = f32::INFINITY; // Single-line, horizontal scrolling.
    let neutral = egui::Color32::from_gray(160);
    for segment in text.split_inclusive(',') {
        let (token, comma) = if let Some(token) = segment.strip_suffix(',') {
            (token, true)
        } else {
            (segment, false)
        };
        let color = if token.trim_start().starts_with('-') {
            egui::Color32::from_rgb(245, 112, 112)
        } else {
            egui::Color32::from_rgb(110, 220, 145)
        };
        job.append(token, 0.0, egui::TextFormat {
            font_id: font_id.clone(),
            color,
            ..Default::default()
        });
        if comma {
            job.append(",", 0.0, egui::TextFormat {
                font_id: font_id.clone(),
                color: neutral,
                ..Default::default()
            });
        }
    }
    job
}

impl App {
    pub(in crate::app::egui_menu) fn poll_server_browser_events(&mut self) {
        let mut changed = false;
        loop {
            match self.server_browser_rx.try_recv() {
                Ok(event) => {
                    let completed_server_info = match &event {
                        BrowserEvent::ServerInfo(status) => Some((
                            status.address,
                            server_browser::format_server_status(status),
                        )),
                        _ => None,
                    };
                    self.server_browser.handle_event(event);
                    if let Some((address, dump)) = completed_server_info {
                        // `Get server info` is an explicit fresh getstatus request.
                        // Copy the complete result and mirror the same text into
                        // the console log without routing dozens of status rows
                        // through the transient Con_DrawNotify-style overlay.
                        self.egui_ctx.copy_text(dump.clone());
                        for line in dump.lines() {
                            crate::logging::write_line(
                                crate::logging::Level::Info,
                                format_args!("{line}"),
                            );
                        }
                        self.sync_console_log();
                        let text = format!(
                            "Full getstatus for {address} copied to clipboard and console"
                        );
                        self.server_browser.status_text = text.clone();
                        self.server_browser
                            .source_status
                            .insert(self.server_browser.source, text);
                    }
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

        // Autojoin is intentionally driven from the menu tick rather than the
        // server-browser page itself, so it keeps waiting even if the player
        // closes the browser or switches to another menu page. A full server is
        // polled cheaply in-place; once getinfo reports a free client slot we
        // stop polling and use the normal connect path.
        if let Some((source, address)) = self.server_browser.autojoin {
            let slot_open = self
                .server_browser
                .servers
                .get(&address)
                .is_some_and(|server| {
                    server.max_clients > 0 && server.clients < server.max_clients
                });
            if slot_open {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
                let text = format!("Slot opened on {address}; connecting…");
                self.server_browser
                    .source_status
                    .insert(source, text.clone());
                self.server_browser.status_text = text;
                self.connect_browser_server(address);
                changed = true;
            } else {
                let due = self
                    .server_browser
                    .autojoin_last_query
                    .is_none_or(|last| last.elapsed() >= Duration::from_millis(1500));
                if due {
                    if self
                        .server_browser_tx
                        .send(BrowserCommand::RefreshServer {
                            source,
                            address,
                            quiet: true,
                        })
                        .is_ok()
                    {
                        self.server_browser.autojoin_last_query = Some(Instant::now());
                        let text = format!("Autojoin: waiting for a slot on {address}…");
                        self.server_browser
                            .source_status
                            .insert(source, text.clone());
                        self.server_browser.status_text = text;
                    } else {
                        self.server_browser.autojoin = None;
                        self.server_browser.autojoin_last_query = None;
                        self.server_browser.status_text =
                            "Server browser worker is unavailable".to_owned();
                    }
                    changed = true;
                }
            }
        }
        if changed {
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app::egui_menu) fn request_server_refresh(&mut self, source: ServerSource) {
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
            let text = "Server browser worker is unavailable".to_owned();
            self.server_browser
                .source_status
                .insert(source, text.clone());
            self.server_browser.status_text = text;
        } else {
            self.server_browser.refreshing.insert(source);
            let text = format!("Refreshing {}…", source.label());
            self.server_browser
                .source_status
                .insert(source, text.clone());
            self.server_browser.status_text = text;
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app::egui_menu) fn ensure_server_browser_started(&mut self) {
        if !self.server_browser.internet_requested_once {
            self.request_server_refresh(ServerSource::Internet);
        }
    }

    pub(in crate::app::egui_menu) fn select_browser_server(
        &mut self,
        address: std::net::SocketAddr,
    ) {
        if self.server_browser.selected == Some(address) {
            return;
        }
        self.server_browser.selected = Some(address);
        self.server_browser.details = self.server_browser.status_cache.get(&address).cloned();
        let _ = self
            .server_browser_tx
            .send(BrowserCommand::QueryStatus(address));
        self.egui_repaint_requested = true;
    }

    // Avoid trusting a placeholder or a non-JKA server result as a map hint.
    // A cached entry can be stale, so the caller must still verify it with the
    // actual server's infoResponse and gamestate.
    pub(in crate::app) fn browser_map_hint(
        &self,
        address: std::net::SocketAddr,
    ) -> Option<(String, String)> {
        self.server_browser
            .servers
            .get(&address)
            .filter(|server| server.protocol == 26 && !server.map.trim().is_empty())
            .map(|server| (server.map.clone(), server.game.clone()))
    }

    pub(in crate::app::egui_menu) fn connect_browser_server(
        &mut self,
        address: std::net::SocketAddr,
    ) {
        if self
            .server_browser
            .servers
            .get(&address)
            .is_some_and(|server| server.need_password)
            && self.network.password.is_empty()
        {
            self.show_server_password_prompt(
                address.to_string(),
                Some("This server requires a password.".to_owned()),
            );
            return;
        }
        // Browser refresh already resolved the map and mod for this endpoint.
        // Treat them as hints, never as server admission: a fresh infoResponse
        // (and later gamestate) will reconcile them with authoritative state.
        let hint = self.browser_map_hint(address);
        self.connect_to_server_with_hint(
            &address.to_string(),
            hint.as_ref().map(|(map, game)| (map.as_str(), game.as_str())),
        );
    }

    /// Taffy owns the column cell. Within it, use explicit, pixel-aligned
    /// rectangles for the label and the filter button rather than egui's
    /// horizontal layout (which used to push icons away from their titles).
    pub(in crate::app::egui_menu) fn server_browser_column_header(
        &mut self,
        ui: &mut egui::Ui,
        sort: server_browser::BrowserSort,
        width: f32,
    ) {
        use server_browser::BrowserSort;
        let before = self.server_browser.filters_string();
        let selected = self.server_browser.sort == sort;
        let label = if sort == BrowserSort::LastVisited && width < 105.0 {
            "VISITED"
        } else {
            sort.label()
        };
        let font = egui::FontId::proportional(10.5);
        let text_width = ui.painter().layout_no_wrap(
            label.to_owned(), font.clone(), theme::TEXT_DIM(),
        ).size().x;
        let (cell, _) = ui.allocate_exact_size(
            egui::vec2(width, 24.0), egui::Sense::hover(),
        );
        let painter = ui.painter().with_clip_rect(cell);
        let left = cell.left() + 6.0;
        let arrow_space = if selected { 10.0 } else { 0.0 };
        let icon_width = 20.0;
        let gap = 3.0;
        let usable_text_width = (cell.right() - left - gap - icon_width - arrow_space - 3.0)
            .max(1.0);
        let drawn_text_width = text_width.min(usable_text_width);
        let filter_left = left + drawn_text_width + arrow_space + gap;
        let label_rect = egui::Rect::from_min_max(
            egui::pos2(cell.left(), cell.top()),
            egui::pos2(filter_left - gap, cell.bottom()),
        );
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(filter_left, cell.center().y - 10.0),
            egui::vec2(icon_width, 20.0),
        );
        let sort_response = ui.interact(
            label_rect, ui.id().with(("server_sort", label)), egui::Sense::click(),
        );
        let button = ui.interact(
            icon_rect, ui.id().with(("server_filter", label)), egui::Sense::click(),
        ).on_hover_text(format!("Filter {}", sort.label()));
        let text_clip = egui::Rect::from_min_max(
            egui::pos2(left, cell.top()),
            egui::pos2(left + drawn_text_width, cell.bottom()),
        );
        ui.painter().with_clip_rect(text_clip).text(
            egui::pos2(left, cell.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            if selected { theme::ACCENT() } else { theme::TEXT_DIM() },
        );
        if selected {
            let center = egui::pos2(left + drawn_text_width + 5.0, cell.center().y);
            let verts = if self.server_browser.sort_ascending {
                vec![center + egui::vec2(-3.0, 2.0),
                     center + egui::vec2(3.0, 2.0),
                     center + egui::vec2(0.0, -2.25)]
            } else {
                vec![center + egui::vec2(-3.0, -2.0),
                     center + egui::vec2(3.0, -2.0),
                     center + egui::vec2(0.0, 2.25)]
            };
            painter.add(egui::Shape::convex_polygon(
                verts, theme::ACCENT(), egui::Stroke::NONE,
            ));
        }
        if sort_response.clicked() {
            if selected {
                self.server_browser.sort_ascending = !self.server_browser.sort_ascending;
            } else {
                self.server_browser.sort = sort;
                self.server_browser.sort_ascending = matches!(
                    sort,
                    BrowserSort::Ping | BrowserSort::Name | BrowserSort::Map
                        | BrowserSort::Gametype | BrowserSort::Mod
                );
            }
            self.egui_repaint_requested = true;
        }
        if sort_response.hovered() {
            painter.line_segment(
                [egui::pos2(left, cell.bottom() - 1.0),
                 egui::pos2(filter_left - gap, cell.bottom() - 1.0)],
                egui::Stroke::new(1.0, if selected { theme::ACCENT() } else { theme::LINE_STRONG() }),
            );
        }
        sort_response.on_hover_text("Click to sort; click again to reverse the order.");
        let active = match sort {
                BrowserSort::Name => !self.server_browser.search.trim().is_empty(),
                BrowserSort::Map => !self.server_browser.map_filter.trim().is_empty(),
                BrowserSort::Mod => !self.server_browser.mod_filter.trim().is_empty(),
                BrowserSort::Gametype => self.server_browser.gametype_filter != 0,
                BrowserSort::Players => self.server_browser.hide_empty
                    || self.server_browser.hide_full
                    || self.server_browser.hide_bots
                    || self.server_browser.hide_connecting
                    || !self.server_browser.player_search.trim().is_empty(),
                BrowserSort::Ping => self.server_browser.max_ping != 0,
                BrowserSort::LastVisited => self.server_browser.visit_filter != 0,
            };
        paint_column_filter_icon(ui, &button, active);
        egui::Popup::menu(&button)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(match sort {
                        BrowserSort::Mod => 350.0,
                        BrowserSort::Players => 310.0,
                        _ => 230.0,
                    });
                    theme::label(ui, theme::plain(&format!("{} FILTER", sort.label()), 11.0, theme::TEXT_DIM()));
                    ui.separator();
                    match sort {
                        BrowserSort::Name => {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.server_browser.search)
                                    .desired_width(230.0)
                                    .hint_text("Server name or address"),
                            ).on_hover_text("Case-insensitive server hostname or IP/port search.");
                            if ui.small_button("Clear name filter").clicked() {
                                self.server_browser.search.clear();
                            }
                        }
                        BrowserSort::Map => {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.server_browser.map_filter)
                                    .desired_width(230.0)
                                    .hint_text("Map name contains…"),
                            );
                            if ui.small_button("Clear map filter").clicked() {
                                self.server_browser.map_filter.clear();
                            }
                        }
                        BrowserSort::Mod => {
                            theme::label(ui, theme::plain("Comma-separated mod directories", 11.0, theme::TEXT_DIM()));
                            ui.add_space(3.0);
                            // Explicitly clear inherited text color overrides so
                            // custom LayoutJob colors can appear in TextEdit.
                            ui.scope(|ui| {
                                ui.visuals_mut().override_text_color = None;
                                let font_id = egui::TextStyle::Body.resolve(ui.style());
                                let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _width: f32| {
                                    ui.fonts_mut(|fonts| fonts.layout_job(mod_filter_layout(buffer.as_str(), font_id.clone())))
                                };
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.server_browser.mod_filter)
                                        .font(egui::TextStyle::Body)
                                        .layouter(&mut layouter)
                                        .desired_width(328.0)
                                        .hint_text(server_browser::DEFAULT_MOD_FILTER),
                                );
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Reset default").clicked() {
                                    self.server_browser.mod_filter = server_browser::DEFAULT_MOD_FILTER.to_owned();
                                }
                                if ui.button("Clear").clicked() {
                                    self.server_browser.mod_filter.clear();
                                }
                            });
                        }
                        BrowserSort::Gametype => {
                            if ui.selectable_label(self.server_browser.gametype_filter == 0, "All gametypes").clicked() {
                                self.server_browser.gametype_filter = 0;
                            }
                            ui.separator();
                            for &(gametype, label) in &server_browser::GAMETYPE_FILTER_OPTIONS {
                                let bit = server_browser::gametype_filter_bit(gametype);
                                let mut selected = self.server_browser.gametype_filter & bit != 0;
                                if ui.checkbox(&mut selected, label).changed() {
                                    if selected {
                                        self.server_browser.gametype_filter |= bit;
                                    } else {
                                        self.server_browser.gametype_filter &= !bit;
                                    }
                                }
                            }
                        }
                        BrowserSort::Players => {
                            ui.checkbox(&mut self.server_browser.hide_bots, "Hide bots (0 ping)");
                            ui.checkbox(&mut self.server_browser.hide_connecting, "Hide connecting (999 ping)");
                            ui.checkbox(&mut self.server_browser.hide_empty, "Hide empty servers");
                            ui.checkbox(&mut self.server_browser.hide_full, "Hide full servers");
                            ui.separator();
                            theme::label(ui, theme::plain("FIND PLAYERS", 10.5, theme::TEXT_DIM()));
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.server_browser.player_search)
                                    .desired_width(290.0)
                                    .hint_text("Player names, comma-separated"),
                            );
                            if response.changed() {
                                self.server_browser.player_search_last_query = None;
                            }
                            ui.checkbox(&mut self.server_browser.player_exact_match, "Exact player name");
                            ui.separator();
                            if ui.button("Reset player defaults").clicked() {
                                self.server_browser.hide_empty = false;
                                self.server_browser.hide_full = false;
                                self.server_browser.hide_bots = true;
                                self.server_browser.hide_connecting = true;
                                self.server_browser.player_search.clear();
                                self.server_browser.player_exact_match = false;
                            }
                        }
                        BrowserSort::Ping => {
                            ui.horizontal(|ui| {
                                theme::label(ui, theme::plain("Maximum ping", 11.0, theme::TEXT_DIM()));
                                ui.add(
                                    egui::DragValue::new(&mut self.server_browser.max_ping)
                                        .range(0..=999).suffix(" ms").speed(5),
                                );
                            });
                            theme::label(ui, theme::plain("0 = no ping limit", 10.0, theme::TEXT_FAINT()));
                        }
                        BrowserSort::LastVisited => {
                            for (value, label) in [(0, "All servers"), (1, "Visited only"), (2, "Never visited")] {
                                if ui.selectable_label(self.server_browser.visit_filter == value, label).clicked() {
                                    self.server_browser.visit_filter = value;
                                }
                            }
                        }
                    }
                });
        if self.server_browser.filters_string() != before {
            self.mark_config_dirty();
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app::egui_menu) fn egui_server_browser_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_server_browser_started();

        theme::page_title(
            ui,
            "SERVER BROWSER",
            "Multiplayer servers  ·  click a header to sort, or its arrow to filter",
        );

        // Claim one stable viewport; Taffy splits it into a dominant browser
        // and a secondary inspector, stacking them at narrower widths.
        let available = egui::vec2(ui.available_width().max(1.0), ui.available_height().max(1.0));
        let (workspace_rect, _) = ui.allocate_exact_size(available, egui::Sense::hover());
        let workspace = server_browser_layout::workspace(workspace_rect);
        let painter = ui.painter();
        for (rect, accent) in [(workspace.list, true), (workspace.detail, false)] {
            painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_rgba_unmultiplied(9, 16, 23, 225));
            painter.rect_stroke(rect, egui::CornerRadius::ZERO, egui::Stroke::new(1.0, if accent {
                theme::LINE_STRONG()
            } else {
                theme::LINE()
            }), egui::StrokeKind::Inside);
        }
        let mut selected_after = None;
        let mut connect_after = None;
        let mut refresh_one_after = None;
        let mut favorite_after = None;
        let mut autojoin_after = None;
        let mut get_server_info_after = None;
        let list_region = workspace.list.shrink2(egui::vec2(12.0, 10.0));
        browser_region(ui, list_region, "server_list_region", |ui| {
        // Keep master configuration in a popup anchored to the source toolbar;
        // expanding it must never push down or resize the server table.
        let enabled_masters = self.server_browser.master_servers.iter()
            .filter(|master| !master.trim().is_empty())
            .count();
        let mut pending_master_changes: Vec<(usize, String)> = Vec::new();
        // Source selector + refresh.
        ui.horizontal(|ui| {
            for source in [
                ServerSource::Internet,
                ServerSource::Lan,
                ServerSource::Favorites,
            ] {
                if theme::chip(ui, source.label(), self.server_browser.source == source).clicked() {
                    self.server_browser.source = source;
                    self.server_browser.selected = None;
                    self.server_browser.details = None;
                    self.server_browser.player_search_last_query = None;
                    self.server_browser.status_text = self
                        .server_browser
                        .source_status
                        .get(&source)
                        .cloned()
                        .unwrap_or_else(|| "Ready".to_owned());
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
            // Edit master servers without reserving a whole row when closed.
            // A popup also keeps the list stationary while its settings are open.
            ui.add_space(8.0);
            let master_button = ui.small_button(format!(
                "Master servers ({enabled_masters}/{})",
                server_browser::MAX_MASTER_SLOTS,
            )).on_hover_text("Choose and edit server discovery masters");
            egui::Popup::menu(&master_button)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(465.0);
                    theme::label(ui, theme::plain("MASTER SERVERS", 11.0, theme::TEXT_DIM()));
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.small_button("RESTORE DEFAULTS").clicked() {
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
                                theme::plain(&format!("sv_master{}", slot + 1), 10.5, theme::TEXT_DIM()),
                            );
                            let edit = ui.add_sized(
                                [320.0, 22.0],
                                egui::TextEdit::singleline(&mut draft).hint_text(if slot < 3 {
                                    server_browser::DEFAULT_MASTER_CVARS[slot]
                                } else {
                                    "master.example.org[:port]"
                                }),
                            );
                            theme::label(ui, theme::plain(slot_labels[slot], 10.0, theme::TEXT_FAINT()));
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
            // Reset sits alongside the source tabs, rather than consuming
            // a separate toolbar row above the table.
            ui.add_space(12.0);
            if ui.small_button("Restore filter defaults").clicked() {
                self.server_browser.apply_filters_string(server_browser::DEFAULT_SERVER_FILTERS);
                self.mark_config_dirty();
                self.egui_repaint_requested = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let refreshing = self
                    .server_browser
                    .refreshing
                    .contains(&self.server_browser.source);
                let clicked = ui
                    .add_enabled_ui(!refreshing, |ui| {
                        refresh_icon(
                            ui,
                            if refreshing {
                                "Refreshing server list…"
                            } else {
                                "Refresh server list"
                            },
                        )
                    })
                    .inner;
                if clicked {
                    self.request_server_refresh(self.server_browser.source);
                }
                if refreshing {
                    theme::label(ui, theme::plain("REFRESHING…", 10.5, theme::TEXT_FAINT()));
                }
            });
        });

        // Apply edits after rendering the popup, outside the nested egui Ui.
        for (slot, value) in pending_master_changes {
            let cvar = format!("sv_master{}", slot + 1);
            if let Err(error) = self.set_console_cvar(&cvar, &value) {
                self.server_browser.status_text = error;
            }
        }
        ui.add_space(4.0);

        let search = self.server_browser.search.trim().to_ascii_lowercase();
        let player_terms: Vec<String> = self
            .server_browser
            .player_search
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        let mod_filters = server_browser::ModFilters::parse(&self.server_browser.mod_filter);
        let mut servers: Vec<_> = self
            .server_browser
            .servers_for_current_source()
            .into_iter()
            .filter(|server| {
                // Apply Hide Empty only AFTER status queries have been scheduled.
                // Otherwise getinfo's unreliable bot count can hide a server
                // before we have ever checked its actual getstatus player pings.
                if self.server_browser.hide_full
                    && server.max_clients > 0
                    && server.clients >= server.max_clients
                {
                    return false;
                }
                if self.server_browser.max_ping > 0 && server.ping_ms > self.server_browser.max_ping
                {
                    return false;
                }
                if !mod_filters.matches(&server.game)
                    || !self.server_browser.matches_gametype(server.gametype)
                {
                    return false;
                }
                if !self.server_browser.map_filter.trim().is_empty()
                    && !server.map.to_ascii_lowercase().contains(
                        &self.server_browser.map_filter.trim().to_ascii_lowercase(),
                    )
                {
                    return false;
                }
                let visited = self.server_browser.history_last_visited(server.address).is_some();
                if (self.server_browser.visit_filter == 1 && !visited)
                    || (self.server_browser.visit_filter == 2 && visited)
                {
                    return false;
                }
                if search.is_empty() {
                    return true;
                }
                let hostname =
                    crate::logging::strip_jka_colors(&server.hostname).to_ascii_lowercase();
                hostname.contains(&search) || server.address.to_string().contains(&search)
            })
            .collect();

        // These filters need per-player getstatus pings (EternalJK's 0=bot,
        // 999=connecting), not server-advertised bot/human fields. Query every
        // potentially populated candidate before filtering away bot-only rows.
        if !player_terms.is_empty()
            || self.server_browser.hide_bots
            || self.server_browser.hide_connecting
        {
            let candidates: Vec<_> = servers.iter()
                .filter(|server| server.clients > 0)
                .map(|server| server.address)
                .collect();
            let missing_status = candidates.iter().any(|address| {
                !self.server_browser.status_cache.contains_key(address)
            });
            // Continuous player-name search is more time-sensitive than the
            // optional bot/connecting count refresh. Avoid unnecessary UDP
            // traffic for large server lists when only display filters are on.
            let refresh_interval = if player_terms.is_empty() { 20 } else { 4 };
            let due = (missing_status
                || self.server_browser.player_search_last_query
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(refresh_interval)))
                && self.server_browser.status_pending.is_empty();
            if due && !candidates.is_empty()
                && !self.server_browser.refreshing.contains(&self.server_browser.source)
            {
                if self.server_browser_tx
                    .send(BrowserCommand::QueryStatusBatch(candidates.clone()))
                    .is_ok()
                {
                    self.server_browser.status_pending.extend(candidates);
                    self.server_browser.player_search_last_query = Some(Instant::now());
                } else {
                    self.server_browser.status_text =
                        "Server browser worker is unavailable".to_owned();
                }
            }
        }

        // Final count-based filtering must use the getstatus breakdown when
        // available. Do not prevent status discovery for advertised bot-only
        // servers by filtering them out before the batch is queued.
        if self.server_browser.hide_empty {
            servers.retain(|server| self.server_browser.visible_client_count(server) > 0);
        }

        if !player_terms.is_empty() {
            let exact = self.server_browser.player_exact_match;
            let hide_bots = self.server_browser.hide_bots;
            let hide_connecting = self.server_browser.hide_connecting;
            servers.retain(|server| {
                let Some(status) = self.server_browser.status_cache.get(&server.address) else {
                    // Optimistically keep rows until the first status batch has
                    // had a chance to answer; the status-batch completion caches
                    // an empty result for non-responders.
                    return true;
                };
                status.players.iter().any(|player| {
                    if (hide_bots && player.ping == 0)
                        || (hide_connecting && player.ping == 999)
                    {
                        return false;
                    }
                    let name = crate::logging::strip_jka_colors(&player.name).to_ascii_lowercase();
                    player_terms.iter().any(|term| {
                        if exact {
                            name == *term
                        } else {
                            name.contains(term)
                        }
                    })
                })
            });
        }

        let sort = self.server_browser.sort;
        servers.sort_by(|a, b| {
            if sort == server_browser::BrowserSort::LastVisited {
                // Unknown/unvisited entries remain after visited entries in both
                // directions; reversing this sort should mean newest-vs-oldest,
                // not suddenly put every unknown server at the top.
                return match (
                    self.server_browser.history_last_visited(a.address),
                    self.server_browser.history_last_visited(b.address),
                ) {
                    (Some(a_visited), Some(b_visited)) => {
                        if self.server_browser.sort_ascending {
                            a_visited.cmp(&b_visited)
                        } else {
                            b_visited.cmp(&a_visited)
                        }
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.address.cmp(&b.address),
                };
            }

            let order = match sort {
                server_browser::BrowserSort::Ping => {
                    let ap = if a.ping_ms == 0 { u32::MAX } else { a.ping_ms };
                    let bp = if b.ping_ms == 0 { u32::MAX } else { b.ping_ms };
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Players => {
                    let ap = self.server_browser.visible_client_count(a);
                    let bp = self.server_browser.visible_client_count(b);
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Name => crate::logging::strip_jka_colors(&a.hostname)
                    .to_ascii_lowercase()
                    .cmp(&crate::logging::strip_jka_colors(&b.hostname).to_ascii_lowercase()),
                server_browser::BrowserSort::Map => {
                    a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase())
                }
                server_browser::BrowserSort::Gametype => a.gametype.cmp(&b.gametype),
                server_browser::BrowserSort::Mod => a.game.to_ascii_lowercase().cmp(&b.game.to_ascii_lowercase()),
                server_browser::BrowserSort::LastVisited => unreachable!(),
            };
            if self.server_browser.sort_ascending {
                order
            } else {
                order.reverse()
            }
        });

        let browser_height = (ui.available_height() - 48.0).max(155.0);
        let list_width = ui.available_width();
        let columns = server_browser_layout::Table::compute(list_width);
        ui.allocate_ui_with_layout(
            egui::vec2(list_width, browser_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                    theme::section(
                        ui,
                        self.server_browser.source.label(),
                        &format!("{} visible · {}", servers.len(), self.server_browser.status_text),
                    );
                    let (header_rect, _) = ui.allocate_exact_size(
                        egui::vec2(columns.width, server_browser_layout::HEADER_H),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(
                        header_rect, egui::CornerRadius::ZERO, egui::Color32::from_rgba_unmultiplied(36, 45, 55, 180),
                    );
                    for (index, sort) in [
                        server_browser::BrowserSort::Name,
                        server_browser::BrowserSort::Map,
                        server_browser::BrowserSort::Mod,
                        server_browser::BrowserSort::Gametype,
                        server_browser::BrowserSort::Players,
                        server_browser::BrowserSort::Ping,
                        server_browser::BrowserSort::LastVisited,
                    ].into_iter().enumerate() {
                        let rect = columns.cell(header_rect, index);
                        if index > 0 {
                            ui.painter().line_segment(
                                [egui::pos2(rect.left(), rect.top() + 7.0),
                                 egui::pos2(rect.left(), rect.bottom() - 7.0)],
                                egui::Stroke::new(0.65, theme::LINE()),
                            );
                        }
                        // Header cells share identical Taffy coordinates with
                        // the row cells, irrespective of font or popup width.
                        ui.scope_builder(
                            egui::UiBuilder::new().id_salt(("server_header", index))
                                .max_rect(rect)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            |ui| {
                                ui.set_clip_rect(ui.clip_rect().intersect(rect));
                                self.server_browser_column_header(ui, sort, rect.width());
                            },
                        );
                    }
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("jka_server_browser_list")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for server in &servers {
                                let selected = self.server_browser.selected == Some(server.address);
                                let hostname = if server.hostname.is_empty() {
                                    server.address.to_string()
                                } else {
                                    server.hostname.clone()
                                };
                                let favorite = self.server_browser.is_favorite(server.address);
                                let autojoining = self
                                    .server_browser
                                    .autojoin
                                    .is_some_and(|(_, address)| address == server.address);
                                let full = server.max_clients > 0 && server.clients >= server.max_clients;
                                // One egui interaction surface per row. All
                                // seven visual cells use Taffy's common grid.
                                let (row_rect, response) = ui.allocate_exact_size(
                                    egui::vec2(columns.width, server_browser_layout::ROW_H),
                                    egui::Sense::click(),
                                );
                                let fill = if selected {
                                    egui::Color32::from_rgba_unmultiplied(75, 146, 175, 59)
                                } else if response.hovered() {
                                    egui::Color32::from_rgba_unmultiplied(90, 126, 151, 35)
                                } else {
                                    egui::Color32::TRANSPARENT
                                };
                                ui.painter().rect_filled(row_rect, egui::CornerRadius::ZERO, fill);
                                if selected {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(row_rect.min, egui::vec2(3.0, row_rect.height())),
                                        0.0, theme::ACCENT(),
                                    );
                                }
                                ui.painter().line_segment(
                                    [row_rect.left_bottom(), row_rect.right_bottom()],
                                    egui::Stroke::new(0.5, theme::LINE()),
                                );
                                let label = if server.need_password {
                                    format!("🔒 {hostname}")
                                } else {
                                    hostname
                                };
                                let name_hover = crate::logging::strip_jka_colors(&label);
                                server_cell(ui, columns.cell(row_rect, 0), &name_hover,
                                    jka_colored_text(&label, 11.7, theme::TEXT()), 11.7, &name_hover);
                                let map = if server.map.is_empty() { "-" } else { &server.map };
                                server_cell(ui, columns.cell(row_rect, 1), map,
                                    theme::plain(map, 11.5, theme::TEXT()), 11.5, map);
                                let mod_name = if server.game.trim().is_empty() { "base" } else { server.game.as_str() };
                                server_cell(ui, columns.cell(row_rect, 2), mod_name,
                                    theme::plain(mod_name, 10.8, theme::TEXT_DIM()), 10.8, mod_name);
                                let mode = server.gametype_label();
                                server_cell(ui, columns.cell(row_rect, 3), mode,
                                    theme::plain(mode, 10.8, theme::TEXT_DIM()), 10.8, mode);
                                let visible_clients = self.server_browser.visible_client_count(server);
                                let players = format!("{}/{}", visible_clients, server.max_clients);
                                let player_color = if full { theme::WARNING() } else { theme::TEXT() };
                                server_cell(ui, columns.cell(row_rect, 4), &players,
                                    theme::plain(&players, 11.5, player_color), 11.5, &players);
                                let ping = if server.ping_ms == 0 { "-".to_owned() } else { server.ping_ms.to_string() };
                                let ping_color = match server.ping_ms {
                                    0 => theme::TEXT_FAINT(),
                                    1..=79 => egui::Color32::from_rgb(0x83, 0xD6, 0x8A),
                                    80..=149 => theme::TEXT(),
                                    150..=249 => theme::WARNING(),
                                    _ => theme::DANGER(),
                                };
                                server_cell(ui, columns.cell(row_rect, 5), &ping,
                                    theme::plain(&ping, 11.5, ping_color), 11.5, &ping);
                                let (visited, hover) = self.server_browser.history_last_visited(server.address)
                                    .map(|unix_ms| (
                                        crate::chat_log::format_local_date(unix_ms),
                                        format!("Last visited {}", crate::chat_log::format_local_datetime_minute(unix_ms)),
                                    )).unwrap_or_else(|| (
                                        "-".to_owned(), "This server has not been visited yet.".to_owned(),
                                    ));
                                server_cell(ui, columns.cell(row_rect, 6), &visited,
                                    theme::plain(&visited, 10.7, theme::TEXT_DIM()), 10.7, &hover);
                                if response.clicked() {
                                    selected_after = Some(server.address);
                                }
                                if response.double_clicked() {
                                    connect_after = Some(server.address);
                                }
                                if response.secondary_clicked() {
                                    selected_after = Some(server.address);
                                }
                                response.context_menu(|ui| {
                                    ui.set_min_width(190.0);
                                    if ui.button("Connect").clicked() {
                                        connect_after = Some(server.address);
                                        ui.close();
                                    }
                                    if full {
                                        let label = if autojoining {
                                            "Cancel autojoin"
                                        } else {
                                            "Autojoin when slot opens"
                                        };
                                        if ui.button(label).clicked() {
                                            autojoin_after = Some(server.address);
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if ui
                                        .button(if favorite { "Remove favorite" } else { "Add favorite" })
                                        .clicked()
                                    {
                                        favorite_after = Some(server.address);
                                        ui.close();
                                    }
                                    if ui.button("Refresh this server").clicked() {
                                        refresh_one_after = Some(server.address);
                                        ui.close();
                                    }
                                    if ui.button("Get server info").clicked() {
                                        get_server_info_after = Some(server.address);
                                        ui.close();
                                    }
                                    if ui.button("Copy IP").clicked() {
                                        ui.ctx().copy_text(server.address.ip().to_string());
                                        ui.close();
                                    }
                                    if ui.button("Copy address").clicked() {
                                        ui.ctx().copy_text(server.address.to_string());
                                        ui.close();
                                    }
                                });
                            }
                        });
                },
            );

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("DIRECT CONNECT", 11.5, theme::TEXT_FAINT()));
            let response = ui.add_sized(
                [(ui.available_width() - 105.0).clamp(100.0, 300.0), 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.direct_connect)
                    .hint_text("hostname or address[:port]"),
            );
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if (theme::primary_button(ui, "CONNECT").clicked() || enter)
                && !self.server_browser.direct_connect.trim().is_empty()
            {
                let target = self.server_browser.direct_connect.trim().to_owned();
                self.connect_to_server(&target);
            }
        });
        });

        let detail_region = workspace.detail.shrink2(egui::vec2(12.0, 11.0));
        browser_region(ui, detail_region, "server_detail_region", |ui| {
            egui::ScrollArea::vertical()
                .id_salt("jka_server_detail_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    theme::section(ui, "SERVER DETAILS", "Select a server to inspect it.");
                    let Some(address) = self.server_browser.selected else {
                        theme::label(ui, theme::plain("No server selected.", 12.5, theme::TEXT_FAINT()));
                        return;
                    };
                    let server = self.server_browser.servers.get(&address).cloned()
                        .unwrap_or_else(|| server_browser::ServerEntry::placeholder(address));
                    let hostname = if server.hostname.is_empty() {
                        address.to_string()
                    } else {
                        server.hostname.clone()
                    };
                    let hostname_width = ui.available_width();
                    let hostname_response = ui.add_sized(
                        [hostname_width, 22.0],
                        egui::Label::new(jka_colored_text(&hostname, 17.0, theme::TEXT()))
                            .truncate(),
                    );
                    hover_if_truncated(
                        ui,
                        hostname_response,
                        &hostname,
                        crate::logging::strip_jka_colors(&hostname),
                        17.0,
                        0.0,
                    );
                    theme::label(ui, theme::plain(&address.to_string(), 11.5, theme::TEXT_FAINT()));
                    ui.add_space(8.0);
                    theme::label(ui, theme::plain(&format!("Map: {}", if server.map.is_empty() { "-" } else { &server.map }), 12.0, theme::TEXT_DIM()));
                    theme::label(ui, theme::plain(&format!("Mode: {}", server.gametype_label()), 12.0, theme::TEXT_DIM()));
                    let visible_clients = self.server_browser.visible_client_count(&server);
                    let (_, humans, bots, connecting) = self.server_browser.player_breakdown(&server);
                    let connecting_hidden = if self.server_browser.hide_connecting { connecting } else { 0 };
                    let mut composition_parts = Vec::new();
                    if self.server_browser.hide_bots && bots > 0 {
                        composition_parts.push(format!("{bots} bots hidden"));
                    } else if bots > 0 {
                        composition_parts.push(format!("{humans} humans + {bots} bots"));
                    }
                    if connecting_hidden > 0 {
                        composition_parts.push(format!("{connecting_hidden} connecting hidden"));
                    }
                    let composition = if composition_parts.is_empty() {
                        String::new()
                    } else {
                        format!("  •  {}", composition_parts.join("  •  "))
                    };
                    theme::label(ui, theme::plain(&format!("Players: {}/{}{}  •  Ping: {} ms", visible_clients, server.max_clients, composition, server.ping_ms), 12.0, theme::TEXT_DIM()));
                    theme::label(ui, theme::plain(&format!("Game: {}  •  Protocol: {}", if server.game.is_empty() { "base" } else { &server.game }, server.protocol), 12.0, theme::TEXT_DIM()));
                    if let Some(last_visited) = self.server_browser.history_last_visited(address) {
                        theme::label(
                            ui,
                            theme::plain(
                                &format!(
                                    "Last visited: {}",
                                    crate::chat_log::format_local_datetime_minute(last_visited)
                                ),
                                12.0,
                                theme::TEXT_DIM(),
                            ),
                        );
                    }
                    if server.need_password {
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            theme::label(ui, theme::plain("PASSWORD", 11.0, theme::TEXT_FAINT()));
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
                        let full = server.max_clients > 0 && server.clients >= server.max_clients;
                        if full {
                            let autojoining = self
                                .server_browser
                                .autojoin
                                .is_some_and(|(_, autojoin_address)| autojoin_address == address);
                            let label = if autojoining {
                                "CANCEL AUTOJOIN"
                            } else {
                                "AUTOJOIN"
                            };
                            if theme::ghost_button(ui, label)
                                .on_hover_text(if autojoining {
                                    "Stop waiting for a free slot on this server."
                                } else {
                                    "Poll this full server and connect automatically when a slot opens."
                                })
                                .clicked()
                            {
                                autojoin_after = Some(address);
                            }
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
                            theme::label(ui, theme::plain(&format!("Server: {version}"), 11.2, theme::TEXT_FAINT()));
                            ui.add_space(4.0);
                        }
                        let visible_players: Vec<_> = status
                            .players
                            .iter()
                            .filter(|player| {
                                (!self.server_browser.hide_bots || player.ping != 0)
                                    && (!self.server_browser.hide_connecting || player.ping != 999)
                            })
                            .collect();
                        if visible_players.is_empty() {
                            let text = if !status.players.is_empty()
                                && (self.server_browser.hide_bots
                                    || self.server_browser.hide_connecting)
                            {
                                "No player rows remain after the active player filters."
                            } else {
                                "No player rows returned."
                            };
                            theme::label(ui, theme::plain(text, 11.5, theme::TEXT_FAINT()));
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("jka_server_player_list")
                                .max_height(ui.available_height().max(180.0))
                                .show(ui, |ui| {
                                    for player in visible_players {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [72.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4} ms", player.ping),
                                                    11.5,
                                                    theme::TEXT_DIM(),
                                                )),
                                            );
                                            ui.add_sized(
                                                [42.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4}", player.score),
                                                    11.5,
                                                    theme::TEXT_DIM(),
                                                )),
                                            );
                                            let player_name_width = ui.available_width();
                                            let name_response = ui.add_sized(
                                                [player_name_width, 20.0],
                                                egui::Label::new(jka_colored_text(
                                                    &player.name,
                                                    11.5,
                                                    theme::TEXT(),
                                                ))
                                                .truncate(),
                                            );
                                            hover_if_truncated(
                                                ui,
                                                name_response,
                                                &player.name,
                                                crate::logging::strip_jka_colors(&player.name),
                                                11.5,
                                                0.0,
                                            );
                                        });
                                    }
                                });
                        }
                    } else {
                        theme::label(ui, theme::plain("Querying status…", 11.5, theme::TEXT_FAINT()));
                    }
                });
        });

        if let Some(address) = selected_after {
            self.select_browser_server(address);
        }
        if let Some(address) = get_server_info_after {
            if self
                .server_browser_tx
                .send(BrowserCommand::GetServerInfo(address))
                .is_ok()
            {
                self.server_browser.status_text =
                    format!("Querying full getstatus from {address}…");
            } else {
                self.server_browser.status_text =
                    "Server browser worker is unavailable".to_owned();
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = refresh_one_after {
            if self
                .server_browser_tx
                .send(BrowserCommand::RefreshServer {
                    source: self.server_browser.source,
                    address,
                    quiet: false,
                })
                .is_ok()
            {
                let text = format!("Refreshing {address}…");
                self.server_browser
                    .source_status
                    .insert(self.server_browser.source, text.clone());
                self.server_browser.status_text = text;
            } else {
                self.server_browser.status_text = "Server browser worker is unavailable".to_owned();
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = favorite_after {
            match self.server_browser.toggle_favorite(address) {
                Ok(true) => self.server_browser.status_text = "Added favorite".to_owned(),
                Ok(false) => self.server_browser.status_text = "Removed favorite".to_owned(),
                Err(error) => self.server_browser.status_text = error,
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = autojoin_after {
            if self
                .server_browser
                .autojoin
                .is_some_and(|(_, autojoin_address)| autojoin_address == address)
            {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
                self.server_browser.status_text = "Autojoin cancelled".to_owned();
                self.server_browser.source_status.insert(
                    self.server_browser.source,
                    self.server_browser.status_text.clone(),
                );
            } else {
                self.server_browser.autojoin = Some((self.server_browser.source, address));
                self.server_browser.autojoin_last_query = None;
                self.server_browser.status_text =
                    format!("Autojoin: waiting for a slot on {address}…");
                self.server_browser.source_status.insert(
                    self.server_browser.source,
                    self.server_browser.status_text.clone(),
                );
            }
            self.egui_repaint_requested = true;
        }
        if let Some(address) = connect_after {
            if self
                .server_browser
                .autojoin
                .is_some_and(|(_, autojoin_address)| autojoin_address == address)
            {
                self.server_browser.autojoin = None;
                self.server_browser.autojoin_last_query = None;
            }
            self.connect_browser_server(address);
            return;
        }

    }
}

#[cfg(test)]
mod mod_filter_color_tests {
    use super::*;

    #[test]
    fn mod_filter_edit_color_follows_comma_delimited_sign() {
        let green = egui::Color32::from_rgb(110, 220, 145);
        let red = egui::Color32::from_rgb(245, 112, 112);
        let comma = egui::Color32::from_gray(160);
        let job = super::mod_filter_layout(
            "base,-MBII,jaPRO, -MBIIOpenBeta",
            egui::FontId::proportional(14.0),
        );
        assert_eq!(job.text, "base,-MBII,jaPRO, -MBIIOpenBeta");
        let colors: Vec<_> = job.sections.iter().map(|section| section.format.color).collect();
        assert_eq!(colors, vec![green, comma, red, comma, green, comma, red]);
    }
}
