//! Selection and source controls for independent local river patches.
use super::{theme, App};
use crate::river_authoring::{Id, SourceParameters};
use crate::river_runtime::SourceMode;

impl App {
    pub(super) fn egui_river_target(&mut self, ui: &mut egui::Ui) {
        let label = self
            .river_editor
            .selected
            .and_then(|id| {
                self.river_editor
                    .entries
                    .iter()
                    .find(|e| e.definition.id == id)
            })
            .map(|e| e.name.as_str())
            .unwrap_or("New river defaults")
            .to_owned();
        theme::row(
            ui,
            "River",
            "Select a placed river or edit the defaults for the next source.",
            theme::Reset::None,
            |ui| {
                egui::ComboBox::from_id_salt("river_target")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.river_editor.selected,
                            None,
                            "New river defaults",
                        );
                        for entry in &self.river_editor.entries {
                            let p = entry.definition.source.center;
                            ui.selectable_value(
                                &mut self.river_editor.selected,
                                Some(entry.definition.id),
                                format!("{} ({:.0}, {:.0}, {:.0})", entry.name, p[0], p[1], p[2]),
                            );
                        }
                    });
            },
        );
        let selected = self.river_editor.selected;
        if let Some(id) = selected {
            if let Some(entry) = self.river_editor.entry_mut(id) {
                theme::row(
                    ui,
                    "Name",
                    "Name this river for the selector.",
                    theme::Reset::None,
                    |ui| {
                        ui.text_edit_singleline(&mut entry.name);
                    },
                );
                let p = entry.definition.source.center;
                theme::hint(
                    ui,
                    &format!("Source at {:.0}, {:.0}, {:.0}", p[0], p[1], p[2]),
                    theme::TEXT_FAINT(),
                );
            }
        }
        theme::section(ui,"WATER SOURCE","Mode, CFS and solve speed apply live and preserve water and foam. Height, width, radius and resolution require Rebuild river.");
        let (mut params, pending, error) =
            match selected.and_then(|id| self.river_editor.entry_mut(id)) {
                Some(entry) => (entry.draft, entry.pending, entry.error.clone()),
                None => (self.river_editor.defaults, false, None),
            };
        let previous_live = params.live();
        ui.add_enabled_ui(!pending, |ui| source_controls(ui, &mut params));
        if let Some(id) = selected {
            if let Some(entry) = self.river_editor.entry_mut(id) {
                entry.draft = params;
            }
            if !pending && params.live() != previous_live {
                let request = self.river_editor.entry_mut(id).map(|entry| {
                    let controls = params.live();
                    match controls.apply(entry.definition.source) {
                        Ok(_) => { entry.error = None; Some(controls) }
                        Err(error) => { entry.error = Some(error); None }
                    }
                }).flatten();
                if let Some(controls) = request {
                    self.render_command(crate::renderer::RenderCommand::RiverSource(id, controls));
                }
            }
            if pending {
                theme::hint(ui, "Building river…", theme::TEXT_DIM());
            }
            if let Some(error) = error {
                theme::banner(ui, &error, theme::WARNING());
            }
            let valid = params.at([0.0; 3]).validate();
            if let Err(error) = &valid {
                theme::hint(ui, error, theme::WARNING());
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !pending && valid.is_ok(),
                        egui::Button::new("REBUILD RIVER"),
                    )
                    .clicked()
                {
                    self.rebuild_river(id);
                }
                if theme::ghost_button(ui, "REMOVE RIVER").clicked() {
                    self.river_editor.entries.retain(|e| e.definition.id != id);
                    self.river_editor.selected = None;
                    self.render_command(crate::renderer::RenderCommand::RiverRemove(id));
                }
            });
        } else {
            self.river_editor.defaults = params;
            if theme::ghost_button(ui, "PLACE AT CROSSHAIR").clicked() {
                self.cmd_riversource("riversource");
            }
        }
        theme::section(
            ui,
            "SHARED SCENE",
            "Sunlight and the reflection environment follow the scene’s Sky settings.",
        );
    }

    fn rebuild_river(&mut self, id: Id) {
        let key = format!("{:?}", self.initial_source);
        let Some(collision) = self
            .river_collision
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, c)| std::sync::Arc::clone(c))
        else {
            return;
        };
        let Some(entry) = self.river_editor.entry_mut(id) else {
            return;
        };
        if entry.pending {
            return;
        }
        let mut definition = entry.definition;
        definition.source = entry.draft.at(definition.source.center);
        if let Err(error) = definition.source.validate() {
            entry.error = Some(error);
            return;
        }
        entry.pending = true;
        entry.error = None;
        self.render_command(crate::renderer::RenderCommand::RiverSpawn(
            definition, collision,
        ));
    }
}

