@group(0) @binding(0) var color: texture_2d<f32>;
@vertex fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(color, vec2<i32>(p.xy), 0);
}
