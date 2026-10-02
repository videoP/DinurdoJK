//! The `entities` overlay: a 2D blueprint of every map entity.
//!
//! PLAN shows a top-down floor plan with each entity plotted at its world
//! position (brush entities as their bounding box) and target/targetname links
//! drawn as arrows. FLOW lays the selected entity's connected component out as
//! a left-to-right flowchart (trigger -> relay -> door ...). Both views share
//! one selection; the inspector on the right lists every key/value, the links
//! in and out (click to jump), and can teleport or aim the local player.

use super::egui_theme as theme;
use super::*;
use crate::entity_graph::{
    EntityCategory, EntityGraph, FOOTPRINT_FLOOR, FOOTPRINT_WALL, GraphEntity,
};
use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use std::sync::Arc;

const LIST_ROW_H: f32 = 22.0;
const PICK_RADIUS: f32 = 9.0;
const FLOW_LIMIT: usize = 80;
const FLOW_COLUMN_W: f32 = 270.0;
const FLOW_ROW_H: f32 = 64.0;
const FLOW_NODE: Vec2 = Vec2::new(210.0, 44.0);
const BLUEPRINT: Color32 = Color32::from_rgb(0x0B, 0x1E, 0x35);
const GRID_MINOR: Color32 = Color32::from_rgba_premultiplied(0x10, 0x22, 0x36, 0x40);
const GRID_MAJOR: Color32 = Color32::from_rgba_premultiplied(0x1B, 0x3A, 0x5C, 0x70);
const LINK_DIM: Color32 = Color32::from_rgba_premultiplied(0x22, 0x36, 0x52, 0x50);
const LINK_OUT: Color32 = Color32::from_rgb(0xFF, 0xA9, 0x40);
const LINK_IN: Color32 = Color32::from_rgb(0x4F, 0xE0, 0xF0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum GraphMode {
    #[default]
    Plan,
    Flow,
}

pub(super) struct EntityGraphUi {
    mode: GraphMode,
    /// Identity of the graph the state below belongs to.
    graph_id: usize,
    center: [f32; 2],
    /// Screen points per world unit.
    zoom: f32,
    flow_center: [f32; 2],
    flow_zoom: f32,
    fit_pending: bool,
    flow_fit_pending: bool,
    /// Canvas size the last fit used; egui's zoom factor settles after the
    /// first frame, so an untouched view is re-fitted when the size changes.
    fitted_size: Vec2,
    flow_fitted_size: Vec2,
    /// Set once the user pans/zooms, which stops automatic re-fitting.
    plan_touched: bool,
    flow_touched: bool,
    pub selected: Option<usize>,
    search: String,
    hidden: [bool; EntityCategory::ALL.len()],
    linked_only: bool,
    all_links: bool,
    labels: bool,
    layer: usize,
    textures: Vec<Option<egui::TextureHandle>>,
    scroll_list_to: Option<usize>,
    status: String,
}

impl Default for EntityGraphUi {
    fn default() -> Self {
        Self {
            mode: GraphMode::Plan,
            graph_id: 0,
            center: [0.0; 2],
            zoom: 0.1,
            flow_center: [0.0; 2],
            flow_zoom: 1.0,
            fit_pending: true,
            flow_fit_pending: true,
            fitted_size: Vec2::ZERO,
            flow_fitted_size: Vec2::ZERO,
            plan_touched: false,
            flow_touched: false,
            selected: None,
            search: String::new(),
            hidden: [false; EntityCategory::ALL.len()],
            linked_only: false,
            all_links: true,
            labels: true,
            layer: 0,
            textures: Vec::new(),
            scroll_list_to: None,
            status: String::new(),
        }
    }
}

enum GraphAction {
    GoTo(usize),
    LookAt(usize),
    Copy(String, &'static str),
    /// `t_use <targetname>` on the server.
    Use(String),
    Close,
}

fn category_color(category: EntityCategory) -> Color32 {
    let [r, g, b, _] = category.color();
    Color32::from_rgb(r, g, b)
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn category_index(category: EntityCategory) -> usize {
    EntityCategory::ALL.iter().position(|&c| c == category).unwrap_or(0)
}

/// `classname  targetname` as one line for lists and copies.
fn entity_line(entity: &GraphEntity) -> String {
    if entity.targetname.is_empty() {
        entity.classname.clone()
    } else {
        format!("{}  \"{}\"", entity.classname, entity.targetname)
    }
}

fn entity_text(entity: &GraphEntity) -> String {
    let mut text = String::from("{\n");
    for (key, value) in &entity.properties {
        text.push_str(&format!("\"{key}\" \"{value}\"\n"));
    }
    text.push('}');
    text
}

/// Where the player should stand to see `entity`, in JKA units.
fn stand_position(entity: &GraphEntity) -> [f32; 3] {
    match entity.bounds {
        // Centre of a brush volume is often inside the brush itself; stand on
        // its floor instead.
        Some((mins, _)) => [entity.origin[0], entity.origin[1], mins[2] + 1.0],
        None => entity.origin,
    }
}

impl App {
    pub(super) fn open_entity_graph(&mut self, select: Option<&str>) {
        if self.entity_graph.is_none() {
            self.push_console_line("^1entities: no map with an entity lump is loaded.");
            return;
        }
        self.sync_entity_graph_ui();
        if let Some(query) = select {
            let query = query.to_ascii_lowercase();
            let found = self.entity_graph.as_ref().and_then(|graph| {
                graph.entities.iter().position(|entity| {
                    entity.targetname.eq_ignore_ascii_case(&query) || entity.classname.eq_ignore_ascii_case(&query)
                })
            });
            match found {
                Some(index) => {
                    self.entity_graph_ui.selected = Some(index);
                    self.entity_graph_ui.scroll_list_to = None;
                    self.entity_graph_ui.fit_pending = true;
                    self.entity_graph_ui.flow_fit_pending = true;
                }
                None => self.push_console_line(format!("^3entities: nothing named \"{query}\"; showing all.")),
            }
        }
        self.set_overlay(OverlayMode::EntityGraph);
    }

    /// Reset the view state when the loaded map's graph is not the one it was
    /// built for.
    fn sync_entity_graph_ui(&mut self) {
        let Some(id) = self.entity_graph.as_ref().map(|graph| Arc::as_ptr(graph) as usize) else { return };
        if self.entity_graph_ui.graph_id != id {
            self.entity_graph_ui = EntityGraphUi { graph_id: id, ..EntityGraphUi::default() };
        }
    }

    pub(super) fn egui_entity_graph(&mut self, root: &mut egui::Ui) {
        let Some(graph) = self.entity_graph.clone() else {
            egui::CentralPanel::default().show_inside(root, |ui| {
                theme::banner(ui, "No entity data for this map.", theme::WARNING);
                if theme::ghost_button(ui, "CLOSE").clicked() {
                    self.set_overlay(OverlayMode::None);
                }
            });
            return;
        };
        self.sync_entity_graph_ui();
        let mut st = std::mem::take(&mut self.entity_graph_ui);
        if st.selected.is_some_and(|index| index >= graph.entities.len()) {
            st.selected = None;
        }
        let player = self.entity_graph_player_marker();
        let can_teleport = self.local_server.is_some();
        let can_use = self.live_connected();
        let mut actions = Vec::new();

        self.entity_graph_header(root, &graph, &mut st, &mut actions);
        self.entity_graph_footer(root, &st);
        self.entity_graph_list(root, &graph, &mut st);
        self.entity_graph_inspector(root, &graph, &mut st, can_teleport, can_use, &mut actions);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BLUEPRINT).inner_margin(egui::Margin::ZERO))
            .show_inside(root, |ui| match st.mode {
                GraphMode::Plan => plan_canvas(ui, &graph, &mut st, player),
                GraphMode::Flow => flow_canvas(ui, &graph, &mut st, can_use, &mut actions),
            });

        self.entity_graph_ui = st;
        for action in actions {
            self.apply_entity_graph_action(&graph, action);
        }
    }

    /// Player position (JKA) and yaw (radians) for the plan-view marker.
    fn entity_graph_player_marker(&self) -> Option<([f32; 2], f32)> {
        let position = self.camera.position;
        Some(([position.x, -position.z], self.camera.yaw))
    }

    fn entity_graph_header(
        &mut self,
        root: &mut egui::Ui,
        graph: &EntityGraph,
        st: &mut EntityGraphUi,
        actions: &mut Vec<GraphAction>,
    ) {
        egui::Panel::top("jka_entgraph_header")
            .exact_size(theme::TOP_BAR_H)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(16, 0)),
            )
            .show_inside(root, |ui| {
                ui.horizontal_centered(|ui| {
                    theme::glow_label(ui, "ENTITY BLUEPRINT", 15.0, theme::TEXT);
                    theme::label(
                        ui,
                        theme::plain(
                            &format!("{} entities  {} links", graph.entities.len(), graph.links.len()),
                            11.5,
                            theme::TEXT_FAINT,
                        ),
                    );
                    ui.separator();
                    for (mode, label) in [(GraphMode::Plan, "PLAN"), (GraphMode::Flow, "FLOW")] {
                        if theme::chip(ui, label, st.mode == mode).clicked() {
                            st.mode = mode;
                        }
                    }
                    ui.separator();
                    match st.mode {
                        GraphMode::Plan => {
                            if let Some(footprint) = graph.footprint.as_ref().filter(|f| f.layers.len() > 1) {
                                theme::label(ui, theme::plain("FLOOR", 11.0, theme::TEXT_FAINT));
                                if theme::chip(ui, "ALL", st.layer == 0).clicked() {
                                    st.layer = 0;
                                }
                                for band in 1..footprint.layers.len() {
                                    let [low, high] = footprint.bands[band - 1];
                                    let tip = match (low == f32::MIN, high == f32::MAX) {
                                        (true, _) => format!("below {:.0}", high),
                                        (_, true) => format!("above {:.0}", low),
                                        _ => format!("{:.0} to {:.0}", low, high),
                                    };
                                    if theme::chip(ui, &band.to_string(), st.layer == band)
                                        .on_hover_text(format!("Floors at z {tip}"))
                                        .clicked()
                                    {
                                        st.layer = band;
                                    }
                                }
                                ui.separator();
                            }
                            if theme::chip(ui, "ALL LINKS", st.all_links).clicked() {
                                st.all_links = !st.all_links;
                            }
                            if theme::chip(ui, "LINKED ONLY", st.linked_only).clicked() {
                                st.linked_only = !st.linked_only;
                            }
                            if theme::chip(ui, "LABELS", st.labels).clicked() {
                                st.labels = !st.labels;
                            }
                            if theme::ghost_button(ui, "FIT").clicked() {
                                st.fit_pending = true;
                            }
                        }
                        GraphMode::Flow => {
                            if theme::ghost_button(ui, "FIT").clicked() {
                                st.flow_fit_pending = true;
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::ghost_button(ui, "CLOSE").clicked() {
                            actions.push(GraphAction::Close);
                        }
                    });
                });
            });
    }

    fn entity_graph_footer(&mut self, root: &mut egui::Ui, st: &EntityGraphUi) {
        egui::Panel::bottom("jka_entgraph_footer")
            .exact_size(26.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::CHROME)
                    .inner_margin(egui::Margin::symmetric(16, 0)),
            )
            .show_inside(root, |ui| {
                ui.horizontal_centered(|ui| {
                    let hint = match st.mode {
                        GraphMode::Plan => "Drag: pan   Wheel: zoom   Click: select   ESC: close",
                        GraphMode::Flow => "Drag: pan   Wheel: zoom   Click: select and re-centre   USE chip: t_use that targetname   ESC: close",
                    };
                    theme::label(ui, theme::plain(hint, 10.5, theme::TEXT_FAINT));
                    if !st.status.is_empty() {
                        ui.separator();
                        theme::label(ui, theme::plain(&st.status, 10.5, theme::ACCENT));
                    }
                });
            });
    }

    fn entity_graph_list(&mut self, root: &mut egui::Ui, graph: &EntityGraph, st: &mut EntityGraphUi) {
        egui::Panel::left("jka_entgraph_list")
            .exact_size(300.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show_inside(root, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.search)
                        .hint_text("search class, targetname, value")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for (index, category) in EntityCategory::ALL.iter().copied().enumerate() {
                        let count = graph.entities.iter().filter(|e| e.category == category).count();
                        if count == 0 {
                            continue;
                        }
                        let visible = !st.hidden[index];
                        let text = format!("{} {count}", category.label());
                        let response = theme::chip(ui, &text, visible);
                        let rect = response.rect;
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height())),
                            egui::CornerRadius::ZERO,
                            category_color(category),
                        );
                        if response.clicked() {
                            st.hidden[index] = visible;
                        }
                    }
                });
                ui.add_space(6.0);

                let rows = filtered_entities(graph, st);
                theme::label(ui, theme::plain(&format!("{} shown", rows.len()), 10.5, theme::TEXT_FAINT));
                let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if let Some(row) = st.scroll_list_to.take().and_then(|entity| rows.iter().position(|&r| r == entity)) {
                    scroll = scroll.vertical_scroll_offset(
                        (row as f32 * (LIST_ROW_H + ui.spacing().item_spacing.y) - 120.0).max(0.0),
                    );
                }
                scroll.show_rows(ui, LIST_ROW_H, rows.len(), |ui, range| {
                    for &index in &rows[range] {
                        let entity = &graph.entities[index];
                        let selected = st.selected == Some(index);
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), LIST_ROW_H),
                            egui::Sense::click(),
                        );
                        let fill = match (selected, response.hovered()) {
                            (true, _) => theme::CONTROL_SELECTED,
                            (false, true) => theme::CONTROL_HOVER,
                            (false, false) => Color32::TRANSPARENT,
                        };
                        let painter = ui.painter();
                        painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
                        painter.rect_filled(
                            Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height())),
                            egui::CornerRadius::ZERO,
                            category_color(entity.category),
                        );
                        let font = egui::FontId::proportional(12.0);
                        let class_galley =
                            painter.layout_no_wrap(entity.classname.clone(), font.clone(), theme::TEXT);
                        let class_width = class_galley.size().x;
                        painter.galley(
                            Pos2::new(rect.left() + 10.0, rect.center().y - class_galley.size().y * 0.5),
                            class_galley,
                            theme::TEXT,
                        );
                        if !entity.targetname.is_empty() {
                            painter.with_clip_rect(rect).text(
                                Pos2::new(rect.left() + 16.0 + class_width, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &entity.targetname,
                                font,
                                theme::TEXT_FAINT,
                            );
                        }
                        if response.clicked() {
                            st.selected = Some(index);
                            focus_selected(graph, st);
                        }
                    }
                });
            });
    }

    fn entity_graph_inspector(
        &mut self,
        root: &mut egui::Ui,
        graph: &EntityGraph,
        st: &mut EntityGraphUi,
        can_teleport: bool,
        can_use: bool,
        actions: &mut Vec<GraphAction>,
    ) {
        egui::Panel::right("jka_entgraph_inspector")
            .exact_size(340.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show_inside(root, |ui| {
                let Some(index) = st.selected else {
                    theme::label(ui, theme::plain("Nothing selected", 13.0, theme::TEXT_DIM));
                    ui.add_space(4.0);
                    wrapped(ui, "Click an entity on the plan or in the list. Arrows follow target / targetname.", theme::TEXT_FAINT);
                    return;
                };
                let entity = &graph.entities[index];
                let color = category_color(entity.category);
                theme::label(ui, theme::plain(&entity.classname, 15.0, color));
                if !entity.targetname.is_empty() {
                    theme::label(ui, theme::plain(&format!("targetname  {}", entity.targetname), 12.5, theme::TEXT));
                }
                ui.add_space(6.0);

                let stand = stand_position(entity);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    let go = ui.add_enabled_ui(can_teleport && entity.positioned, |ui| theme::primary_button(ui, "GO TO"));
                    if go.inner.clicked() {
                        actions.push(GraphAction::GoTo(index));
                    }
                    let look = ui.add_enabled_ui(can_teleport && entity.positioned, |ui| theme::ghost_button(ui, "LOOK AT"));
                    if look.inner.on_hover_text("Aim at it from where you are standing").clicked() {
                        actions.push(GraphAction::LookAt(index));
                    }
                    if theme::ghost_button(ui, "FLOW").on_hover_text("Show the flowchart around this entity").clicked() {
                        st.mode = GraphMode::Flow;
                        st.flow_fit_pending = true;
                    }
                    if theme::ghost_button(ui, "FRAME").on_hover_text("Centre the plan on it").clicked() {
                        st.mode = GraphMode::Plan;
                        focus_selected(graph, st);
                    }
                    if theme::ghost_button(ui, "COPY").on_hover_text("Copy the entity as .map text").clicked() {
                        actions.push(GraphAction::Copy(entity_text(entity), "entity"));
                    }
                    if theme::ghost_button(ui, "COPY POS")
                        .on_hover_text("Copy a setviewpos command")
                        .clicked()
                    {
                        actions.push(GraphAction::Copy(
                            format!("setviewpos {:.0} {:.0} {:.0} 0", stand[0], stand[1], stand[2]),
                            "setviewpos",
                        ));
                    }
                });
                // t_use fires every entity carrying that targetname, exactly
                // like a trigger reaching it through `target`.
                let mut target_names: Vec<&str> = graph.outgoing[index]
                    .iter()
                    .map(|&link| graph.entities[graph.links[link].to].targetname.as_str())
                    .filter(|name| !name.is_empty())
                    .collect();
                target_names.sort_by_key(|name| name.to_ascii_lowercase());
                target_names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    ui.add_enabled_ui(can_use, |ui| {
                        if !entity.targetname.is_empty()
                            && theme::ghost_button(ui, "USE THIS")
                                .on_hover_text(format!("t_use {}", entity.targetname))
                                .clicked()
                        {
                            actions.push(GraphAction::Use(entity.targetname.clone()));
                        }
                        if !target_names.is_empty()
                            && theme::ghost_button(ui, &format!("USE TARGETS ({})", target_names.len()))
                                .on_hover_text("t_use each target listed below")
                                .clicked()
                        {
                            actions.extend(target_names.iter().map(|name| GraphAction::Use((*name).to_owned())));
                        }
                    });
                });
                if !can_teleport {
                    wrapped(ui, "GO TO / LOOK AT need a local game (solo map).", theme::TEXT_FAINT);
                }
                if !can_use {
                    wrapped(
                        ui,
                        "USE sends t_use to a connected server (cheats must be enabled). The solo shim has no game module to fire targets.",
                        theme::TEXT_FAINT,
                    );
                }
                ui.add_space(8.0);

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    theme::label(ui, theme::plain("LOCATION", 10.5, theme::TEXT_FAINT));
                    if entity.positioned {
                        info_row(
                            ui,
                            "origin",
                            &format!("{:.0} {:.0} {:.0}", entity.origin[0], entity.origin[1], entity.origin[2]),
                        );
                    } else {
                        info_row(ui, "origin", "none (not placed in the world)");
                    }
                    if let Some(size) = entity.size() {
                        info_row(ui, "brush size", &format!("{:.0} x {:.0} x {:.0}", size[0], size[1], size[2]));
                    }
                    info_row(ui, "lump index", &entity.bsp_index.to_string());
                    ui.add_space(8.0);

                    let outgoing = &graph.outgoing[index];
                    theme::label(ui, theme::plain(&format!("TARGETS  ({})", outgoing.len() + entity.dangling.len()), 10.5, theme::TEXT_FAINT));
                    for &link in outgoing {
                        let link = graph.links[link];
                        link_row(ui, graph, st, link.to, link.key, LINK_OUT, "->", can_use.then_some(&mut *actions));
                    }
                    for (key, value) in &entity.dangling {
                        theme::label(
                            ui,
                            theme::plain(&format!("x  {key}  \"{value}\"  (no entity has that targetname)"), 11.5, theme::DANGER),
                        );
                    }
                    ui.add_space(8.0);

                    let incoming = &graph.incoming[index];
                    theme::label(ui, theme::plain(&format!("TARGETED BY  ({})", incoming.len()), 10.5, theme::TEXT_FAINT));
                    for &link in incoming {
                        let link = graph.links[link];
                        link_row(ui, graph, st, link.from, link.key, LINK_IN, "<-", None);
                    }
                    ui.add_space(8.0);

                    theme::label(
                        ui,
                        theme::plain(&format!("KEYS  ({})", entity.properties.len()), 10.5, theme::TEXT_FAINT),
                    );
                    for (key, value) in &entity.properties {
                        info_row(ui, key, value);
                    }
                });
            });
    }

    fn apply_entity_graph_action(&mut self, graph: &EntityGraph, action: GraphAction) {
        match action {
            GraphAction::Close => self.set_overlay(OverlayMode::None),
            GraphAction::Copy(text, what) => {
                self.entity_graph_ui.status = match crate::clipboard::set_text(&text) {
                    Ok(()) => format!("Copied {what} to the clipboard."),
                    Err(error) => format!("Copy failed: {error}"),
                };
            }
            GraphAction::Use(name) => {
                let status = if self.live_connected() {
                    self.forward_command_to_server(&format!("t_use {name}"));
                    format!("Sent t_use {name} (the server needs cheats enabled).")
                } else {
                    "t_use needs a connected server; the solo shim has no game module.".to_owned()
                };
                self.push_console_line(format!("^2entities:^7 {status}"));
                self.entity_graph_ui.status = status;
            }
            GraphAction::GoTo(index) => {
                let entity = &graph.entities[index];
                let at = stand_position(entity);
                let yaw = self.camera.yaw.to_degrees();
                let args = [
                    format!("{:.1}", at[0]),
                    format!("{:.1}", at[1]),
                    format!("{:.1}", at[2]),
                    format!("{yaw:.1}"),
                ];
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                self.push_console_line(format!("^2entities:^7 go to {}", entity_line(entity)));
                self.set_overlay(OverlayMode::None);
                self.local_setviewpos(&refs);
            }
            GraphAction::LookAt(index) => {
                let entity = &graph.entities[index];
                let Some(server) = self.local_server.as_ref() else { return };
                let feet = crate::scene::jka_position(server.world_position().to_array());
                // Eye height above the player origin.
                let eye = [feet[0], feet[1], feet[2] + 36.0];
                let (dx, dy, dz) = (entity.origin[0] - eye[0], entity.origin[1] - eye[1], entity.origin[2] - eye[2]);
                let yaw = dy.atan2(dx).to_degrees();
                // JKA pitch is positive looking down.
                let pitch = (-dz).atan2((dx * dx + dy * dy).sqrt()).to_degrees();
                let args = [
                    format!("{:.1}", feet[0]),
                    format!("{:.1}", feet[1]),
                    format!("{:.1}", feet[2]),
                    format!("{yaw:.1}"),
                    format!("{pitch:.1}"),
                ];
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                self.push_console_line(format!("^2entities:^7 look at {}", entity_line(entity)));
                self.set_overlay(OverlayMode::None);
                self.local_setviewpos(&refs);
            }
        }
    }
}

