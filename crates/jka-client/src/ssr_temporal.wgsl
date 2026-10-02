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
    // Reversed-Z: unproject on the far plane (see post.wgsl world_ray).
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
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

// What the screen-space trace needs to know about a wet surface. Shares the
// puddle shape, ripples and water normal with the material pass (weather_surface.wgsl),
// so the reflection and the surface it lies on always agree.
struct SsrWeather {
    wetness: f32,
    puddle: f32,
    water_normal: vec3<f32>,
};

fn ssr_weather(world: vec3<f32>, normal: vec3<f32>) -> SsrWeather {
    var result: SsrWeather;
    result.wetness = 0.0;
    result.puddle = 0.0;
    result.water_normal = vec3<f32>(0.0, 1.0, 0.0);
    if ((weather_surface.amount_distance.x <= 0.001 && weather_surface.puddle.x <= 0.001)
        || weather_surface.occlusion_size.z == 0u
        || weather_surface.occlusion_size.x == 0u
        || weather_surface.occlusion_size.y == 0u) {
        return result;
    }

    let to_surface = world - settings.camera_pos_time.xyz;
    let fade_start = weather_surface.film_fade.x;
    let distance_weight = 1.0 - smoothstep(
        fade_start,
        max(weather_surface.film_fade.y, fade_start + 1.0),
        length(to_surface)
    );
    let field = weather_sample_field(
        weather_occlusion_height,
        world,
        weather_surface.occlusion_uv.xy,
        weather_surface.occlusion_uv.zw,
        normalize(normal).y
    );

    let normal_y = normalize(normal).y;
    let orientation = smoothstep(-0.45, 0.12, normal_y);
    var wetness = clamp(weather_surface.amount_distance.x * weather_surface.amount_distance.w
        * distance_weight * field.exposure * orientation, 0.0, 1.0);

    let accumulation = clamp(weather_surface.puddle.x, 0.0, 1.0);
    if (accumulation > 0.001 && field.exposure > 0.001) {
        let shape = weather_puddle_shape(field, world.xz, accumulation, weather_surface.look.x);
        let upright = smoothstep(0.75, 0.95, normal_y);
        let coverage = shape.coverage * field.exposure * upright;
        result.puddle = coverage;
        wetness = max(wetness, max(coverage * 0.96, shape.damp * field.exposure * upright * 0.85));
        if (coverage > 0.001) {
            let gradient = (weather_water_gradient(
                world.xz,
                weather_surface.puddle.y,
                clamp(weather_surface.puddle.z, 0.0, 1.0),
                weather_surface.wind.xy
            ) + weather_wake_gradient(world)) * smoothstep(0.0, 0.5, shape.depth);
            result.water_normal = weather_water_normal(gradient, 0.12);
        }
    }
    result.wetness = wetness;
    return result;
}

fn scene_level(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(
        scene_texture,
        scene_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)),
        0.0
    ).rgb;
}

