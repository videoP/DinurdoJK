//! Solo.
use crate::app::egui_menu::{filesystem_refresh_icon, rail_item, scene, theme, ui_catalog, App};

impl App {
    /// The menu lists the picked game; in a session, console map completion
    /// and callvote need the maps of the game actually mounted.
    fn solo_listing_game(&self) -> Option<u8> {
        if self.front_end { self.solo_asset_game } else { self.fs_roots.active_jk2 }
    }

    pub(in crate::app) fn ensure_solo_map_catalog(&mut self) {
        let jk2 = self.solo_listing_game();
        if self.solo_catalog_loaded && self.solo_catalog_for == jk2 {
            return;
        }

        self.solo_catalog_loaded = true;
        self.solo_catalog_for = jk2;
        self.solo_maps.clear();
        self.solo_catalog_error = None;
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
        // A JK2 install without its retail PK3s has no JK2 namespace to list.
        if let Some(version) = jk2 {
            if let Some(missing) = jka_assets::pk3::missing_jk2_stock_archive(&self.base, &self.fs_roots, version) {
                self.solo_catalog_error = Some(format!(
                    "JK2 1.0{version} needs {missing} in fs_jk2basepath/base or fs_jk2cdpath/base."
                ));
                return;
            }
        }
        self.ui_catalog.pending.solo_maps = true;
        self.ui_catalog.request_as(
            &self.base,
            self.game.as_deref(),
            jk2,
            ui_catalog::CatalogRequest::SoloMaps { jk2 },
        );
    }

    /// Switch the page's asset game; the list and levelshot rescan lazily.
    fn select_solo_asset_game(&mut self, jk2: Option<u8>) {
        if self.solo_asset_game == jk2 {
            return;
        }
        self.solo_asset_game = jk2;
        self.solo_map_selected = 0;
        self.egui_repaint_requested = true;
    }

    pub(in crate::app::egui_menu) fn ensure_solo_levelshot_texture(&mut self) {
        let Some(entry) = self.solo_maps.get(self.solo_map_selected) else {
            self.solo_levelshot_texture = None;
            self.solo_levelshot_texture_map = None;
            return;
        };
        let map_name = entry.map_name.clone();
        if self.solo_levelshot_texture_map.as_deref() == Some(map_name.as_str()) {
            return;
        }

        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = Some(map_name.clone());
        self.ui_catalog.request_as(
            &self.base,
            self.game.as_deref(),
            self.solo_catalog_for,
            ui_catalog::CatalogRequest::Levelshot {
                source: false,
                map_name,
            },
        );
    }

    pub(in crate::app::egui_menu) fn egui_solo_game_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "SOLO GAME",
            "Compiled BSPs discovered from loose maps/ files and mounted PK3s of the selected game.",
        );
        // Fixed-height row: a bare `with_layout` would hand the icon the whole
        // remaining page height and center it vertically, collapsing the list.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 28.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if filesystem_refresh_icon(ui) {
                    self.restart_filesystem();
                    self.egui_repaint_requested = true;
                }
                // Right-to-left: listed in reverse so they read JKA, 1.02, 1.04.
                ui.add_space(12.0);
                let mut picked = self.solo_asset_game;
                for (jk2, label) in [(Some(4), "JK2 1.04"), (Some(2), "JK2 1.02"), (None, "JKA")] {
                    if theme::chip(ui, label, picked == jk2).clicked() {
                        picked = jk2;
                    }
                }
                self.select_solo_asset_game(picked);
            },
        );

        self.ensure_solo_map_catalog();

        if self.ui_catalog.pending.solo_maps {
            theme::label(
                ui,
                theme::plain("Scanning maps...", 12.0, theme::TEXT_FAINT()),
            );
            return;
        }
        if let Some(error) = &self.solo_catalog_error {
            theme::banner(ui, &format!("Map scan failed: {error}"), theme::WARNING());
            return;
        }
        if self.solo_maps.is_empty() {
            theme::banner(ui, "No maps/*.bsp assets were found.", theme::WARNING());
            return;
        }

        self.solo_map_selected = self.solo_map_selected.min(self.solo_maps.len() - 1);
        self.ensure_solo_levelshot_texture();
        let mut selected = self.solo_map_selected;
        let selected_name = self.solo_maps[selected].map_name.clone();

        let browser_height = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(330.0, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::vec2(330.0, browser_height));
                    theme::section(ui, "MAPS", &format!("{} found", self.solo_maps.len()));
                    let list_height = ui.available_height().max(1.0);
                    egui::ScrollArea::vertical()
                        .id_salt("jka_solo_map_list")
                        .auto_shrink([false, false])
                        .max_height(list_height)
                        .show(ui, |ui| {
                            for (index, entry) in self.solo_maps.iter().enumerate() {
                                if rail_item(ui, &entry.map_name, index == selected).clicked() {
                                    selected = index;
                                }
                            }
                        });
                },
            );

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(ui, "SELECTED MAP", "");
                    theme::glow_label(ui, &selected_name, 18.0, theme::TEXT());
                    ui.add_space(10.0);

                    let preview_w = ui.available_width().max(1.0);
                    let preview_box = egui::vec2(preview_w, 360.0);
                    if let Some(texture) = &self.solo_levelshot_texture {
                        let source = texture.size_vec2();
                        let scale = (preview_box.x / source.x.max(1.0))
                            .min(preview_box.y / source.y.max(1.0));
                        let preview_size = source * scale;
                        ui.add(egui::Image::new(texture).fit_to_exact_size(preview_size));
                    } else {
                        let placeholder = egui::vec2(preview_w, (preview_w * 0.5).min(360.0));
                        let (rect, _) = ui.allocate_exact_size(placeholder, egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL());
                        ui.painter().rect_stroke(
                            rect,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(1.0_f32, theme::LINE()),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "NO LEVELSHOT",
                            egui::FontId::proportional(13.0),
                            theme::TEXT_DISABLED(),
                        );
                    }
                    ui.add_space(14.0);
                    if theme::primary_button(ui, "LOAD MAP").clicked() {
                        let source = scene::MapSource::Bsp(selected_name.clone());
                        if let Some(version) = self.solo_asset_game {
                            self.launch_jk2_local_map(version, source);
                        } else {
                            // Leave a JK2 asset namespace before a JKA map.
                            self.select_demo_asset_game(None);
                            self.enter_local_game_dir();
                            self.request_map(source);
                        }
                    }
                },
            );
        });

        if selected != self.solo_map_selected {
            self.solo_map_selected = selected;
            self.solo_levelshot_texture = None;
            self.solo_levelshot_texture_map = None;
            self.egui_repaint_requested = true;
        }
    }
}
