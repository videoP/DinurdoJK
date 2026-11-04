// Pixel-exact fullscreen copy; no texture filtering or color-space conversion.
// Both input and output use render-target pixel coordinates.
@group(0) @binding(0) var source_texture: texture_2d<f32>;

struct Varyings {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> Varyings {
    var points = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: Varyings;
    out.position = vec4<f32>(points[vertex_index], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return textureLoad(source_texture, vec2<i32>(in.position.xy), 0);
}
