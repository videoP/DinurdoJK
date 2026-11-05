//! Studio camera keys as cameras, and actor stage placement in JKA coordinates.
use crate::render_studio_timeline::StudioCameraKey;
use jka_render_core::camera::Camera;

pub fn camera_from_key(key:StudioCameraKey)->Camera {
    let mut camera=Camera::new_with_fov(jka_render_core::coordinates::render_position(key.position),key.yaw.to_radians(),key.fov);
    camera.pitch=-key.pitch.to_radians();key.lens_for_camera(&mut camera);camera
}
pub fn key_from_camera(camera:Camera)->StudioCameraKey {
    StudioCameraKey {position:jka_render_core::coordinates::jka_position(camera.position.to_array()),yaw:camera.yaw.to_degrees(),
        pitch:-camera.pitch.to_degrees(),fov:camera.cg_fov(),..Default::default()}
}
/// Stage placement in native JKA coordinates, shared by actors and authoring tools.
pub fn motion_origin(position:[f32;3],lane:usize,zoom:f32)->[f32;3] {
    [88.*zoom.clamp(0.45,2.5)+position[0],lane as f32*26.+position[1],-22.+position[2]]
}
pub fn motion_world(position:[f32;3],lane:usize,zoom:f32)->[f32;3] {
    jka_render_core::coordinates::render_position(motion_origin(position,lane,zoom))
}
