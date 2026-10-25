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
        ui.label(format!("Local extensions and preferences: {}", self.extensions.root().display()));
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
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(210.0);
                ui.strong("Installed extensions");
                egui::ScrollArea::vertical()
                    .id_salt("extension_list")
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if rows.is_empty() {
                            ui.label("No extensions found. Create one above.");
                        }
                        for ext in &rows {
                            let selected = self.extensions.selected.as_deref() == Some(&ext.id);
                            if ui.selectable_label(selected, ext.name.as_str()).clicked() {
                                next_selected = Some(ext.id.clone());
                            }
                            ui.horizontal(|ui| {
                                let mut enabled = ext.enabled;
                                if ui.checkbox(&mut enabled, "Enabled").changed() {
                                    toggle = Some((ext.id.clone(), enabled));
                                }
                                if ext.error.is_some() {
                                    ui.colored_label(theme::WARNING, "ERROR");
                                } else if ext.running {
                                    ui.small(format!("rev {}", ext.revision));
                                }
                            });
                            ui.add_space(5.0);
                        }
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_min_width(360.0);
                if let Some(entry) = self.extensions.selected_entry().cloned() {
                    ui.strong(format!("{} / main script", entry.name));
                    ui.label(format!("ID: {}  |  revision: {}", entry.id, entry.revision));
                    if let Some(error) = &entry.error {
                        ui.colored_label(theme::WARNING, error.as_str());
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
                    let editor = egui::TextEdit::multiline(&mut self.extensions.editor)
                        .code_editor()
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(19);
                    if ui.add_enabled(entry.editable, editor).changed() { self.extensions.editor_dirty = true; }
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
            });
        });
        if let Some(id) = next_selected { self.extensions.select(&id); }
        if let Some((id, enabled)) = toggle { self.extensions.enable(&id, enabled); }
        ui.add_space(10.0);
        ui.label("Stage 3: hud_register(), hud_text(), hud_rect(), hud_line(), and hud_circle() are available. Customize positions in HUD Edit mode. To replace native elements, list them in extension.toml: hides_native_hud = [\"health\", \"shield\", \"ammo\", \"force\", \"speedometer\"].");
    }
}
