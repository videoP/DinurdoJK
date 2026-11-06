// Shitter native BSP, one shared raw image on the existing index/vertex stream.
// No authored JKA material stages, lightmaps, texture changes or extra draws.
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
@group(1) @binding(0) var world_image: texture_2d<f32>;
@group(1) @binding(1) var world_sampler: sampler;

struct TexturedVertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) shade: f32,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> TexturedVertexOut {
    var output: TexturedVertexOut;
    output.position = camera.view_proj * vec4<f32>(position, 1.0);
    output.shade = 0.16 + 0.20 * abs(normal.y) + 0.06 * abs(normal.z);

    // Renderer is Y-up. Project along the dominant face normal, using world
    // units instead of authored UVs. One sample, no triplanar blending.
    // One texture repeat per 128 JKA units, regardless of source image size.
    let axis = abs(normal);
    if (axis.y >= axis.x && axis.y >= axis.z) {
        output.uv = position.xz * (1.0 / 128.0);
    } else if (axis.x >= axis.z) {
        output.uv = position.zy * (1.0 / 128.0);
    } else {
        output.uv = position.xy * (1.0 / 128.0);
    }
    return output;
}

@fragment
fn fs_main(@location(0) shade: f32, @location(1) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let albedo = textureSample(world_image, world_sampler, uv).rgb;
    // A mid-gray texel retains the current flat shade; texture preserves its
    // authored colour and adds low-cost local variation.
    return vec4<f32>(albedo * (shade * 2.0), 1.0);
}
