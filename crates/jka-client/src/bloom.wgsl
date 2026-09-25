@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

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

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn source(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(
        source_texture,
        source_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
}

fn tent_downsample(uv: vec2<f32>) -> vec3<f32> {
    let dims_u = textureDimensions(source_texture);
    let texel = 1.0 / vec2<f32>(f32(dims_u.x), f32(dims_u.y));
    // Much wider neutral veiling glare with the same five taps and same three
    // pyramid passes. Each lower-resolution stage compounds this footprint.
    let diagonal = texel * 4.25;
    var color = source(uv) * 0.25;
    color += source(uv + vec2<f32>(-diagonal.x, -diagonal.y)) * 0.1875;
    color += source(uv + vec2<f32>( diagonal.x, -diagonal.y)) * 0.1875;
    color += source(uv + vec2<f32>(-diagonal.x,  diagonal.y)) * 0.1875;
    color += source(uv + vec2<f32>( diagonal.x,  diagonal.y)) * 0.1875;
    return color;
}

@fragment
fn fs_extract(input: VertexOut) -> @location(0) vec4<f32> {
    let color = tent_downsample(input.uv);
    let bright = smoothstep(0.52, 0.86, luminance(color));
    return vec4<f32>(color * bright, 1.0);
}

@fragment
fn fs_downsample(input: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(tent_downsample(input.uv), 1.0);
}
