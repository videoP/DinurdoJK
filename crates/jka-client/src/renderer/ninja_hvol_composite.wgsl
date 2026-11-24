// UE HeterogeneousVolumesComposite: accumulated radiance and transmittance
// (shared by every ninja_hvol instance).
@group(0) @binding(0) var hv_radiance: texture_2d<f32>;
@group(0) @binding(1) var hv_upsample: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The selected 2x bilinear upsample followed by the 3x3 Gaussian.
    let size = vec2<i32>(textureDimensions(hv_radiance) * 2u);
    let pixel = vec2<i32>(position.xy);
    var radiance = vec4<f32>(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let coord = clamp(pixel + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            let uv = min(vec2<f32>(coord + 1), vec2<f32>(size - 1)) / vec2<f32>(size);
            let weight = f32((2 - abs(x)) * (2 - abs(y)));
            radiance += textureSampleLevel(hv_radiance, hv_upsample, uv, 0.0) * weight;
        }
    }
    return radiance / 16.0;
}
