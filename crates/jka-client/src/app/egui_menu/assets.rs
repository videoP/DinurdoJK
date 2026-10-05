//! Assets.
use crate::app::egui_menu::{
    asset_size_label, filesystem_refresh_icon, theme, App, AssetFilter, AssetKind, BTreeMap,
};

impl App {
    pub(in crate::app::egui_menu) fn egui_asset_viewer_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_asset_viewer_catalog();
        if self.ui_catalog.pending.asset_viewer {
            theme::label(
                ui,
                theme::plain("Scanning assets...", 12.0, theme::TEXT_FAINT),
            );
            return;
        }
        if let Some(error) = &self.asset_viewer_catalog_error {
            theme::banner(ui, &format!("Asset scan failed: {error}"), theme::WARNING);
            return;
        }

        let mut shader_files = self
            .asset_viewer_entries
            .iter()
            .filter(|entry| entry.kind == AssetKind::Shader)
            .map(|entry| entry.qpath.clone())
            .collect::<Vec<_>>();
        shader_files.sort();
        shader_files.dedup();

        let search = self.asset_viewer_search.trim().to_ascii_lowercase();
        let folder = self
            .asset_viewer_folder
            .trim()
            .replace('\\', "/")
            .trim_matches('/')
            .to_ascii_lowercase();
        let shader_file = self.asset_viewer_shader_file.as_str();

        // Build the folder picker from the catalog we already scanned. Every
        // ancestor is included, so typing `ships` can offer both `effects/ships`
        // and deeper folders even when no asset lives directly in the parent.
        // Counts are for the current asset type / shader-file scope and include
        // descendants, which makes the suggestions useful rather than decorative.
        let mut folder_counts = BTreeMap::<String, usize>::new();
        for entry in self.asset_viewer_entries.iter().filter(|entry| {
            self.asset_viewer_filter.matches(entry.kind)
                && (self.asset_viewer_filter != AssetFilter::Shader
                    || shader_file.is_empty()
                    || entry.qpath == shader_file)
        }) {
            let mut ancestor = String::new();
            for part in entry
                .folder
                .replace('\\', "/")
                .split('/')
                .filter(|part| !part.is_empty())
            {
                if !ancestor.is_empty() {
                    ancestor.push('/');
                }
                ancestor.push_str(part);
                *folder_counts.entry(ancestor.clone()).or_default() += 1;
            }
        }
        let folder_is_exact = !folder.is_empty()
            && folder_counts
                .keys()
                .any(|candidate| candidate.eq_ignore_ascii_case(&folder));

