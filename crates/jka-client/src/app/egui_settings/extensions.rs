//! Setup -> Extensions. The editor edits the real .rhai source file.
//! Neither the UI nor this module executes extension code.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app) fn egui_extensions_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "EXTENSIONS / RHAI",
            "Edit scripts directly in-game. Save & Reload recompiles on the extension worker; external file edits are detected automatically.",
        );
        ui.label(format!("Loose extension scripts: {}", self.extensions.root().display()));
        ui.label(format!("Enabled/disabled preferences: {}", jka_assets::pk3::dinurdojk_directory(&self.base).join("extensions.json").display()));
        ui.label("Loads extensions/ from the active game and base, including PK3s, using normal asset precedence.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("RESCAN").clicked() { self.extensions.refresh(); }
            ui.label("New ID:");
            ui.add(egui::TextEdit::singleline(&mut self.extensions.new_id)
                .hint_text("my-extension").desired_width(150.0));
            if ui.button("CREATE").clicked() { self.extensions.create(); }
        });
        if !self.extensions.diagnostic.is_empty() {
            ui.label(format!("Last message: {}", self.extensions.diagnostic));
        }
        ui.separator();

        let mut next_selected = None;
        let mut toggle = None;
        let rows = self.extensions.entries.clone();
        // Use the full content height for both panes. The old 210x360 list
        // left most of the screen empty, even with dozens of extensions.
        let content_width = ui.available_width();
        let content_height = ui.available_height().max(240.0);
        let list_width = (content_width * 0.31).clamp(260.0, 450.0)
            .min((content_width - 350.0).max(180.0));
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(list_width, content_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                ui.set_width(list_width);
                ui.strong("Installed extensions");
                egui::ScrollArea::vertical()
                    .id_salt("extension_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if rows.is_empty() {
                            ui.label("No extensions found. Create one above.");
                        }
                        for ext in &rows {
                            let selected = self.extensions.selected.as_deref() == Some(&ext.id);
                            // One row per extension: selecting, enabling and
                            // health are visible without wasting vertical space.
                            ui.horizontal(|ui| {
                                let name_width = (list_width - 115.0).max(90.0);
                                let label = egui::Button::selectable(selected, ext.name.as_str());
                                if ui.add_sized([name_width, 22.0], label).clicked() {
                                    next_selected = Some(ext.id.clone());
                                }
                                let mut enabled = ext.enabled;
                                if ui.checkbox(&mut enabled, "").on_hover_text("Enabled").changed() {
                                    toggle = Some((ext.id.clone(), enabled));
                                }
                                if ext.error.is_some() {
                                    ui.colored_label(theme::WARNING(), "ERROR");
                                } else if ext.running {
                                    ui.small(format!("r{}", ext.revision));
                                }
                            });
                            ui.add_space(2.0);
                        }
                    });
                },
            );
            ui.separator();
            let editor_width = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(editor_width, content_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                ui.set_width(editor_width);
                if let Some(entry) = self.extensions.selected_entry().cloned() {
                    ui.strong(format!("{} / main script", entry.name));
                    ui.label(format!("ID: {}  |  revision: {}", entry.id, entry.revision));
                    if let Some(error) = &entry.error {
                        ui.colored_label(theme::WARNING(), error.as_str());
                        if entry.running { ui.label("Previous working revision is still active."); }
                    } else if !entry.enabled {
                        ui.label("Disabled (no script running)");
                    } else if entry.running {
                        ui.label("Active");
                    }
                    if !entry.hides.is_empty() {
                        ui.label(format!("Replaces native HUD: {}", entry.hides.names().join(", ")));
                    }
                    if self.extensions.editor_dirty {
                        ui.label("Unsaved editor changes");
                    }
                    if !entry.editable { ui.label("Packaged script (read-only). Edit its source package to make changes."); }
                    // Size the code region to the actual remaining height, not
                    // a fixed 19 lines that overflows smaller windows.
                    let editor_height = (ui.available_height() - 42.0).max(160.0);
                    let editor = egui::TextEdit::multiline(&mut self.extensions.editor)
                        .code_editor()
                        .font(egui::TextStyle::Monospace)
                        .desired_width(editor_width - 8.0)
                        .desired_rows(1);
                    if ui.add_enabled_ui(entry.editable, |ui| ui.add_sized([editor_width - 8.0, editor_height], editor)).inner.changed() {
                        self.extensions.editor_dirty = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(entry.editable, egui::Button::new("SAVE & RELOAD")).clicked() {
                            self.extensions.save_selected();
                        }
                        if ui.button("RELOAD FROM DISK").clicked() {
                            self.extensions.editor_dirty = false;
                            self.extensions.reload_selected();
                        }
                    });
                } else {
                    ui.label("Choose an extension on the left.");
                }
                },
            );
        });
        if let Some(id) = next_selected { self.extensions.select(&id); }
        if let Some((id, enabled)) = toggle { self.extensions.enable(&id, enabled); }
    }
}
