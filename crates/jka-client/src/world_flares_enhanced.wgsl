// Enhanced BSP MST_FLARE: WGSL port of mu6k's Shadertoy 4sX3Rs lensflare().
// Reference: https://www.shadertoy.com/view/4sX3Rs
// The f0/f2/f4/f5/f6 math, constants, and channel offsets follow the source;
// only Shadertoy's noise-texture input is substituted with deterministic
// smooth 1D noise and the interactive mouse becomes the projected BSP source.
// Authored BSP/material color is applied AFTER the lens shape is generated.
// This file is concatenated with world_flares.wgsl (shared data/material helpers).
struct EnhancedOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) source_ndc: vec2<f32>,
    @location(1) tint: vec3<f32>,
};

struct LensComponents {
    wash: vec3<f32>,
    detail: vec3<f32>,
};

// 2x UNORM16 in the spare camera.frame.z field: intensity and core boost.
fn unpack_enhanced_gain(bits: u32) -> vec2<f32> {
    return vec2<f32>(
        f32(bits & 0xffffu) / 65535.0,
        f32((bits >> 16u) & 0xffffu) * (8.0 / 65535.0),
    );
}

fn unpack_enhanced_params(bits: u32) -> vec2<f32> {
    return vec2<f32>(
        f32(bits & 0xffffu) / 65535.0,
        f32((bits >> 16u) & 0xffffu) / 65535.0,
    );
}

@vertex fn vs_enhanced(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> EnhancedOut {
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0));
    let flare = flares[instance];
    let visibility = states[u32(flare.normal.w)];
    var out: EnhancedOut;
    out.clip = vec4<f32>(2.0, 2.0, 1.0, 1.0);
    out.source_ndc = vec2<f32>(0.0);
    out.tint = vec3<f32>(0.0);
    // Each invisible source becomes a degenerate off-frustum triangle;
    // fragments (including the expensive lens math) are never generated.
    // Occlusion and fade history are still updated in the same-frame compute pass.
    if (visibility.visible == 0u || visibility.last_frame != camera.frame.x) {
        return out;
    }
    let origin = flare.origin_radius.xyz + flare.normal.xyz * 3.0;
    let projected = camera.view_proj * vec4<f32>(origin, 1.0);
    if (projected.w <= 0.0) { return out; }
    out.source_ndc = projected.xy / projected.w;
    // f0 has a nonzero tail across the screen in the original shader.
    // Full viewport coverage avoids the old rectangular cutoff artifacts.
    out.clip = vec4<f32>(corners[vertex], 1.0, 1.0);

    // Authored material coordinates and two color samples do not depend on
    // the destination pixel. Sampling in the vertex stage makes the cost
    // 12 fetches per flare (6 vertices x 2) instead of per screen pixel.
    let source_uv = texcoords(vec2<f32>(0.5), origin);
    let center_color = textureSampleLevel(flare_texture, flare_sampler, source_uv, 0.0);
    let coarse_lod = f32(textureNumLevels(flare_texture) - 1u);
    let average_color = textureSampleLevel(flare_texture, flare_sampler, source_uv, coarse_lod);
    let authored = max(center_color.rgb * center_color.a,
        average_color.rgb * clamp(average_color.a * 3.0, 0.12, 1.0));
    var tint = authored * flare.color.rgb;
    let toward_eye = camera.eye.xyz - flare.origin_radius.xyz;
    let distance = max(length(toward_eye), 0.0001);
    let normal = flare.normal.xyz;
    let safe_normal = select(vec3<f32>(0.0, 1.0, 0.0), normal, dot(normal, normal) > 0.000001);
    let facing = select(1.0, abs(dot(toward_eye / distance, normalize(safe_normal))), dot(normal, normal) > 0.000001);
    if (stage.rgb.w == 1.0) { tint *= facing; }
    if (stage.rgb.w == 2.0) { tint *= 1.0 - facing; }
    if (stage.rgb.w == 3.0) { tint *= stage.rgb.rgb; }
    let time = min(camera.viewport_time.z, select(3.402823e38, stage.wave_funcs.z, stage.wave_funcs.z > 0.0));
    if (stage.rgb.w == 4.0) {
        tint *= clamp(wave_value(u32(stage.wave_funcs.x), stage.rgb_wave, time), 0.0, 1.0);
    }
    if (stage.rgb.w == 5.0) { tint = vec3<f32>(0.0); }
    var opacity = stage.alpha.x;
    if (stage.alpha.z == 1.0) { opacity *= facing; }
    if (stage.alpha.z == 2.0) { opacity *= 1.0 - facing; }
    if (stage.alpha.z == 3.0) { opacity *= clamp(distance / max(stage.fade_control.z, 0.001), 0.0, 1.0); }
    if (stage.alpha.z == 4.0) {
        opacity *= clamp(wave_value(u32(stage.wave_funcs.y), stage.alpha_wave, time), 0.0, 1.0);
    }

    var fog_factor = 1.0;
    if (camera.viewport_time.w > 0.5 && flare.fog.w > 0.0) {
        var transmission = max(1.0 - distance / max(flare.fog.w, 0.001), 0.0);
        if (flare.color.w > 0.5) {
            let forward = normalize(cross(camera.up.xyz, camera.right.xyz));
            let depth = max(dot(origin - camera.eye.xyz, forward), 0.0) / max(flare.fog.w, 0.001);
            if (camera.viewport_time.w >= 1.5) {
                transmission = exp(-5.5412635 * depth * depth);
            } else {
                transmission = 1.0 - openjk_global_fog_alpha(depth);
            }
        }
        fog_factor = clamp(transmission, 0.0, 1.0);
    }
    let fade = clamp((camera.viewport_time.z - visibility.fade_start) / 0.5, 0.0, 1.0);
    // Final gain is applied once per vertex, not per shaded pixel. The mu6k
    // lens functions retain every original relative intensity and color.
    let enhanced_gain = unpack_enhanced_gain(camera.frame.z).x;
    out.tint = tint * clamp(opacity, 0.0, 1.0) * fade * fog_factor * enhanced_gain;
    return out;
}

