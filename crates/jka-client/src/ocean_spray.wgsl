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
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var ocean_displacement: texture_2d_array<f32>;
@group(1) @binding(1) var ocean_normal: texture_2d_array<f32>;
@group(1) @binding(2) var ocean_sampler: sampler;
@group(1) @binding(3) var<uniform> ocean: OceanRenderSettings;
@group(1) @binding(4) var ocean_spray_albedo: texture_2d<f32>;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dissolve_factor: f32,
    @location(2) alpha_fade: f32,
    @location(3) distance_m: f32,
    @location(4) active_flag: f32,
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
    return out;
}

@vertex fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let surface_count = min(u32(ocean.spray_info.x + 0.5), 4u);
    let total_particles = max(u32(ocean.spray_info.y + 0.5), 1u);
    if (ocean.surface.z < 0.5 || surface_count == 0u || instance_index >= total_particles) {
        return hidden_vertex();
    }

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
        gradient += vec3<f32>(sample.x, sample.y, sample.w);
    }
    let water_normal = normalize(vec3<f32>(-gradient.x, 1.0, -gradient.y));
    let foam = gradient.z;
    // Exact activation/shaping thresholds from upstream sea_spray_particle.gdshader.
    let normal_factor = mix(0.25, 1.0, min((water_normal.y - 0.92) / (0.99 - 0.92), 1.0));
    let foam_factor = mix(0.25, 1.0, min((foam - 0.9) / (1.0 - 0.9), 1.0));
    if (normal_factor < 0.0 || normal_factor > 1.0 || foam <= 0.9) {
        return hidden_vertex();
    }
    let wind_strength = clamp(length(ocean.wind_motion.xy) / 20.0, 0.0, 2.0);
    let scale_factor = normal_factor * foam_factor * ocean.wind_motion.z * (0.15 + wind_strength);

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
    let phase = dot(start_m, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    let horizontal = ocean.swell.w * ocean.swell_motion.y * cos(phase);
    displacement_m += vec3<f32>(ocean.swell.x * horizontal, ocean.swell.w * sin(phase), ocean.swell.y * horizontal);
    displacement_m += vec3<f32>(ocean.wind_motion.x,0.0,ocean.wind_motion.y) * t * 0.15;
    displacement_m += vec3<f32>(0.0, -5.0 * pow(2.5 * t - 0.45, 2.0) * scale_factor + 0.5, 0.0);

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
    let particle_scale = particle_scale_m * units_per_meter * ocean.wind_motion.z * (0.15 + wind_strength);

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
    let alpha = albedo_tex.a * 0.666 * distance_fade * dissolve;
    if (alpha < 0.002) {
        discard;
    }
    // The original spray material is deliberately unlit.
    let color = albedo_tex.rgb * ocean.foam_color.rgb * vec3<f32>(1.65, 1.75, 1.65);
    return vec4<f32>(color, alpha);
}
