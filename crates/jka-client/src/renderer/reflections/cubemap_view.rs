//! Camera counterpart of rend2 prefilterEnvMap.glsl's six cube directions.
use glam::{Mat4, Vec3};

pub(super) fn view_projection(position: Vec3, face: usize) -> (Mat4, Vec3) {
    let directions = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z];
    let up = match face { 2 => -Vec3::Z, 3 => Vec3::Z, _ => Vec3::Y };
    let convert = |v: Vec3| Vec3::new(v.x, v.z, -v.y);
    let forward = convert(directions[face]);
    let mut projection = glam::camera::rh::proj::directx::perspective_infinite_reverse(
        std::f32::consts::FRAC_PI_2, 1.0, 4.0);
    // wgpu's cube lookup and top-left framebuffer coordinates require this
    // handedness change. The capture pipelines reverse culling to match it.
    projection.x_axis.x = -projection.x_axis.x;
    (projection * glam::camera::rh::view::look_at_mat4(position, position + forward, convert(up)), forward)
}
