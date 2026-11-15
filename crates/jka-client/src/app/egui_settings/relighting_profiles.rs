//! Profile library controls live outside the relighting enabled/disabled gate.
use super::{theme, App};

enum Action {
    Load(u64),
    Save,
    SaveAs,
    Rename(u64),
    Delete(u64),
    Default(Option<u64>),
    Normal,
}

impl App {
    pub(super) fn egui_relighting_profiles(&mut self, ui: &mut egui::Ui) {
        theme::row(ui, "Saved profiles", "Save relighting, sun/moon, sky and cloud settings together. Stored in DinurdoJK/light_profiles by map, shared across mods. Profiles only change when you save; other video settings remain independent.", theme::Reset::None, |_| {});
        if self.render_studio.graphics_gameplay.is_some() {
            theme::hint(ui, "Render Studio uses its own graphics profiles. Leave the workspace to manage gameplay relighting profiles.", theme::TEXT_DIM());
            return;
        }
        self.ensure_relighting_profiles();
        if let Some(error) = self.relighting_profiles.read_error.clone() {
            theme::banner(ui, &error, theme::WARNING());
            if theme::ghost_button(ui, "RETRY READING PROFILES").clicked() {
                self.relighting_profiles.read_error = None;
                self.ensure_relighting_profiles();
            }
            return;
        }
        let state = &self.relighting_profiles;
        let library = state.library.as_ref().unwrap();
        let mut profiles: Vec<_> = library
            .profiles
            .iter()
            .map(|(id, profile)| (*id, profile.name.clone(), profile.map.clone()))
            .collect();
        profiles.sort_by(|a, b| {
            let local_a = a.2.is_some() && a.2 == state.map;
            let local_b = b.2.is_some() && b.2 == state.map;
            local_b
                .cmp(&local_a)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        let selected = state.selected;
        let active = state.active;
        let default = state
            .map
            .as_ref()
            .and_then(|map| library.map_defaults.get(map))
            .copied();
        let map = state.map.clone();
        let mut picked = selected;
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Profile");
            let name = profiles
                .iter()
                .find(|p| Some(p.0) == selected)
                .map(|p| p.1.as_str())
                .unwrap_or("Choose a profile…");
            egui::ComboBox::from_id_salt("relighting_profile_picker")
                .selected_text(name)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (id, name, origin) in &profiles {
                        let label = if origin.is_some() && *origin == map {
                            format!("{name}  ·  this map")
                        } else if let Some(origin) = origin {
                            format!("{name}  ·  {origin}")
                        } else {
                            format!("{name}  ·  any map")
                        };
                        ui.selectable_value(&mut picked, Some(*id), label);
                    }
                });
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Load"))
                .clicked()
            {
                action = selected.map(Action::Load);
            }
            if ui
                .add_enabled(
                    active.is_some() && active == selected,
                    egui::Button::new("Save"),
                )
                .clicked()
            {
                action = Some(Action::Save);
            }
            if ui
                .add_enabled(active.is_some(), egui::Button::new("Revert"))
                .on_hover_text("Reload the active profile's saved settings")
                .clicked()
            {
                action = active.map(Action::Load);
            }
        });
        if picked != selected {
            self.relighting_profiles.selected = picked;
            self.relighting_profiles.delete_pending = false;
            if let Some((_, name, _)) = profiles.iter().find(|p| Some(p.0) == picked) {
                self.relighting_profiles.name_input = name.clone();
            }
        }
        let selected = picked;
        if let Some((_, name, _)) = profiles.iter().find(|p| Some(p.0) == active) {
            let modified = if self.relighting_profile_modified() {
                " · Modified"
            } else {
                ""
            };
            theme::hint(ui, &format!("Active: {name}{modified}"), theme::TEXT_DIM());
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut self.relighting_profiles.name_input)
                    .desired_width(220.0)
                    .char_limit(80)
                    .hint_text("Warm sunset"),
            );
            if ui.button("Save As…").clicked() {
                action = Some(Action::SaveAs);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Rename"))
                .clicked()
            {
                action = selected.map(Action::Rename);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Delete…"))
                .clicked()
            {
                self.relighting_profiles.delete_pending = true;
            }
        });
        if self.relighting_profiles.delete_pending {
            ui.horizontal_wrapped(|ui| {
                ui.label("Delete this profile and its map defaults?");
                if ui.button("Confirm delete").clicked() {
                    action = selected.map(Action::Delete);
                }
                if ui.button("Cancel").clicked() {
                    self.relighting_profiles.delete_pending = false;
                }
            });
        }
        if let Some(map) = map {
            theme::hint(ui, &format!("Map: {map}"), theme::TEXT_DIM());
            let mut automatic = selected.is_some() && default == selected;
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Checkbox::new(
                        &mut automatic,
                        "Automatically load selected profile for this map",
                    ),
                )
                .changed()
            {
                action = Some(Action::Default(if automatic { selected } else { None }));
            }
            if let Some((_, name, _)) = profiles.iter().find(|p| Some(p.0) == default) {
                theme::hint(ui, &format!("Map default: {name}"), theme::TEXT_DIM());
            }
            if default.is_some()
                && default != selected
                && ui.small_button("Clear map default").clicked()
            {
                action = Some(Action::Default(None));
            }
        } else {
            theme::hint(ui, "Load a map to assign an automatic profile. Profiles can still be saved and loaded manually.", theme::TEXT_DIM());
        }
        if self.has_relighting_overlay() && theme::ghost_button(ui, "USE NORMAL LIGHTING").clicked()
        {
            action = Some(Action::Normal);
        }
        if let Some(action) = action {
            let result = match action {
                Action::Load(id) => self.load_relighting_profile(id),
                Action::Save => self.save_relighting_profile(false),
                Action::SaveAs => self.save_relighting_profile(true),
                Action::Rename(id) => self.rename_relighting_profile(id),
                Action::Delete(id) => self.delete_relighting_profile(id),
                Action::Default(id) => self.set_relighting_map_default(id),
                Action::Normal => self.restore_normal_relighting(),
            };
            if let Err(error) = result {
                self.relighting_profiles.status = error;
            }
        }
        if !self.relighting_profiles.status.is_empty() {
            theme::hint(ui, &self.relighting_profiles.status, theme::TEXT_DIM());
        }
        theme::hint(ui, "Saved profiles exclude temporary diagnostics and session-only experiments. Unsaved profile edits are replaced when you load another profile or leave the map.", theme::TEXT_DIM());
        ui.separator();
    }
}
