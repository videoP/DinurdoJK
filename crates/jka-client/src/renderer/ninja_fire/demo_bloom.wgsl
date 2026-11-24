struct Filter {
    axis_tint: vec4<f32>,
    taps: array<vec4<f32>, 32>,
}

@group(0) @binding(0) var source_image: texture_2d<f32>;
@group(0) @binding(1) var coarser_bloom: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
@group(0) @binding(3) var<uniform> blur_uniform: Filter;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn fullscreen(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Vertex(vec4<f32>(2.0 * uv.x - 1.0, 1.0 - 2.0 * uv.y, 0.0, 1.0), uv);
}

fn read_source(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source_image, bilinear, uv, 0.0).rgb;
}

@fragment
fn half_res(input: Vertex) -> @location(0) vec4<f32> {
    return vec4<f32>(read_source(input.uv), 0.0);
}

@fragment
fn extract(input: Vertex) -> @location(0) vec4<f32> {
    // PostProcessBloom.usf BloomSetupCommon, with the Fire Smoke threshold.
    let color = read_source(input.uv);
    let luma = dot(vec3<f32>(0.2126, 0.7152, 0.0722), color);
    return vec4<f32>(color * clamp((luma - 1.0) * 0.5, 0.0, 1.0), 0.0);
}

@fragment
fn downsample(input: Vertex) -> @location(0) vec4<f32> {
    // High-quality PostProcessDownsample, four bilinear samples.
    let texel = 1.0 / vec2<f32>(textureDimensions(source_image));
    let a = read_source(input.uv - texel);
    let b = read_source(input.uv + texel);
    let c = read_source(input.uv + vec2<f32>(texel.x, -texel.y));
    let d = read_source(input.uv + vec2<f32>(-texel.x, texel.y));
    return vec4<f32>(max((a + b + c + d) * 0.25, vec3<f32>(0.0)), 0.0);
}

@fragment
fn gaussian(input: Vertex) -> @location(0) vec4<f32> {
    var sum = vec3<f32>(0.0);
    for (var index = 0u; index < 32u; index++) {
        let tap = blur_uniform.taps[index].xy;
        if (tap.y <= 0.0) { break; }
        sum += read_source(input.uv + tap.x * blur_uniform.axis_tint.xy) * tap.y;
    }
    if (blur_uniform.axis_tint.w != 0.0) {
        sum += textureSampleLevel(coarser_bloom, bilinear, input.uv, 0.0).rgb;
    }
    return vec4<f32>(sum, 0.0);
}
