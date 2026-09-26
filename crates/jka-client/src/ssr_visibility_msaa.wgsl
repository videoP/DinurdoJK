// MSAA form of ssr_visibility.wgsl.
//
// Resolve the closest covered reverse-Z sample after the complete opaque scene
// has rendered. The post/SSR shaders reconstruct world-space distance from this
// value and compare it with the BSP linear-depth prepass.

@group(0) @binding(0) var scene_depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var prepass_depth: texture_depth_2d;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    let p = positions[vertex_index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) f32 {
    let dims = textureDimensions(scene_depth);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let maximum = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = clamp(
        vec2<i32>(
            i32(clamp(input.uv.x, 0.0, 0.999999) * dims_f.x),
            i32(clamp(input.uv.y, 0.0, 0.999999) * dims_f.y)
        ),
        vec2<i32>(0),
        maximum
    );

    var nearest_depth = 0.0;
    let sample_count = textureNumSamples(scene_depth);
    for (var sample = 0u; sample < sample_count; sample = sample + 1u) {
        nearest_depth = max(nearest_depth, textureLoad(scene_depth, pixel, i32(sample)));
    }
    return nearest_depth;
}