pub(super) fn wrapped(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(theme::plain(text, 11.5, color)).wrap().selectable(false));
}

pub(super) fn info_row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        theme::label(ui, theme::plain(key, 11.5, theme::TEXT_FAINT));
        theme::label(ui, theme::plain(value, 11.5, theme::TEXT));
    });
}

fn link_row(
    ui: &mut egui::Ui,
    graph: &EntityGraph,
    st: &mut EntityGraphUi,
    other: usize,
    key: &str,
    color: Color32,
    arrow: &str,
    use_actions: Option<&mut Vec<GraphAction>>,
) {
    let entity = &graph.entities[other];
    let text = format!("{arrow}  {key}   {}", entity_line(entity));
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::Label::new(theme::plain(&text, 11.5, color))
                .selectable(false)
                .sense(egui::Sense::click()),
        );
        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            st.selected = Some(other);
            st.scroll_list_to = Some(other);
            st.flow_fit_pending = st.mode == GraphMode::Flow;
            focus_selected(graph, st);
        }
        if let (Some(actions), false) = (use_actions, entity.targetname.is_empty()) {
            if ui.small_button("USE").on_hover_text(format!("t_use {}", entity.targetname)).clicked() {
                actions.push(GraphAction::Use(entity.targetname.clone()));
            }
        }
    });
}

