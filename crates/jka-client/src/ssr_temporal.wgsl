// Adapted for JKA's existing scene-color + linear-depth renderer from:
// - Bevy SSR hybrid ray marcher: crates/bevy_pbr/src/ssr/raymarch.wesl
//   https://github.com/bevyengine/bevy
// - AMD FidelityFX SSSR/DNSR temporal reflection history:
//   https://github.com/GPUOpen-Effects/FidelityFX-SSSR
// This intentionally omits FidelityFX's general-purpose normal/roughness/variance
// G-buffer to keep the lightweight forward renderer intact. Rain wetness follows
// the useful Screen Space Wetness idea instead: one shared weather-surface field
// modifies the effective normal/smoothness response before SSR traces reflections.

struct SsrTemporalSettings {
    viewport_history: vec4<f32>, // full width, full height, history valid, frame index
    camera_pos_time: vec4<f32>,
    previous_camera_pos: vec4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
};

struct WeatherSurfaceSettings {
    amount_distance: vec4<f32>, // wetness, fade start/end, rain-intensity response
    puddle: vec4<f32>,          // accumulation, time, active-rain ripple strength, reserved
    occlusion_uv: vec4<f32>,    // min X/Z, inverse map width/depth
    occlusion_size: vec4<u32>,  // width, height, active, reserved
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var linear_depth_texture: texture_2d<f32>;
@group(0) @binding(3) var history_radiance_texture: texture_2d<f32>;
@group(0) @binding(4) var history_depth_texture: texture_2d<f32>;
@group(0) @binding(5) var<uniform> settings: SsrTemporalSettings;
@group(0) @binding(6) var weather_occlusion_height: texture_2d<f32>;
@group(0) @binding(7) var weather_occlusion_sampler: sampler;
@group(0) @binding(8) var<uniform> weather_surface: WeatherSurfaceSettings;
@group(0) @binding(9) var reflection_policy_texture: texture_2d<f32>;
@group(0) @binding(10) var ssr_visibility_texture: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct SsrSample {
    radiance: vec3<f32>,
    weight: f32,
};

struct SsrFragmentOut {
    @location(0) radiance: vec4<f32>,
    @location(1) depth: f32,
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

fn valid_depth(depth: f32) -> bool {
    return depth > 0.0 && depth < 999999.0;
}

fn scene(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(
        scene_texture,
        scene_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
}

fn depth_at_pixel(pixel: vec2<i32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture);
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    return textureLoad(linear_depth_texture, clamp(pixel, vec2<i32>(0), max_pixel), 0).x;
}

fn depth_at_uv(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let pixel = vec2<i32>(
        i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
        i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
    );
    return depth_at_pixel(pixel);
}

fn reflection_policy_at_uv(uv: vec2<f32>) -> vec4<f32> {
    let dims = textureDimensions(reflection_policy_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = vec2<i32>(
        i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
        i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
    );
    return textureLoad(reflection_policy_texture, clamp(pixel, vec2<i32>(0), max_pixel), 0);
}

fn ssr_frontmost_at_uv(uv: vec2<f32>) -> bool {
    let bsp_depth = depth_at_uv(uv);
    if (!valid_depth(bsp_depth)) {
        return false;
    }

    let dims = textureDimensions(ssr_visibility_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = clamp(
        vec2<i32>(
            i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
            i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
        ),
        vec2<i32>(0),
        max_pixel
    );
    let device_depth = textureLoad(ssr_visibility_texture, pixel, 0).x;
    if (device_depth <= 0.0) {
        return false;
    }

    let pixel_uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / dims_f;
    let ndc = vec4<f32>(
        pixel_uv.x * 2.0 - 1.0,
        1.0 - pixel_uv.y * 2.0,
        device_depth,
        1.0
    );
    var world = settings.inv_view_proj * ndc;
    if (abs(world.w) <= 1.0e-6) {
        return false;
    }
    world = world / world.w;
    let scene_depth = distance(world.xyz, settings.camera_pos_time.xyz);
    let tolerance = max(2.0, bsp_depth * 0.002);
    return scene_depth + tolerance >= bsp_depth;
}

fn history_depth_at_uv(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(history_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = vec2<i32>(
        i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
        i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
    );
    return textureLoad(history_depth_texture, clamp(pixel, vec2<i32>(0), max_pixel), 0).x;
}

fn world_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    var world_far = settings.inv_view_proj * ndc;
    world_far = world_far / max(abs(world_far.w), 1e-6);
    return normalize(world_far.xyz - settings.camera_pos_time.xyz);
}

fn world_position(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    return settings.camera_pos_time.xyz + world_ray(uv) * depth;
}

fn project_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = settings.view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 1e-5) {
        return vec3<f32>(-10.0, -10.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}

fn previous_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = settings.prev_view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 1e-5) {
        return vec3<f32>(-1.0, -1.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}

fn surface_normal(uv: vec2<f32>, centre_depth: f32) -> vec3<f32> {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let centre_pixel = vec2<i32>(
        i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
        i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
    );
    let px = centre_pixel + vec2<i32>(1, 0);
    let py = centre_pixel + vec2<i32>(0, 1);
    let dx_depth = depth_at_pixel(px);
    let dy_depth = depth_at_pixel(py);
    if (!valid_depth(dx_depth) || !valid_depth(dy_depth)) {
        return normalize(-world_ray(uv));
    }
    let uvx = (vec2<f32>(px) + vec2<f32>(0.5)) / dims_f;
    let uvy = (vec2<f32>(py) + vec2<f32>(0.5)) / dims_f;
    let p0 = world_position(uv, centre_depth);
    let p1 = world_position(uvx, dx_depth);
    let p2 = world_position(uvy, dy_depth);
    var n = normalize(cross(p1 - p0, p2 - p0));
    let toward_camera = normalize(settings.camera_pos_time.xyz - p0);
    if (dot(n, toward_camera) < 0.0) {
        n = -n;
    }
    return n;
}

fn weather_exposure_from_cover(cover_y: f32, surface_y: f32) -> f32 {
    if (cover_y <= -1.0e19) {
        return 1.0;
    }
    return 1.0 - smoothstep(6.0, 24.0, cover_y - surface_y);
}

fn hash12(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    let p3b = p3 + vec3<f32>(dot(p3, p3.yzx + vec3<f32>(33.33)));
    return fract((p3b.x + p3b.y) * p3b.z);
}

fn weather_puddle_noise(world_xz: vec2<f32>) -> f32 {
    let p = world_xz / 170.0;
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let n00 = hash12(cell);
    let n10 = hash12(cell + vec2<f32>(1.0, 0.0));
    let n01 = hash12(cell + vec2<f32>(0.0, 1.0));
    let n11 = hash12(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

fn weather_puddle_mask(world_xz: vec2<f32>, accumulation: f32, depression: f32) -> f32 {
    let low = mix(0.46, 0.18, accumulation);
    let high = mix(0.72, 0.34, accumulation);
    let basin = smoothstep(low, high, depression);
    let edge_breakup = mix(0.82, 1.0, weather_puddle_noise(world_xz));
    return clamp(basin * edge_breakup, 0.0, 1.0);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        hash12(p + vec2<f32>(17.17, 3.11)),
        hash12(p + vec2<f32>(5.73, 41.91))
    );
}

fn weather_ripple_layer(
    world_xz: vec2<f32>,
    time_seconds: f32,
    uv_offset: vec2<f32>,
    phase_offset: f32,
    cell_size: f32,
) -> vec2<f32> {
    let uv = world_xz / cell_size + uv_offset;
    let cell = floor(uv);
    let local = fract(uv);
    let rnd = hash22(cell + uv_offset * 19.0);
    let centre = vec2<f32>(0.14) + rnd * 0.72;
    let delta = local - centre;
    let dist = max(length(delta), 0.001);
    let age = fract(time_seconds * 0.52 + phase_offset
        + hash12(cell + uv_offset * 31.0));
    let radius = mix(0.025, 0.78, age);
    let signed_distance = dist - radius;
    let band = 1.0 - smoothstep(0.018, 0.095, abs(signed_distance));
    let birth = smoothstep(0.0, 0.06, age);
    let death = 1.0 - smoothstep(0.72, 1.0, age);
    let slope_sign = select(-1.0, 1.0, signed_distance >= 0.0);
    return (delta / dist) * band * birth * death * slope_sign;
}

fn weather_ripple_gradient(
    world_xz: vec2<f32>,
    time_seconds: f32,
    rain_strength: f32,
) -> vec2<f32> {
    let weights = clamp(
        (vec4<f32>(rain_strength) - vec4<f32>(0.0, 0.25, 0.50, 0.75)) * 4.0,
        vec4<f32>(0.0),
        vec4<f32>(1.0)
    );
    let r1 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.25,  0.00), 0.00, 64.0);
    let r2 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>(-0.55,  0.30), 0.31, 67.0);
    let r3 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.60,  0.85), 0.57, 61.0);
    let r4 = weather_ripple_layer(world_xz, time_seconds, vec2<f32>( 0.50, -0.75), 0.79, 70.0);
    let capillary = vec2<f32>(
        sin(world_xz.x * 0.092 + world_xz.y * 0.037 + time_seconds * 2.7)
            + 0.55 * sin(world_xz.y * 0.121 - time_seconds * 3.2),
        cos(world_xz.y * 0.086 - world_xz.x * 0.031 - time_seconds * 2.9)
            + 0.50 * cos(world_xz.x * 0.115 + time_seconds * 3.5)
    ) * (0.11 * rain_strength);
    return r1 * weights.x + r2 * weights.y + r3 * weights.z + r4 * weights.w + capillary;
}