// Ported directly from the 4sX3Rs lensflare function. Only the source noise
// texture input is replaced with the shared deterministic smooth noise helper.
// UVs must be [-aspect/2,+aspect/2] x [-0.5,+0.5], NOT [-aspect,+aspect].
fn mu6k_lensflare_components(uv: vec2<f32>, pos: vec2<f32>) -> LensComponents {
    let main = uv - pos;
    let uvd = uv * length(uv);
    let ang = atan2(main.x, main.y);
    let dist = pow(length(main), 0.1);
    let ray_noise = wave_noise(sin(ang * 2.0 + pos.x) * 4.0 - cos(ang * 3.0 + pos.y)) * 0.5 + 0.5;
    var f0 = 1.0 / (length(uv - pos) * 16.0 + 1.0);
    f0 = f0 + f0 * (sin(ray_noise * 16.0) * 0.1 + dist * 0.1 + 0.8);

    let f2  = 1.0 / (1.0 + 32.0 * pow(length(uvd + 0.80 * pos), 2.0)) * 0.25;
    let f22 = 1.0 / (1.0 + 32.0 * pow(length(uvd + 0.85 * pos), 2.0)) * 0.23;
    let f23 = 1.0 / (1.0 + 32.0 * pow(length(uvd + 0.90 * pos), 2.0)) * 0.21;

    var uvx = mix(uv, uvd, -0.5);
    let f4  = max(0.01 - pow(length(uvx + 0.40 * pos), 2.4), 0.0) * 6.0;
    let f42 = max(0.01 - pow(length(uvx + 0.45 * pos), 2.4), 0.0) * 5.0;
    let f43 = max(0.01 - pow(length(uvx + 0.50 * pos), 2.4), 0.0) * 3.0;

    uvx = mix(uv, uvd, -0.4);
    let f5  = max(0.01 - pow(length(uvx + 0.20 * pos), 5.5), 0.0) * 2.0;
    let f52 = max(0.01 - pow(length(uvx + 0.40 * pos), 5.5), 0.0) * 2.0;
    let f53 = max(0.01 - pow(length(uvx + 0.60 * pos), 5.5), 0.0) * 2.0;

    uvx = mix(uv, uvd, -0.5);
    let f6  = max(0.01 - pow(length(uvx - 0.300 * pos), 1.6), 0.0) * 6.0;
    let f62 = max(0.01 - pow(length(uvx - 0.325 * pos), 1.6), 0.0) * 3.0;
    let f63 = max(0.01 - pow(length(uvx - 0.350 * pos), 1.6), 0.0) * 5.0;

    var detail = vec3<f32>(f2 + f4 + f5 + f6,
                           f22 + f42 + f52 + f62,
                           f23 + f43 + f53 + f63);
    detail = detail * 1.3 - vec3<f32>(length(uvd) * 0.05);
    return LensComponents(vec3<f32>(f0), detail);
}

