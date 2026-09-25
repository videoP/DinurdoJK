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
        return input.color;
    }
    var glyph: vec4<f32>;
    if input.textured < 1.5 {
        glyph = textureSampleLevel(ui_font, ui_font_sampler, input.uv, 0.0);
    } else if input.textured < 2.5 {
        glyph = textureSampleLevel(ui_small_font, ui_font_sampler, input.uv, 0.0);
    } else {
        let splash = textureSampleLevel(ui_splash, ui_font_sampler, input.uv, 0.0);
        return splash * input.color;
    }
    return vec4<f32>(input.color.rgb, input.color.a * glyph.a);
}