fn weather_surface_response(world: vec3<f32>, normal: vec3<f32>) -> vec2<f32> {
    if ((weather_surface.amount_distance.x <= 0.001 && weather_surface.puddle.x <= 0.001)
        || weather_surface.occlusion_size.z == 0u
        || weather_surface.occlusion_size.x == 0u
        || weather_surface.occlusion_size.y == 0u) {
        return vec2<f32>(0.0);
    }

    let to_surface = world - settings.camera_pos_time.xyz;
    let distance_squared = dot(to_surface, to_surface);
    let fade_end = max(weather_surface.amount_distance.z, 1.0);
    let distance_weight = 1.0 - smoothstep(weather_surface.amount_distance.y, fade_end, sqrt(distance_squared));

    let uv = (world.xz - weather_surface.occlusion_uv.xy) * weather_surface.occlusion_uv.zw;
    if (!all(uv > vec2<f32>(0.0)) || !all(uv < vec2<f32>(1.0))) {
        return vec2<f32>(0.0);
    }

    let dimensions = weather_surface.occlusion_size.xy;
    let dimensions_f = vec2<f32>(f32(dimensions.x), f32(dimensions.y));
    let texel_position = uv * dimensions_f - vec2<f32>(0.5);
    let blend = fract(texel_position);
    let covers = textureGather(0, weather_occlusion_height, weather_occlusion_sampler, uv);
    let surface_heights = textureGather(1, weather_occlusion_height, weather_occlusion_sampler, uv);
    let upness = textureGather(2, weather_occlusion_height, weather_occlusion_sampler, uv);
    let basins = textureGather(3, weather_occlusion_height, weather_occlusion_sampler, uv);
    let e00 = weather_exposure_from_cover(covers.w, world.y);
    let e10 = weather_exposure_from_cover(covers.z, world.y);
    let e01 = weather_exposure_from_cover(covers.x, world.y);
    let e11 = weather_exposure_from_cover(covers.y, world.y);
    let exposure = mix(mix(e00, e10, blend.x), mix(e01, e11, blend.x), blend.y);
    let s00 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.w - world.y));
    let s10 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.z - world.y));
    let s01 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.x - world.y));
    let s11 = 1.0 - smoothstep(6.0, 20.0, abs(surface_heights.y - world.y));
    let physical_upness = mix(
        mix(upness.w * s00, upness.z * s10, blend.x),
        mix(upness.x * s01, upness.y * s11, blend.x),
        blend.y
    );
    let local_depression = mix(
        mix(basins.w * s00, basins.z * s10, blend.x),
        mix(basins.x * s01, basins.y * s11, blend.x),
        blend.y
    );

    let geometric_normal = normalize(normal);
    let orientation = smoothstep(-0.45, 0.12, geometric_normal.y);
    let film = clamp(weather_surface.amount_distance.x * weather_surface.amount_distance.w
        * distance_weight * exposure * orientation, 0.0, 1.0);
    var puddle = 0.0;
    if (weather_surface.puddle.x > 0.001 && physical_upness > 0.70 && local_depression > 0.001) {
        let accumulation = clamp(weather_surface.puddle.x, 0.0, 1.0);
        let flatness = smoothstep(0.70, 0.985, physical_upness);
        let basin = weather_puddle_mask(world.xz, accumulation, local_depression);
        puddle = clamp((0.28 + accumulation * 1.04) * exposure * flatness * basin, 0.0, 1.0);
    }
    return vec2<f32>(max(film, puddle * 0.96), puddle);
}

