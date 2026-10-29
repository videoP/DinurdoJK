//! Preview-only free flight. Timeline keys are authored only by explicit actions.
use super::render_studio_timeline::StudioCameraKey;
use super::{App, FrontendPage};
use glam::Vec3;

pub(in crate::app) struct StudioPreviewNavigation {
    pub selected: bool,
    pub looking: bool,
    pub raw_delta: [f32; 2],
    pub speed: f32,
}

impl Default for StudioPreviewNavigation {
    fn default() -> Self {
        Self {
            selected: false,
            looking: false,
            raw_delta: [0.0; 2],
            speed: 160.0,
        }
    }
}

impl StudioPreviewNavigation {
    pub fn change_speed(&mut self, wheel_lines: f32) {
        if wheel_lines.is_finite() {
            self.speed =
                (self.speed * 1.25_f32.powf(wheel_lines.clamp(-20.0, 20.0))).clamp(1.0, 10000.0);
        }
    }

    pub fn step(
        &mut self,
        camera: &mut StudioCameraKey,
        movement: Vec3,
        dt: f32,
        mouse_scale: [f32; 2],
    ) -> bool {
        let mouse = std::mem::take(&mut self.raw_delta);
        if !self.selected {
            return false;
        }
        let looking = self.looking && mouse != [0.0; 2];
        if looking {
            // Same yaw/pitch signs and sensitivity as JKA's free camera.
            camera.yaw = (camera.yaw - mouse[0] * mouse_scale[0]).rem_euclid(360.0);
            camera.pitch = (camera.pitch - mouse[1] * mouse_scale[1]).clamp(-89.0, 89.0);
        }
        let moving = movement.length_squared() > 0.0 && dt > 0.0;
        if moving {
            let yaw = camera.yaw.to_radians();
            let pitch = camera.pitch.to_radians();
            let forward = Vec3::new(
                yaw.cos() * pitch.cos(),
                pitch.sin(),
                -yaw.sin() * pitch.cos(),
            );
            let right = Vec3::new(yaw.sin(), 0.0, yaw.cos());
            let direction = (right * movement.x + Vec3::Y * movement.y + forward * movement.z)
                .normalize_or_zero();
            let delta = direction * self.speed * dt.clamp(0.0, 0.05);
            camera.position = (Vec3::from_array(camera.position) + delta)
                .clamp(Vec3::splat(-50000.0), Vec3::splat(50000.0))
                .to_array();
        }
        looking || moving
    }
}

impl App {
    pub(in crate::app) fn studio_navigation_window_event(
        &mut self,
        event: &winit::event::WindowEvent,
    ) -> bool {
        if !self.render_studio.navigation.looking {
            return false;
        }
        use winit::{
            event::{ElementState, MouseButton, WindowEvent},
            keyboard::{KeyCode, PhysicalKey},
        };
        let escape = matches!(event, WindowEvent::KeyboardInput { event, .. }
            if event.state == ElementState::Pressed && event.physical_key == PhysicalKey::Code(KeyCode::Escape));
        if escape
            || !self.studio_navigation_available()
            || matches!(
                event,
                WindowEvent::Focused(false)
                    | WindowEvent::CloseRequested
                    | WindowEvent::MouseInput {
                        state: ElementState::Released,
                        button: MouseButton::Right,
                        ..
                    }
            )
        {
            self.set_studio_mouse_look(false);
            if escape || matches!(event, WindowEvent::Focused(false)) {
                self.render_studio.navigation.selected = false;
            }
        }
        escape
    }

    pub(in crate::app) fn studio_navigation_available(&self) -> bool {
        self.frontend_page == FrontendPage::RenderStudio
            && (self.profile_preview_active || self.render_studio.spatial.scene_camera.is_some())
            && self.window_focused
            && matches!(self.overlay, super::OverlayMode::None | super::OverlayMode::Game)
            && (self.front_end || self.overlay == super::OverlayMode::Game)
            && self.render_studio.export.is_none()
            && self.render_studio.still.is_none()
    }

