struct GammaSettings {
    values: vec4<f32>, // gamma, LUT strength, vignette, reserved
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: GammaSettings;
@group(0) @binding(3) var color_lut_texture: texture_3d<f32>;
@group(0) @binding(4) var color_lut_sampler: sampler;

const COLOR_LUT_SIZE: f32 = 33.0;

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

fn apply_color_lut(color: vec3<f32>) -> vec3<f32> {
    let strength = clamp(settings.values.y, 0.0, 1.0);
    if (strength <= 0.001) {
        return color;
    }
    let input = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    let coord = (input * (COLOR_LUT_SIZE - 1.0) + vec3<f32>(0.5)) / COLOR_LUT_SIZE;
    let graded = textureSample(color_lut_texture, color_lut_sampler, coord).rgb;
    return mix(color, graded, strength);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(
        scene_texture,
        scene_sampler,
        clamp(input.uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
    let gamma = max(settings.values.x, 0.05);
    var corrected = color;
    if (abs(gamma - 1.0) > 0.001) {
        corrected = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
    }
    corrected = apply_color_lut(corrected);
    if (settings.values.z > 0.5) {
        let edge = smoothstep(0.28, 0.78, distance(input.uv, vec2<f32>(0.5)));
        corrected *= 1.0 - edge * 0.28;
    }
    return vec4<f32>(corrected, 1.0);
}
