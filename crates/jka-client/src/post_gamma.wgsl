struct GammaSettings {
    values: vec4<f32>, // gamma, LUT strength, vignette, manual exposure EV
    flags: vec4<f32>, // x: HDR scene, y: LUT headroom, z: output overbright, w: JK2 SDR LUT mode
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: GammaSettings;
@group(0) @binding(3) var color_lut_texture: texture_3d<f32>;
@group(0) @binding(4) var color_lut_sampler: sampler;
@group(0) @binding(5) var jk2_gamma_lut: texture_2d<f32>;
const JK2_GAMMA_LUT_SIZE: f32 = 1024.0;

fn jk2_gamma_lut_rgb(linear: vec3<f32>) -> vec3<f32> {
    let uv = (sqrt(clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0))) * (JK2_GAMMA_LUT_SIZE - 1.0) + vec3<f32>(0.5)) / JK2_GAMMA_LUT_SIZE;
    return vec3<f32>(
        textureSampleLevel(jk2_gamma_lut, color_lut_sampler, vec2<f32>(uv.r, 0.5), 0.0).r,
        textureSampleLevel(jk2_gamma_lut, color_lut_sampler, vec2<f32>(uv.g, 0.5), 0.0).r,
        textureSampleLevel(jk2_gamma_lut, color_lut_sampler, vec2<f32>(uv.b, 0.5), 0.0).r
    );
}

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
    var coord_input = input;
    if (settings.flags.w > 1.5) {
        coord_input = sqrt(input);
    }
    let coord = (coord_input * (COLOR_LUT_SIZE - 1.0) + vec3<f32>(0.5)) / COLOR_LUT_SIZE;
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
    // JK2MV applies gamma/overbright in encoded display space. Reproduce that
    // via a precomputed transfer lookup rather than six full-screen pow calls.
    // The CPU selects the transfer by output policy, not scene precision.
    // JKA and native HDR keep their existing linear-space presentation.
    let jk2_sdr = settings.flags.w > 0.5;
    if (!(jk2_sdr && settings.flags.w > 1.5)) {
        // If the grading cube contains JK2's transfer, skip this entirely.
        if (jk2_sdr && (abs(gamma - 1.0) > 0.001 || settings.flags.z > 0.5)) {
            corrected = jk2_gamma_lut_rgb(corrected);
        } else if (abs(gamma - 1.0) > 0.001) {
            corrected = pow(max(corrected, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
        }
    }
    corrected = apply_color_lut(corrected);
    if (settings.values.z > 0.5) {
        let edge = smoothstep(0.28, 0.78, distance(input.uv, vec2<f32>(0.5)));
        corrected *= 1.0 - edge * 0.28;
    }
    // The JK2 SDR branch already applied the output shift before sRGB decode.
    // Keep OpenJK's existing linear output gain for JKA and native HDR.
    if (settings.flags.z > 0.5 && !jk2_sdr) {
        corrected *= 2.0;
    }
    // Dither every SDR presentation; a float scene still quantizes at the
    // 8-bit swapchain. Only native HDR output (flags.y) skips it.
    if (settings.flags.y < 0.5) {
        corrected = output_dither_srgb8(corrected, input.position.xy);
    }
    return vec4<f32>(corrected, 1.0);
}
