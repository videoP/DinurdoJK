// Native ShitteR grid: slope classification was cached per *triangle* on
// activation, and the CPU draw list submits each class through its own PSO.
// No normal-based color branching (or per-fragment triangle slope math).
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
@group(1) @binding(0) var orange_grid: texture_2d<f32>;
@group(1) @binding(1) var gray_grid: texture_2d<f32>;
@group(1) @binding(2) var world_sampler: sampler;
override GRID_GRAY: bool = false;

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
    // Specialization removes the unused texture and branch from each PSO.
    var albedo: vec3<f32>;
    if (GRID_GRAY) {
        albedo = textureSample(gray_grid, world_sampler, uv).rgb;
    } else {
        albedo = textureSample(orange_grid, world_sampler, uv).rgb;
    }
    return vec4<f32>(albedo * (shade * 2.0), 1.0);
}
