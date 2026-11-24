@group(5) @binding(0) var ninja_coarse_color: texture_2d<f32>;
@group(5) @binding(1) var ninja_coarse_depth: texture_depth_2d;
@group(5) @binding(2) var<storage, read> ninja_coarse_tiles: array<u32>;
@group(5) @binding(3) var<uniform> ninja_coarse_params: vec4<f32>;

fn ninja_reconstructible(input: VertexOut) -> bool {
    let dimensions = vec2<i32>(textureDimensions(ninja_coarse_color));
    let tile = vec2<i32>(input.clip.xy) / 16;
    let across = (dimensions.x + 7) / 8;
    if ninja_coarse_tiles[tile.y * across + tile.x] != 0u { return false; }
    // The coarse sample can miss thin geometry. Both entries use this same
    // centre-depth test before either accepts a reconstructed pixel.
    let coarse = min(vec2<i32>(input.clip.xy * camera.viewport.zw * vec2<f32>(dimensions)), dimensions - 1);
    return abs(input.clip.z - textureLoad(ninja_coarse_depth, coarse, 0)) * 600.0 <= 0.5;
}

fn ninja_reconstruct(input: VertexOut) -> vec4<f32> {
    let color = textureSampleLevel(ninja_coarse_color, ninja_clamp, input.clip.xy * camera.viewport.zw, 0.0);
    return ninja_adaptive_debug(vec4<f32>(color.rgb, 1.0), vec3<f32>(0.05, 0.9, 0.15));
}

fn ninja_adaptive_debug(color: vec4<f32>, tint: vec3<f32>) -> vec4<f32> {
    if ninja_coarse_params.z <= 0.5 { return color; }
    return vec4<f32>(mix(color.rgb, tint, 0.65), color.a);
}
