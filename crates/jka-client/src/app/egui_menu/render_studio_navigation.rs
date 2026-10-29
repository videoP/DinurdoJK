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
        let pressed_here = response.contains_pointer() && ui.ctx().input(|i| {
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
            && !self.render_studio.background_popup_open
            && !egui::Popup::is_any_open(ui.ctx())
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
        // Keys first pressed inside a Ctrl/Alt chord (Ctrl+A select all,
        // Ctrl+D ...) never move the camera, even if the modifier is released
        // before the letter: that used to throw the view into Scene mode.
        let chord_id = egui::Id::new("studio_navigation_chorded_keys");
        // Context accessors share a lock: read stored keys before taking the
        // input lock, otherwise selecting the preview deadlocks the UI thread.
        let mut chorded: Vec<egui::Key> = ui.ctx().data(|d| d.get_temp(chord_id)).unwrap_or_default();
        let chorded = ui.ctx().input(|i| {
            let keys = &mut chorded;
            keys.retain(|k| i.key_down(*k));
            if i.modifiers.ctrl || i.modifiers.command || i.modifiers.alt {
                for k in [egui::Key::W, egui::Key::A, egui::Key::S, egui::Key::D, egui::Key::E, egui::Key::C] {
                    if i.key_down(k) && !keys.contains(&k) { keys.push(k); }
                }
            }
            keys.clone()
        });
        ui.ctx().data_mut(|d| d.insert_temp(chord_id, chorded.clone()));
        let (movement, dt) = ui.ctx().input(|i| {
            let key = |k| if i.key_down(k) && !chorded.contains(&k) { 1.0 } else { 0.0 };
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
        // Entering Scene view from demo footage starts at the camera the demo
        // sampler actually rendered, not the stage key (which sits near the
        // map origin, often under the floor).
        let demo_shot = self.render_studio.spatial.scene_camera.is_none()
            && self.render_studio.cinematic_preview.is_none()
            && self.render_studio.timeline.demo_at(self.render_studio.timeline_time()).is_some();
        let base = if demo_shot {
            self.render_studio.scene_frame.as_ref().map(|frame| frame.outgoing.shot_camera)
        } else { None };
        let mut camera = key_from_camera(base.unwrap_or_else(|| self.asset_preview_camera()));
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
