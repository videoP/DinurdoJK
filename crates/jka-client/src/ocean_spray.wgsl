// Port/adaptation of 2Retr0/GodotOceanWaves (MIT).
// Copyright (c) 2024 Ethan Truong. The MIT copyright/permission notice is
// retained in ocean.rs alongside this shader.
// WGPU port of GodotOceanWaves' sea_spray_particle.gdshader and sea_spray.gdshader.
// Particle placement/shaping shares the FFT displacement/normal textures with the
// water surface, while the billboard albedo comes from the shipped upstream-style
// textures/japro/sea_spray asset rather than a generated silhouette.

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

struct OceanRenderSettings {
    map_scales: array<vec4<f32>, 3>,
    water_color: vec4<f32>,
    foam_color: vec4<f32>,
    ocean_info: vec4<f32>, // enabled, map size, JKA units per meter, windrow domain metres
    surface: vec4<f32>,
    clipmap: vec4<f32>,
    sun_direction_intensity: vec4<f32>,
    sun_color: vec4<f32>,
    sky_ambient: vec4<f32>,
    spray_info: vec4<f32>,
    spray_bounds: array<vec4<f32>, 8>,
    spray_planes: array<vec4<f32>, 8>,
    swell: vec4<f32>,
    swell_motion: vec4<f32>,
    wind_motion: vec4<f32>,
    authored_bounds: vec4<f32>,
    authored_plane: vec4<f32>,
    mask_ranges: array<vec4<f32>,8>,
    mask_bounds: array<vec4<f32>,8>,
    mask_tiles: array<vec4<u32>,8>,
    mask_triangles: array<vec4<f32>,512>,
};

struct SplashEvent {
    position: vec4<f32>, // world XYZ, intensity
    motion: vec4<f32>,   // incoming XYZ velocity, event time
    kind: vec4<f32>,     // zero for entry, one for sustained wake spray
};

struct SplashParams {
    controls: vec4<f32>, // interaction time, style, event count, spare
    events: array<SplashEvent, 32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var ocean_displacement: texture_2d_array<f32>;
@group(1) @binding(1) var ocean_normal: texture_2d_array<f32>;
@group(1) @binding(2) var ocean_sampler: sampler;
@group(1) @binding(3) var<uniform> ocean: OceanRenderSettings;
@group(1) @binding(4) var ocean_spray_albedo: texture_2d<f32>;
@group(1) @binding(8) var<uniform> splash: SplashParams;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dissolve_factor: f32,
    @location(2) alpha_fade: f32,
    @location(3) distance_m: f32,
    @location(4) active_flag: f32,
    @location(5) effect_alpha: f32,
};

fn hash32(p_in: vec2<u32>) -> vec3<f32> {
    var p = 1103515245u * ((p_in >> vec2<u32>(1u)) ^ p_in.yx);
    let h32 = 1103515245u * (p.x ^ (p.y >> 3u));
    let n = h32 ^ (h32 >> 16u);
    let rz = vec3<u32>(n, n * 16807u, n * 48271u);
    return vec3<f32>((rz >> vec3<u32>(1u)) & vec3<u32>(0x7fffffffu)) / f32(0x7fffffffu);
}

fn exp_impulse(x: f32, k: f32) -> f32 {
    let h = k * x;
    return h * exp(1.0 - h);
}

fn hidden_vertex() -> VertexOut {
    var out: VertexOut;
    out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.dissolve_factor = 0.0;
    out.alpha_fade = 0.0;
    out.distance_m = 0.0;
    out.active_flag = 0.0;
    out.effect_alpha = 0.0;
    return out;
}

