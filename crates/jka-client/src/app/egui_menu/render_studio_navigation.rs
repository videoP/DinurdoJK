use super::App;
use crate::app::render_studio_spatial::key_from_camera;

impl App {
    pub(super) fn egui_studio_preview_navigation(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        overlay_busy: bool,
        locked: bool,
    ) {
        let pressed_here = ui.ctx().input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| response.rect.contains(p))
        });
        if pressed_here && !overlay_busy && !locked {
            self.render_studio.navigation.selected = true;
            response.request_focus();
        }
        let can_navigate = !locked
            && self.studio_navigation_available()
            && self.render_studio.navigation.selected
            && !ui.ctx().text_edit_focused();
        if !can_navigate || !ui.ctx().input(|i| i.pointer.secondary_down()) {
            self.set_studio_mouse_look(false);
        } else if pressed_here && !overlay_busy && ui.ctx().input(|i| i.pointer.secondary_pressed())
        {
            self.set_studio_mouse_look(true);
        }
        if !can_navigate {
            return;
        }
        if self.render_studio.navigation.looking {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
        let scroll = if !overlay_busy && response.hovered() || self.render_studio.navigation.looking
        {
            ui.ctx().input_mut(|i| {
                let mut lines = 0.0;
                i.events.retain(|event| {
                    if let egui::Event::MouseWheel { unit, delta, .. } = event {
                        lines += delta.y
                            * match unit {
                                egui::MouseWheelUnit::Line => 1.0,
                                egui::MouseWheelUnit::Point => 1.0 / 40.0,
                                egui::MouseWheelUnit::Page => 3.0,
                            };
                        false
                    } else {
                        true
                    }
                });
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                lines
            })
        } else {
            0.0
        };
        self.render_studio.navigation.change_speed(scroll);
        let (movement, dt) = ui.ctx().input(|i| {
            let key = |k| if i.key_down(k) { 1.0 } else { 0.0 };
            let movement = if i.modifiers.ctrl || i.modifiers.command || i.modifiers.alt {
                glam::Vec3::ZERO
            } else {
                glam::Vec3::new(
                    key(egui::Key::D) - key(egui::Key::A),
                    key(egui::Key::E) - key(egui::Key::C),
                    key(egui::Key::W) - key(egui::Key::S),
                )
            };
            (movement, i.stable_dt)
        });
        let mut camera = key_from_camera(self.asset_preview_camera());
        let scale = [
            self.mouse_input.sensitivity * self.mouse_input.yaw,
            self.mouse_input.sensitivity * self.mouse_input.pitch,
        ];
        if self
            .render_studio
            .navigation
            .step(&mut camera, movement, dt, scale)
        {
            if let Some(preview) = self.render_studio.cinematic_preview.as_mut() {
                preview.camera = camera;
            } else {
                self.render_studio.spatial.scene_camera = Some(camera);
            }
            self.profile_preview_key = None;
            ui.ctx().request_repaint();
        }
    }
}
