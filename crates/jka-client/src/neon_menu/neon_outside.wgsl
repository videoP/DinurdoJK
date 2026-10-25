// The street, seen from inside the café: the live scene graded toward a
// rainy night (teal shadows, faded magenta/sodium highlights, greens pushed to
// teal), with the neon signs and their halo in the wet air added on top. The
// result is the "outside" image that the Toadstorm rain-glass pass refracts.
struct Outside {
    // x = scene is gamma-encoded (UNORM swapchain), y = seconds,
    // z = halo strength, w = grade strength
    control: vec4<f32>,
    // x = scene exposure, y = halo levels, z = cached still strength
    tone: vec4<f32>,
    // xy = centre-crop UV scale (matching egui Cover) for the cached image
    cache_uv: vec4<f32>,
    // Columns of the homography from view UV to the TV screen's own UV.
    tv_inverse: array<vec4<f32>, 3>,
    // x = brightness, y = 1 when the scene has a TV
    tv: vec4<f32>,
};
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var signs: texture_2d<f32>;
@group(0) @binding(2) var halo: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var<uniform> outside: Outside;
@group(0) @binding(5) var cached_scene: texture_2d<f32>;
// The CRT picture (crt-lottes output), shown on the scene's wall screen.
@group(0) @binding(6) var crt: texture_2d<f32>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0))[index];
    var out: Out;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * vec2<f32>(0.5, -0.5) + 0.5;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn night_grade(input: vec3<f32>) -> vec3<f32> {
    var c = input * outside.tone.x;
    // Foliage and sodium-green to teal: move green that exceeds both other
    // channels into blue. Wakui's frames have almost no true green.
    let green = max(c.g - max(c.r, c.b), 0.0);
    c = c + vec3<f32>(0.0, -0.30, 0.75) * green;
    // Warm yellows lean to orange/red, keeping the sodium accent.
    let yellow = max(min(c.r, c.g) - c.b, 0.0);
    c = c + vec3<f32>(0.04, -0.12, 0.0) * yellow;
    let luma = dot(c, LUMA);
    // Split tone: cool teal shadows, faded rose highlights.
    let shadow = 1.0 - smoothstep(0.0, 0.30, luma);
    let high = smoothstep(0.20, 0.85, luma);
    c = c + vec3<f32>(-0.006, 0.012, 0.022) * shadow;
    c = mix(c, c * vec3<f32>(1.10, 0.90, 1.00), high * 0.55);
    // Faded, not neon-saturated: the neon supplies the saturation.
    c = mix(vec3<f32>(dot(c, LUMA)), c, 0.82);
    return max(c, vec3<f32>(0.0));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    var base = textureSampleLevel(scene, linear_sampler, in.uv, 0.0).rgb;
    if (outside.control.x > 0.5) {
        base = srgb_to_linear(base);
    }
    if (outside.tone.z > 0.0) {
        let cache_uv = (in.uv - vec2<f32>(0.5)) * outside.cache_uv.xy + vec2<f32>(0.5);
        // Rgba8UnormSrgb is decoded to linear automatically by the sampler.
        let still = textureSampleLevel(cached_scene, linear_sampler, cache_uv, 0.0).rgb;
        base = mix(base, still, outside.tone.z);
    }
    var graded = mix(base, night_grade(base), outside.control.w);
    // The CRT replaces the wall screen in the street. It is mapped through the
    // screen's perspective, so the rain glass and condensation act on it like
    // anything else outside. Emissive: not graded.
    let to_tv = mat3x3<f32>(outside.tv_inverse[0].xyz, outside.tv_inverse[1].xyz, outside.tv_inverse[2].xyz);
    let q = to_tv * vec3<f32>(in.uv, 1.0);
    let local = q.xy / select(q.z, 1e-4, abs(q.z) < 1e-4);
    let edge = min(min(local.x, 1.0 - local.x), min(local.y, 1.0 - local.y));
    let on_screen = clamp(edge / max(fwidth(edge), 1e-5) + 0.5, 0.0, 1.0) * outside.tv.y * step(0.0, q.z);
    if (on_screen > 0.0) {
        let picture = textureSampleLevel(crt, linear_sampler, clamp(local, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
        graded = mix(graded, picture * outside.tv.x, on_screen);
    }
    let neon = textureSampleLevel(signs, linear_sampler, in.uv, 0.0).rgb;
    let glow = textureSampleLevel(halo, linear_sampler, in.uv, 0.0).rgb / max(outside.tone.y, 1.0);
    // Wet air scatters the signs; the halo also lifts the graded scene a
    // little, as the real colour cast of neon on a rainy street does.
    let color = graded + neon + glow * outside.control.z + graded * glow * 0.6;
    return vec4<f32>(color, 1.0);
}