fn splash_vertex(vertex_index: u32, splash_instance: u32) -> VertexOut {
    let mode = u32(splash.controls.y + 0.5);
    let event_count = min(u32(splash.controls.z + 0.5), 32u);
    let event_index = splash_instance / 30u;
    let particle_index = splash_instance % 30u;
    if (mode == 0u || event_index >= event_count) {
        return hidden_vertex();
    }
    let event = splash.events[event_index];
    let is_wake = event.kind.x > 0.5;
    let is_jet = particle_index >= 24u;
    let entry_droplets = mode == 1u || mode == 3u || mode == 5u;
    let wake_spray = mode == 2u || mode == 3u || mode == 5u;
    let entry_jets = mode == 4u || mode == 5u;
    if ((is_wake && (!wake_spray || is_jet))
        || (!is_wake && ((!is_jet && !entry_droplets) || (is_jet && !entry_jets)))) {
        return hidden_vertex();
    }

    var surface_index = -1;
    let surface_count = min(u32(ocean.spray_info.x + 0.5), 8u);
    for (var i = 0u; i < surface_count; i += 1u) {
        let bounds = ocean.spray_bounds[i];
        if (event.position.x < bounds.x || event.position.z < bounds.y
            || event.position.x > bounds.z || event.position.z > bounds.w
            || abs(event.position.y - ocean.spray_planes[i].x) > 1.0) {
            continue;
        }
        let promoted = ocean.authored_plane.w > 0.5;
        if (promoted && !ocean_mask_contains(i32(ocean.spray_planes[i].z), event.position.xz)) {
            continue;
        }
        surface_index = i32(i);
        break;
    }
    if (surface_index < 0) {
        return hidden_vertex();
    }

    let random = hash32(vec2<u32>(u32(event.kind.y + 0.5) + 17u, particle_index + 101u));
    let intensity = clamp(event.position.w, 0.0, 2.0);
    let normalized_intensity = clamp(intensity / 1.25, 0.0, 1.0);
    let age = splash.controls.x - event.motion.w;
    let entry_life = mix(0.42, 1.05, random.y) * mix(0.8, 1.15, normalized_intensity);
    let wake_life = mix(0.2, 0.62, random.y) * mix(0.8, 1.15, normalized_intensity);
    let life = select(entry_life, wake_life, is_wake);
    if (age < 0.0 || age > life || intensity < 0.015) {
        return hidden_vertex();
    }
    if (is_wake) {
        let wake_count = u32(round(mix(4.0, 24.0, normalized_intensity)));
        if (particle_index >= wake_count) {
            return hidden_vertex();
        }
    } else if (is_jet) {
        let jet_count = u32(round(mix(2.0, 6.0, normalized_intensity)));
        if (particle_index - 24u >= jet_count) {
            return hidden_vertex();
        }
    } else {
        let droplet_count = u32(round(mix(4.0, 24.0, normalized_intensity)));
        if (particle_index >= droplet_count) {
            return hidden_vertex();
        }
    }

    let angle = random.x * 6.28318530718;
    let random_dir = vec2<f32>(cos(angle), sin(angle));
    var impact_dir = event.motion.xz;
    if (dot(impact_dir, impact_dir) > 1.0e-5) {
        impact_dir = normalize(impact_dir);
    } else {
        impact_dir = vec2<f32>(0.0, 0.0);
    }
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let root = event.position.xyz;
    var centre = root;
    var particle_scale = vec3<f32>(4.0 + 7.0 * normalized_intensity);
    if (is_wake) {
        let direction = normalize(random_dir + impact_dir * 0.85);
        let drift = vec3<f32>(direction.x, 0.0, direction.y)
            * (16.0 + 58.0 * normalized_intensity);
        let launch = vec3<f32>(drift.x, 24.0 + 82.0 * normalized_intensity, drift.z);
        centre += vec3<f32>(direction.x * (2.0 + 5.0 * normalized_intensity), 1.5,
            direction.y * (2.0 + 5.0 * normalized_intensity))
            + launch * age - vec3<f32>(0.0, 80.0 * age * age, 0.0);
        particle_scale = vec3<f32>(2.5 + 3.5 * normalized_intensity,
            3.0 + 5.0 * normalized_intensity, 2.5 + 3.5 * normalized_intensity);
    } else if (is_jet) {
        let jet_random = random_dir * (5.0 + 12.0 * normalized_intensity);
        let launch = vec3<f32>(jet_random.x, 115.0 + 235.0 * normalized_intensity, jet_random.y);
        centre += vec3<f32>(jet_random.x * 0.15, 2.0, jet_random.y * 0.15)
            + launch * age - vec3<f32>(0.0, 80.0 * age * age, 0.0);
        particle_scale = vec3<f32>(3.5 + 4.0 * normalized_intensity,
            10.0 + 30.0 * normalized_intensity, 3.5 + 4.0 * normalized_intensity);
    } else {
        let direction = normalize(random_dir + impact_dir * 0.65);
        let ring = sqrt(random.z) * (3.0 + 8.0 * normalized_intensity);
        let launch = vec3<f32>(direction.x * (30.0 + 105.0 * normalized_intensity),
            45.0 + 145.0 * normalized_intensity + random.y * 45.0,
            direction.y * (30.0 + 105.0 * normalized_intensity));
        centre += vec3<f32>(direction.x * ring, 2.0, direction.y * ring)
            + launch * age - vec3<f32>(0.0, 80.0 * age * age, 0.0);
    }

    let forward = normalize(camera.camera_forward.xyz);
    var right = cross(forward, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(right, right) < 1.0e-5) {
        right = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        right = normalize(right);
    }
    let up = normalize(cross(right, forward));
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(-0.5, 0.5),
        vec2<f32>(-0.5, 0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5)
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0)
    );
    let corner = corners[vertex_index];
    let world = centre + right * corner.x * particle_scale.x + up * corner.y * particle_scale.y;
    var out: VertexOut;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.uv = uvs[vertex_index];
    out.dissolve_factor = random.x;
    out.alpha_fade = 1.0 - clamp(age / life, 0.0, 1.0);
    out.distance_m = distance(centre.xz, camera.camera_pos_time.xz) / units_per_meter;
    out.active_flag = 1.0;
    out.effect_alpha = 2.5 + 2.0 * normalized_intensity;
    return out;
}

