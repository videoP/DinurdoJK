// Half-float distance and octahedral 8+8-bit normal fit in one word. This lets
// the common one-sample mode keep both visibility and validation in one texel.
fn rt_rate_pack_surface(depth: f32, normal: vec3<f32>) -> u32 {
    var oct = normal.xy / (abs(normal.x) + abs(normal.y) + abs(normal.z));
    if (normal.z < 0.0) { oct = (1.0 - abs(oct.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), oct >= vec2<f32>(0.0)); }
    return (pack2x16float(vec2<f32>(depth, 0.0)) & 65535u)
        | ((pack4x8snorm(vec4<f32>(oct, 0.0, 0.0)) & 65535u) << 16u);
}

fn rt_rate_depth(surface: u32) -> f32 {
    return unpack2x16float(surface & 65535u).x;
}

fn rt_rate_normal(surface: u32) -> vec3<f32> {
    let oct = unpack4x8snorm(surface >> 16u).xy;
    var normal = vec3<f32>(oct, 1.0 - abs(oct.x) - abs(oct.y));
    if (normal.z < 0.0) { normal = vec3<f32>((1.0 - abs(normal.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), normal.xy >= vec2<f32>(0.0)), normal.z); }
    return normalize(normal);
}