fn current_ssr(uv: vec2<f32>, centre_depth: f32, frag_coord: vec2<f32>) -> SsrSample {
    var result: SsrSample;
    result.radiance = vec3<f32>(0.0);
    result.weight = 0.0;
    if (!valid_depth(centre_depth)) {
        return result;
    }

    let quality = u32(settings.previous_camera_pos.w + 0.5);
    if (quality < 2u) {
        return result;
    }

    // Static material/geometry eligibility was cached at map load and rasterized
    // into the depth prepass. Reject before reconstructing normals or ray marching.
    let reflection_policy = reflection_policy_at_uv(uv);
    if (reflection_policy.x < 0.5) {
        return result;
    }
    // A surface that actually won a planar slot this frame already has the
    // highest-quality source. Candidates that lost the finite planar budget keep
    // y=0 in the prepass policy mask and are therefore free to fall back to SSR.
    if (quality >= 3u && reflection_policy.y >= 0.5) {
        return result;
    }

    let world = world_position(uv, centre_depth);
    let geometric_normal = surface_normal(uv, centre_depth);
    let weather = weather_surface_response(world, geometric_normal);
    let wetness = weather.x;
    let puddle = weather.y;

    // Spend dry SSR on progressively rougher materials only as quality rises.
    // Current rain/puddle response is intentionally allowed to override this
    // static roughness hint: weather is view/time dependent and cannot safely be
    // rejected by the map-load cache. Ultra restores the old broad SSR coverage.
    var max_ssr_roughness = 0.45;
    if (quality >= 3u) {
        max_ssr_roughness = 0.70;
    }
    if (quality >= 4u) {
        max_ssr_roughness = 1.0;
    }
    let weather_reflective = wetness > 0.01 || puddle > 0.01;
    if (!weather_reflective && reflection_policy.z > max_ssr_roughness) {
        return result;
    }

    // Screen Space Wetness modifies the normal/smoothness buffers before later
    // screen-space lighting consumes them. This forward-renderer adaptation does
    // the same thing locally for SSR: standing rain film slightly flattens an
    // upward-facing response normal and adds a low-roughness dielectric Fresnel
    // term. Dry SSR remains byte-for-byte equivalent in the wetness == 0 case.
    let upward = smoothstep(0.25, 0.90, geometric_normal.y);
    // Thin wet film respects the reconstructed surface slope. Puddle placement
    // has already been validated against the cached physical BSP plane, so its
    // water normal can flatten independently of a noisy/interpolated screen normal.
    let film_flatten = wetness * upward * 0.14;
    let puddle_flatten = smoothstep(0.05, 0.85, puddle) * 0.90;
    var normal = normalize(mix(geometric_normal, vec3<f32>(0.0, 1.0, 0.0), clamp(max(film_flatten, puddle_flatten), 0.0, 0.92)));
    let ripple_strength = clamp(weather_surface.puddle.z, 0.0, 1.0) * puddle;
    if (ripple_strength > 0.001) {
        let ripple = weather_ripple_gradient(
            world.xz,
            weather_surface.puddle.y,
            clamp(weather_surface.puddle.z, 0.0, 1.0)
        );
        normal = normalize(normal + vec3<f32>(-ripple.x, 0.0, -ripple.y) * (0.080 * ripple_strength));
    }
    let view_dir = normalize(settings.camera_pos_time.xyz - world);
    let facing = clamp(dot(normal, view_dir), 0.0, 1.0);
    let grazing = pow(1.0 - facing, 5.0);
    let horizontal = clamp(normal.y * 1.4, 0.0, 1.0);
    let dry_strength = grazing * (0.08 + horizontal * 0.32);

    // Water-like F0 (~2%) keeps head-on reflections restrained while making
    // wet grazing angles much more obvious. Wetness is already intensity-,
    // shelter-, and distance-weighted by the shared weather surface system.
    let wet_fresnel = 0.02 + 0.98 * grazing;
    let wet_smoothness = mix(0.72, 0.96, pow(wetness, 1.20));
    let puddle_smoothness = mix(wet_smoothness, 0.999, smoothstep(0.05, 0.80, puddle));
    let wet_strength = wetness * wet_fresnel * puddle_smoothness * (0.10 + horizontal * 0.56);
    let puddle_strength = smoothstep(0.03, 0.75, puddle) * wet_fresnel * (0.44 + horizontal * 0.78);
    let strength = dry_strength + wet_strength + puddle_strength;
    if (strength < 0.01) {
        return result;
    }

    // Bevy's SSR ray marcher uses a hybrid root finder: a cheap linear search
    // first, then bisection (and optionally secant) once the depth crossing is
    // bracketed. At half resolution, 10 stochastic linear steps + 3 bisections
    // give us a more accurate hit than the old 18 fixed world-space taps while
    // doing far less total work over the frame.
    let ray_dir = normalize(reflect(-view_dir, normal));
    let ray_origin = world + normal * 4.0;
    var linear_steps = 6u;
    var refinement_steps = 2u;
    var distance_scale = 0.75;
    if (quality >= 3u) {
        linear_steps = 10u;
        refinement_steps = 3u;
        distance_scale = 1.0;
    }
    if (quality >= 4u) {
        linear_steps = 16u;
        refinement_steps = 4u;
        distance_scale = 1.25;
    }
    let max_ray_distance = max(216.0, centre_depth * mix(0.22, mix(0.28, 0.34, puddle), wetness)) * distance_scale;
    let frame = settings.viewport_history.w;
    let jitter = hash12(frag_coord + vec2<f32>(frame * 17.0, frame * 11.0));

    var miss_t = 0.0;
    var hit_t = 0.0;
    var found_hit = false;
    for (var i = 0u; i < linear_steps; i = i + 1u) {
        let linear_t = (f32(i) + jitter) / f32(linear_steps);
        let candidate_t = pow(clamp(linear_t, 0.0, 1.0), 1.15);
        let sample_world = ray_origin + ray_dir * max_ray_distance * candidate_t;
        let projected = project_uv(sample_world);
        if (projected.x <= 0.0 || projected.x >= 1.0 ||
            projected.y <= 0.0 || projected.y >= 1.0) {
            break;
        }
        let sampled_depth = depth_at_uv(projected.xy);
        if (!valid_depth(sampled_depth)) {
            miss_t = candidate_t;
            continue;
        }
        let expected_depth = distance(settings.camera_pos_time.xyz, sample_world);
        let penetration = expected_depth - sampled_depth;
        // Bracket the *first* depth crossing, then validate thickness after
        // refinement. This mirrors Bevy's root finder more closely than only
        // accepting a coarse step that already happens to fall inside the
        // thickness window.
        if (penetration > 0.0) {
            hit_t = candidate_t;
            found_hit = true;
            break;
        }
        miss_t = candidate_t;
    }

    if (!found_hit) {
        return result;
    }

    // Narrow the crossing. This is the bisection refinement used by Bevy's
    // hybrid root finder, adapted to this renderer's linear-depth convention.
    var lo = miss_t;
    var hi = hit_t;
    for (var step = 0u; step < refinement_steps; step = step + 1u) {
        let mid = (lo + hi) * 0.5;
        let sample_world = ray_origin + ray_dir * max_ray_distance * mid;
        let projected = project_uv(sample_world);
        if (projected.x <= 0.0 || projected.x >= 1.0 ||
            projected.y <= 0.0 || projected.y >= 1.0) {
            hi = mid;
            continue;
        }
        let sampled_depth = depth_at_uv(projected.xy);
        if (!valid_depth(sampled_depth)) {
            lo = mid;
            continue;
        }
        let expected_depth = distance(settings.camera_pos_time.xyz, sample_world);
        let penetration = expected_depth - sampled_depth;
        if (penetration > 0.0) {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    let final_world = ray_origin + ray_dir * max_ray_distance * hi;
    let hit_projection = project_uv(final_world);
    if (hit_projection.x <= 0.0 || hit_projection.x >= 1.0 ||
        hit_projection.y <= 0.0 || hit_projection.y >= 1.0) {
        return result;
    }

    let final_depth = depth_at_uv(hit_projection.xy);
    if (!valid_depth(final_depth)) {
        return result;
    }
    let expected_final_depth = distance(settings.camera_pos_time.xyz, final_world);
    let final_penetration = expected_final_depth - final_depth;
    let thickness = max(18.0, expected_final_depth * 0.008);
    if (final_penetration <= 0.0 || final_penetration >= thickness) {
        return result;
    }

    let edge_distance = min(
        min(hit_projection.x, 1.0 - hit_projection.x),
        min(hit_projection.y, 1.0 - hit_projection.y)
    );
    let edge_fade = smoothstep(0.015, 0.08, edge_distance);
    let travel_fade = 1.0 - hi;
    result.radiance = scene(hit_projection.xy);
    let max_weight = mix(mix(0.42, 0.60, wetness), 0.92, smoothstep(0.05, 0.80, puddle));
    result.weight = clamp(strength * travel_fade * edge_fade, 0.0, max_weight);
    return result;
}

fn temporal_ssr(uv: vec2<f32>, centre_depth: f32, current: SsrSample) -> SsrSample {
    if (settings.viewport_history.z <= 0.5 || !valid_depth(centre_depth)) {
        return current;
    }

    let world = world_position(uv, centre_depth);
    let reprojection = previous_uv(world);
    if (reprojection.x <= 0.0 || reprojection.x >= 1.0 ||
        reprojection.y <= 0.0 || reprojection.y >= 1.0) {
        return current;
    }

    // FidelityFX DNSR keeps previous depth alongside reflection radiance so
    // reprojected samples from a different surface can be rejected before they
    // form trails. Do the same here, using source-surface linear depth.
    let previous_depth = history_depth_at_uv(reprojection.xy);
    if (!valid_depth(previous_depth)) {
        return current;
    }
    let expected_previous_depth = distance(settings.previous_camera_pos.xyz, world);
    let depth_tolerance = max(10.0, expected_previous_depth * 0.018);
    if (abs(previous_depth - expected_previous_depth) > depth_tolerance) {
        return current;
    }

    var previous = textureSample(
        history_radiance_texture,
        scene_sampler,
        reprojection.xy
    );
    let history_dims = vec2<f32>(textureDimensions(history_radiance_texture));
    let motion_pixels = length((reprojection.xy - uv) * history_dims);
    var history_weight = clamp(0.90 - motion_pixels * 0.055, 0.0, 0.90);

    // A fresh hit should appear immediately instead of being diluted by empty
    // history. Conversely, a stochastic miss is allowed to retain some valid
    // history for a frame or two, which is the whole point of temporal SSR.
    if (current.weight > 0.001 && previous.a <= 0.001) {
        history_weight = 0.0;
    } else if (current.weight <= 0.001) {
        history_weight *= 0.68;
    }

    if (current.weight > 0.001 && previous.a > 0.001) {
        // Compact temporal clamp. FidelityFX uses a fuller variance-guided
        // denoiser; this keeps the same anti-firefly principle without adding
        // another spatial pass or variance texture to this lightweight client.
        let current_luma = dot(current.radiance, vec3<f32>(0.2126, 0.7152, 0.0722));
        let previous_luma = dot(previous.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let max_luma = current_luma * 3.5 + 0.20;
        if (previous_luma > max_luma && previous_luma > 1e-5) {
            previous = vec4<f32>(previous.rgb * (max_luma / previous_luma), previous.a);
        }
    }

    var result: SsrSample;
    result.radiance = mix(current.radiance, previous.rgb, history_weight);
    result.weight = mix(current.weight, previous.a, history_weight);
    return result;
}

@fragment
fn fs_main(input: VertexOut) -> SsrFragmentOut {
    var out: SsrFragmentOut;
    // The policy/linear-depth buffers describe BSP geometry.  If the final
    // opaque scene has a closer depth owner at this pixel (player, vehicle,
    // ocean, grass/snow shell, etc.), erase SSR history here rather than merely
    // returning a current-frame miss that temporal accumulation could revive.
    if (!ssr_frontmost_at_uv(input.uv)) {
        out.radiance = vec4<f32>(0.0);
        out.depth = 0.0;
        return out;
    }
    let centre_depth = depth_at_uv(input.uv);
    if (!valid_depth(centre_depth)) {
        out.radiance = vec4<f32>(0.0);
        out.depth = 0.0;
        return out;
    }

    let current = current_ssr(input.uv, centre_depth, input.position.xy);
    let resolved = temporal_ssr(input.uv, centre_depth, current);
    out.radiance = vec4<f32>(max(resolved.radiance, vec3<f32>(0.0)), clamp(resolved.weight, 0.0, 0.60));
    out.depth = centre_depth;
    return out;
}
