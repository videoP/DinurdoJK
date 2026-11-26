// Exact integer-ratio box integration of the rendered subpixel grid.
const FACTOR: i32 = FACTOR_VALUE;
@group(0) @binding(0) var source: texture_2d<f32>;
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}
@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let base = vec2i(pos.xy) * FACTOR;
    var sum = vec4f(0.0);
    for (var y = 0i; y < FACTOR; y++) {
        for (var x = 0i; x < FACTOR; x++) {
            sum += textureLoad(source, base + vec2i(x,y), 0i);
        }
    }
    return sum / f32(FACTOR * FACTOR);
}
