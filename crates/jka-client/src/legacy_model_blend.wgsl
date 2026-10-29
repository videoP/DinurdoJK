// Custom effect shells were authored for a gamma-encoded framebuffer. The
// dedicated legacy model pass blends encoded RGB, then returns to linear.
const LEGACY_ENCODED_BLEND: bool = false;
fn legacy_effect_encode(color: vec3<f32>) -> vec3<f32> {
    let c = max(color, vec3<f32>(0.0));
    return select(1.055 * pow(c, vec3<f32>(1.0/2.4)) - 0.055, c * 12.92,
        c <= vec3<f32>(0.0031308));
}
fn legacy_effect_decode(color: vec3<f32>) -> vec3<f32> {
    let c = max(color, vec3<f32>(0.0));
    return select(pow((c + 0.055)/1.055, vec3<f32>(2.4)), c/12.92,
        c <= vec3<f32>(0.04045));
}
fn legacy_effect_source(tex: vec4<f32>, tint: vec4<f32>) -> vec4<f32> {
    if (LEGACY_ENCODED_BLEND) {
        // shaderRGBA is an authored byte colour, multiplied in display space.
        return vec4<f32>(legacy_effect_decode(legacy_effect_encode(tex.rgb) * tint.rgb), tex.a*tint.a);
    }
    return tex * tint;
}
fn legacy_effect_output(color: vec4<f32>) -> vec4<f32> {
    if (LEGACY_ENCODED_BLEND) { return vec4<f32>(legacy_effect_encode(color.rgb), color.a); }
    return color;
}
