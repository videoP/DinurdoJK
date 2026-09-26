// Full-resolution resolved scene depth used by SSR.
//
// The linear-depth prepass holds BSP and depth-writing entities, but not grass,
// promoted ocean or other main-pass-only depth writers. SSR resolves the final
// depth here to reject reflections under those. Fog does not use this.

@group(0) @binding(0) var scene_depth: texture_depth_2d;
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
    return max(textureLoad(scene_depth, pixel, 0), 0.0);
}