        let filtered = self
            .asset_viewer_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.asset_viewer_filter.matches(entry.kind))
            .filter(|(_, entry)| {
                search.is_empty()
                    || entry.qpath.to_ascii_lowercase().contains(&search)
                    || entry.leaf.to_ascii_lowercase().contains(&search)
                    || entry.display_name.to_ascii_lowercase().contains(&search)
                    || entry
                        .shader_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&search))
            })
            .filter(|(_, entry)| {
                if folder.is_empty() {
                    return true;
                }
                let entry_folder = entry.folder.replace('\\', "/").to_ascii_lowercase();
                if folder_is_exact {
                    entry_folder == folder
                        || entry_folder
                            .strip_prefix(&folder)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                } else {
                    // While the user is still typing, preserve the old useful
                    // live-filter behavior. Once a real folder is selected or
                    // typed exactly, switch to boundary-safe folder semantics.
                    entry_folder.contains(&folder)
                }
            })
            .filter(|(_, entry)| {
                self.asset_viewer_filter != AssetFilter::Shader
                    || shader_file.is_empty()
                    || entry.qpath == shader_file
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        if self.asset_viewer_selected.as_ref().is_some_and(|selected| {
            !filtered
                .iter()
                .any(|&index| self.asset_viewer_entries[index].id == *selected)
        }) {
            self.asset_viewer_selected = None;
            self.asset_viewer_detail_path = None;
            self.asset_viewer_shader_name = None;
            self.clear_asset_preview_runtime();
        }

        let body_height = ui.available_height().max(320.0);
        let total_width = ui.available_width().max(1.0);
        let gap = 8.0_f32;
        let (browser_width, preview_width, inspector_width) =
            Self::browser_workspace_widths(total_width);
        let mut keyboard_selection = None::<String>;
        let mut refresh_requested = false;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            // Compact single-column browser. Search/filter controls belong to
            // this pane because they only affect this list; they should not
            // steal a full-width toolbar from the actual preview workspace.
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
                                egui::ComboBox::from_id_salt("asset_viewer_type")
                                    .selected_text(self.asset_viewer_filter.label())
                                    .width((ui.available_width() - 112.0).max(88.0))
                                    .show_ui(ui, |ui| {
                                        for filter in [
                                            AssetFilter::All,
                                            AssetFilter::Models,
                                            AssetFilter::Md3,
                                            AssetFilter::Glm,
                                            AssetFilter::Efx,
                                            AssetFilter::Shader,
                                        ] {
                                            ui.selectable_value(
                                                &mut self.asset_viewer_filter,
                                                filter,
                                                filter.label(),
                                            );
                                        }
                                    });
                                if theme::ghost_button(ui, "CLEAR").clicked() {
                                    self.asset_viewer_filter = AssetFilter::All;
                                    self.asset_viewer_search.clear();
                                    self.asset_viewer_folder.clear();
                                    self.asset_viewer_folder_suggestion = 0;
                                    self.asset_viewer_shader_file.clear();
                                }
                                refresh_requested |= filesystem_refresh_icon(ui);
                            });

                            let search_response = ui.add_sized(
                                [ui.available_width(), 25.0],
                                egui::TextEdit::singleline(&mut self.asset_viewer_search)
                                    .hint_text("Search assets..."),
                            );

                            // Folder is a discoverable typeahead, not a magic
                            // substring box. The field still filters live while
                            // typing, but real catalog folders appear directly
                            // underneath and can be selected with mouse or keys.
                            let folder_response = ui
                                .horizontal(|ui| {
                                    let clear_width = if self.asset_viewer_folder.is_empty() {
                                        0.0
                                    } else {
                                        48.0
                                    };
                                    let response = ui.add_sized(
                                        [(ui.available_width() - clear_width).max(60.0), 23.0],
                                        egui::TextEdit::singleline(&mut self.asset_viewer_folder)
                                            .hint_text("Filter folder..."),
                                    );
                                    if !self.asset_viewer_folder.is_empty()
                                        && theme::ghost_button(ui, "×")
                                            .on_hover_text("Clear folder filter")
                                            .clicked()
                                    {
                                        self.asset_viewer_folder.clear();
                                        self.asset_viewer_folder_suggestion = 0;
                                        self.egui_repaint_requested = true;
                                    }
                                    response
                                })
                                .inner;

                            if folder_response.changed() {
                                self.asset_viewer_folder_suggestion = 0;
                            }

                            let folder_query = self
                                .asset_viewer_folder
                                .trim()
                                .replace('\\', "/")
                                .trim_matches('/')
                                .to_ascii_lowercase();
                            let mut folder_suggestions = folder_counts
                                .iter()
                                .filter(|(path, _)| {
                                    if folder_query.is_empty() {
                                        !path.contains('/')
                                    } else {
                                        path.to_ascii_lowercase().contains(&folder_query)
                                    }
                                })
                                .map(|(path, count)| (path.clone(), *count))
                                .collect::<Vec<_>>();
                            folder_suggestions.sort_by(|(a, _), (b, _)| {
                                if folder_query.is_empty() {
                                    return a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
                                }
                                let a_lower = a.to_ascii_lowercase();
                                let b_lower = b.to_ascii_lowercase();
                                let a_pos = a_lower.find(&folder_query).unwrap_or(usize::MAX);
                                let b_pos = b_lower.find(&folder_query).unwrap_or(usize::MAX);
                                a_pos
                                    .cmp(&b_pos)
                                    .then_with(|| a_lower.len().cmp(&b_lower.len()))
                                    .then_with(|| a_lower.cmp(&b_lower))
                            });
                            folder_suggestions.truncate(8);

                            let mut picked_folder = None::<String>;
                            let (folder_up, folder_down, folder_enter) = ui.input(|input| {
                                (
                                    input.key_pressed(egui::Key::ArrowUp),
                                    input.key_pressed(egui::Key::ArrowDown),
                                    input.key_pressed(egui::Key::Enter),
                                )
                            });
                            let folder_typeahead_active = folder_response.has_focus()
                                || (folder_response.lost_focus() && folder_enter);
                            if folder_typeahead_active {
                                if !folder_suggestions.is_empty() {
                                    self.asset_viewer_folder_suggestion = self
                                        .asset_viewer_folder_suggestion
                                        .min(folder_suggestions.len().saturating_sub(1));
                                    let up = folder_up;
                                    let down = folder_down;
                                    let enter = folder_enter;
                                    if down {
                                        self.asset_viewer_folder_suggestion =
                                            (self.asset_viewer_folder_suggestion + 1)
                                                % folder_suggestions.len();
                                    } else if up {
                                        self.asset_viewer_folder_suggestion = (self
                                            .asset_viewer_folder_suggestion
                                            + folder_suggestions.len()
                                            - 1)
                                            % folder_suggestions.len();
                                    }
                                    if enter {
                                        picked_folder = Some(
                                            folder_suggestions[self.asset_viewer_folder_suggestion]
                                                .0
                                                .clone(),
                                        );
                                    }

                                    ui.horizontal(|ui| {
                                        theme::label(
                                            ui,
                                            theme::plain("FOLDERS", 8.5, theme::TEXT_FAINT),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                theme::label(
                                                    ui,
                                                    theme::plain(
                                                        "↑ ↓ · ENTER",
                                                        8.0,
                                                        theme::TEXT_FAINT,
                                                    ),
                                                );
                                            },
                                        );
                                    });
                                    egui::Frame::new()
                                        .fill(theme::CONTROL)
                                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                                        .inner_margin(egui::Margin::same(2))
                                        .show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            for (index, (path, count)) in
                                                folder_suggestions.iter().enumerate()
                                            {
                                                let keyboard_selected =
                                                    index == self.asset_viewer_folder_suggestion;
                                                let (rect, response) = ui.allocate_exact_size(
                                                    egui::vec2(ui.available_width(), 21.0),
                                                    egui::Sense::click(),
                                                );
                                                let hovered = response.hovered();
                                                if keyboard_selected || hovered {
                                                    ui.painter().rect_filled(
                                                        rect,
                                                        egui::CornerRadius::same(2),
                                                        if keyboard_selected {
                                                            theme::CONTROL_SELECTED
                                                        } else {
                                                            theme::CONTROL_HOVER
                                                        },
                                                    );
                                                }
                                                let path_rect = egui::Rect::from_min_max(
                                                    rect.min,
                                                    egui::pos2(rect.right() - 42.0, rect.bottom()),
                                                );
                                                ui.painter().with_clip_rect(path_rect).text(
                                                    rect.left_center() + egui::vec2(6.0, 0.0),
                                                    egui::Align2::LEFT_CENTER,
                                                    path,
                                                    egui::FontId::monospace(9.5),
                                                    if keyboard_selected {
                                                        theme::TEXT
                                                    } else {
                                                        theme::TEXT_DIM
                                                    },
                                                );
                                                ui.painter().text(
                                                    rect.right_center() - egui::vec2(6.0, 0.0),
                                                    egui::Align2::RIGHT_CENTER,
                                                    count.to_string(),
                                                    egui::FontId::monospace(8.5),
                                                    theme::TEXT_FAINT,
                                                );
                                                let response = response.on_hover_text(path);
                                                if response.clicked() {
                                                    picked_folder = Some(path.clone());
                                                }
                                            }
                                        });
                                } else if !folder_query.is_empty() {
                                    theme::label(
                                        ui,
                                        theme::plain("No matching folders", 9.0, theme::TEXT_FAINT),
                                    );
                                }
                            }

                            if let Some(path) = picked_folder {
                                self.asset_viewer_folder = path;
                                self.asset_viewer_folder_suggestion = 0;
                                ui.memory_mut(|memory| memory.surrender_focus(folder_response.id));
                                self.egui_repaint_requested = true;
                            }

                            if self.asset_viewer_filter == AssetFilter::Shader {
                                let selected_file = if self.asset_viewer_shader_file.is_empty() {
                                    "All shader files".to_owned()
                                } else {
                                    self.asset_viewer_shader_file
                                        .rsplit('/')
                                        .next()
                                        .unwrap_or(self.asset_viewer_shader_file.as_str())
                                        .to_owned()
                                };
                                egui::ComboBox::from_id_salt("asset_viewer_shader_file")
                                    .selected_text(selected_file)
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.asset_viewer_shader_file,
                                            String::new(),
                                            "All shader files",
                                        );
                                        for qpath in &shader_files {
                                            let label = qpath.rsplit('/').next().unwrap_or(qpath);
                                            ui.selectable_value(
                                                &mut self.asset_viewer_shader_file,
                                                qpath.clone(),
                                                label,
                                            )
                                            .on_hover_text(qpath);
                                        }
                                    });
                            }

                            ui.horizontal(|ui| {
                                theme::label(
                                    ui,
                                    theme::plain(
                                        &format!(
                                            "{} / {}",
                                            filtered.len(),
                                            self.asset_viewer_entries.len()
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
                                            theme::plain("↑ ↓ browse", 9.0, theme::TEXT_FAINT),
                                        );
                                    },
                                );
                            });
                            ui.separator();

                            // Arrow navigation intentionally stays dormant while
                            // either text field owns focus so cursor movement in
                            // search/filter boxes remains standard text editing.
                            if !search_response.has_focus()
                                && !folder_response.has_focus()
                                && !filtered.is_empty()
                            {
                                let nav = ui.input(|input| {
                                    if input.key_pressed(egui::Key::ArrowDown) {
                                        Some(1_i32)
                                    } else if input.key_pressed(egui::Key::ArrowUp) {
                                        Some(-1_i32)
                                    } else {
                                        None
                                    }
                                });
                                if let Some(direction) = nav {
                                    let current = self.asset_viewer_selected.as_deref().and_then(
                                        |selected| {
                                            filtered.iter().position(|&index| {
                                                self.asset_viewer_entries[index].id == selected
                                            })
                                        },
                                    );
                                    let next = match (current, direction) {
                                        (Some(position), 1) => {
                                            (position + 1).min(filtered.len() - 1)
                                        }
                                        (Some(position), -1) => position.saturating_sub(1),
                                        (Some(position), _) => position,
                                        (None, 1) => 0,
                                        (None, -1) => filtered.len() - 1,
                                        (None, _) => 0,
                                    };
                                    let entry_id =
                                        self.asset_viewer_entries[filtered[next]].id.clone();
                                    if self.asset_viewer_selected.as_deref()
                                        != Some(entry_id.as_str())
                                    {
                                        self.asset_viewer_selected = Some(entry_id.clone());
                                        self.asset_viewer_detail_path = None;
                                        self.asset_viewer_shader_name = None;
                                        self.clear_asset_preview_runtime();
                                        keyboard_selection = Some(entry_id);
                                        self.egui_repaint_requested = true;
                                    }
                                }
                            }

                            let list_height = ui.available_height().max(1.0);
                            egui::ScrollArea::vertical()
                                .id_salt("asset_viewer_list")
                                .auto_shrink([false, false])
                                .max_height(list_height)
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    for &index in &filtered {
                                        let entry = self.asset_viewer_entries[index].clone();
                                        let selected = self.asset_viewer_selected.as_deref()
                                            == Some(entry.id.as_str());
                                        let (rect, response) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 24.0),
                                            egui::Sense::click(),
                                        );
                                        let painter = ui.painter().with_clip_rect(rect);
                                        if selected {
                                            painter.rect_filled(
                                                rect,
                                                egui::CornerRadius::same(2),
                                                theme::CONTROL_SELECTED,
                                            );
                                            painter.rect_filled(
                                                egui::Rect::from_min_size(
                                                    rect.left_top(),
                                                    egui::vec2(2.0, rect.height()),
                                                ),
                                                egui::CornerRadius::ZERO,
                                                theme::ACCENT,
                                            );
                                        } else if response.hovered() {
                                            painter.rect_filled(
                                                rect,
                                                egui::CornerRadius::same(2),
                                                theme::CONTROL,
                                            );
                                        }
                                        painter.text(
                                            rect.left_center() + egui::vec2(7.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            &entry.display_name,
                                            egui::FontId::monospace(10.5),
                                            if selected {
                                                theme::TEXT
                                            } else {
                                                theme::TEXT_DIM
                                            },
                                        );
                                        painter.text(
                                            rect.right_center() - egui::vec2(5.0, 0.0),
                                            egui::Align2::RIGHT_CENTER,
                                            entry.kind.label(),
                                            egui::FontId::monospace(8.0),
                                            if selected {
                                                theme::ACCENT
                                            } else {
                                                theme::TEXT_FAINT
                                            },
                                        );

                                        let hover = entry.shader_name.as_ref().map_or_else(
                                            || entry.qpath.clone(),
                                            |shader| format!("{shader}\n{}", entry.qpath),
                                        );
                                        let response = response.on_hover_text(hover);
                                        if response.clicked() && !selected {
                                            self.asset_viewer_selected = Some(entry.id.clone());
                                            self.asset_viewer_detail_path = None;
                                            self.asset_viewer_shader_name = None;
                                            self.clear_asset_preview_runtime();
                                            self.egui_repaint_requested = true;
                                        }
                                        if selected
                                            && keyboard_selection.as_deref()
                                                == Some(entry.id.as_str())
                                        {
                                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                                        }
                                    }
                                });
                        });
                },
            );

            self.ensure_asset_viewer_detail();

            let detail_snapshot = self
                .asset_viewer_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().ok())
                .cloned();
            let detail_error = self
                .asset_viewer_detail
                .as_ref()
                .and_then(|detail| detail.as_ref().err())
                .cloned();

            // The preview gets the largest share of the workspace. Controls are
            // deliberately direct-manipulation: drag in both axes for models,
            // wheel for zoom, and a compact reset control in the inspector.
            ui.allocate_ui_with_layout(
                egui::vec2(preview_width, body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let (preview_rect, preview_response) = ui.allocate_exact_size(
                        egui::vec2(preview_width, body_height),
                        egui::Sense::click_and_drag(),
                    );
                    ui.painter().rect_stroke(
                        preview_rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    let preview_label = match detail_snapshot.as_ref().map(|detail| detail.kind) {
                        Some(AssetKind::Md3) | Some(AssetKind::Glm) => {
                            "MODEL · drag rotate · wheel zoom"
                        }
                        Some(AssetKind::Efx) => "EFX · looping · wheel zoom",
                        Some(AssetKind::Shader) => "SHADER · wheel zoom",
                        None => "PREVIEW",
                    };
                    ui.painter().text(
                        preview_rect.left_top() + egui::vec2(10.0, 9.0),
                        egui::Align2::LEFT_TOP,
                        preview_label,
                        egui::FontId::proportional(10.5),
                        theme::TEXT_FAINT,
                    );
                    self.set_asset_preview_viewport(
                        preview_rect.shrink(1.0),
                        ui.ctx().pixels_per_point(),
                    );

                    let kind = detail_snapshot.as_ref().map(|detail| detail.kind);
                    let zoom_max = match kind {
                        Some(AssetKind::Efx) => 64.0,
                        Some(AssetKind::Shader) => 20.0,
                        Some(AssetKind::Md3) | Some(AssetKind::Glm) => 12.0,
                        None => 12.0,
                    };
                    let mut preview_changed = false;
                    if kind.is_some_and(|kind| kind.is_model()) && preview_response.dragged() {
                        let delta = ui.input(|input| input.pointer.delta());
                        if delta.x.abs() > f32::EPSILON {
                            self.asset_viewer_model_yaw =
                                (self.asset_viewer_model_yaw + delta.x * 0.45).rem_euclid(360.0);
                            preview_changed = true;
                        }
                        if delta.y.abs() > f32::EPSILON {
                            self.asset_viewer_model_pitch =
                                (self.asset_viewer_model_pitch - delta.y * 0.35).clamp(-89.0, 89.0);
                            preview_changed = true;
                        }
                    }
                    if preview_response.hovered() {
                        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                        if scroll.abs() > f32::EPSILON {
                            self.asset_viewer_model_zoom = (self.asset_viewer_model_zoom
                                * (-scroll * 0.0015).exp())
                            .clamp(0.20, zoom_max);
                            preview_changed = true;
                        }
                    }
                    if preview_changed {
                        self.asset_preview_model_key = None;
                        self.asset_preview_shader_key = None;
                        self.asset_preview_fx = None;
                        self.update_asset_preview_content();
                        self.egui_repaint_requested = true;
                    }
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

                            match detail_snapshot.as_ref() {
                                Some(detail) => {
                                    let detail_title = if detail.kind == AssetKind::Shader {
                                        self.asset_viewer_shader_name
                                            .as_deref()
                                            .unwrap_or(detail.qpath.as_str())
                                    } else {
                                        detail.qpath.as_str()
                                    };
                                    theme::glow_label(ui, detail_title, 12.5, theme::TEXT);
                                    if detail.kind == AssetKind::Shader {
                                        theme::label(
                                            ui,
                                            theme::plain(
                                                &format!("File: {}", detail.qpath),
                                                9.0,
                                                theme::TEXT_FAINT,
                                            ),
                                        );
                                    }
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            &format!(
                                                "{}  •  {}",
                                                detail.kind.label(),
                                                asset_size_label(detail.size_bytes)
                                            ),
                                            10.0,
                                            theme::TEXT_DIM,
                                        ),
                                    );
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            &format!("Source: {}", detail.source),
                                            9.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );

                                    ui.add_space(6.0);
                                    theme::section(ui, "VIEW", "");
                                    let zoom_max = match detail.kind {
                                        AssetKind::Efx => 64.0,
                                        AssetKind::Shader => 20.0,
                                        AssetKind::Md3 | AssetKind::Glm => 12.0,
                                    };
                                    let mut view_changed = false;
                                    if detail.kind.is_model() {
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain("Yaw", 10.0, theme::TEXT_FAINT),
                                            );
                                            view_changed |= ui
                                                .add(
                                                    egui::Slider::new(
                                                        &mut self.asset_viewer_model_yaw,
                                                        0.0..=360.0,
                                                    )
                                                    .show_value(false),
                                                )
                                                .changed();
                                        });
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain("Pitch", 10.0, theme::TEXT_FAINT),
                                            );
                                            view_changed |= ui
                                                .add(
                                                    egui::Slider::new(
                                                        &mut self.asset_viewer_model_pitch,
                                                        -89.0..=89.0,
                                                    )
                                                    .show_value(false),
                                                )
                                                .changed();
                                        });
                                    }
                                    ui.horizontal(|ui| {
                                        theme::label(
                                            ui,
                                            theme::plain("Zoom", 10.0, theme::TEXT_FAINT),
                                        );
                                        view_changed |= ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut self.asset_viewer_model_zoom,
                                                    0.20..=zoom_max,
                                                )
                                                .logarithmic(true)
                                                .show_value(false),
                                            )
                                            .changed();
                                    });
                                    if theme::ghost_button(ui, "RESET VIEW").clicked() {
                                        self.asset_viewer_model_yaw = 180.0;
                                        self.asset_viewer_model_pitch = 0.0;
                                        self.asset_viewer_model_zoom = 1.0;
                                        view_changed = true;
                                    }
                                    if view_changed {
                                        self.asset_preview_model_key = None;
                                        self.asset_preview_shader_key = None;
                                        self.asset_preview_fx = None;
                                        self.update_asset_preview_content();
                                        self.egui_repaint_requested = true;
                                    }

                                    ui.add_space(7.0);
                                    theme::section(ui, "STATS", &format!("{}", detail.stats.len()));
                                    for (name, value) in &detail.stats {
                                        ui.horizontal(|ui| {
                                            theme::label(
                                                ui,
                                                theme::plain(name, 10.0, theme::TEXT_FAINT),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    theme::label(
                                                        ui,
                                                        theme::plain(value, 10.0, theme::TEXT),
                                                    );
                                                },
                                            );
                                        });
                                    }

                                    // Stats stay outside the scroll area so the
                                    // useful summary remains visible. Long
                                    // surface/stage/source dumps get the remaining
                                    // vertical space instead.
                                    ui.add_space(7.0);
                                    let list_title = match detail.kind {
                                        AssetKind::Shader => "STAGES",
                                        kind if kind.is_model() => "SURFACES",
                                        _ => "PRIMITIVES",
                                    };
                                    theme::section(
                                        ui,
                                        list_title,
                                        &format!("{}", detail.items.len()),
                                    );
                                    let details_height = ui.available_height().max(1.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("asset_viewer_inspector_details")
                                        .auto_shrink([false, false])
                                        .max_height(details_height)
                                        .show(ui, |ui| {
                                            for item in &detail.items {
                                                ui.label(
                                                    egui::RichText::new(item)
                                                        .monospace()
                                                        .size(9.25)
                                                        .color(theme::TEXT_DIM),
                                                );
                                            }
                                            if let Some(text) = &detail.raw_text {
                                                ui.add_space(8.0);
                                                ui.collapsing("SOURCE", |ui| {
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .monospace()
                                                            .size(9.0)
                                                            .color(theme::TEXT_DIM),
                                                    );
                                                });
                                            }
                                        });
                                }
                                None if detail_error.is_some() => {
                                    theme::banner(
                                        ui,
                                        detail_error
                                            .as_deref()
                                            .unwrap_or("Asset inspection failed"),
                                        theme::WARNING,
                                    );
                                }
                                None => {
                                    theme::label(
                                        ui,
                                        theme::plain(
                                            "Select an asset from the browser.",
                                            11.0,
                                            theme::TEXT_FAINT,
                                        ),
                                    );
                                }
                            }
                        });
                },
            );
        });

        // EFX needs to advance every frontend frame even when no UI control is
        // moving; model/shader paths cheaply return when their cache key matches.
        self.update_asset_preview_content();

        // refresh_filesystem() invalidates this catalog. Defer it until every
        // list index/reference from the current frame has been consumed.
        if refresh_requested {
            self.refresh_filesystem();
            self.egui_repaint_requested = true;
        }
    }
}