/// Move the plan view onto the selection (keeping the current zoom, zooming in
/// if the view is very far out) and queue the flow view to re-centre.
fn focus_selected(graph: &EntityGraph, st: &mut EntityGraphUi) {
    let Some(entity) = st.selected.map(|index| &graph.entities[index]) else { return };
    if entity.positioned {
        st.center = [entity.origin[0], entity.origin[1]];
        st.plan_touched = true;
        let extent = entity.size().map_or(0.0, |s| s[0].max(s[1]));
        let want = if extent > 0.0 { 200.0 / extent } else { 0.35 };
        st.zoom = st.zoom.max(want.min(0.6));
    }
    st.flow_fit_pending = true;
}

fn filtered_entities(graph: &EntityGraph, st: &EntityGraphUi) -> Vec<usize> {
    let needle = st.search.trim().to_ascii_lowercase();
    graph
        .entities
        .iter()
        .enumerate()
        .filter(|(_, entity)| {
            if st.hidden[category_index(entity.category)] {
                return false;
            }
            if needle.is_empty() {
                return true;
            }
            entity.classname.to_ascii_lowercase().contains(&needle)
                || entity.targetname.to_ascii_lowercase().contains(&needle)
                || entity.properties.iter().any(|(_, value)| value.to_ascii_lowercase().contains(&needle))
        })
        .map(|(index, _)| index)
        .collect()
}

