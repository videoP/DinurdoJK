// Preserve the current lightmap RGB (including AO) and replace only sun flux.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var mask: texture_2d<f32>;
@group(0) @binding(2) var mask_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let rgb = textureLoad(source, vec2<i32>(position.xy), 0).rgb;
    let uv = position.xy / vec2<f32>(textureDimensions(source));
    let flux = textureSampleLevel(mask, mask_sampler, uv, 0.0).r;
    return vec4<f32>(rgb, flux);
}
