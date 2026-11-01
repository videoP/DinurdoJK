struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) textured: f32,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) alpha_test: f32,
};

@group(0) @binding(0) var effect_texture: texture_2d<f32>;
@group(0) @binding(1) var effect_sampler: sampler;

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    out.alpha_test = input.textured;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSampleLevel(effect_texture, effect_sampler, input.uv, 0.0) * input.color;
    if (input.alpha_test > 1.5 && input.alpha_test < 2.5 && texel.a < 0.5) { discard; }
    if (input.alpha_test > 2.5 && input.alpha_test < 3.5 && texel.a < 0.75) { discard; }
    if (input.alpha_test > 3.5 && input.alpha_test < 4.5 && texel.a <= 0.0) { discard; }
    if (input.alpha_test > 4.5 && texel.a >= 0.5) { discard; }
    return texel;
}