// ------------------------------------------------------------------- plan --

struct PlanView {
    rect: Rect,
    center: [f32; 2],
    zoom: f32,
}

impl PlanView {
    fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.rect.center().x + (x - self.center[0]) * self.zoom,
            self.rect.center().y - (y - self.center[1]) * self.zoom,
        )
    }

    fn to_world(&self, point: Pos2) -> [f32; 2] {
        [
            self.center[0] + (point.x - self.rect.center().x) / self.zoom,
            self.center[1] - (point.y - self.rect.center().y) / self.zoom,
        ]
    }
}

/// Pan on drag, zoom on wheel about the cursor. Shared by both views.
fn navigate(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    center: &mut [f32; 2],
    zoom: &mut f32,
    zoom_range: (f32, f32),
) -> bool {
    let mut touched = false;
    if response.dragged() {
        let delta = response.drag_delta();
        center[0] -= delta.x / *zoom;
        center[1] += delta.y / *zoom;
        touched = true;
    }
    if !response.hovered() {
        return touched;
    }
    let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
    let factor = (scroll * 0.0035).exp() * pinch;
    if (factor - 1.0).abs() < 1e-4 {
        return touched;
    }
    let Some(cursor) = response.hover_pos() else { return touched };
    let before = PlanView { rect, center: *center, zoom: *zoom };
    let world = before.to_world(cursor);
    *zoom = (*zoom * factor).clamp(zoom_range.0, zoom_range.1);
    // Keep the world point under the cursor fixed.
    center[0] = world[0] - (cursor.x - rect.center().x) / *zoom;
    center[1] = world[1] + (cursor.y - rect.center().y) / *zoom;
    true
}