fn ocean_mask_contains(slot: i32, p: vec2<f32>) -> bool {
    if (slot < 0 || slot >= 8) {
        return true;
    }
    let range = ocean.mask_ranges[slot];
    let count = u32(range.y);
    if (count == 0u) {
        return true;
    }
    let bounds = ocean.mask_bounds[slot];
    if (any(p < bounds.xy) || any(p > bounds.zw)) {
        return false;
    }
    let tile_xy = vec2<u32>(clamp(
        floor((p - bounds.xy) / max(bounds.zw - bounds.xy, vec2<f32>(1.0e-4)) * 8.0),
        vec2<f32>(0.0), vec2<f32>(7.0)));
    let tile = tile_xy.y * 8u + tile_xy.x;
    let code = (ocean.mask_tiles[slot][tile >> 4u] >> ((tile & 15u) * 2u)) & 3u;
    if (code == 1u) { return false; }
    if (code == 2u) { return true; }
    let first = u32(range.x);
    for (var i = 0u; i < count; i = i + 1u) {
        let t0 = ocean.mask_triangles[(first + i) * 2u];
        let t1 = ocean.mask_triangles[(first + i) * 2u + 1u];
        let a = t0.xy;
        let b = t0.zw;
        let c = t1.xy;
        let ab = b - a;
        let bc = c - b;
        let ca = a - c;
        let e0 = (ab.x * (p.y - a.y) - ab.y * (p.x - a.x)) / max(length(ab), 1.0e-4);
        let e1 = (bc.x * (p.y - b.y) - bc.y * (p.x - b.x)) / max(length(bc), 1.0e-4);
        let e2 = (ca.x * (p.y - c.y) - ca.y * (p.x - c.x)) / max(length(ca), 1.0e-4);
        let tolerance = 0.75;
        if ((e0 >= -tolerance && e1 >= -tolerance && e2 >= -tolerance)
            || (e0 <= tolerance && e1 <= tolerance && e2 <= tolerance)) {
            return true;
        }
    }
    return false;
}

