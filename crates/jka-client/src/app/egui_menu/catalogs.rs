//! Catalogs.
use crate::app::egui_menu::{frontend, theme, ui_catalog, App, CatalogPayload, PathBuf};

impl App {
    pub(in crate::app::egui_menu) fn ensure_asset_viewer_catalog(&mut self) {
        if self.asset_viewer_catalog_loaded {
            return;
        }
        self.asset_viewer_catalog_loaded = true;
        self.asset_viewer_entries.clear();
        self.asset_viewer_catalog_error = None;
        self.ui_catalog.pending.asset_viewer = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::AssetViewer,
        );
    }

    pub(in crate::app::egui_menu) fn ensure_screenshot_catalog(&mut self) {
        if self.screenshot_catalog_loaded || self.ui_catalog.pending.screenshots {
            return;
        }
        self.screenshot_catalog_loaded = true;
        self.screenshot_catalog_error = None;
        self.ui_catalog.pending.screenshots = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ScreenshotCatalog,
        );
    }

    pub(in crate::app::egui_menu) fn request_screenshot_preview(&mut self, path: PathBuf) {
        if self.screenshot_preview_path.as_ref() == Some(&path)
            || self.screenshot_preview_pending.as_ref() == Some(&path)
        {
            return;
        }
        self.screenshot_preview_pending = Some(path.clone());
        self.screenshot_preview_error = None;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ScreenshotImage { path },
        );
    }

    /// Install finished background catalog scans and image decodes.
    pub(in crate::app::egui_menu) fn poll_ui_catalog(&mut self) {
        let results = self.ui_catalog.drain();
        if results.is_empty() {
            return;
        }
        let ctx = self.egui_ctx.clone();
        for result in results {
            match result.payload {
                CatalogPayload::AssetViewer(result) => {
                    self.ui_catalog.pending.asset_viewer = false;
                    match result {
                        Ok(entries) => self.asset_viewer_entries = entries,
                        Err(error) => {
                            eprintln!("Could not build Asset Viewer catalog: {error}");
                            self.asset_viewer_catalog_error = Some(error);
                        }
                    }
                }
                CatalogPayload::ChatLogs(result) => {
                    self.ui_catalog.pending.chat_logs = false;
                    match result {
                        Ok(entries) => {
                            self.chat_log_browser_entries = entries;
                            if self
                                .chat_log_browser_selected
                                .as_ref()
                                .is_some_and(|selected| {
                                    !self
                                        .chat_log_browser_entries
                                        .iter()
                                        .any(|entry| &entry.path == selected)
                                })
                            {
                                self.chat_log_browser_selected = None;
                                self.chat_log_browser_detail_path = None;
                                self.chat_log_browser_detail = None;
                            }
                        }
                        Err(error) => {
                            eprintln!("Could not build Chat Logs catalog: {error}");
                            self.chat_log_browser_catalog_error = Some(error);
                        }
                    }
                }
                CatalogPayload::ChatLogDetail { path, detail } => {
                    // Selection can change while an older parse is in flight.
                    // Only the result for the currently requested path owns the
                    // pending flag or detail slot.
                    if self.chat_log_browser_detail_path.as_ref() == Some(&path) {
                        self.ui_catalog.pending.chat_log_detail = false;
                        self.chat_log_browser_detail = Some(detail);
                    }
                }
                CatalogPayload::ScreenshotCatalog(result) => {
                    self.ui_catalog.pending.screenshots = false;
                    match result {
                        Ok(entries) => {
                            self.screenshot_entries = entries;
                            self.screenshot_catalog_error = None;
                            if let Some(path) = self.startup_screenshot_select.take() {
                                if self
                                    .screenshot_entries
                                    .iter()
                                    .any(|entry| entry.path == path)
                                {
                                    self.screenshot_selected = Some(path);
                                }
                            }
                            if self.screenshot_selected.as_ref().is_none_or(|selected| {
                                !self
                                    .screenshot_entries
                                    .iter()
                                    .any(|entry| &entry.path == selected)
                            }) {
                                self.screenshot_selected = self
                                    .screenshot_entries
                                    .first()
                                    .map(|entry| entry.path.clone());
                            }
                            self.screenshot_preview_texture = None;
                            self.screenshot_preview_path = None;
                            self.screenshot_preview_pending = None;
                        }
                        Err(error) => {
                            eprintln!("Could not build Screenshot Browser catalog: {error}");
                            self.screenshot_catalog_error = Some(error);
                        }
                    }
                }
                CatalogPayload::ScreenshotImage { path, image } => {
                    if self.screenshot_preview_pending.as_ref() == Some(&path) {
                        self.screenshot_preview_pending = None;
                        match image {
                            Ok(image) => {
                                self.screenshot_preview_texture = Some(ctx.load_texture(
                                    format!("screenshot-preview:{}", path.display()),
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        image.size,
                                        &image.rgba,
                                    ),
                                    egui::TextureOptions::LINEAR,
                                ));
                                self.screenshot_preview_path = Some(path);
                                self.screenshot_preview_error = None;
                            }
                            Err(error) => {
                                self.screenshot_preview_texture = None;
                                self.screenshot_preview_path = None;
                                self.screenshot_preview_error = Some(error);
                            }
                        }
                    }
                }
                CatalogPayload::Profile(result) => {
                    self.ui_catalog.pending.profile = false;
                    self.apply_profile_catalog(result);
                }
                CatalogPayload::SoloMaps(result) => {
                    self.ui_catalog.pending.solo_maps = false;
                    match result {
                        Ok(maps) => self.solo_maps = maps,
                        Err(error) => {
                            eprintln!("Could not build Solo Game map catalog: {error}");
                            self.solo_catalog_error = Some(error);
                        }
                    }
                    self.solo_map_selected = self
                        .solo_map_selected
                        .min(self.solo_maps.len().saturating_sub(1));
                    self.solo_levelshot_texture = None;
                    self.solo_levelshot_texture_map = None;
                }
                CatalogPayload::SourceMaps(result) => {
                    self.ui_catalog.pending.source_maps = false;
                    match result {
                        Ok(maps) => self.source_maps = maps,
                        Err(error) => {
                            eprintln!("Could not build Map Viewer catalog: {error}");
                            self.source_map_catalog_error = Some(error);
                        }
                    }
                    self.source_map_selected = self
                        .source_map_selected
                        .min(self.source_maps.len().saturating_sub(1));
                    self.source_map_levelshot_texture = None;
                    self.source_map_levelshot_texture_map = None;
                }
                CatalogPayload::Levelshot {
                    source,
                    map_name,
                    image,
                } => {
                    let (wanted, slot, prefix) = if source {
                        (
                            &self.source_map_levelshot_texture_map,
                            &mut self.source_map_levelshot_texture,
                            "source-levelshot",
                        )
                    } else {
                        (
                            &self.solo_levelshot_texture_map,
                            &mut self.solo_levelshot_texture,
                            "levelshot",
                        )
                    };
                    // The selection may have moved on while this was decoding.
                    if wanted.as_deref() == Some(map_name.as_str()) {
                        *slot = image.map(|image| {
                            ctx.load_texture(
                                format!("{prefix}:{map_name}"),
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            )
                        });
                    }
                }
                CatalogPayload::ProfileIcon { key, image } => {
                    self.ui_catalog.icons_inflight.remove(&key);
                    match image {
                        Some(image) => {
                            let texture = ctx.load_texture(
                                format!("profile-model-icon:{key}"),
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            );
                            self.profile_model_icon_textures.insert(key, texture);
                        }
                        None => {
                            self.ui_catalog.icons_missing.insert(key);
                        }
                    }
                }
                CatalogPayload::CrosshairImage { index, image } => {
                    let key = Self::crosshair_image_key(index);
                    self.ui_catalog.icons_inflight.remove(&key);
                    match image {
                        Some(image) => {
                            let texture = ctx.load_texture(
                                key,
                                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                                egui::TextureOptions::LINEAR,
                            );
                            self.crosshair_image_textures.insert(index, texture);
                        }
                        None => {
                            self.ui_catalog.icons_missing.insert(key);
                        }
                    }
                }
            }
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app::egui_menu) fn ensure_chat_log_browser_catalog(&mut self) {
        if self.chat_log_browser_catalog_loaded {
            return;
        }
        self.chat_log_browser_catalog_loaded = true;
        self.chat_log_browser_entries.clear();
        self.chat_log_browser_catalog_error = None;
        self.ui_catalog.pending.chat_logs = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ChatLogs,
        );
    }

    pub(in crate::app::egui_menu) fn refresh_chat_log_browser_catalog(&mut self) {
        self.chat_log_browser_catalog_loaded = false;
        self.chat_log_browser_catalog_error = None;
        self.chat_log_browser_detail_path = None;
        self.chat_log_browser_detail = None;
        self.ensure_chat_log_browser_catalog();
    }

    pub(in crate::app::egui_menu) fn ensure_chat_log_browser_detail(&mut self) {
        let Some(path) = self.chat_log_browser_selected.clone() else {
            self.chat_log_browser_detail_path = None;
            self.chat_log_browser_detail = None;
            return;
        };
        if self.chat_log_browser_detail_path.as_ref() == Some(&path) {
            return;
        }
        self.chat_log_browser_detail_path = Some(path.clone());
        self.chat_log_browser_detail = None;
        self.ui_catalog.pending.chat_log_detail = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ChatLogDetail { path },
        );
    }

    pub(in crate::app::egui_menu) fn ensure_asset_viewer_detail(&mut self) {
        let Some(selected_id) = self.asset_viewer_selected.clone() else {
            self.asset_viewer_detail_path = None;
            self.asset_viewer_detail = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
            return;
        };
        if self.asset_viewer_detail_path.as_deref() == Some(selected_id.as_str()) {
            self.update_asset_preview_content();
            return;
        }
        let Some(entry) = self
            .asset_viewer_entries
            .iter()
            .find(|entry| entry.id == selected_id)
            .cloned()
        else {
            self.asset_viewer_detail_path = None;
            self.asset_viewer_detail = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
            return;
        };
        self.asset_viewer_detail_path = Some(selected_id);
        let selected_shader = entry.shader_name.clone();
        let detail = frontend::inspect_asset(&self.base, self.game.as_deref(), &entry);
        self.asset_viewer_shader_name = selected_shader.or_else(|| {
            detail
                .as_ref()
                .ok()
                .and_then(|detail| detail.shader_names.first().cloned())
        });
        self.asset_viewer_detail = Some(detail);
        self.clear_asset_preview_runtime();
        self.update_asset_preview_content();
    }

    pub(in crate::app::egui_menu) fn browser_workspace_widths(total_width: f32) -> (f32, f32, f32) {
        let gap = 8.0_f32;
        let browser_width = (total_width * 0.21)
            .clamp(250.0, 320.0)
            .min(total_width * 0.30);
        let inspector_width = (total_width * 0.25)
            .clamp(280.0, 380.0)
            .min(total_width * 0.32);
        let preview_width = (total_width - browser_width - inspector_width - gap * 2.0).max(1.0);
        (browser_width, preview_width, inspector_width)
    }

    pub(in crate::app::egui_menu) fn screenshot_metadata_row(
        ui: &mut egui::Ui,
        label: &str,
        value: impl AsRef<str>,
    ) {
        ui.horizontal_wrapped(|ui| {
            theme::label(ui, theme::plain(label, 8.5, theme::TEXT_FAINT));
            theme::label(ui, theme::plain(value.as_ref(), 9.5, theme::TEXT));
        });
    }
}