// A reflection in wet ground smears along the screen-space vertical: ripples and
// the roughness of the wet film tilt the reflected ray mostly in the view plane, so
// a neon tube or a lit window becomes a long soft streak rather than a mirror
// image. Five taps along the projected world-up axis, jittered per pixel and per
// frame so the temporal accumulation below integrates them into a continuous blur.
fn ssr_streaked_scene(hit_uv: vec2<f32>, hit_world: vec3<f32>, length_px: f32, jitter: f32) -> vec3<f32> {
    let viewport = max(settings.viewport_history.xy, vec2<f32>(1.0));
    var axis_px = (project_uv(hit_world + vec3<f32>(0.0, 96.0, 0.0)).xy - hit_uv) * viewport;
    let axis_length = length(axis_px);
    if (length_px < 1.0 || axis_length < 0.5) {
        return scene_level(hit_uv);
    }
    axis_px = axis_px / axis_length;
    let spacing_uv = axis_px * (length_px * 0.25) / viewport;
    let origin_uv = hit_uv + spacing_uv * (jitter - 0.5);
    return scene_level(origin_uv - spacing_uv * 2.0) * 0.06
        + scene_level(origin_uv - spacing_uv) * 0.24
        + scene_level(origin_uv) * 0.40
        + scene_level(origin_uv + spacing_uv) * 0.24
        + scene_level(origin_uv + spacing_uv * 2.0) * 0.06;
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
    let weather = ssr_weather(world, geometric_normal);
    let wetness = weather.wetness;
    let puddle = weather.puddle;

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
    let high_quality = weather_surface.look.z > 0.5;
    let water = smoothstep(0.05, 0.85, puddle);
    let upward = smoothstep(0.25, 0.90, geometric_normal.y);
    // Thin wet film respects the reconstructed surface slope. Puddle placement
    // has already been validated against the cached physical BSP plane, so its
    // water surface replaces a noisy/interpolated screen normal outright, exactly
    // as it does in the material pass.
    var normal = normalize(mix(
        geometric_normal,
        vec3<f32>(0.0, 1.0, 0.0),
        clamp(wetness * upward * 0.14, 0.0, 0.92)
    ));
    normal = normalize(mix(normal, weather.water_normal, water * 0.985));
    if (high_quality && puddle < 0.5 && geometric_normal.y > 0.97) {
        // Level wet asphalt is never optically flat: static micro-relief breaks the
        // reflection into the fine grain seen on wet streets. Slopes are left
        // alone: world-space grain stretches along them into visible bands.
        let grain = vec2<f32>(
            weather_value_noise(world.xz / 9.0),
            weather_value_noise(world.xz / 9.0 + vec2<f32>(53.1, 17.7))
        ) - vec2<f32>(0.5);
        normal = normalize(normal + vec3<f32>(grain.x, 0.0, grain.y) * (0.12 * wetness * upward));
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
    let puddle_smoothness = mix(wet_smoothness, 0.999, water);
    let wet_strength = wetness * wet_fresnel * puddle_smoothness
        * (0.10 + horizontal * 0.56) * weather_surface.look.w;
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
    var ray_dir = normalize(reflect(-view_dir, normal));
    // A ray that dips into the surface it leaves (a perturbed normal at a grazing
    // angle, a sloped floor) would hit that surface again at once and reflect
    // itself in stripes: keep every ray above the geometry it starts on.
    let lift = dot(ray_dir, geometric_normal);
    if (lift < 0.05) {
        ray_dir = normalize(ray_dir + geometric_normal * (0.05 - lift));
    }
    let ray_origin = world + geometric_normal * max(4.0, centre_depth * 0.004);
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
    var max_ray_distance = max(216.0, centre_depth * mix(0.22, mix(0.28, 0.34, puddle), wetness)) * distance_scale;
    var step_curve = 1.15;
    if (high_quality && weather_reflective) {
        // A wet street mirrors things a block away: facades, signs, the far end
        // of the road. Reach that far, and bunch the steps toward the surface so
        // nearby objects still resolve.
        max_ray_distance = max(max_ray_distance, (600.0 + centre_depth * 0.9) * distance_scale);
        step_curve = 1.6;
    }
    let frame = settings.viewport_history.w;
    let jitter = weather_hash12(frag_coord + vec2<f32>(frame * 17.0, frame * 11.0));

    var miss_t = 0.0;
    var hit_t = 0.0;
    var found_hit = false;
    for (var i = 0u; i < linear_steps; i = i + 1u) {
        let linear_t = (f32(i) + jitter) / f32(linear_steps);
        let candidate_t = pow(clamp(linear_t, 0.0, 1.0), step_curve);
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
        if (high_quality && weather_reflective && ray_dir.y > 0.03) {
            // The ray left the screen or the world without touching anything
            // visible: what the water mirrors is the sky. Look for the point on
            // screen where that sky is.
            let sky_uv = project_uv(ray_origin + ray_dir * 60000.0);
            if (sky_uv.x > 0.0 && sky_uv.x < 1.0 && sky_uv.y > 0.0 && sky_uv.y < 1.0
                && !valid_depth(depth_at_uv(sky_uv.xy))) {
                let edge = min(min(sky_uv.x, 1.0 - sky_uv.x), min(sky_uv.y, 1.0 - sky_uv.y));
                result.radiance = scene_level(sky_uv.xy);
                let max_sky_weight = mix(mix(0.42, 0.60, wetness), 0.92, smoothstep(0.05, 0.80, puddle));
                result.weight = clamp(strength * smoothstep(0.01, 0.10, edge), 0.0, max_sky_weight);
            }
        }
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
    // Long reach must not dim far facades away: only the last stretch of a
    // high-quality wet ray fades with travel.
    let travel_fade = select(1.0 - hi, 1.0 - hi * hi, high_quality && weather_reflective);
    result.radiance = scene(hit_projection.xy);
    if (high_quality && weather_surface.look.y > 0.001) {
        // Rougher film smears further than still water; a patchy noise keeps
        // neighbouring stretches of street from streaking identically.
        let patchiness = 0.6 + 0.8 * weather_value_noise(world.xz / 140.0);
        let length_px = settings.viewport_history.y
            * mix(0.030, 0.006, water) * patchiness * weather_surface.look.y;
        result.radiance = ssr_streaked_scene(hit_projection.xy, final_world, length_px, jitter);
    }
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
    // Standing water may reflect almost fully; everything else is capped lower by
    // current_ssr itself.
    out.radiance = vec4<f32>(max(resolved.radiance, vec3<f32>(0.0)), clamp(resolved.weight, 0.0, 0.94));
    out.depth = centre_depth;
    return out;
}
