//! Editors.
use crate::app::egui_menu::{
    scene, theme, ui, App, HudElementId, HudLayout, MapEditTool, OverlayMode,
};

impl App {
    pub(in crate::app::egui_menu) fn egui_hud_editor(&mut self, root: &mut egui::Ui) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let pixels_per_point = root.ctx().pixels_per_point().max(0.001);
        let to_points = |value: f32| value / pixels_per_point;
        let painter = root.painter().clone();
        let full = egui::Rect::from_min_size(
            root.max_rect().min,
            egui::vec2(to_points(size.width as f32), to_points(size.height as f32)),
        );

        // Recover from a drag that was interrupted by focus loss, reset, or an
        // egui ownership change. A stale resize state must never disable chat
        // movement/resizing on subsequent frames.
        if self.hud_edit_chat_resize_origin.is_some()
            && !root.input(|input| input.pointer.primary_down())
        {
            self.hud_edit_chat_resize_origin = None;
            self.hud_edit_chat_resize_corner = None;
        }

        // The grid is visual only. Layout values remain framebuffer-pixel offsets
        // from their anchors and are snapped in that same unit, so the renderer
        // never needs to parse cvar text or know that edit mode exists.
        if self.hud_layout.snap_to_grid {
            let spacing_px = self.hud_layout.grid_size.clamp(1.0, 64.0);
            let spacing = to_points(spacing_px);
            let line = egui::Color32::from_rgba_premultiplied(128, 180, 220, 24);
            let major = egui::Color32::from_rgba_premultiplied(128, 180, 220, 42);
            let mut x = full.left();
            let mut column = 0usize;
            while x <= full.right() {
                painter.line_segment(
                    [egui::pos2(x, full.top()), egui::pos2(x, full.bottom())],
                    egui::Stroke::new(1.0_f32, if column % 4 == 0 { major } else { line }),
                );
                x += spacing;
                column += 1;
            }
            let mut y = full.top();
            let mut row = 0usize;
            while y <= full.bottom() {
                painter.line_segment(
                    [egui::pos2(full.left(), y), egui::pos2(full.right(), y)],
                    egui::Stroke::new(1.0_f32, if row % 4 == 0 { major } else { line }),
                );
                y += spacing;
                row += 1;
            }
        }

        let rect_ctx = ui::HudRectContext::new(
            self.movement_keys_hud,
            &self.video,
            &self.perf,
            self.threads.len(),
            &self.japro_cg,
        );
        // The profiler panel is huge; register it first so the elements it
        // overlaps stay on top and remain grabbable.
        let mut draw_order = HudElementId::ALL.to_vec();
        draw_order.sort_by_key(|id| *id != HudElementId::Fps);

