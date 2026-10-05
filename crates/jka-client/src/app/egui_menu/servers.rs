//! Servers.
use crate::app::egui_menu::{
    jka_colored_text, mpsc, refresh_icon, server_browser, theme, App, BrowserCommand, Duration,
    Instant, ServerSource,
};

impl App {
    pub(in crate::app::egui_menu) fn poll_server_browser_events(&mut self) {
        let mut changed = false;
        loop {
            match self.server_browser_rx.try_recv() {
                Ok(event) => {
                    self.server_browser.handle_event(event);
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
        self.connect_to_server(&address.to_string());
    }

    pub(in crate::app::egui_menu) fn server_browser_sort_header(
        &mut self,
        ui: &mut egui::Ui,
        sort: server_browser::BrowserSort,
        width: f32,
    ) {
        let selected = self.server_browser.sort == sort;
        let label = sort.label();
        let color = if selected {
            theme::ACCENT
        } else {
            theme::TEXT_DIM
        };
        let response = ui.add_sized(
            [width, 20.0],
            egui::Label::new(theme::plain(label, 10.5, color)).sense(egui::Sense::click()),
        );
        if selected {
            // Do not rely on a font glyph for the sort arrow. The bundled UI
            // font does not contain U+2191/U+2193 on some installs, which made
            // the old arrow render as the familiar missing-glyph square.
            let center = egui::pos2(response.rect.right() - 7.0, response.rect.center().y);
            let points = if self.server_browser.sort_ascending {
                vec![
                    center + egui::vec2(-3.5, 2.0),
                    center + egui::vec2(3.5, 2.0),
                    center + egui::vec2(0.0, -2.5),
                ]
            } else {
                vec![
                    center + egui::vec2(-3.5, -2.0),
                    center + egui::vec2(3.5, -2.0),
                    center + egui::vec2(0.0, 2.5),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                theme::ACCENT,
                egui::Stroke::NONE,
            ));
        }
        if response.clicked() {
            if selected {
                self.server_browser.sort_ascending = !self.server_browser.sort_ascending;
            } else {
                self.server_browser.sort = sort;
                self.server_browser.sort_ascending = matches!(
                    sort,
                    server_browser::BrowserSort::Ping
                        | server_browser::BrowserSort::Name
                        | server_browser::BrowserSort::Map
                        | server_browser::BrowserSort::Gametype
                );
            }
            self.egui_repaint_requested = true;
        }
        if response.hovered() {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(response.rect.left(), response.rect.bottom() - 1.0),
                    egui::vec2(response.rect.width(), 1.0),
                ),
                egui::CornerRadius::ZERO,
                if selected {
                    theme::ACCENT
                } else {
                    theme::LINE_STRONG
                },
            );
        }
        response.on_hover_text("Click to sort; click again to reverse the order.");
    }

    pub(in crate::app::egui_menu) fn egui_server_browser_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_server_browser_started();

        theme::page_title(
            ui,
            "SERVER BROWSER",
            "Jedi Academy protocol 26 discovery with asynchronous Internet, LAN, favorites, and history queries.",
        );

        // Source selector + refresh.
        ui.horizontal(|ui| {
            for source in [
                ServerSource::Internet,
                ServerSource::Lan,
                ServerSource::Favorites,
                ServerSource::History,
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
                    theme::label(ui, theme::plain("REFRESHING…", 10.5, theme::TEXT_FAINT));
                }
            });
        });
        ui.add_space(7.0);

        let enabled_masters = self
            .server_browser
            .master_servers
            .iter()
            .filter(|master| !master.trim().is_empty())
            .count();
        let mut pending_master_changes: Vec<(usize, String)> = Vec::new();
        egui::CollapsingHeader::new(format!(
            "MASTER SERVERS ({enabled_masters}/{})",
            server_browser::MAX_MASTER_SLOTS
        ))
        .default_open(false)
        .show(ui, |ui| {
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
                        theme::plain(&format!("sv_master{}", slot + 1), 10.5, theme::TEXT_DIM),
                    );
                    let edit = ui.add_sized(
                        [320.0, 22.0],
                        egui::TextEdit::singleline(&mut draft).hint_text(if slot < 3 {
                            server_browser::DEFAULT_MASTER_CVARS[slot]
                        } else {
                            "master.example.org[:port]"
                        }),
                    );
                    theme::label(ui, theme::plain(slot_labels[slot], 10.0, theme::TEXT_FAINT));
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
        for (slot, value) in pending_master_changes {
            let cvar = format!("sv_master{}", slot + 1);
            if let Err(error) = self.set_console_cvar(&cvar, &value) {
                self.server_browser.status_text = error;
            }
        }
        ui.add_space(7.0);

        // Search and practical filters are all local and instantaneous.
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("SEARCH", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [260.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.search)
                    .hint_text("name, map, mod, address"),
            );
            if response.changed() {
                self.egui_repaint_requested = true;
            }
            ui.add_space(10.0);
            if theme::chip(ui, "HIDE EMPTY", self.server_browser.hide_empty).clicked() {
                self.server_browser.hide_empty = !self.server_browser.hide_empty;
            }
            if theme::chip(ui, "HIDE FULL", self.server_browser.hide_full).clicked() {
                self.server_browser.hide_full = !self.server_browser.hide_full;
            }
            if theme::chip(ui, "HIDE BOTS", self.server_browser.hide_bots)
                .on_hover_text("Exclude bots from player counts and hide 0-ping rows in the selected server's player list.")
                .clicked()
            {
                self.server_browser.hide_bots = !self.server_browser.hide_bots;
            }
            ui.add_space(10.0);
            theme::label(ui, theme::plain("MAX PING", 11.5, theme::TEXT_FAINT));
            ui.add(
                egui::DragValue::new(&mut self.server_browser.max_ping)
                    .range(0..=999)
                    .suffix(" ms")
                    .speed(5),
            )
            .on_hover_text("0 disables the ping filter");
        });

        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("MODS", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [360.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.mod_filter)
                    .hint_text("comma-separated fs_game values, e.g. base,japro,japlus"),
            );
            if response.changed() {
                self.egui_repaint_requested = true;
            }
            response.on_hover_text("Case-insensitive exact matches. 'base' matches servers with no mod game directory.");
        });
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("PLAYERS", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [360.0, 24.0],
                egui::TextEdit::singleline(&mut self.server_browser.player_search)
                    .hint_text("names/substrings, comma-separated"),
            );
            if response.changed() {
                // Keep cached status rows for instant filtering, but force a
                // fresh getstatus batch immediately so joins/leaves are picked
                // up without the user having to Refresh the whole browser.
                self.server_browser.player_search_last_query = None;
                self.egui_repaint_requested = true;
            }
            response.on_hover_text(
                "Comma-separated player names. Matches any term, ignoring JKA color codes and case.",
            );
            if theme::chip(
                ui,
                "EXACT MATCH",
                self.server_browser.player_exact_match,
            )
            .on_hover_text("Match the whole color-stripped player name instead of a substring.")
            .clicked()
            {
                self.server_browser.player_exact_match = !self.server_browser.player_exact_match;
            }
        });
        ui.add_space(7.0);

        let search = self.server_browser.search.trim().to_ascii_lowercase();
        let player_terms: Vec<String> = self
            .server_browser
            .player_search
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        let mod_filters: Vec<String> = self
            .server_browser
            .mod_filter
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        let mut servers: Vec<_> = self
            .server_browser
            .servers_for_current_source()
            .into_iter()
            .filter(|server| {
                let visible_clients = if self.server_browser.hide_bots {
                    server.humans
                } else {
                    server.clients
                };
                if self.server_browser.hide_empty && visible_clients == 0 {
                    return false;
                }
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
                if !mod_filters.is_empty() {
                    let game = if server.game.trim().is_empty() {
                        "base".to_owned()
                    } else {
                        server.game.trim().to_ascii_lowercase()
                    };
                    if !mod_filters.iter().any(|filter| filter == &game) {
                        return false;
                    }
                }
                if search.is_empty() {
                    return true;
                }
                let hostname =
                    crate::logging::strip_jka_colors(&server.hostname).to_ascii_lowercase();
                hostname.contains(&search)
                    || server.map.to_ascii_lowercase().contains(&search)
                    || server.game.to_ascii_lowercase().contains(&search)
                    || server.address.to_string().contains(&search)
                    || server
                        .gametype_label()
                        .to_ascii_lowercase()
                        .contains(&search)
            })
            .collect();

        if !player_terms.is_empty() {
            let missing_status = servers.iter().any(|server| {
                !self
                    .server_browser
                    .status_cache
                    .contains_key(&server.address)
            });
            let due = (missing_status
                || self
                    .server_browser
                    .player_search_last_query
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(4)))
                && self.server_browser.status_pending.is_empty();
            if due {
                let addresses: Vec<_> = servers.iter().map(|server| server.address).collect();
                if !addresses.is_empty() {
                    if self
                        .server_browser_tx
                        .send(BrowserCommand::QueryStatusBatch(addresses.clone()))
                        .is_ok()
                    {
                        self.server_browser.status_pending.extend(addresses);
                        self.server_browser.player_search_last_query = Some(Instant::now());
                    } else {
                        self.server_browser.status_text =
                            "Server browser worker is unavailable".to_owned();
                    }
                }
            }

            let exact = self.server_browser.player_exact_match;
            let hide_bots = self.server_browser.hide_bots;
            servers.retain(|server| {
                let Some(status) = self.server_browser.status_cache.get(&server.address) else {
                    // Optimistically keep rows until the first status batch has
                    // had a chance to answer; the status-batch completion caches
                    // an empty result for non-responders.
                    return true;
                };
                status.players.iter().any(|player| {
                    if hide_bots && player.ping == 0 {
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
            let order = match sort {
                server_browser::BrowserSort::Ping => {
                    let ap = if a.ping_ms == 0 { u32::MAX } else { a.ping_ms };
                    let bp = if b.ping_ms == 0 { u32::MAX } else { b.ping_ms };
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Players => {
                    let ap = if self.server_browser.hide_bots {
                        a.humans
                    } else {
                        a.clients
                    };
                    let bp = if self.server_browser.hide_bots {
                        b.humans
                    } else {
                        b.clients
                    };
                    ap.cmp(&bp)
                }
                server_browser::BrowserSort::Name => crate::logging::strip_jka_colors(&a.hostname)
                    .to_ascii_lowercase()
                    .cmp(&crate::logging::strip_jka_colors(&b.hostname).to_ascii_lowercase()),
                server_browser::BrowserSort::Map => {
                    a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase())
                }
                server_browser::BrowserSort::Gametype => a.gametype.cmp(&b.gametype),
            };
            if self.server_browser.sort_ascending {
                order
            } else {
                order.reverse()
            }
        });

        let browser_height = (ui.available_height() - 78.0).max(220.0);
        let mut selected_after = None;
        let mut connect_after = None;
        let mut refresh_one_after = None;
        let mut favorite_after = None;
        let mut autojoin_after = None;
        ui.horizontal(|ui| {
            let list_width = (ui.available_width() * 0.67).max(560.0);
            // The name column absorbs whatever the fixed columns leave, so the
            // table always spans its pane instead of hugging the left edge.
            let name_w = (list_width - 299.0 - 32.0 - 24.0).max(250.0);
            ui.allocate_ui_with_layout(
                egui::vec2(list_width, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(
                        ui,
                        self.server_browser.source.label(),
                        &format!("{} visible — {}", servers.len(), self.server_browser.status_text),
                    );
                    ui.horizontal(|ui| {
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Name, name_w);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Map, 118.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Gametype, 82.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Players, 55.0);
                        self.server_browser_sort_header(ui, server_browser::BrowserSort::Ping, 44.0);
                    });
                    ui.separator();
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
                                let response = ui
                                    .horizontal(|ui| {
                                        let label = if server.need_password {
                                            format!("🔒 {hostname}")
                                        } else {
                                            hostname
                                        };
                                        let row_text = theme::TEXT;
                                        let mut response = ui.add_sized(
                                            [name_w, 24.0],
                                            egui::Button::selectable(
                                                selected,
                                                jka_colored_text(&label, 11.5, theme::TEXT),
                                            ),
                                        );
                                        response |= ui.add_sized(
                                            [118.0, 24.0],
                                            egui::Label::new(theme::plain(&server.map, 11.5, row_text))
                                                .sense(egui::Sense::click()),
                                        );
                                        response |= ui.add_sized(
                                            [82.0, 24.0],
                                            egui::Label::new(theme::plain(server.gametype_label(), 10.8, row_text))
                                                .sense(egui::Sense::click()),
                                        );
                                        let visible_clients = if self.server_browser.hide_bots { server.humans } else { server.clients };
                                        let player_color = if full { theme::WARNING } else { row_text };
                                        response |= ui.add_sized(
                                            [55.0, 24.0],
                                            egui::Label::new(theme::plain(&format!("{}/{}", visible_clients, server.max_clients), 11.5, player_color))
                                                .sense(egui::Sense::click()),
                                        );
                                        let ping = if server.ping_ms == 0 { "—".to_owned() } else { server.ping_ms.to_string() };
                                        let ping_color = match server.ping_ms {
                                            0 => theme::TEXT_FAINT,
                                            1..=79 => egui::Color32::from_rgb(0x83, 0xD6, 0x8A),
                                            80..=149 => theme::TEXT,
                                            150..=249 => theme::WARNING,
                                            _ => theme::DANGER,
                                        };
                                        response |= ui.add_sized(
                                            [44.0, 24.0],
                                            egui::Label::new(theme::plain(&ping, 11.5, ping_color))
                                                .sense(egui::Sense::click()),
                                        );
                                        response
                                    })
                                    .inner;
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

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SERVER DETAILS", "Select a server to inspect it.");
                    let Some(address) = self.server_browser.selected else {
                        theme::label(ui, theme::plain("No server selected.", 12.5, theme::TEXT_FAINT));
                        return;
                    };
                    let server = self.server_browser.servers.get(&address).cloned()
                        .unwrap_or_else(|| server_browser::ServerEntry::placeholder(address));
                    let hostname = if server.hostname.is_empty() {
                        address.to_string()
                    } else {
                        server.hostname.clone()
                    };
                    ui.add(egui::Label::new(jka_colored_text(&hostname, 17.0, theme::TEXT)));
                    theme::label(ui, theme::plain(&address.to_string(), 11.5, theme::TEXT_FAINT));
                    ui.add_space(8.0);
                    theme::label(ui, theme::plain(&format!("Map: {}", if server.map.is_empty() { "—" } else { &server.map }), 12.0, theme::TEXT_DIM));
                    theme::label(ui, theme::plain(&format!("Mode: {}", server.gametype_label()), 12.0, theme::TEXT_DIM));
                    let visible_clients = if self.server_browser.hide_bots { server.humans } else { server.clients };
                    let composition = if self.server_browser.hide_bots && server.bots > 0 {
                        format!("  •  {} bots hidden", server.bots)
                    } else if server.bots > 0 {
                        format!("  •  {} humans + {} bots", server.humans, server.bots)
                    } else {
                        String::new()
                    };
                    theme::label(ui, theme::plain(&format!("Players: {}/{}{}  •  Ping: {} ms", visible_clients, server.max_clients, composition, server.ping_ms), 12.0, theme::TEXT_DIM));
                    theme::label(ui, theme::plain(&format!("Game: {}  •  Protocol: {}", if server.game.is_empty() { "base" } else { &server.game }, server.protocol), 12.0, theme::TEXT_DIM));
                    if server.need_password {
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            theme::label(ui, theme::plain("PASSWORD", 11.0, theme::TEXT_FAINT));
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
                            theme::label(ui, theme::plain(&format!("Server: {version}"), 11.2, theme::TEXT_FAINT));
                            ui.add_space(4.0);
                        }
                        let visible_players: Vec<_> = status
                            .players
                            .iter()
                            .filter(|player| !self.server_browser.hide_bots || player.ping != 0)
                            .collect();
                        if visible_players.is_empty() {
                            let text = if self.server_browser.hide_bots && !status.players.is_empty() {
                                "No human player rows returned (0-ping bot rows hidden)."
                            } else {
                                "No player rows returned."
                            };
                            theme::label(ui, theme::plain(text, 11.5, theme::TEXT_FAINT));
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("jka_server_player_list")
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    for player in visible_players {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [72.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4} ms", player.ping),
                                                    11.5,
                                                    theme::TEXT_DIM,
                                                )),
                                            );
                                            ui.add_sized(
                                                [42.0, 20.0],
                                                egui::Label::new(theme::plain(
                                                    &format!("{:>4}", player.score),
                                                    11.5,
                                                    theme::TEXT_DIM,
                                                )),
                                            );
                                            ui.add(egui::Label::new(jka_colored_text(
                                                &player.name,
                                                11.5,
                                                theme::TEXT,
                                            )));
                                        });
                                    }
                                });
                        }
                    } else {
                        theme::label(ui, theme::plain("Querying status…", 11.5, theme::TEXT_FAINT));
                    }
                },
            );
        });

        if let Some(address) = selected_after {
            self.select_browser_server(address);
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

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain("DIRECT CONNECT", 11.5, theme::TEXT_FAINT));
            let response = ui.add_sized(
                [300.0, 24.0],
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
    }
}