@vertex fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let surface_count = min(u32(ocean.spray_info.x + 0.5), 8u);
    let total_particles = max(u32(ocean.spray_info.y + 0.5), 1u);
    if (surface_count == 0u) {
        return hidden_vertex();
    }
    if (instance_index >= 32768u) {
        return splash_vertex(vertex_index, instance_index - 32768u);
    }
    if (ocean.surface.z < 0.5 || instance_index >= total_particles) { return hidden_vertex(); }

    let surface_index = instance_index % surface_count;
    let particle_index = instance_index / surface_count;
    let per_surface = (total_particles + surface_count - 1u) / surface_count;
    let grid_width = max(u32(ceil(sqrt(f32(per_surface)))), 2u);

    // Upstream lays particles on a roughly even 10x10 local box and scales the
    // emitter by 15: a 150 m square centered on the camera-following ocean.
    let grid = vec2<f32>(f32(particle_index / grid_width), f32(particle_index % grid_width));
    let denom = max(f32(grid_width - 1u), 1.0);
    let coords_m = (grid / denom - vec2<f32>(0.5)) * 150.0;
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let start_xz = camera.camera_pos_time.xz + coords_m * units_per_meter;

    let bounds = ocean.spray_bounds[surface_index];
    if (start_xz.x < bounds.x || start_xz.y < bounds.y || start_xz.x > bounds.z || start_xz.y > bounds.w) {
        return hidden_vertex();
    }

    let promoted = ocean.authored_plane.w > 0.5;
    let activity = select(1.0, ocean.spray_planes[surface_index].y, promoted);
    let foam_gain = select(1.0, ocean.spray_planes[surface_index].w, promoted);
    if (promoted && !ocean_mask_contains(i32(ocean.spray_planes[surface_index].z), start_xz)) {
        return hidden_vertex();
    }
    let time = max(camera.camera_pos_time.w, 0.0);
    let lifetime = max(ocean.spray_info.z, 0.001);
    let lifetime_randomness = clamp(ocean.spray_info.w, 0.0, 1.0);
    // The upstream GPUParticles3D node has a 6 s emitter lifetime while the
    // particle shader gives each visible splash a randomized ~2.25..3 s life.
    // Recreate its randomized START_TIME/dormant interval analytically, so no
    // persistent CPU particle state is required.
    let emitter_lifetime = 6.0;
    let cycle = u32(floor(time / emitter_lifetime));
    let cycle_time = time - f32(cycle) * emitter_lifetime;
    let base_rand = hash32(vec2<u32>(instance_index, cycle + 1u));
    let particle_lifetime = lifetime * (1.0 - lifetime_randomness * base_rand.y);
    let start_time = base_rand.z * max(emitter_lifetime - particle_lifetime, 0.0);
    if (cycle_time < start_time || cycle_time > start_time + particle_lifetime) {
        return hidden_vertex();
    }
    let local_time = cycle_time - start_time;
    let t = clamp(local_time / max(particle_lifetime, 0.001), 0.0, 1.0);

    let start_m = start_xz / units_per_meter;
    var gradient = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i += 1u) {
        let sample = textureSampleLevel(
            ocean_normal,
            ocean_sampler,
            start_m * vec2<f32>(1.0,-1.0) * ocean.map_scales[i].xy,
            i32(i),
            0.0
        );
        gradient += vec3<f32>(sample.x * ocean.map_scales[i].w,
            sample.y * ocean.map_scales[i].w, sample.w * activity * foam_gain);
    }
    let phase = dot(start_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    gradient = vec3<f32>(gradient.xy + ocean.swell.xy * ocean.swell.w * ocean.swell.z * cos(phase), gradient.z);
    let water_normal = normalize(vec3<f32>(-gradient.x, 1.0, -gradient.y));
    let foam = gradient.z;
    // Exact activation/shaping thresholds from upstream sea_spray_particle.gdshader.
    let normal_factor = mix(0.25, 1.0, min((water_normal.y - 0.92) / (0.99 - 0.92), 1.0));
    let foam_factor = mix(0.25, 1.0, min((foam - 0.9) / (1.0 - 0.9), 1.0));
    if (normal_factor < 0.0 || normal_factor > 1.0 || foam <= 0.9) {
        return hidden_vertex();
    }
    let wind_strength = clamp(length(ocean.wind_motion.xy) / 20.0, 0.0, 2.0);
    let scale_factor = normal_factor * foam_factor * ocean.wind_motion.z * (0.15 + wind_strength) * activity;

    var displacement_m = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i += 1u) {
        let scales = ocean.map_scales[i].xyz;
        displacement_m += textureSampleLevel(
            ocean_displacement,
            ocean_sampler,
            start_m * vec2<f32>(1.0,-1.0) * scales.xy,
            i32(i),
            0.0
        ).xyz * vec3<f32>(1.0,1.0,-1.0) * scales.z;
    }
    displacement_m *= vec3<f32>(ocean.swell_motion.z, 1.0, ocean.swell_motion.z);
    var crest_scale = 1.0;
    if ocean.swell.w > 0.0 && ocean.map_scales[0].z > 0.0 {
        let crest_tag = textureLoad(ocean_displacement, vec2<i32>(0), 0, 0).w;
        if crest_tag < 0.0 { crest_scale = -crest_tag; }
    }
    let horizontal = ocean.swell.w * ocean.swell_motion.y * cos(phase) * crest_scale;
    displacement_m += vec3<f32>(ocean.swell.x * horizontal, ocean.swell.w * sin(phase), ocean.swell.y * horizontal);
    displacement_m += vec3<f32>(ocean.wind_motion.x,0.0,ocean.wind_motion.y) * t * 0.15;
    displacement_m += vec3<f32>(0.0, (-5.0 * pow(2.5 * t - 0.45, 2.0) * scale_factor + 0.5) * activity, 0.0);

    let centre = vec3<f32>(start_xz.x, ocean.spray_planes[surface_index].x, start_xz.y)
        + displacement_m * units_per_meter;

    // Exact upstream scale shaping: particle_scale=(20,8.5,20), shortened
    // lifetimes create smaller particles, Y gets an impulse, XZ spreads log-like.
    let life_size = particle_lifetime / lifetime;
    var scale_modifier = vec3<f32>(life_size * life_size);
    scale_modifier.y *= exp_impulse(t, 3.0);
    scale_modifier.x *= log(1.0 + t);
    scale_modifier.z *= log(1.0 + t);
    let particle_scale_m = vec3<f32>(20.0, 8.5, 20.0)
        * vec3<f32>(foam_factor, foam_factor * normal_factor, foam_factor)
        * scale_modifier;
    let particle_scale = particle_scale_m * units_per_meter * ocean.wind_motion.z * (0.15 + wind_strength) * activity;

    var forward = normalize(camera.camera_forward.xyz);
    var right = cross(forward, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(right, right) < 1e-5) {
        right = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        right = normalize(right);
    }
    let up = normalize(cross(right, forward));

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5), vec2<f32>( 0.5, -0.5), vec2<f32>(-0.5,  0.5),
        vec2<f32>(-0.5,  0.5), vec2<f32>( 0.5, -0.5), vec2<f32>( 0.5,  0.5)
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0)
    );
    let corner = corners[vertex_index];
    let world = centre + right * corner.x * particle_scale.x + up * corner.y * particle_scale.y;

    var out: VertexOut;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.uv = uvs[vertex_index];
    out.dissolve_factor = base_rand.x;
    out.alpha_fade = exp_impulse(t, 10.0);
    out.distance_m = distance(centre.xz, camera.camera_pos_time.xz) / units_per_meter;
    out.active_flag = 1.0;
    out.effect_alpha = 1.0;
    return out;
}

