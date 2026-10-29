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
    @location(2) textured: f32,
};

@group(0) @binding(0) var ui_font: texture_2d<f32>;
@group(0) @binding(1) var ui_font_sampler: sampler;
@group(0) @binding(2) var ui_small_font: texture_2d<f32>;
@group(0) @binding(3) var ui_splash: texture_2d<f32>;
@group(0) @binding(4) var ui_keys: texture_2d<f32>;
@group(0) @binding(5) var ui_icons: texture_2d<f32>;
@group(0) @binding(6) var ui_team_icons: texture_2d<f32>;
@group(0) @binding(7) var ui_ext_images: texture_2d<f32>;

// HW-scene-only compensates just the native HUD pixels before the desktop ramp.
// Gamma applies to encoded display values, *not* linear lighting values.
@group(1) @binding(0) var<uniform> ui_gamma: vec4<f32>;
fn encode_srgb(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c > vec3<f32>(0.0031308));
}
fn decode_srgb(v: vec3<f32>) -> vec3<f32> {
    return select(v / 12.92, pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v > vec3<f32>(0.04045));
}
fn compensate_ui(v: vec4<f32>) -> vec4<f32> {
    let encoded = select(v.rgb, encode_srgb(v.rgb), ui_gamma.y > 0.5);
    let corrected = pow(clamp(encoded, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(ui_gamma.x));
    return vec4<f32>(select(corrected, decode_srgb(corrected), ui_gamma.y > 0.5), v.a);
}

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    out.textured = input.textured;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    if input.textured < 0.5 {
        return compensate_ui(input.color);
    }
    var glyph: vec4<f32>;
    if input.textured < 1.5 {
        glyph = textureSampleLevel(ui_font, ui_font_sampler, input.uv, 0.0);
    } else if input.textured < 2.5 {
        glyph = textureSampleLevel(ui_small_font, ui_font_sampler, input.uv, 0.0);
    } else if input.textured < 3.5 {
        let splash = textureSampleLevel(ui_splash, ui_font_sampler, input.uv, 0.0);
        return compensate_ui(splash * input.color);
    } else if input.textured < 4.5 {
        // gfx/hud/keys art: an ordinary alpha-blended 2D pic, tinted by vertex colour.
        return compensate_ui(textureSampleLevel(ui_keys, ui_font_sampler, input.uv, 0.0) * input.color);
    } else if input.textured < 5.5 {
        // gfx/2d/lag and gfx/2d/net (lagometer): likewise an ordinary 2D pic.
        return compensate_ui(textureSampleLevel(ui_icons, ui_font_sampler, input.uv, 0.0) * input.color);
    } else if input.textured < 6.5 {
        // Dynamic CG_DrawTeamOverlay model/weapon/powerup atlas.
        return compensate_ui(textureSampleLevel(ui_team_icons, ui_font_sampler, input.uv, 0.0) * input.color);
    } else {
        // Game images requested by extension hud_image() calls.
        return compensate_ui(textureSampleLevel(ui_ext_images, ui_font_sampler, input.uv, 0.0) * input.color);
    }
    return compensate_ui(vec4<f32>(input.color.rgb, input.color.a * glyph.a));
}
