// Shitter: native compiled BSP geometry, PVS and index ranges.
// No material, texture, lightmap, or acoustic/collision geometry is accessed.
struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
    unjittered_view_proj: mat4x4<f32>,
    previous_unjittered_view_proj: mat4x4<f32>,
    jump_shade: vec4<f32>,
    foliage_wind: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct FlatVertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) shade: f32,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> FlatVertexOut {
    var output: FlatVertexOut;
    output.position = camera.view_proj * vec4<f32>(position, 1.0);
    // Deliberately inexpensive, high-contrast direction-based coloration.
    // Native BSP normals are already available; no secondary vertex buffer.
    // BSP normals are already unit length; no extra normalize/sqrt.
    output.shade = 0.16 + 0.20 * abs(normal.y) + 0.06 * abs(normal.z);
    return output;
}

@fragment
fn fs_main(@location(0) shade: f32) -> @location(0) vec4<f32> {
    return vec4<f32>(vec3<f32>(shade), 1.0);
}
