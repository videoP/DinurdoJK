//! Maps.
use crate::app::egui_menu::{
    filesystem_refresh_icon, rail_item, scene, theme, ui_catalog, App, RenderCommand,
};

impl App {
    pub(in crate::app::egui_menu) fn ensure_source_map_catalog(&mut self) {
        if self.source_map_catalog_loaded {
            return;
        }
        self.source_map_catalog_loaded = true;
        self.source_maps.clear();
        self.source_map_catalog_error = None;
        self.ui_catalog.pending.source_maps = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::SourceMaps,
        );
    }

    pub(in crate::app::egui_menu) fn ensure_source_map_levelshot_texture(&mut self) {
        let Some(entry) = self.source_maps.get(self.source_map_selected) else {
            self.source_map_levelshot_texture = None;
            self.source_map_levelshot_texture_map = None;
            return;
        };
        let map_name = entry.map_name.clone();
        if self.source_map_levelshot_texture_map.as_deref() == Some(map_name.as_str()) {
            return;
        }
        self.source_map_levelshot_texture = None;
        self.source_map_levelshot_texture_map = Some(map_name.clone());
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::Levelshot {
                source: true,
                map_name,
            },
        );
    }

    pub(in crate::app::egui_menu) fn egui_map_viewer_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "MAP VIEWER", "Source .map files discovered from the active VFS; launch uses the existing direct .map path.");
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 28.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if filesystem_refresh_icon(ui) {
                    self.refresh_filesystem();
                    self.egui_repaint_requested = true;
                }
            },
        );
        self.ensure_source_map_catalog();
        if self.ui_catalog.pending.source_maps {
            theme::label(
                ui,
                theme::plain("Scanning maps...", 12.0, theme::TEXT_FAINT),
            );
            return;
        }
        if let Some(error) = &self.source_map_catalog_error {
            theme::banner(
                ui,
                &format!("Source map scan failed: {error}"),
                theme::WARNING,
            );
            return;
        }
        if self.source_maps.is_empty() {
            theme::banner(ui, "No maps/*.map assets were found.", theme::WARNING);
            return;
        }
        self.source_map_selected = self.source_map_selected.min(self.source_maps.len() - 1);
        self.ensure_source_map_levelshot_texture();
        let mut selected = self.source_map_selected;
        let selected_name = self.source_maps[selected].map_name.clone();
        let browser_height = ui.available_height().max(1.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(390.0, browser_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    theme::section(
                        ui,
                        "SOURCE MAPS",
                        &format!("{} found", self.source_maps.len()),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("jka_source_map_list")
                        .auto_shrink([false, false])
                        .max_height(ui.available_height().max(1.0))
                        .show(ui, |ui| {
                            for (index, entry) in self.source_maps.iter().enumerate() {
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
                    theme::section(ui, "SELECTED SOURCE MAP", "");
                    theme::glow_label(ui, &format!("maps/{selected_name}.map"), 18.0, theme::TEXT);
                    ui.add_space(10.0);
                    let preview_w = ui.available_width().max(1.0);
                    let preview_box = egui::vec2(preview_w, 360.0);
                    if let Some(texture) = &self.source_map_levelshot_texture {
                        let source = texture.size_vec2();
                        let scale = (preview_box.x / source.x.max(1.0))
                            .min(preview_box.y / source.y.max(1.0));
                        ui.add(egui::Image::new(texture).fit_to_exact_size(source * scale));
                    } else {
                        let placeholder = egui::vec2(preview_w, (preview_w * 0.5).min(360.0));
                        let (rect, _) = ui.allocate_exact_size(placeholder, egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, egui::CornerRadius::ZERO, theme::CONTROL);
                        ui.painter().rect_stroke(
                            rect,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(1.0_f32, theme::LINE),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "NO LEVELSHOT",
                            egui::FontId::proportional(13.0),
                            theme::TEXT_DISABLED,
                        );
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if theme::primary_button(ui, "LOAD SOURCE MAP").clicked() {
                            self.source_map_edit_on_load = false;
                            self.render_command(RenderCommand::SetAssetPreviewMode(false));
                            self.request_map(scene::MapSource::Map(selected_name.clone()));
                        }
                        if theme::ghost_button(ui, "EDIT SOURCE MAP").clicked() {
                            self.source_map_edit_on_load = true;
                            self.render_command(RenderCommand::SetAssetPreviewMode(false));
                            self.request_map(scene::MapSource::Map(selected_name.clone()));
                        }
                    });
                },
            );
        });
        if selected != self.source_map_selected {
            self.source_map_selected = selected;
            self.source_map_levelshot_texture = None;
            self.source_map_levelshot_texture_map = None;
            self.egui_repaint_requested = true;
        }
    }
}