fn ensure_layer_texture(ctx: &egui::Context, graph: &EntityGraph, st: &mut EntityGraphUi) -> Option<egui::TextureId> {
    let footprint = graph.footprint.as_ref()?;
    let layer = st.layer.min(footprint.layers.len() - 1);
    if st.textures.len() != footprint.layers.len() {
        st.textures = vec![None; footprint.layers.len()];
    }
    if st.textures[layer].is_none() {
        let floor = [70u8, 130, 190, 78];
        let wall = [170u8, 215, 255, 235];
        let mut rgba = Vec::with_capacity(footprint.width * footprint.height * 4);
        for &pixel in &footprint.layers[layer] {
            rgba.extend_from_slice(match pixel {
                FOOTPRINT_FLOOR => &floor,
                FOOTPRINT_WALL => &wall,
                _ => &[0u8; 4],
            });
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([footprint.width, footprint.height], &rgba);
        st.textures[layer] =
            Some(ctx.load_texture(format!("entity-graph-floor-{layer}"), image, egui::TextureOptions::LINEAR));
    }
    st.textures[layer].as_ref().map(egui::TextureHandle::id)
}

fn plan_canvas(ui: &mut egui::Ui, graph: &EntityGraph, st: &mut EntityGraphUi, player: Option<([f32; 2], f32)>) {
    let rect = ui.available_rect_before_wrap();
    let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());
    let painter = painter.with_clip_rect(rect);

    if !st.plan_touched && rect.size() != st.fitted_size {
        st.fit_pending = true;
    }
    if st.fit_pending && rect.width() > 8.0 {
        st.fitted_size = rect.size();
        st.plan_touched = false;
        let span = [(graph.max[0] - graph.min[0]).max(64.0), (graph.max[1] - graph.min[1]).max(64.0)];
        st.zoom = (rect.width() / span[0]).min(rect.height() / span[1]) * 0.92;
        st.center = [(graph.min[0] + graph.max[0]) * 0.5, (graph.min[1] + graph.max[1]) * 0.5];
        st.fit_pending = false;
    }
    st.plan_touched |= navigate(ui, &response, rect, &mut st.center, &mut st.zoom, (0.004, 8.0));
    let view = PlanView { rect, center: st.center, zoom: st.zoom };

    // Grid: pick a power-of-two spacing that lands 40..160 px apart.
    let mut spacing = 32.0_f32;
    while spacing * view.zoom < 40.0 {
        spacing *= 2.0;
    }
    let world_min = view.to_world(rect.left_bottom());
    let world_max = view.to_world(rect.right_top());
    for (step, color) in [(spacing, GRID_MINOR), (spacing * 4.0, GRID_MAJOR)] {
        let mut x = (world_min[0] / step).floor() * step;
        while x <= world_max[0] {
            let screen = view.to_screen(x, 0.0).x;
            painter.line_segment(
                [Pos2::new(screen, rect.top()), Pos2::new(screen, rect.bottom())],
                Stroke::new(1.0_f32, color),
            );
            x += step;
        }
        let mut y = (world_min[1] / step).floor() * step;
        while y <= world_max[1] {
            let screen = view.to_screen(0.0, y).y;
            painter.line_segment(
                [Pos2::new(rect.left(), screen), Pos2::new(rect.right(), screen)],
                Stroke::new(1.0_f32, color),
            );
            y += step;
        }
    }

    if let (Some(texture), Some(footprint)) = (ensure_layer_texture(ui.ctx(), graph, st), graph.footprint.as_ref()) {
        let image_rect = Rect::from_min_max(
            view.to_screen(footprint.min[0], footprint.max[1]),
            view.to_screen(footprint.max[0], footprint.min[1]),
        );
        painter.image(
            texture,
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    let needle = st.search.trim().to_ascii_lowercase();
    let searching = !needle.is_empty();
    let matches = |entity: &GraphEntity| {
        !searching
            || entity.classname.to_ascii_lowercase().contains(&needle)
            || entity.targetname.to_ascii_lowercase().contains(&needle)
            || entity.properties.iter().any(|(_, value)| value.to_ascii_lowercase().contains(&needle))
    };
    let selected = st.selected;
    let mut neighbors = vec![false; graph.entities.len()];
    if let Some(sel) = selected {
        for &link in graph.outgoing[sel].iter() {
            neighbors[graph.links[link].to] = true;
        }
        for &link in graph.incoming[sel].iter() {
            neighbors[graph.links[link].from] = true;
        }
    }
    let visible = |index: usize, entity: &GraphEntity| {
        entity.positioned
            && !st.hidden[category_index(entity.category)]
            && (!st.linked_only
                || Some(index) == selected
                || !graph.outgoing[index].is_empty()
                || !graph.incoming[index].is_empty())
    };

    // Brush volumes first so they sit behind everything else.
    for (index, entity) in graph.entities.iter().enumerate() {
        let (Some((mins, maxs)), true) = (entity.bounds, visible(index, entity)) else { continue };
        let volume = Rect::from_two_pos(view.to_screen(mins[0], mins[1]), view.to_screen(maxs[0], maxs[1]));
        if !volume.intersects(rect) {
            continue;
        }
        let color = category_color(entity.category);
        let dim = if searching && !matches(entity) { 0.3 } else { 1.0 };
        let hot = selected == Some(index);
        painter.rect_filled(volume, egui::CornerRadius::ZERO, with_alpha(color, (if hot { 70.0 } else { 26.0 } * dim) as u8));
        painter.rect_stroke(
            volume,
            egui::CornerRadius::ZERO,
            Stroke::new(if hot { 2.0_f32 } else { 1.0_f32 }, with_alpha(color, (if hot { 255.0 } else { 150.0 } * dim) as u8)),
            egui::StrokeKind::Inside,
        );
    }

    // Links.
    let center_of = |index: usize| {
        let entity = &graph.entities[index];
        entity.positioned.then(|| view.to_screen(entity.origin[0], entity.origin[1]))
    };
    for link in &graph.links {
        let touches = selected == Some(link.from) || selected == Some(link.to);
        if !touches && !st.all_links {
            continue;
        }
        let (from, to) = (&graph.entities[link.from], &graph.entities[link.to]);
        if !visible(link.from, from) || !visible(link.to, to) {
            continue;
        }
        let (Some(a), Some(b)) = (center_of(link.from), center_of(link.to)) else { continue };
        if !Rect::from_two_pos(a, b).expand(4.0).intersects(rect) {
            continue;
        }
        if touches {
            let color = if selected == Some(link.from) { LINK_OUT } else { LINK_IN };
            painter.line_segment([a, b], Stroke::new(1.8_f32, color));
            arrow_head(&painter, a, b, 10.0, color);
        } else {
            painter.line_segment([a, b], Stroke::new(1.0_f32, LINK_DIM));
        }
    }

    // Point entities.
    let hover_pos = response.hover_pos();
    let mut best: Option<(usize, f32)> = None;
    for (index, entity) in graph.entities.iter().enumerate() {
        if !visible(index, entity) {
            continue;
        }
        let at = view.to_screen(entity.origin[0], entity.origin[1]);
        if !rect.expand(12.0).contains(at) {
            continue;
        }
        let color = category_color(entity.category);
        let dim = searching && !matches(entity);
        let hot = selected == Some(index);
        let radius = if hot { 6.5 } else if entity.bounds.is_some() { 3.0 } else { 4.5 };
        let alpha = if dim { 60 } else { 255 };
        painter.circle_filled(at, radius, with_alpha(color, alpha));
        if hot {
            painter.circle_stroke(at, radius + 3.0, Stroke::new(2.0_f32, Color32::WHITE));
        } else if neighbors[index] {
            painter.circle_stroke(at, radius + 3.0, Stroke::new(1.5_f32, with_alpha(color, 220)));
        }
        if let Some(pointer) = hover_pos {
            let distance = at.distance(pointer);
            if distance <= PICK_RADIUS && best.is_none_or(|(_, d)| distance < d) {
                best = Some((index, distance));
            }
        }
    }
    // Brush volumes are pickable by area when no point is under the cursor.
    if best.is_none() {
        if let Some(pointer) = hover_pos {
            let mut smallest = f32::MAX;
            for (index, entity) in graph.entities.iter().enumerate() {
                let (Some((mins, maxs)), true) = (entity.bounds, visible(index, entity)) else { continue };
                let volume = Rect::from_two_pos(view.to_screen(mins[0], mins[1]), view.to_screen(maxs[0], maxs[1]));
                let area = volume.width() * volume.height();
                if volume.contains(pointer) && area < smallest {
                    smallest = area;
                    best = Some((index, 0.0));
                }
            }
        }
    }

    // Labels: the selection, its neighbours and the hovered entity always;
    // everything else once zoomed in far enough for them not to pile up.
    let font = egui::FontId::proportional(11.0);
    let hovered = best.map(|(index, _)| index);
    for (index, entity) in graph.entities.iter().enumerate() {
        if !visible(index, entity) {
            continue;
        }
        let important = selected == Some(index) || neighbors[index] || hovered == Some(index);
        let ambient = st.labels && st.zoom > 0.25 && (!searching || matches(entity));
        if !important && !ambient {
            continue;
        }
        let at = view.to_screen(entity.origin[0], entity.origin[1]);
        if !rect.contains(at) {
            continue;
        }
        let text = entity_line(entity);
        let galley = painter.layout_no_wrap(text, font.clone(), theme::TEXT);
        let label_rect = Rect::from_min_size(at + egui::vec2(8.0, -7.0), galley.size()).expand2(egui::vec2(3.0, 1.0));
        painter.rect_filled(label_rect, egui::CornerRadius::ZERO, Color32::from_rgba_unmultiplied(6, 14, 26, 210));
        painter.galley(label_rect.min + egui::vec2(3.0, 1.0), galley, theme::TEXT);
    }

    if let Some((at, yaw)) = player {
        let center = view.to_screen(at[0], at[1]);
        let forward = Vec2::new(yaw.cos(), -yaw.sin());
        let side = Vec2::new(-forward.y, forward.x);
        painter.add(egui::Shape::convex_polygon(
            vec![center + forward * 11.0, center - forward * 7.0 + side * 6.0, center - forward * 7.0 - side * 6.0],
            Color32::from_rgba_unmultiplied(255, 255, 255, 230),
            Stroke::new(1.0_f32, Color32::BLACK),
        ));
    }

    if response.clicked() {
        st.selected = hovered;
        if let Some(index) = hovered {
            st.scroll_list_to = Some(index);
            st.flow_fit_pending = true;
        }
    }
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn arrow_head(painter: &egui::Painter, from: Pos2, to: Pos2, size: f32, color: Color32) {
    let direction = (to - from).normalized();
    if !direction.x.is_finite() {
        return;
    }
    // Sit the head a little short of the target so it clears the node.
    let tip = to - direction * 7.0;
    let side = Vec2::new(-direction.y, direction.x);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, tip - direction * size + side * size * 0.45, tip - direction * size - side * size * 0.45],
        color,
        Stroke::NONE,
    ));
}

