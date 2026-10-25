// The café window, inside surface. Input is the street after the Toadstorm
// rain-glass pass (drops on the outside of the pane). This pass adds what is
// on the inside: condensation that diffuses the street, finger wipes that clear
// it where a hand wipes across the sign under the pointer, condensation beads that grow and run down as liquid-glass lenses, the wet
// sill reflecting the window, and the film look. (The CRT is part of the
// street now: neon_outside.wgsl maps it onto the scene's wall screen.)
//
// Bead optics are the Liquid glass bevel from glass_common.wgsl
// (glass_profile + the edge refraction in glass_shade), evaluated against an
// analytic shape instead of the world-glass edge field. `glass` is the same
// nine-vec4 GlassSettings block, filled from the Liquid preset.
struct Window {
    size: vec4<f32>,         // xy = target px, z = seconds, w = output is gamma-encoded
    fog: vec4<f32>,          // x = density, y = blur lod, z = milk scatter, w = halation
    sill: vec4<f32>,         // x = sill top px (>= size.y: none), y = reflection, z = ripple, w = reveal
    film: vec4<f32>,         // x = fade, y = grain, z = vignette, w = halo normalisation
    counts: vec4<f32>,       // x = wipes, y = drips, z = wipes that also clear the rain drops
    // Two vec4 per wipe: rect px; (progress, clear, tilt radians, seed).
    wipes: array<vec4<f32>, 18>,
    // Two vec4 per drip: (x, y, radius, alpha); (trail top y, trail alpha, stretch, seed).
    drips: array<vec4<f32>, 32>,
};
@group(0) @binding(0) var street: texture_2d<f32>;
@group(0) @binding(1) var fog_chain: texture_2d<f32>;
@group(0) @binding(2) var halo: texture_2d<f32>;
@group(0) @binding(4) var linear_sampler: sampler;
@group(0) @binding(5) var<uniform> window: Window;
@group(0) @binding(6) var<uniform> glass: array<vec4<f32>, 9>;
@group(0) @binding(7) var scene: texture_2d<f32>;
// The street before the rain-glass pass: what a wiped pane shows.
@group(0) @binding(8) var dry_street: texture_2d<f32>;

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

// Snapshot frames: the cached menu photograph must be the plain scene.
@fragment
fn fs_passthrough(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(scene, linear_sampler, in.uv, 0.0).rgb, 1.0);
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn hash12(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}
fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash12(i), hash12(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash12(i + vec2<f32>(0.0, 1.0)), hash12(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}
fn fbm(p: vec2<f32>) -> f32 {
    return value_noise(p) * 0.55 + value_noise(p * 2.13 + 7.1) * 0.30 + value_noise(p * 4.37 + 3.7) * 0.15;
}

// glass_common.wgsl glass_profile(), with the profile passed in.
fn liquid_profile(t: f32) -> vec2<f32> {
    let x = clamp(t, 0.001, 0.999);
    let circle = sqrt(max(1.0 - (1.0 - x) * (1.0 - x), 0.0001));
    let circle_slope = (1.0 - x) / circle;
    let quartic = max(1.0 - pow(1.0 - x, 4.0), 0.0001);
    let squircle = pow(quartic, 0.25);
    let squircle_slope = pow(1.0 - x, 3.0) * pow(quartic, -0.75);
    let profile = u32(glass[5].z + 0.5);
    if (profile == 1u) { return vec2<f32>(squircle, squircle_slope); }
    if (profile == 2u) { return vec2<f32>(1.0 - circle, -circle_slope); }
    if (profile == 3u) {
        let blend = x * x * (3.0 - 2.0 * x);
        let derivative = 6.0 * x * (1.0 - x);
        return vec2<f32>(mix(squircle, 1.0 - circle, blend), mix(squircle_slope, -circle_slope, blend) + (1.0 - circle - squircle) * derivative);
    }
    return vec2<f32>(circle, circle_slope);
}

// glass_shade()'s bevel refraction in pixels. `edge_direction` points from
// the sample toward the nearest edge; `edge_width` is the bevel width.
fn liquid_offset(edge_distance: f32, edge_width: f32, edge_direction: vec2<f32>) -> vec2<f32> {
    let edge_t = clamp(edge_distance / max(edge_width, 0.001), 0.0, 1.0);
    if (edge_t >= 1.0) { return vec2<f32>(0.0); }
    let curve = liquid_profile(edge_t);
    let bevel_normal = normalize(vec3<f32>(edge_direction * curve.y, -1.0));
    let bevel_ray = refract(vec3<f32>(0.0, 0.0, 1.0), bevel_normal, 1.0 / glass[0].x);
    let height = curve.x * edge_width + glass[0].y;
    return -bevel_ray.xy / max(abs(bevel_ray.z), 0.05) * height * glass[5].y;
}

// glass_shade()'s rim: a lobe toward the rim light angle.
fn liquid_rim(edge_distance: f32, edge_direction: vec2<f32>) -> f32 {
    let light = vec2<f32>(cos(glass[6].w), -sin(glass[6].w));
    let lobe = pow(abs(dot(edge_direction, light)), 4.0);
    let rim = 1.0 - smoothstep(glass[6].y, glass[6].y + glass[6].z, edge_distance);
    return clamp(rim * lobe * glass[6].x, 0.0, 1.0);
}

fn rotate(v: vec2<f32>, angle: f32) -> vec2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(c * v.x + s * v.y, -s * v.x + c * v.y);
}

