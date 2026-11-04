// Position-only visible solid BSP surfaces, independent of Steam Audio.
// Player geometry and gameplay are unchanged; their materials may be overridden.
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

struct CollisionVertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) shade: f32,
};

@vertex
fn vs_main(@location(0) position_and_shade: vec4<f32>) -> CollisionVertexOut {
    var output: CollisionVertexOut;
    output.position = camera.view_proj * vec4<f32>(position_and_shade.xyz, 1.0);
    output.shade = position_and_shade.w;
    return output;
}

@fragment
fn fs_main(@location(0) shade: f32) -> @location(0) vec4<f32> {
    // Orientation shading is computed only on initial mesh preparation.
    // No texture, lightmap, sampled scene color or additional render pass.
    return vec4<f32>(vec3<f32>(shade), 1.0);
}