        let mut layout_changed = false;
        let mut drag_ended = false;
        for id in draw_order {
            let layout = self.hud_layout.element(id);
            let hud_rect = ui::hud_element_rect(id, layout, &rect_ctx, size.width, size.height);
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    full.left() + to_points(hud_rect.x),
                    full.top() + to_points(hud_rect.y),
                ),
                egui::vec2(to_points(hud_rect.width), to_points(hud_rect.height)),
            );
            // Keep the chat move target away from the corner grips. Previously the
            // full panel and all four resize handles competed for the same pointer
            // press, so egui could give the drag to the panel instead of the grip.
            // Make the handle size physical-pixel based and shrink it as the chat
            // becomes very short so opposite-corner hit boxes never overlap.
            let chat_selected = id == HudElementId::Chat && self.hud_edit_selected == Some(id);
            let chat_handle_px = if chat_selected {
                (hud_rect.height * 0.45).clamp(8.0, 16.0)
            } else {
                0.0
            };
            let move_rect = if chat_selected {
                let inset = to_points(chat_handle_px * 0.65);
                egui::Rect::from_center_size(
                    rect.center(),
                    egui::vec2(
                        (rect.width() - inset * 2.0).max(1.0),
                        (rect.height() - inset * 2.0).max(1.0),
                    ),
                )
            } else {
                rect
            };
            let response = root.interact(
                move_rect,
                egui::Id::new(("hud_edit_item", id)),
                egui::Sense::click_and_drag(),
            );
            let mut chat_resize_active =
                id == HudElementId::Chat && self.hud_edit_chat_resize_origin.is_some();
            if chat_selected {
                // Resize the chat bounds directly from its four corners. The bounds
                // control wrapping/capacity; glyph size remains owned by HUD Scale.
                let handle_size = to_points(chat_handle_px);
                let handles = [
                    (
                        "nw",
                        rect.left_top(),
                        true,
                        true,
                        egui::CursorIcon::ResizeNwSe,
                    ),
                    (
                        "ne",
                        rect.right_top(),
                        false,
                        true,
                        egui::CursorIcon::ResizeNeSw,
                    ),
                    (
                        "sw",
                        rect.left_bottom(),
                        true,
                        false,
                        egui::CursorIcon::ResizeNeSw,
                    ),
                    (
                        "se",
                        rect.right_bottom(),
                        false,
                        false,
                        egui::CursorIcon::ResizeNwSe,
                    ),
                ];
                for (corner, position, move_left, move_top, cursor) in handles {
                    let handle_rect = egui::Rect::from_center_size(
                        position,
                        egui::vec2(handle_size, handle_size),
                    );
                    let handle = root
                        .interact(
                            handle_rect,
                            egui::Id::new(("hud_edit_chat_resize", corner)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(cursor);

                    // Visible diagonal resize grip on the element corner itself.
                    let inward_x = if move_left { 1.0 } else { -1.0 };
                    let inward_y = if move_top { 1.0 } else { -1.0 };
                    let grip_outer = to_points((chat_handle_px * 0.58).max(4.0));
                    for fraction in [0.24_f32, 0.43, 0.62] {
                        let inset = to_points((chat_handle_px * fraction).max(1.0));
                        painter.line_segment(
                            [
                                position + egui::vec2(inward_x * inset, inward_y * grip_outer),
                                position + egui::vec2(inward_x * grip_outer, inward_y * inset),
                            ],
                            egui::Stroke::new(1.5_f32, theme::ACCENT),
                        );
                    }

                    let this_corner = [move_left, move_top];
                    if handle.drag_started() {
                        // Snapshot the immutable start layout. Each resize frame is
                        // rebuilt from this layout plus egui's total drag displacement,
                        // so no per-frame delta can fight the live resized rectangle.
                        self.hud_edit_chat_resize_origin = Some(layout);
                        self.hud_edit_chat_resize_corner = Some(this_corner);
                        self.hud_edit_drag_origin = None;
                        self.hud_edit_drag_delta = [0.0, 0.0];
                        chat_resize_active = true;
                        self.egui_repaint_requested = true;
                    }
                    let active_corner = self.hud_edit_chat_resize_corner == Some(this_corner);
                    if active_corner && handle.dragged() {
                        chat_resize_active = true;
                        let start_layout = self.hud_edit_chat_resize_origin.unwrap_or(layout);
                        let start = ui::hud_element_rect(
                            HudElementId::Chat,
                            start_layout,
                            &rect_ctx,
                            size.width,
                            size.height,
                        );
                        let drag = handle
                            .total_drag_delta()
                            .unwrap_or(egui::Vec2::ZERO)
                            * pixels_per_point;
                        let mut left = start.x;
                        let mut top = start.y;
                        let mut right = start.x + start.width;
                        let mut bottom = start.y + start.height;
                        if move_left {
                            left += drag.x;
                        } else {
                            right += drag.x;
                        }
                        if move_top {
                            top += drag.y;
                        } else {
                            bottom += drag.y;
                        }

                        let alt_bypass = root.input(|input| input.modifiers.alt);
                        if self.hud_layout.snap_to_grid && !alt_bypass {
                            let grid = self.hud_layout.grid_size.clamp(1.0, 64.0);
                            if move_left {
                                left = (left / grid).round() * grid;
                            } else {
                                right = (right / grid).round() * grid;
                            }
                            if move_top {
                                top = (top / grid).round() * grid;
                            } else {
                                bottom = (bottom / grid).round() * grid;
                            }
                        }

                        let mut base_layout = start_layout;
                        base_layout.offset = [0.0, 0.0];
                        base_layout.extent = [1.0, 1.0];
                        let base = ui::hud_element_rect(
                            HudElementId::Chat,
                            base_layout,
                            &rect_ctx,
                            size.width,
                            size.height,
                        );
                        let screen_w = size.width as f32;
                        let screen_h = size.height as f32;
                        let min_w = (base.width * 0.25).min(screen_w);
                        let min_h = (base.height * 0.25).min(screen_h);
                        let max_w = (base.width * 4.0).min(screen_w).max(min_w);
                        let max_h = (base.height * 8.0).min(screen_h).max(min_h);
                        if move_left {
                            left = left.clamp((right - max_w).max(0.0), right - min_w);
                        } else {
                            right = right.clamp(left + min_w, (left + max_w).min(screen_w));
                        }
                        if move_top {
                            top = top.clamp((bottom - max_h).max(0.0), bottom - min_h);
                        } else {
                            bottom = bottom.clamp(top + min_h, (top + max_h).min(screen_h));
                        }

                        let next_extent = [
                            ((right - left) / base.width.max(1.0)).clamp(0.25, 4.0),
                            ((bottom - top) / base.height.max(1.0)).clamp(0.25, 8.0),
                        ];
                        // Chat is bottom-left anchored. Left/top handles change the
                        // offset necessary to keep the opposite edge fixed; right/
                        // bottom handles naturally leave the corresponding origin.
                        let next_offset = [left - base.x, bottom - (base.y + base.height)];
                        let target = self.hud_layout.element_mut(HudElementId::Chat);
                        if target.extent != next_extent || target.offset != next_offset {
                            target.extent = next_extent;
                            target.offset = next_offset;
                            layout_changed = true;
                        }
                    }
                    if active_corner && handle.drag_stopped() {
                        // Keep the parent move response suppressed for this release
                        // frame even though the persistent resize state is cleared.
                        chat_resize_active = true;
                        self.hud_edit_chat_resize_origin = None;
                        self.hud_edit_chat_resize_corner = None;
                        drag_ended = true;
                    }
                }
            }

            if !chat_resize_active && (response.clicked() || response.drag_started()) {
                self.hud_edit_selected = Some(id);
                self.hud_edit_drag_origin = Some(layout.offset);
                self.hud_edit_drag_delta = [0.0, 0.0];
                self.hud_edit_chat_resize_origin = None;
                self.hud_edit_chat_resize_corner = None;
                self.egui_repaint_requested = true;
            }
            if !chat_resize_active && response.dragged() {
                let origin = self.hud_edit_drag_origin.unwrap_or(layout.offset);
                // Use total displacement from the immutable mouse-down snapshot.
                // `drag_delta()` is frame-local in egui and would pull the item
                // back toward its origin on every frame if used this way.
                let delta = response
                    .total_drag_delta()
                    .unwrap_or(egui::Vec2::ZERO)
                    * pixels_per_point;
                self.hud_edit_drag_delta = [delta.x, delta.y];
                let mut offset = [
                    origin[0] + self.hud_edit_drag_delta[0],
                    origin[1] + self.hud_edit_drag_delta[1],
                ];
                let alt_bypass = root.input(|input| input.modifiers.alt);
                if self.hud_layout.snap_to_grid && !alt_bypass {
                    let grid = self.hud_layout.grid_size.clamp(1.0, 64.0);
                    offset[0] = (offset[0] / grid).round() * grid;
                    offset[1] = (offset[1] / grid).round() * grid;
                }
                let target = self.hud_layout.element_mut(id);
                if target.offset != offset {
                    target.offset = offset;
                    layout_changed = true;
                }
            }
            if !chat_resize_active && response.drag_stopped() {
                drag_ended = true;
            }

            let selected = self.hud_edit_selected == Some(id);
            let outline = if selected {
                theme::ACCENT
            } else if response.hovered() {
                theme::TEXT
            } else {
                egui::Color32::from_rgba_premultiplied(170, 205, 230, 150)
            };
            painter.rect_stroke(
                rect.expand(3.0),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, outline),
                egui::StrokeKind::Outside,
            );
            let disabled = match id {
                HudElementId::MovementKeys => self.movement_keys_hud.mode == 0,
                HudElementId::Fps => self.video.draw_fps == 0,
                _ => false,
            };
            let label = if disabled {
                format!("{} (off)", id.label())
            } else {
                id.label().to_owned()
            };
            // Keep the tag on screen for elements hugging the top edge.
            let (label_pos, label_align) = if rect.top() < 24.0 {
                (
                    rect.left_bottom() + egui::vec2(0.0, 8.0),
                    egui::Align2::LEFT_TOP,
                )
            } else {
                (
                    rect.left_top() + egui::vec2(0.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                )
            };
            painter.text(
                label_pos,
                label_align,
                label,
                egui::FontId::proportional(11.0),
                outline,
            );
        }
        if drag_ended {
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
            self.hud_edit_chat_resize_origin = None;
            self.hud_edit_chat_resize_corner = None;
        }

        if let Some(selected) = self.hud_edit_selected {
            let (left, right, up, down, shift) = root.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowLeft),
                    input.key_pressed(egui::Key::ArrowRight),
                    input.key_pressed(egui::Key::ArrowUp),
                    input.key_pressed(egui::Key::ArrowDown),
                    input.modifiers.shift,
                )
            });
            let mut delta = [0.0_f32, 0.0_f32];
            let step = if shift {
                self.hud_layout.grid_size.clamp(1.0, 64.0)
            } else {
                1.0
            };
            if left {
                delta[0] -= step;
            }
            if right {
                delta[0] += step;
            }
            if up {
                delta[1] -= step;
            }
            if down {
                delta[1] += step;
            }
            if delta != [0.0, 0.0] {
                let target = self.hud_layout.element_mut(selected);
                target.offset[0] += delta[0];
                target.offset[1] += delta[1];
                layout_changed = true;
            }

            // HUD edit owns the gameplay surface, so the wheel is free to be a
            // direct scale gesture. Keep it fine-grained; the toolbar slider is
            // still available for exact values.
            let scroll = root.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > f32::EPSILON {
                let target = self.hud_layout.element_mut(selected);
                let next = (target.scale + scroll * 0.0025).clamp(0.5, 2.0);
                if (target.scale - next).abs() > f32::EPSILON {
                    target.scale = next;
                    layout_changed = true;
                }
            }
        }

        let mut close_editor = false;
        let mut reset_selected = false;
        let mut reset_all = false;
        let mut snap = self.hud_layout.snap_to_grid;
        let mut grid = self.hud_layout.grid_size;
        let mut selected_scale = self
            .hud_edit_selected
            .map(|id| self.hud_layout.element(id).scale)
            .unwrap_or(1.0);
        egui::Area::new(egui::Id::new("jka_hud_edit_toolbar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 14.0))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(10, 15, 22, 236))
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::glow_label(ui, "HUD EDIT", 13.0, theme::TEXT);
                            ui.separator();
                            if ui.checkbox(&mut snap, "Snap").changed() {
                                layout_changed = true;
                            }
                            ui.label("Grid");
                            if ui
                                .add(egui::DragValue::new(&mut grid).range(1.0..=64.0).speed(1.0))
                                .changed()
                            {
                                layout_changed = true;
                            }
                            ui.separator();
                            ui.add_enabled_ui(self.hud_edit_selected.is_some(), |ui| {
                                ui.label("Scale");
                                if ui
                                    .add(
                                        egui::Slider::new(&mut selected_scale, 0.5..=2.0)
                                            .show_value(true),
                                    )
                                    .changed()
                                {
                                    layout_changed = true;
                                }
                                if ui.button("RESET SELECTED").clicked() {
                                    reset_selected = true;
                                }
                            });
                            if ui.button("RESET ALL").clicked() {
                                reset_all = true;
                            }
                            if ui.button("DONE").clicked() {
                                close_editor = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            theme::label(
                                ui,
                                theme::plain(
                                    "Drag panels. Mouse wheel scales the selected item. Drag the chat corner grips to resize its wrapping area. ALT bypasses snapping; arrows nudge 1 px; SHIFT+arrow uses the grid step.",
                                    10.5,
                                    theme::TEXT_FAINT,
                                ),
                            );
                        });
                    });
            });

        if self.hud_layout.snap_to_grid != snap {
            self.hud_layout.snap_to_grid = snap;
        }
        let clamped_grid = grid.clamp(1.0, 64.0);
        if (self.hud_layout.grid_size - clamped_grid).abs() > f32::EPSILON {
            self.hud_layout.grid_size = clamped_grid;
        }
        if let Some(selected) = self.hud_edit_selected {
            let scale = selected_scale.clamp(0.5, 2.0);
            let target = self.hud_layout.element_mut(selected);
            if (target.scale - scale).abs() > f32::EPSILON {
                target.scale = scale;
                layout_changed = true;
            }
            if selected == HudElementId::Chat {
                // Scaling glyphs also scales the chat bounds. Keep the current resized
                // extent physically recoverable on-screen; corner dragging is the only
                // UI for changing width/height.
                let mut unit = *target;
                unit.offset = [0.0, 0.0];
                unit.extent = [1.0, 1.0];
                let base = ui::hud_element_rect(
                    HudElementId::Chat,
                    unit,
                    &rect_ctx,
                    size.width,
                    size.height,
                );
                let max_x = (size.width as f32 / base.width.max(1.0)).clamp(0.25, 4.0);
                let max_y = (size.height as f32 / base.height.max(1.0)).clamp(0.25, 8.0);
                let extent = [target.extent[0].min(max_x), target.extent[1].min(max_y)];
                if target.extent != extent {
                    target.extent = extent;
                    layout_changed = true;
                }
            }
        }

        // HUD editor invariant: the resizable chat rectangle remains on-screen.
        // This also clamps a dragged box immediately instead of allowing it to
        // disappear and relying on a later reset to recover it.
        if self.hud_edit_selected == Some(HudElementId::Chat) {
            let layout = self.hud_layout.chat;
            let chat = ui::hud_element_rect(
                HudElementId::Chat,
                layout,
                &rect_ctx,
                size.width,
                size.height,
            );
            let mut correction = [0.0_f32, 0.0_f32];
            if chat.x < 0.0 {
                correction[0] -= chat.x;
            }
            if chat.x + chat.width > size.width as f32 {
                correction[0] -= chat.x + chat.width - size.width as f32;
            }
            if chat.y < 0.0 {
                correction[1] -= chat.y;
            }
            if chat.y + chat.height > size.height as f32 {
                correction[1] -= chat.y + chat.height - size.height as f32;
            }
            if correction != [0.0, 0.0] {
                self.hud_layout.chat.offset[0] += correction[0];
                self.hud_layout.chat.offset[1] += correction[1];
                layout_changed = true;
            }
        }
        if reset_selected {
            if let Some(selected) = self.hud_edit_selected {
                self.hud_layout.reset_element(selected);
                self.hud_edit_drag_origin = None;
                self.hud_edit_drag_delta = [0.0, 0.0];
                self.hud_edit_chat_resize_origin = None;
                self.hud_edit_chat_resize_corner = None;
                layout_changed = true;
            }
        }
        if reset_all {
            self.hud_layout = HudLayout::default();
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
            self.hud_edit_chat_resize_origin = None;
            self.hud_edit_chat_resize_corner = None;
            layout_changed = true;
        }
        if layout_changed {
            self.mark_config_dirty();
            self.publish_ui();
            self.egui_repaint_requested = true;
        }
        if close_editor {
            self.hud_edit_drag_origin = None;
            self.hud_edit_drag_delta = [0.0, 0.0];
            self.hud_edit_chat_resize_origin = None;
            self.hud_edit_chat_resize_corner = None;
            self.set_overlay(OverlayMode::None);
        }
    }

    /// `r_drawEntities` classname labels: one line of text above each
    /// positioned map entity, color-matched to its category and the box drawn
    /// by `App::rebuild_entity_markers`/`DebugVolumeRenderer`. Called first in
    /// `build_egui_menu`, before any menu/overlay content, and painted onto
    /// `ui`'s own layer (not a separate one — a same-order sibling layer
    /// created mid-frame paints on top of the frame's base layer regardless of
    /// call order, which is what put labels over the menu) so immediate-mode
    /// draw order does what it looks like: labels first, so anything the menu
    /// draws afterward on the same layer covers them.
    pub(in crate::app::egui_menu) fn egui_entity_labels(&mut self, ui: &mut egui::Ui) {
        const MAX_LABEL_DISTANCE: f32 = 4096.0;
        let Some(graph) = self.entity_graph.as_deref() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let pixels_per_point = ui.ctx().pixels_per_point().max(0.001);
        let painter = ui.painter().clone();
        let eye = self.camera.position;
        let font = egui::FontId::proportional(12.0);
        for entity in &graph.entities {
            if !entity.positioned || entity.classname.is_empty() {
                continue;
            }
            let origin = self.live_entity_origin(entity);
            let render_point = glam::Vec3::from_array(scene::render_position(origin));
            if eye.distance(render_point) > MAX_LABEL_DISTANCE {
                continue;
            }
            let Some(screen) = self
                .camera
                .project_to_screen(size.width, size.height, render_point)
            else {
                continue;
            };
            let point = egui::pos2(screen.x / pixels_per_point, screen.y / pixels_per_point);
            let [r, g, b, _] = entity.category.color();
            theme::glow_text(
                &painter,
                point,
                egui::Align2::CENTER_BOTTOM,
                &entity.classname,
                font.clone(),
                egui::Color32::from_rgb(r, g, b),
            );
        }
    }

    pub(in crate::app::egui_menu) fn egui_map_editor(&mut self, root: &mut egui::Ui) {
        self.map_edit_toolbar_rect = None;
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        let pixels_per_point = root.ctx().pixels_per_point().max(0.001);
        let viewport_points = egui::vec2(
            size.width as f32 / pixels_per_point,
            size.height as f32 / pixels_per_point,
        );
        let view_proj = self.camera.view_projection(size.width, size.height);
        let project = |point_jka: [f64; 3]| -> Option<egui::Pos2> {
            let render = scene::render_position(point_jka.map(|v| v as f32));
            let clip = view_proj * glam::Vec3::from_array(render).extend(1.0);
            if clip.w <= 0.001 {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            if ndc.z < -0.1 || ndc.z > 1.1 {
                return None;
            }
            Some(egui::pos2(
                (ndc.x * 0.5 + 0.5) * viewport_points.x,
                (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_points.y,
            ))
        };

        if let Some(editor) = self.map_editor.as_ref() {
            let painter = root.painter().clone();
            let changed = editor.changed_geometries();
            let selected = editor.selected_geometry();
            let selected_face = editor.selected.map(|selection| selection.face);

            for geometry in changed {
                for face in geometry.faces {
                    if face.vertices.len() < 2 {
                        continue;
                    }
                    for edge in 0..face.vertices.len() {
                        let a = face.vertices[edge];
                        let b = face.vertices[(edge + 1) % face.vertices.len()];
                        if let (Some(a), Some(b)) = (project(a), project(b)) {
                            painter.line_segment(
                                [a, b],
                                egui::Stroke::new(
                                    0.75_f32,
                                    egui::Color32::from_rgba_unmultiplied(255, 166, 64, 90),
                                ),
                            );
                        }
                    }
                }
            }
            if let Some(geometry) = selected {
                for face in geometry.faces {
                    if face.vertices.len() < 2 {
                        continue;
                    }
                    let selected_face_line = selected_face == Some(face.face_index);
                    for edge in 0..face.vertices.len() {
                        let a = face.vertices[edge];
                        let b = face.vertices[(edge + 1) % face.vertices.len()];
                        if let (Some(a), Some(b)) = (project(a), project(b)) {
                            painter.line_segment(
                                [a, b],
                                egui::Stroke::new(
                                    if selected_face_line {
                                        1.75_f32
                                    } else {
                                        1.0_f32
                                    },
                                    if selected_face_line {
                                        egui::Color32::from_rgba_unmultiplied(255, 214, 92, 210)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 220, 255, 150)
                                    },
                                ),
                            );
                        }
                    }
                }
            }
        }

        let mut save_requested = false;
        let mut revert_requested = false;
        let mut close_requested = false;
        let mut snapshot_refresh_requested = false;
        let toolbar = egui::Area::new(egui::Id::new("jka_map_edit_toolbar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(8, 13, 20, 242))
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        let Some(editor) = self.map_editor.as_mut() else {
                            ui.horizontal(|ui| {
                                theme::glow_label(ui, "MAP EDIT", 13.0, theme::TEXT);
                                ui.label(egui::RichText::new("This source map is not a writable loose .map file.").color(theme::WARNING));
                                if ui.button("DONE").clicked() { close_requested = true; }
                            });
                            return;
                        };

                        ui.horizontal(|ui| {
                            theme::glow_label(ui, "MAP EDIT", 13.0, theme::TEXT);
                            ui.separator();
                            if ui.selectable_label(editor.tool == MapEditTool::MoveBrush, "BRUSH MOVE").clicked() {
                                editor.cancel_drag();
                                snapshot_refresh_requested = true;
                                editor.tool = MapEditTool::MoveBrush;
                                editor.status = "Brush Move: click a brush and drag in the camera plane.".into();
                            }
                            if ui.selectable_label(editor.tool == MapEditTool::MoveFace, "FACE RESIZE").clicked() {
                                editor.cancel_drag();
                                snapshot_refresh_requested = true;
                                editor.tool = MapEditTool::MoveFace;
                                editor.status = "Face Resize: click a face and drag along its normal.".into();
                            }
                            ui.separator();
                            ui.label("Grid");
                            ui.add(egui::DragValue::new(&mut editor.grid).range(0.125..=256.0).speed(1.0));
                            ui.separator();
                            let pending = editor.pending_changes;
                            ui.label(egui::RichText::new(format!("{pending} pending change{}", if pending == 1 { "" } else { "s" }))
                                .color(if pending == 0 { theme::TEXT_DIM } else { theme::WARNING }));
                            if ui.add_enabled(pending != 0, egui::Button::new("SAVE")).clicked() {
                                save_requested = true;
                            }
                            if ui.add_enabled(pending != 0, egui::Button::new("REVERT")).clicked() {
                                revert_requested = true;
                            }
                            if ui.button("DONE").clicked() { close_requested = true; }
                        });
                        ui.horizontal(|ui| {
                            let selection = editor.selected.map_or_else(
                                || "No brush selected".to_owned(),
                                |selection| format!(
                                    "entity {} / brush {} / face {}  {}",
                                    selection.entity,
                                    selection.brush,
                                    selection.face,
                                    editor.selected_shader().unwrap_or("<no shader>")
                                ),
                            );
                            theme::label(ui, theme::plain(
                                &format!("{}  —  {}  —  LMB selects/drags. ESC exits edit mode without discarding pending edits.", selection, editor.status),
                                10.5,
                                theme::TEXT_FAINT,
                            ));
                        });
                    });
            });
        self.map_edit_toolbar_rect = Some(toolbar.response.rect);
        if snapshot_refresh_requested {
            self.publish_snapshot();
        }

        if revert_requested {
            if let Some(editor) = self.map_editor.as_mut() {
                editor.revert_all();
            }
            self.queue_map_edit_preview();
            self.publish_snapshot();
            self.egui_repaint_requested = true;
        }
        if save_requested {
            let result = self.map_editor.as_mut().map(|editor| {
                editor.save()?;
                Ok::<_, String>(editor.source_path.clone())
            });
            match result {
                Some(Ok(path)) => {
                    self.console_status = format!("MAP EDIT SAVED: {}", path.display());
                    self.push_console_path_line(format!("^2{}", self.console_status), path);
                    // Save may happen while a live textured drag preview is still
                    // building. Queue the newest reparsed document instead of
                    // superseding it with another full rebuild immediately.
                    self.queue_map_edit_preview();
                    self.publish_snapshot();
                }
                Some(Err(error)) => {
                    self.console_status = format!("MAP EDIT SAVE FAILED: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                    if let Some(editor) = self.map_editor.as_mut() {
                        editor.status = error;
                    }
                }
                None => {}
            }
            self.egui_repaint_requested = true;
        }
        if close_requested {
            if let Some(editor) = self.map_editor.as_mut() {
                editor.cancel_drag();
            }
            self.set_overlay(OverlayMode::None);
            self.publish_snapshot();
        }
    }
}