// A hand wiping fogged glass, left to right. Returns (clear, pushed):
// how much condensation is gone here, and how much was shoved ahead of the
// moving hand. No hard outline: the band's edges feather out in streaks that
// run along the stroke, the leading end is a smear, the inside keeps faint
// lines where fingers pressed less. As `clear` falls the fog closes in again
// from the top/bottom edges and the ends, in uneven tongues.
fn wipe_smear(p: vec2<f32>, rect: vec4<f32>, params: vec4<f32>) -> vec2<f32> {
    let centre = (rect.xy + rect.zw) * 0.5;
    let half = (rect.zw - rect.xy) * 0.5;
    let q = rotate(p - centre, params.z);
    let seed = params.w * 17.0;
    let u = (q.x + half.x) / max(2.0 * half.x, 1.0);
    let v = q.y / max(half.y, 1.0);
    // Streak field stretched along the stroke direction.
    let streak = fbm(vec2<f32>(q.x * 0.006, q.y * 0.16) + seed);
    let fine = value_noise(vec2<f32>(q.x * 0.012, q.y * 0.55) + seed * 1.3);
    let front = params.x;
    let returning = 1.0 - params.y;
    // Along the stroke: soft start, smeared trailing edge behind the hand.
    let reach = front - returning * (0.25 + 0.3 * streak);
    let start = 0.0 + returning * (0.25 + 0.3 * fine);
    var along = smoothstep(start - 0.06, start + 0.10, u) * (1.0 - smoothstep(reach - 0.14 - streak * 0.08, reach + 0.01, u));
    // Across: a band whose top and bottom wander slowly and fray in streaks.
    let edge = 1.0 - 0.14 * value_noise(vec2<f32>(u * 2.5, seed)) - returning * (0.75 + 0.35 * streak) + (streak - 0.5) * 0.18;
    let feather = 0.22 + 0.22 * fine;
    let across = 1.0 - smoothstep(edge - feather, edge + feather * 0.35, abs(v));
    // Uneven pressure: fingers leave faint lines of residue.
    let pressure = 0.78 + 0.22 * smoothstep(0.25, 0.75, fine) - 0.12 * pow(0.5 + 0.5 * sin(v * 9.0 + streak * 4.0 + seed), 6.0);
    let clear = clamp(along * across * pressure, 0.0, 1.0) * smoothstep(0.0, 0.15, params.y);
    // Condensation bunched up just ahead of the moving hand.
    let moving = 1.0 - smoothstep(0.92, 1.0, front);
    let pushed = exp(-pow((u - front - 0.03) / 0.035, 2.0)) * across * moving * 0.35;
    return vec2<f32>(clear, pushed);
}