// ------------------------------------------------------------------- flow --

fn flow_canvas(
    ui: &mut egui::Ui,
    graph: &EntityGraph,
    st: &mut EntityGraphUi,
    can_use: bool,
    actions: &mut Vec<GraphAction>,
) {
    let rect = ui.available_rect_before_wrap();
    let (response, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());
    let painter = painter.with_clip_rect(rect);

    let Some(root) = st.selected else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Select an entity to see what triggers it and what it triggers.",
            egui::FontId::proportional(14.0),
            theme::TEXT_DIM,
        );
        return;
    };
    let members = graph.neighborhood(root, FLOW_LIMIT);
    if members.len() == 1 && graph.entities[root].dangling.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "This entity has no target links.",
            egui::FontId::proportional(14.0),
            theme::TEXT_DIM,
        );
        return;
    }

    // Layout: one column per link depth, rows in lump order, each column
    // centred on the vertical middle.
    let columns_min = members.iter().map(|&(_, column)| column).min().unwrap_or(0);
    let columns_max = members.iter().map(|&(_, column)| column).max().unwrap_or(0);
    let mut by_column: Vec<Vec<usize>> = vec![Vec::new(); (columns_max - columns_min + 1) as usize];
    for &(entity, column) in &members {
        by_column[(column - columns_min) as usize].push(entity);
    }
    let mut position = std::collections::HashMap::new();
    for (column, entities) in by_column.iter_mut().enumerate() {
        entities.sort_by_key(|&entity| graph.entities[entity].bsp_index);
        let top = -(entities.len() as f32 - 1.0) * 0.5 * FLOW_ROW_H;
        for (row, &entity) in entities.iter().enumerate() {
            position.insert(entity, [column as f32 * FLOW_COLUMN_W, top + row as f32 * FLOW_ROW_H]);
        }
    }

    if !st.flow_touched && rect.size() != st.flow_fitted_size {
        st.flow_fit_pending = true;
    }
    if st.flow_fit_pending && rect.width() > 8.0 {
        st.flow_fitted_size = rect.size();
        st.flow_touched = false;
        let width = (by_column.len() as f32 - 1.0) * FLOW_COLUMN_W + FLOW_NODE.x + 80.0;
        let tallest = by_column.iter().map(Vec::len).max().unwrap_or(1) as f32;
        let height = (tallest - 1.0) * FLOW_ROW_H + FLOW_NODE.y + 80.0;
        st.flow_zoom = (rect.width() / width).min(rect.height() / height).clamp(0.3, 1.4);
        st.flow_center = [(by_column.len() as f32 - 1.0) * FLOW_COLUMN_W * 0.5, 0.0];
        st.flow_fit_pending = false;
    }
    st.flow_touched |= navigate(ui, &response, rect, &mut st.flow_center, &mut st.flow_zoom, (0.1, 3.0));
    let view = PlanView { rect, center: st.flow_center, zoom: st.flow_zoom };
    // Flow space has Y pointing down; PlanView flips Y, so negate on the way in.
    let node_rect = |entity: usize| {
        let [x, y] = position[&entity];
        Rect::from_center_size(view.to_screen(x, -y), FLOW_NODE * view.zoom)
    };

    // Edges under nodes.
    for link in &graph.links {
        if !position.contains_key(&link.from) || !position.contains_key(&link.to) {
            continue;
        }
        let (a, b) = (node_rect(link.from), node_rect(link.to));
        let (start, end) = (a.right_center(), b.left_center());
        let touches = link.from == root || link.to == root;
        let color = if link.from == root {
            LINK_OUT
        } else if link.to == root {
            LINK_IN
        } else {
            Color32::from_rgba_unmultiplied(120, 170, 220, 170)
        };
        let reach = ((end.x - start.x).abs() * 0.5).max(40.0 * view.zoom);
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [start, start + Vec2::new(reach, 0.0), end - Vec2::new(reach, 0.0), end],
            false,
            Color32::TRANSPARENT,
            Stroke::new(if touches { 2.0_f32 } else { 1.3_f32 }, color),
        ));
        arrow_head(&painter, end - Vec2::new(reach * 0.35, 0.0), end + Vec2::new(7.0, 0.0), 9.0 * view.zoom.max(0.7), color);
        if link.key != "target" {
            painter.text(
                start.lerp(end, 0.5) - Vec2::new(0.0, 8.0),
                egui::Align2::CENTER_BOTTOM,
                link.key,
                egui::FontId::proportional(10.5),
                color,
            );
        }
    }

    let pointer = response.hover_pos();
    let mut hovered = None;
    let mut use_hit = None;
    for &(entity_index, _) in &members {
        let entity = &graph.entities[entity_index];
        let node = node_rect(entity_index);
        if !node.intersects(rect) {
            continue;
        }
        let color = category_color(entity.category);
        let hot = entity_index == root;
        let over = pointer.is_some_and(|p| node.contains(p));
        if over {
            hovered = Some(entity_index);
        }
        painter.rect_filled(
            node,
            egui::CornerRadius::same(3),
            if over { Color32::from_rgb(0x1A, 0x33, 0x52) } else { Color32::from_rgb(0x10, 0x27, 0x44) },
        );
        painter.rect_stroke(
            node,
            egui::CornerRadius::same(3),
            Stroke::new(if hot { 2.5_f32 } else { 1.2_f32 }, if hot { Color32::WHITE } else { color }),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            Rect::from_min_size(node.left_top(), Vec2::new(4.0 * view.zoom.max(0.6), node.height())),
            egui::CornerRadius::same(2),
            color,
        );
        if can_use && view.zoom >= 0.45 && !entity.targetname.is_empty() {
            let scale = view.zoom.clamp(0.7, 1.4);
            let chip = Rect::from_min_size(
                node.right_top() + Vec2::new(-40.0 * scale, 4.0),
                Vec2::new(36.0, 16.0) * scale,
            );
            let chip_hot = pointer.is_some_and(|p| chip.contains(p));
            painter.rect_filled(
                chip,
                egui::CornerRadius::same(2),
                if chip_hot { theme::ACCENT_HOVER } else { theme::ACCENT_DEEP },
            );
            painter.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                "USE",
                egui::FontId::proportional(10.0 * scale),
                Color32::WHITE,
            );
            if chip_hot {
                use_hit = Some(entity_index);
            }
        }
        if view.zoom >= 0.45 {
            let clip = painter.with_clip_rect(node.shrink(2.0));
            let inset = 12.0 * view.zoom;
            clip.text(
                node.left_top() + Vec2::new(inset, 6.0 * view.zoom),
                egui::Align2::LEFT_TOP,
                &entity.classname,
                egui::FontId::proportional(11.0 * view.zoom.clamp(0.7, 1.6)),
                color,
            );
            let (second, second_color) = if entity.targetname.is_empty() {
                ("(no targetname)", theme::TEXT_DISABLED)
            } else {
                (entity.targetname.as_str(), theme::TEXT)
            };
            clip.text(
                node.left_top() + Vec2::new(inset, 23.0 * view.zoom),
                egui::Align2::LEFT_TOP,
                second,
                egui::FontId::proportional(12.5 * view.zoom.clamp(0.7, 1.6)),
                second_color,
            );
        }
    }
    if response.clicked() {
        if let Some(index) = use_hit {
            actions.push(GraphAction::Use(graph.entities[index].targetname.clone()));
        } else if let Some(index) = hovered {
            st.selected = Some(index);
            st.scroll_list_to = Some(index);
            st.flow_fit_pending = true;
        }
    }
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