    pub(in crate::app) fn set_studio_mouse_look(&mut self, active: bool) {
        let active = active && self.studio_navigation_available();
        if self.render_studio.navigation.looking == active {
            return;
        }
        self.render_studio.navigation.looking = active;
        self.render_studio.navigation.raw_delta = [0.0; 2];
        if let Some(window) = &self.window {
            if active {
                let _ = window
                    .set_cursor_grab(winit::window::CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
                window.set_cursor_visible(false);
            } else if !self.captured {
                let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
                window.set_cursor_visible(true);
            }
        }
    }

    pub(in crate::app) fn capture_studio_preview_camera(&mut self, time: f64) {
        let is_demo=self.render_studio.timeline.demo_at(time).is_some();
        let preview_camera=if is_demo {
            self.render_studio.scene_frame.as_ref().map(|scene|
                if scene.progress>=0.5 {scene.incoming.as_ref().unwrap_or(&scene.outgoing).camera}
                else {scene.outgoing.camera})
                .unwrap_or_else(||self.asset_preview_camera())
        } else {self.asset_preview_camera()};
        let mut key = super::render_studio_spatial::key_from_camera(preview_camera);
        let previous=self.render_studio.timeline.camera_at(time);
        let inherited=previous.source;
        // A new camera key inherits the current per-shot demo rig as well as
        // its source, otherwise capturing a shot silently resets distance/FOV.
        key.demo_follow=previous.demo_follow;
        key.lens=previous.lens;
        key.focus_target=previous.focus_target.map(|_|(preview_camera.position+preview_camera.forward()*previous.lens.focus_distance).to_array());
        key.lens.aperture=key.lens.aperture.map(|n|n.min(128.0));
        key.source=inherited;
        if is_demo && !matches!(inherited,super::render_studio_timeline::StudioCameraSource::Demo{..}) {
            Self::studio_default_demo_camera(&mut key);
            key.fov=self.studio_default_fov();
        }
        if !is_demo && self.render_studio.timeline.camera_rig.position.enabled {
            let mut captured_orbit=false;
            if let Some(frame)=self.studio_orbit_edit_frame(self.render_studio.timeline_time()) {
                let mut binding=self.render_studio.timeline.camera_rig.position.clone();
                binding.offset=frame.offset(key.position,binding.local_offset);
                if let Some(orbit)=super::render_studio_camera::StudioCameraOrbit::from_binding(&binding,frame,[0.;3]) {
                    if self.set_studio_orbit_key(time,orbit) {
                        key.position=self.render_studio.timeline.camera_rig.reference_position;
                        key.rig_orbit=Some(orbit);captured_orbit=true;
                    }
                }
            }
            // A positional rig adds its displacement to the authored key. Store
            // the inverse so capturing a resolved shot does not apply it twice.
            if !captured_orbit {
                let shot = self.studio_shot_camera();
                let base = self.render_studio.timeline.camera_at(self.render_studio.timeline_time());
                key.position=std::array::from_fn(|a|base.position[a]+key.position[a]-shot.position[a]);
            }
        }
        key.time = time;
        self.render_studio.timeline.add_camera_key(key);
        self.profile_preview_key = None;
        self.render_studio.status = if self.render_studio.timeline.camera_rig.target.enabled {
            "Captured preview camera; the camera rig still controls shot aim".into()
        } else {
            "Camera key captured from the current preview view".into()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn studio_preview_seeking_and_navigation_do_not_author_keys_and_capture_inherits_view() {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let event_loop =
            winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
                .with_any_thread(true)
                .build()
                .unwrap();
        let fixture = crate::cvar_compatibility::Fixture::new();
        let mut app = App::new(
            fixture.0.clone(),
            None,
            crate::scene::MapSource::Bsp("mp/ffa3".into()),
            false,
            event_loop.create_proxy(),
        )
        .unwrap();
        app.frontend_page = FrontendPage::RenderStudio;
        app.profile_preview_active = true;
        let count = app.render_studio.timeline.camera.len();
        let mut view = StudioCameraKey {
            position: [12.0, 34.0, 56.0],
            yaw: 110.0,
            pitch: -25.0,
            fov: 83.0,
            ..Default::default()
        };
        app.render_studio.navigation.selected = true;
        app.render_studio
            .navigation
            .step(&mut view, Vec3::Z, 0.016, [0.2; 2]);
        app.render_studio.spatial.scene_camera = Some(view);
        app.render_studio.timeline.seek(0.5);
        assert_eq!(app.render_studio.timeline.camera.len(), count);
        app.capture_studio_preview_camera(0.5);
        assert_eq!(app.render_studio.timeline.camera.len(), count + 1);
        let captured =
            app.render_studio.timeline.camera[app.render_studio.timeline.selected_camera.unwrap()];
        assert_eq!(captured.time, 0.5);
        assert_eq!(captured.position, view.position);
        assert!((captured.yaw - view.yaw).abs() < 0.0001);
        assert!((captured.pitch - view.pitch).abs() < 0.0001);
        assert!((captured.fov - view.fov).abs() < 0.0001);
        // Demo worlds disable the isolated-asset preview flag. The independent
        // Scene observer must still supply its camera and accept navigation.
        app.profile_preview_active=false;app.window_focused=true;
        app.overlay=super::super::OverlayMode::None;
        assert!(app.studio_navigation_available());
        assert_eq!(app.asset_preview_camera().position.to_array(),view.position);
    }
    #[test]
    fn studio_free_flight_uses_jka_axes_normalizes_motion_and_requires_selection() {
        let mut nav = StudioPreviewNavigation::default();
        let mut camera = StudioCameraKey {
            position: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        assert!(!nav.step(&mut camera, Vec3::Z, 0.05, [0.2; 2]));
        assert_eq!(camera.position, [0.0; 3]);
        nav.selected = true;
        nav.step(&mut camera, Vec3::Z, 0.05, [0.2; 2]);
        assert_eq!(camera.position, [8.0, 0.0, 0.0]);
        camera.position = [0.0; 3];
        nav.step(&mut camera, Vec3::X + Vec3::Y + Vec3::Z, 0.05, [0.2; 2]);
        assert!((Vec3::from_array(camera.position).length() - 8.0).abs() < 0.0001);
        let before = camera.position;
        nav.step(&mut camera, Vec3::Y, 0.05, [0.2; 2]);
        assert_eq!(camera.position[1], before[1] + 8.0);
    }
    #[test]
    fn studio_free_flight_looks_in_place_and_wheel_only_changes_speed() {
        let mut nav = StudioPreviewNavigation {
            selected: true,
            looking: true,
            ..Default::default()
        };
        let mut camera = StudioCameraKey::default();
        let position = camera.position;
        nav.raw_delta = [10.0, 1000.0];
        nav.step(&mut camera, Vec3::ZERO, 0.016, [0.2; 2]);
        assert_eq!(camera.position, position);
        assert_eq!(camera.yaw, 358.0);
        assert_eq!(camera.pitch, -89.0);
        nav.change_speed(1.0);
        assert_eq!(nav.speed, 200.0);
        nav.change_speed(-1.0);
        assert_eq!(nav.speed, 160.0);
        assert_eq!(camera.position, position);
        assert_eq!(nav.raw_delta, [0.0; 2]);
    }
}