fn street_at(uv: vec2<f32>, dispersion: vec2<f32>) -> vec3<f32> {
    let g = textureSampleLevel(street, linear_sampler, uv, 0.0);
    if (dot(dispersion, dispersion) < 1e-10) { return g.rgb; }
    let r = textureSampleLevel(street, linear_sampler, uv - dispersion, 0.0).r;
    let b = textureSampleLevel(street, linear_sampler, uv + dispersion, 0.0).b;
    return vec3<f32>(r, g.g, b);
}

fn rounded_box(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let size = window.size.xy;
    let p = in.position.xy;
    let uv = in.uv;
    let time = window.size.z;
    let reveal = window.sill.w;
    let sill_top = window.sill.x;

    // --- Beads: liquid-glass lenses on the inside of the pane. ---
    var offset = vec2<f32>(0.0);
    var bead = 0.0;
    var highlight = 0.0;
    var clear = 0.0;
    let drip_count = u32(window.counts.y);
    for (var i = 0u; i < drip_count; i += 1u) {
        let a = window.drips[i * 2u];
        let b = window.drips[i * 2u + 1u];
        let r = a.z;
        let delta = p - a.xy;
        // Egg shape: a sliding bead drags a longer upper body behind it.
        let stretch = select(1.0, b.z, delta.y < 0.0);
        let e = delta / vec2<f32>(1.0, stretch);
        let d = length(e) - r;
        // The run it leaves behind wipes the condensation.
        let top = b.x;
        if (b.y > 0.001 && p.y > top - r && p.y < a.y) {
            let wobble = (value_noise(vec2<f32>(p.y * 0.05, b.w * 17.0)) - 0.5) * r * 0.5;
            let trail = abs(p.x - a.x - wobble) - r * 0.62 * (0.75 + 0.25 * value_noise(vec2<f32>(p.y * 0.11, b.w)));
            clear = max(clear, (1.0 - smoothstep(-1.0, 1.5, trail)) * b.y);
        }
        if (d < 1.5 && a.w > 0.001) {
            let coverage = (1.0 - smoothstep(-1.0, 1.0, d)) * a.w;
            let edge_distance = max(-d, 0.0);
            let direction = normalize(delta + vec2<f32>(1e-4, 0.0));
            offset += liquid_offset(edge_distance, r, direction) * coverage;
            highlight = max(highlight, liquid_rim(edge_distance, direction) * coverage);
            // Specular glint of the room light, upper left of every bead.
            let glint = exp(-dot(delta - vec2<f32>(-0.35, -0.42) * r, delta - vec2<f32>(-0.35, -0.42) * r) / (r * r * 0.025));
            highlight = max(highlight, glint * coverage * 0.9);
            bead = max(bead, coverage);
        }
    }

    // --- Hand wipes over the sign under the pointer. ---
    var pushed = 0.0;
    var dry = 0.0;
    let wipe_count = u32(window.counts.x);
    let drying_wipes = u32(window.counts.z);
    for (var i = 0u; i < wipe_count; i += 1u) {
        let rect = window.wipes[i * 2u];
        let params = window.wipes[i * 2u + 1u];
        if (params.x < 0.001 || params.y < 0.001) { continue; }
        let reach = (rect.w - rect.y) * 0.6 + 8.0;
        if (p.x < rect.x - reach || p.x > rect.z + reach || p.y < rect.y - reach || p.y > rect.w + reach) { continue; }
        let smear = wipe_smear(p, rect, params);
        clear = max(clear, smear.x);
        pushed = max(pushed, smear.y);
        // The hand takes the rain drops with it, not just the condensation.
        if (i < drying_wipes) {
            dry = max(dry, smear.x);
        }
    }
    let max_offset = max(glass[5].w, 4.0) * 1.6;
    offset *= min(1.0, max_offset / max(length(offset), 1e-4));

    // --- Condensation. Thicker toward the bottom of the pane, uneven. ---
    var density = window.fog.x * (0.72 + 0.28 * fbm(uv * vec2<f32>(3.1, 2.3) + vec2<f32>(0.0, time * 0.004)));
    density += 0.16 * smoothstep(0.55, 1.0, uv.y);
    density = clamp(clamp(density, 0.0, 0.97) * (1.0 - clear) + pushed, 0.0, 0.97) * (1.0 - bead);

    let sample_uv = clamp(uv + offset / size, vec2<f32>(0.0), vec2<f32>(1.0));
    let dispersion = offset / size * glass[2].x * glass[8].z * 6.0;
    var sharp = street_at(sample_uv, dispersion);
    if (dry > 0.001) {
        sharp = mix(sharp, textureSampleLevel(dry_street, linear_sampler, sample_uv, 0.0).rgb, dry);
    }
    let diffuse = textureSampleLevel(fog_chain, linear_sampler, sample_uv, window.fog.y).rgb;
    let wider = textureSampleLevel(fog_chain, linear_sampler, uv, window.fog.y + 1.5).rgb;
    let glow = textureSampleLevel(halo, linear_sampler, uv, 0.0).rgb / max(window.film.w, 1.0);
    let milk = window.fog.z * (vec3<f32>(0.030, 0.038, 0.050) + dot(wider, LUMA) * vec3<f32>(0.20, 0.22, 0.26));
    let fogged = mix(diffuse, wider, 0.25) * 0.92 + milk;
    var color = mix(sharp, fogged, density);
    color += glow * window.fog.w * (0.18 + density * 0.9);
    // Liquid glass highlights: white rim (rim type 1 in the Liquid preset).
    let lit_rim = highlight;
    color = mix(color, vec3<f32>(1.0) * (0.55 + dot(glow, LUMA) * 2.0), lit_rim * 0.75);
    // Water beads also darken at their silhouette (total internal reflection).
    color *= 1.0 - bead * 0.12;

    // --- Wet sill: dark stone mirroring the window, rippled by drips. ---
    if (p.y > sill_top) {
        let depth = p.y - sill_top;
        let ripple = vec2<f32>(value_noise(vec2<f32>(p.x * 0.018, depth * 0.07 - time * 0.55)) - 0.5,
            value_noise(vec2<f32>(p.x * 0.011 + 9.0, depth * 0.05 - time * 0.4)) - 0.5) * (2.0 + depth * 0.09) * window.sill.z;
        // Long vertical streaks: reflections stretch away from the viewer.
        let mirror = vec2<f32>(p.x + ripple.x, sill_top - depth * 1.7 + ripple.y * 3.0);
        let ruv = clamp(mirror / size, vec2<f32>(0.0), vec2<f32>(1.0));
        let lod = window.fog.y + 0.5 + depth / size.y * 12.0;
        var reflected = textureSampleLevel(fog_chain, linear_sampler, ruv, lod).rgb;
        reflected += textureSampleLevel(halo, linear_sampler, ruv, 0.0).rgb / max(window.film.w, 1.0) * 1.2;
        let fresnel = 0.28 + 0.42 * exp(-depth / (size.y * 0.035));
        let stone = vec3<f32>(0.008, 0.009, 0.011);
        var sill = stone + reflected * fresnel * window.sill.y;
        // The sill's front edge catches the light from outside.
        sill += (glow * 1.6 + vec3<f32>(0.02, 0.022, 0.026)) * exp(-pow((depth - 1.5) / 1.2, 2.0));
        color = mix(color, sill, smoothstep(sill_top - 0.5, sill_top + 0.5, p.y));
    }

    // --- Film: soft shoulder for the hot neon cores, lifted teal blacks,
    // fine animated grain, a gentle vignette. ---
    let knee = 0.78;
    let over = max(color - vec3<f32>(knee), vec3<f32>(0.0));
    color = min(color, vec3<f32>(knee)) + (1.0 - knee) * (vec3<f32>(1.0) - exp(-over / (1.0 - knee)));
    color = color * (1.0 - window.film.x) + window.film.x * vec3<f32>(0.42, 0.55, 0.70) * 0.6;
    let luma = dot(color, LUMA);
    let grain = (hash12(p + fract(time * 7.13) * vec2<f32>(173.0, 311.0)) - 0.5) * window.film.y * (0.35 + sqrt(max(luma, 0.0)));
    color += vec3<f32>(grain);
    let v = (uv - 0.5) * vec2<f32>(1.0, 0.82);
    color *= 1.0 - window.film.z * pow(min(length(v) * 1.35, 1.0), 2.4);
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    if (window.size.w > 0.5) {
        color = linear_to_srgb(color);
    }
    return vec4<f32>(color, 1.0);
}
