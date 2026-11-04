struct GammaSettings {
    values: vec4<f32>, // gamma, LUT strength, vignette, manual exposure EV
    flags: vec4<f32>, // x: HDR scene, y: HDR LUT headroom, z: r_overBrightBits emulation
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

// Legacy OpenGL enables GL_DITHER by default before converting fragment colors
// to a fixed-point framebuffer. WGPU exposes no equivalent raster state, so do
// a tiny deterministic ordered dither immediately before the 8-bit sRGB
// presentation target. The amplitude approximates one sRGB code step in linear
// space from scene luminance; using one scalar offset preserves neutral fog hues.
fn output_dither_srgb8(color: vec3<f32>, pixel_position: vec2<f32>) -> vec3<f32> {
    let bayer4 = array<f32, 16>(
         0.0,  8.0,  2.0, 10.0,
        12.0,  4.0, 14.0,  6.0,
         3.0, 11.0,  1.0,  9.0,
        15.0,  7.0, 13.0,  5.0
    );
    let x = u32(pixel_position.x) & 3u;
    let y = u32(pixel_position.y) & 3u;
    let ordered = (bayer4[y * 4u + x] + 0.5) / 16.0 - 0.5;
    let clamped = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    let luma = clamp(dot(clamped, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let dark_lsb = 1.0 / (255.0 * 12.92);
    let bright_lsb = 2.275 / 255.0;
    let linear_lsb = mix(dark_lsb, bright_lsb, sqrt(luma));
    return clamp(color + vec3<f32>(ordered * linear_lsb), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn apply_color_lut(color: vec3<f32>) -> vec3<f32> {
    let strength = clamp(settings.values.y, 0.0, 1.0);
    if (strength <= 0.001) {
        return color;
    }
    let scale = select(1.0, max(1.0, max(color.r, max(color.g, color.b))), settings.flags.y > 0.5);
    let input = clamp(color / scale, vec3<f32>(0.0), vec3<f32>(1.0));
    let coord = (input * (COLOR_LUT_SIZE - 1.0) + vec3<f32>(0.5)) / COLOR_LUT_SIZE;
    let graded = textureSample(color_lut_texture, color_lut_sampler, coord).rgb;
    return mix(color, graded * scale, strength);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(
        scene_texture,
        scene_sampler,
        clamp(input.uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
    let gamma = max(settings.values.x, 0.05);
    var corrected = color * exp2(settings.values.w);
    if (abs(gamma - 1.0) > 0.001) {
        corrected = pow(max(corrected, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
    }
    corrected = apply_color_lut(corrected);
    if (settings.values.z > 0.5) {
        let edge = smoothstep(0.28, 0.78, distance(input.uv, vec2<f32>(0.5)));
        corrected *= 1.0 - edge * 0.28;
    }
    // OpenJK PR #1349: multiply the final scene after gamma, independent of
    // hardware gamma or fullscreen. 0 leaves the preexisting output unchanged.
    if (settings.flags.z > 0.5) {
        corrected *= 2.0;
    }
    if (settings.flags.x < 0.5) {
        corrected = output_dither_srgb8(corrected, input.position.xy);
    }
    return vec4<f32>(corrected, 1.0);
}