fn source_controls(ui: &mut egui::Ui, p: &mut SourceParameters) {
    theme::row(
        ui,
        "Source mode",
        "Water Height maintains a target level (optionally CFS-limited); Discharge continuously supplies a CFS volume without a height cap.",
        theme::Reset::None,
        |ui| {
            if ui.selectable_label(p.mode == SourceMode::WaterLevel, "Water Height").clicked() {
                p.mode = SourceMode::WaterLevel;
            }
            if ui.selectable_label(p.mode == SourceMode::Discharge, "Discharge (CFS)").clicked() {
                p.mode = SourceMode::Discharge;
                p.discharge_cfs.get_or_insert(100.0);
            }
        },
    );
    if p.mode == SourceMode::WaterLevel {
        theme::row(
            ui,
            "Limit inflow (CFS)",
            "Optional volume limit while filling to the target height. Unchecked retains the original unlimited water-height source.",
            theme::Reset::None,
            |ui| {
                let mut enabled = p.discharge_cfs.is_some();
                if ui.checkbox(&mut enabled, "Enable").changed() {
                    if enabled { p.discharge_cfs = Some(100.0); }
                    else { p.discharge_cfs = None; }
                }
            },
        );
    }
    if let Some(cfs) = &mut p.discharge_cfs {
        theme::row(
            ui,
            if p.mode == SourceMode::WaterLevel { "Max inflow (CFS)" } else { "Discharge (CFS)" },
            "Cubic feet of water per simulated second, independent of Solve Speed. A limited height source adds water only while below its target level.",
            theme::Reset::None,
            |ui| {
                ui.add(egui::DragValue::new(cfs)
                    .speed(1.0).range(0.01..=1_000_000.0)
                    .max_decimals(2).suffix(" ft³/s"));
            },
        );
    }
    theme::row(
        ui,
        "Source height",
        "Height above the placement point, in map units. Water Height uses this as its target. Discharge uses it as the maximum inlet-bed elevation (not a water level clamp).",
        theme::Reset::None,
        |ui| { ui.add(egui::DragValue::new(&mut p.height).speed(1.0).range(0.1..=16384.0)); },
    );
    theme::row(
        ui,
        "Patch radius",
        "Half-width of the simulated square around the source, in map units.",
        theme::Reset::None,
        |ui| {
            ui.add(
                egui::DragValue::new(&mut p.radius)
                    .speed(16.0)
                    .range(64.0..=65536.0),
            );
        },
    );
    theme::row(ui,"Source width","Width of the square inlet, in map units. Smaller inlets concentrate the same discharge into fewer cells.",theme::Reset::None,|ui| {ui.add(egui::DragValue::new(&mut p.width).speed(1.0).range(8.0..=p.radius*2.0));});
    theme::row(ui,"Grid resolution","Cells along the patch axis. Higher resolution captures finer riverbed detail and uses more GPU memory.",theme::Reset::None,|ui| {ui.add(egui::DragValue::new(&mut p.cells).speed(1.0).range(16..=1024));});
    theme::row(ui,"Solve speed","Initial simulation acceleration. Once steady flow is detected, the river automatically returns to 1×; CFS remains measured per simulated second.",theme::Reset::None,|ui| {ui.add(egui::DragValue::new(&mut p.speed).speed(0.1).range(0.01..=100.0).suffix("×"));});
}
