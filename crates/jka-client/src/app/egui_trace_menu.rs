//! The `/trace` popup (`OverlayMode::Trace`): a small floating window listing
//! everything traced this session. Each `/trace` key press (center-crosshair
//! raycast, see `App::trace_surface_center`) appends an entry here instead of
//! only showing the latest hit, so you can aim at several things in sequence
//! and come back to any of them. It's a compact `egui::Window`, not a
//! fullscreen menu: the game keeps rendering behind it, and opening it just
//! swaps aim for a mouse cursor (`OverlayMode::Trace` releases capture) so you
//! can click the entry picker and the action buttons.

use super::egui_entity_graph::{info_row, wrapped};
use super::egui_theme as theme;
use super::*;

enum TraceAction {
    Close,
    Clear,
    Select(usize),
    TeleportTo(u16),
    Use(u16),
}

impl App {
    pub(super) fn egui_trace_menu(&mut self, root: &mut egui::Ui) {
        let mut actions = Vec::new();
        let mut open = true;
        egui::Window::new("TRACE")
            .id(egui::Id::new("jka_trace_window"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(egui::pos2(24.0, 64.0))
            .default_width(380.0)
            .max_height(520.0)
            .show(root.ctx(), |ui| {
                self.trace_window_body(ui, &mut actions);
            });
        if !open {
            actions.push(TraceAction::Close);
        }
        for action in actions {
            self.apply_trace_action(action);
        }
    }

    fn trace_window_body(&mut self, ui: &mut egui::Ui, actions: &mut Vec<TraceAction>) {
        let entries_len = self.trace_menu_entries.len();
        ui.horizontal(|ui| {
            theme::label(ui, theme::plain(&format!("{entries_len} traced"), 11.5, theme::TEXT_FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost_button(ui, "CLEAR").clicked() {
                    actions.push(TraceAction::Clear);
                }
            });
        });
        if entries_len == 0 {
            ui.add_space(4.0);
            wrapped(ui, "Aim at something and press the trace key to add it here.", theme::TEXT_FAINT);
            return;
        }
        ui.add_space(4.0);

        let selected = self.trace_menu_selected.unwrap_or(entries_len - 1).min(entries_len - 1);
        let selected_label = {
            let entry = &self.trace_menu_entries[selected];
            format!("#{}  {}  -  {}", selected + 1, entry.kind, entry.title)
        };
        egui::ComboBox::from_id_salt("jka_trace_entry")
            .selected_text(selected_label)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for index in (0..entries_len).rev() {
                    let entry = &self.trace_menu_entries[index];
                    let label = format!("#{}  {}  -  {}", index + 1, entry.kind, entry.title);
                    if ui.selectable_label(selected == index, label).clicked() {
                        actions.push(TraceAction::Select(index));
                    }
                }
            });
        ui.add_space(6.0);

        let entry = &self.trace_menu_entries[selected];
        theme::label(ui, theme::plain(&entry.title, 15.0, theme::TEXT));
        ui.add_space(6.0);

        let can_act = self.local_server.is_some();
        let entity_num = entry.hit_entity_num;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let enabled = can_act && entity_num.is_some();
            let teleport = ui.add_enabled_ui(enabled, |ui| theme::primary_button(ui, "TELEPORT TO"));
            if teleport.inner.clicked() {
                actions.push(TraceAction::TeleportTo(entity_num.expect("gated by enabled")));
            }
            let use_toggle = ui.add_enabled_ui(enabled, |ui| theme::ghost_button(ui, "USE / TOGGLE"));
            if use_toggle.inner.clicked() {
                actions.push(TraceAction::Use(entity_num.expect("gated by enabled")));
            }
        });
        if !can_act {
            wrapped(ui, "TELEPORT TO / USE need a local game (solo map).", theme::TEXT_FAINT);
        } else if entity_num.is_none() {
            wrapped(ui, "This trace didn't hit a live entity (bare world geometry).", theme::TEXT_FAINT);
        }
        ui.add_space(6.0);
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if !entry.summary.is_empty() {
                    for (label, value) in &entry.summary {
                        info_row(ui, label, value);
                    }
                    ui.add_space(8.0);
                }
                for section in &entry.sections {
                    theme::label(ui, theme::plain(&section.title, 10.5, theme::TEXT_FAINT));
                    for line in &section.lines {
                        wrapped(ui, line, theme::TEXT);
                    }
                    ui.add_space(8.0);
                }
            });
    }

    fn apply_trace_action(&mut self, action: TraceAction) {
        match action {
            TraceAction::Close => self.close_trace_overlay(),
            TraceAction::Clear => self.clear_surface_inspection(),
            TraceAction::Select(index) => {
                self.trace_menu_selected = Some(index);
                self.surface_inspector = self.trace_menu_entries.get(index).cloned();
            }
            TraceAction::TeleportTo(entity_num) => self.teleport_to_traced_entity(entity_num),
            TraceAction::Use(entity_num) => self.use_traced_entity(entity_num),
        }
    }

    /// "TELEPORT TO": resolves the entity's live origin from the current
    /// snapshot and reuses the same `setviewpos` path `entities`' GO TO action
    /// already does (`local_setviewpos`), nudged up slightly so the player
    /// doesn't land stuck in the floor.
    fn teleport_to_traced_entity(&mut self, entity_num: u16) {
        if self.local_server.is_none() {
            return;
        }
        let Some(session) = self.game_session.as_ref() else {
            self.push_console_line("^1trace: no active game session.".to_owned());
            return;
        };
        let Some(entity) = session.presented_entities.iter().find(|entity| entity.number == entity_num) else {
            self.push_console_line("^1trace: that entity is no longer present.".to_owned());
            return;
        };
        let origin = [entity.origin[0], entity.origin[1], entity.origin[2] + 8.0];
        let yaw = self.camera.yaw.to_degrees();
        let args = [
            format!("{:.1}", origin[0]),
            format!("{:.1}", origin[1]),
            format!("{:.1}", origin[2]),
            format!("{yaw:.1}"),
        ];
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.local_setviewpos(&refs);
    }

    /// "USE / TOGGLE": fires the traced entity exactly like a player use,
    /// without the proximity/facing check (see `MoverGame::use_entity`).
    fn use_traced_entity(&mut self, entity_num: u16) {
        let Some(server) = self.local_server.as_mut() else {
            return;
        };
        match server.use_entity(entity_num) {
            Ok(()) => {
                self.push_local_snapshot(true);
                self.update_solo_player_view_and_presentation();
                self.publish_ui();
            }
            Err(error) => self.push_console_line(format!("^1trace use: {error}")),
        }
    }
}