// mu6k's final color modifier. Apply only to the light contribution:
// grading the existing game framebuffer (as the Shadertoy demo does) would
// require a separate full-screen postprocess and change unrelated pixels.
fn mu6k_cc(color: vec3<f32>, factor: f32, factor2: f32) -> vec3<f32> {
    let w = color.x + color.y + color.z;
    return mix(color, vec3<f32>(w) * factor, w * factor2);
}

// EnhancedOut already has @builtin(position); in a fragment entry point it
// contains pixel coordinates, not clip-space coordinates.
@fragment fn fs_enhanced(v: EnhancedOut) -> @location(0) vec4<f32> {
    let resolution = max(camera.viewport_time.xy, vec2<f32>(1.0));
    let aspect = resolution.x / resolution.y;
    let uv = vec2<f32>((v.clip.x / resolution.x - 0.5) * aspect,
                         0.5 - v.clip.y / resolution.y);
    let pos = vec2<f32>(v.source_ndc.x * 0.5 * aspect, v.source_ndc.y * 0.5);
    let params = unpack_enhanced_params(camera.frame.w);
    let wash_strength = params.x;
    let locality = params.y;
    let gains = unpack_enhanced_gain(camera.frame.z);
    let core_gain = gains.y;
    let distance_from_source = length(uv - pos);
    let detail_mask = pow(clamp(1.0 - smoothstep(0.0, 0.70, distance_from_source), 0.0, 1.0), 2.0);
    let wash_mask = pow(clamp(1.0 - smoothstep(0.0, 1.05, distance_from_source), 0.0, 1.0), 3.0);
    // Split f0 itself: it is BOTH the bright origin AND its persistent
    // screen-wide tail in mu6k's reference. Never scale its whole value by
    // the Wash slider, or the source becomes dim when the veil is disabled.
    // This is screen-space distance in half-viewport-height units, so the
    // radial profile is aspect-independent. Higher locality narrows the core.
    let core_radius = mix(0.14, 0.065, locality);
    let core_mask = 1.0 - smoothstep(0.012, core_radius, distance_from_source);
    let components = mu6k_lensflare_components(uv, pos);
    // The reference adds its lens contribution to a background image. We
    // likewise use additive blending, without recoloring the entire game
    // frame via the Shadertoy demo's final scene-wide cc() operation.
    let localized_detail = components.detail * mix(1.0, detail_mask, locality);
    let source_core = components.wash * core_mask * core_gain;
    let outer_wash = components.wash * (1.0 - core_mask) * wash_strength
        * mix(1.0, wash_mask, locality);
    let light = max(localized_detail + source_core + outer_wash, vec3<f32>(0.0));
    let graded = mu6k_cc(light * v.tint, 0.5, 0.1);
    return vec4<f32>(max(graded, vec3<f32>(0.0)), 1.0);
}
