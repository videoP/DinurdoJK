@group(0) @binding(0) var reflection_texture: texture_2d_array<f32>;
@group(0) @binding(1) var reflection_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0)
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0)
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let uv = clamp(input.uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let reflected = textureSample(reflection_texture, reflection_sampler, uv, 0);
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    if (edge < 0.012) {
        return vec4<f32>(1.0, 0.1, 0.8, 1.0);
    }
    return vec4<f32>(reflected.rgb, 1.0);
}
