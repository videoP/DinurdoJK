// Render egui unchanged into an RGBA surface, then counter-adjust only its
// pixels before the Windows SDR gamma ramp. The game world remains untouched.
struct UiGamma { gamma: f32, srgb: f32, unused1: f32, unused2: f32 };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> control: UiGamma;
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VertexOut;
    out.position = vec4<f32>(p[index], 0.0, 1.0);
    out.uv = vec2<f32>((p[index].x + 1.0) * 0.5, (1.0 - p[index].y) * 0.5);
    return out;
}
fn encode_srgb(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c > vec3<f32>(0.0031308));
}
fn decode_srgb(v: vec3<f32>) -> vec3<f32> {
    return select(v / 12.92, pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v > vec3<f32>(0.04045));
}
@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let premul = textureSample(src, src_sampler, input.uv);
    if (premul.a <= 0.00001) { return vec4<f32>(0.0); }
    // egui's intermediate contains premultiplied RGB. Unpremultiply first,
    // otherwise gamma compensation causes dark fringes on glyph/shape edges.
    let ui_color = clamp(premul.rgb / premul.a, vec3<f32>(0.0), vec3<f32>(1.0));
    let encoded = select(ui_color, encode_srgb(ui_color), control.srgb > 0.5);
    let corrected = pow(encoded, vec3<f32>(control.gamma));
    let linear = select(corrected, decode_srgb(corrected), control.srgb > 0.5);
    return vec4<f32>(linear * premul.a, premul.a);
}