fn noise12(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn smooth_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (vec2<f32>(3.0) - 2.0 * f);
    return mix(
        mix(noise12(i), noise12(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(noise12(i + vec2<f32>(0.0, 1.0)), noise12(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y
    );
}

@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.active_flag < 0.5) {
        discard;
    }
    // GodotOceanWaves samples a fixed billboard albedo and animates only the
    // separate dissolve field. The previous generated silhouette made the spray
    // crawl/shimmer because the particle shape itself changed every frame.
    let albedo_tex = textureSample(ocean_spray_albedo, ocean_sampler, input.uv);

    // Upstream fades near-camera spray out, then dissolves with animated noise.
    // The dissolve texture remains procedurally reproduced for now; the albedo
    // silhouette is the shipped textures/japro/sea_spray asset.
    let distance_fade = 1.0 - exp(-input.distance_m * 0.04);
    let dissolve_noise = smooth_noise(input.uv * 7.0 + vec2<f32>(camera.camera_pos_time.w * 0.35));
    let dissolve = max((input.alpha_fade + input.dissolve_factor) * 0.5 - dissolve_noise, 0.0);
    let alpha = clamp(albedo_tex.a * 0.666 * distance_fade * dissolve * input.effect_alpha, 0.0, 1.0);
    if (alpha < 0.002) {
        discard;
    }
    // The original spray material is deliberately unlit.
    let color = albedo_tex.rgb * ocean.foam_color.rgb * vec3<f32>(1.65, 1.75, 1.65);
    return vec4<f32>(color, alpha);
}
