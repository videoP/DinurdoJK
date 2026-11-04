// Generic HUD extension WGSL contract. This file is owned by the engine.
// Extensions provide a `fn hud_pixel(uv: vec2<f32>) -> vec4<f32>` body only.
// `uv` is local 0..1 quad space, even for clipped/mirrored images.
struct HudShaderParams {
    atlas_rect: vec4<f32>, // source atlas cell: x, y, width, height
    values: vec4<f32>,     // four Rhai parameters
    clock: vec4<f32>,      // elapsed time, gamma, gamma-on, output-srgb
};
@group(0) @binding(7) var hud_atlas: texture_2d<f32>;
@group(0) @binding(1) var hud_sampler: sampler;
@group(1) @binding(0) var<uniform> hud: HudShaderParams;

struct HudVertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) textured: f32,
};
struct HudVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex fn vs_main(input: HudVertexInput) -> HudVertexOutput {
    var o: HudVertexOutput;
    o.position = vec4<f32>(input.position, 0.0, 1.0);
    o.uv = (input.uv - hud.atlas_rect.xy) / hud.atlas_rect.zw;
    o.tint = input.color;
    return o;
}

// All shader textures are the calling image's atlas cell. Repeat inside the
// cell, not across neighbouring extension textures. No new texture bindings.
fn hud_sample(uv: vec2<f32>) -> vec4<f32> {
    // The extension images occupy independent atlas cells. Avoid sampling
    // through their bilinear-filter borders when scrolling UVs wrap.
    let local_uv = clamp(fract(uv), vec2<f32>(0.002), vec2<f32>(0.998));
    let atlas_uv = hud.atlas_rect.xy + local_uv * hud.atlas_rect.zw;
    return textureSampleLevel(hud_atlas, hud_sampler, atlas_uv, 0.0);
}

fn hud_encode_srgb(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c > vec3<f32>(0.0031308));
}
fn hud_decode_srgb(v: vec3<f32>) -> vec3<f32> {
    return select(v / 12.92, pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v > vec3<f32>(0.04045));
}
fn hud_compensate(v: vec4<f32>) -> vec4<f32> {
    if hud.clock.z < 0.5 { return v; }
    let encoded = select(v.rgb, hud_encode_srgb(v.rgb), hud.clock.w > 0.5);
    let corrected = pow(clamp(encoded, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(hud.clock.y));
    return vec4<f32>(select(corrected, hud_decode_srgb(corrected), hud.clock.w > 0.5), v.a);
}
