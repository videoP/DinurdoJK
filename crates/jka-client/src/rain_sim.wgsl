struct RainParticle {
    position_state: vec4<f32>, // xyz world position, w: 0 uninitialized, 1 falling, 2 splash
    velocity_age: vec4<f32>,   // xyz velocity, w: splash age seconds
    misc: vec4<f32>,           // x: respawn generation, remaining reserved
};

struct RainSimUniform {
    camera_dt: vec4<f32>,      // camera xyz, dt seconds
    spawn: vec4<f32>,          // horizontal radius, below camera, above camera, base fall speed
    wind_time: vec4<f32>,      // instantaneous weather wind X/Z JKA/s, absolute time, splash duration
    weather: vec4<f32>,        // base weather wind X/Z JKA/s, remaining lanes reserved
    collision_uv: vec4<f32>,   // min render X/Z, inverse map width/depth
    counts: vec4<u32>,         // active particles, heightfield width/height, splash subset
};

@group(0) @binding(0) var<storage, read_write> particles: array<RainParticle>;
@group(0) @binding(1) var<uniform> sim: RainSimUniform;
@group(0) @binding(2) var collision_height: texture_2d<f32>;
@group(0) @binding(3) var wind_noise: texture_2d<f32>;
@group(0) @binding(4) var wind_noise_sampler: sampler;

// One broad GodotGrass wind sample is shared by an entire 128-drop workgroup.
// This deliberately keeps rain wind low-frequency and extremely cheap instead
// of doing one or more noise-texture reads per particle.
var<workgroup> shared_wind_velocity: vec2<f32>;

const TAU: f32 = 6.28318530717958647692;
const GOLDEN_ANGLE: f32 = 2.39996322972865332;
const JKA_TO_GODOT_METERS: f32 = 1.0 / 64.0;
const RAIN_GROUP_SIZE: u32 = 128u;
const RAIN_WEATHER_WIND_INHERITANCE: f32 = 1.00;

fn hash_u32(x0: u32) -> u32 {
    var x = x0;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}

fn hash01(seed: u32) -> f32 {
    return f32(hash_u32(seed) & 0x00ffffffu) / 16777215.0;
}

fn rain_group_count() -> u32 {
    return max((sim.counts.x + RAIN_GROUP_SIZE - 1u) / RAIN_GROUP_SIZE, 1u);
}

fn rain_group_patch_radius() -> f32 {
    // Keep each 128-drop workgroup in a broad overlapping patch. The patches
    // cover the camera-local rain disc without turning individual drops into
    // independent wind samples.
    return min(
        sim.spawn.x * 0.28,
        sim.spawn.x * 1.35 / sqrt(f32(rain_group_count()))
    );
}

fn rain_group_center(group_index: u32) -> vec2<f32> {
    let patch_radius = rain_group_patch_radius();
    // Irrational radial sequence keeps even the first (splash-capable) groups
    // spread across the disc instead of assigning low indices only near camera.
    let radial_sequence = fract((f32(group_index) + 0.5) * 0.61803398875);
    let angle = f32(group_index) * GOLDEN_ANGLE;
    let max_center_radius = max(sim.spawn.x - patch_radius * 1.05, 0.0);
    let radius = sqrt(radial_sequence) * max_center_radius;
    return sim.camera_dt.xz + vec2<f32>(cos(angle), sin(angle)) * radius;
}

fn weather_wind_offset_jka(time: f32) -> vec2<f32> {
    return sim.weather.xy * time;
}

fn godot_grass_wind(world_xz: vec2<f32>) -> vec2<f32> {
    // Shared Weather wind is the prevailing atmospheric flow. The GodotGrass
    // field adds broad local variance and follows the stable base transport path;
    // instantaneous gust/veer still drive the mean rain slant through wind_time.xy.
    let weather_wind_jka = sim.wind_time.xy;
    let weather_speed_jka = length(weather_wind_jka);
    let prevailing_dir = weather_wind_jka / max(weather_speed_jka, 0.0001);
    let root_m = world_xz * JKA_TO_GODOT_METERS;
    let advected_root = root_m
        - weather_wind_offset_jka(sim.wind_time.z) * JKA_TO_GODOT_METERS;

    let direction_noise = textureSampleLevel(
        wind_noise,
        wind_noise_sampler,
        advected_root.yx * 0.005,
        0.0
    ).x;
    let strength_noise = textureSampleLevel(
        wind_noise,
        wind_noise_sampler,
        advected_root * 0.025,
        0.0
    ).x;
    let direction_offset = (direction_noise * 2.0 - 1.0) * (TAU / 12.0);
    let offset_sin = sin(direction_offset);
    let offset_cos = cos(direction_offset);
    let local_dir = vec2<f32>(
        prevailing_dir.x * offset_cos - prevailing_dir.y * offset_sin,
        prevailing_dir.x * offset_sin + prevailing_dir.y * offset_cos
    );
    var strength = mix(0.25, 1.0, strength_noise);
    strength = strength * strength;

    // Shared Weather wind supplies the mean slant. Perlin only adds broad +/-30 degrees
    // direction variance and a subtle gust-strength variation around that mean.
    let inherited_speed = weather_speed_jka * RAIN_WEATHER_WIND_INHERITANCE;
    let lateral_speed = inherited_speed * mix(0.75, 1.25, strength);
    return local_dir * lateral_speed;
}

fn respawn(index: u32, generation: f32, wind_velocity: vec2<f32>) -> RainParticle {
    let initial_spawn = generation < 0.5;
    let gen = u32(max(generation, 0.0)) + 1u;
    let seed = hash_u32((index + 1u) ^ (gen * 0x9e3779b9u));

    // Particles are loosely spatially grouped by compute workgroup. That makes
    // the single shared wind sample correspond to a real broad patch of rain
    // while overlapping patches keep the overall disc visually uniform.
    let group_index = index / RAIN_GROUP_SIZE;
    let group_center = rain_group_center(group_index);
    let local_angle = hash01(seed ^ 0x243f6a88u) * TAU;
    let local_radius = sqrt(hash01(seed ^ 0xb7e15162u)) * rain_group_patch_radius();
    let spawn_xz = group_center + vec2<f32>(cos(local_angle), sin(local_angle)) * local_radius;

    let height_jitter = hash01(seed ^ 0x3c6ef372u);
    let speed_jitter = mix(0.82, 1.18, hash01(seed ^ 0xa4093822u));
    let spawn_y = select(
        sim.camera_dt.y + sim.spawn.z * mix(0.50, 1.0, height_jitter),
        sim.camera_dt.y + mix(-sim.spawn.y, sim.spawn.z, height_jitter),
        initial_spawn
    );

    var particle: RainParticle;
    particle.position_state = vec4<f32>(spawn_xz.x, spawn_y, spawn_xz.y, 1.0);
    particle.velocity_age = vec4<f32>(
        wind_velocity.x,
        -sim.spawn.w * speed_jitter,
        wind_velocity.y,
        0.0
    );
    particle.misc = vec4<f32>(f32(gen), 0.0, 0.0, 0.0);
    return particle;
}

fn collision_surface_y(position: vec3<f32>) -> f32 {
    let width = sim.counts.y;
    let height = sim.counts.z;
    if (width == 0u || height == 0u) {
        return -1.0e20;
    }

    let uv = (position.xz - sim.collision_uv.xy) * sim.collision_uv.zw;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return -1.0e20;
    }

    let pixel_u = min(
        vec2<u32>(uv * vec2<f32>(f32(width), f32(height))),
        vec2<u32>(width - 1u, height - 1u)
    );
    return textureLoad(
        collision_height,
        vec2<i32>(i32(pixel_u.x), i32(pixel_u.y)),
        0
    ).x;
}

@compute @workgroup_size(128)
fn cs_main(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    // Exactly two source-noise reads per 128-drop workgroup, regardless of rain
    // intensity. Every invocation must reach the barrier, including tail lanes.
    if (local_index == 0u) {
        shared_wind_velocity = godot_grass_wind(rain_group_center(workgroup_id.x));
    }
    workgroupBarrier();

    let index = global_id.x;
    if (index >= sim.counts.x) {
        return;
    }

    var particle = particles[index];
    if (particle.position_state.w < 0.5) {
        // Seed the initial frame throughout the whole camera-local volume so
        // rain reaches full density immediately instead of filling from the top.
        particle = respawn(index, particle.misc.x, shared_wind_velocity);
    }

    let dt = clamp(sim.camera_dt.w, 0.0, 0.05);
    if (particle.position_state.w < 1.5) {
        // Existing drops ease toward the broad workgroup wind, so gust changes
        // travel smoothly instead of snapping only when a particle respawns.
        let wind_follow = clamp(dt * 2.5, 0.0, 0.125);
        particle.velocity_age.x = mix(particle.velocity_age.x, shared_wind_velocity.x, wind_follow);
        particle.velocity_age.z = mix(particle.velocity_age.z, shared_wind_velocity.y, wind_follow);

        let old_y = particle.position_state.y;
        let delta = particle.velocity_age.xyz * dt;
        particle.position_state.x += delta.x;
        particle.position_state.y += delta.y;
        particle.position_state.z += delta.z;

        let relative = particle.position_state.xyz - sim.camera_dt.xyz;
        let outside = length(relative.xz) > sim.spawn.x
            || relative.y < -sim.spawn.y
            || relative.y > sim.spawn.z * 1.35;
        if (outside) {
            particles[index] = respawn(index, particle.misc.x, shared_wind_velocity);
            return;
        }

        let surface_y = collision_surface_y(particle.position_state.xyz);
        if (surface_y > -1.0e19) {
            // If the camera moves under a roof while a drop is already below
            // that roof, do not let the stale drop rain inside. Put it above
            // the top-down cover so ordinary camera depth keeps it invisible.
            if (old_y < surface_y - 2.0) {
                particle.position_state.y = surface_y + max(sim.spawn.z * 0.20, 24.0);
            } else if (particle.position_state.y <= surface_y + 2.0) {
                if (index < sim.counts.w) {
                    // Only a representative subset needs visible splash state.
                    // The rest recycle immediately, keeping splash vertex cost
                    // small while impacts still read as dense in heavy rain.
                    particle.position_state.y = surface_y + 1.5;
                    particle.position_state.w = 2.0;
                    particle.velocity_age.x = 0.0;
                    particle.velocity_age.y = 0.0;
                    particle.velocity_age.z = 0.0;
                    particle.velocity_age.w = 0.0;
                } else {
                    particle = respawn(index, particle.misc.x, shared_wind_velocity);
                }
            }
        }
    } else {
        particle.velocity_age.w += dt;
        if (particle.velocity_age.w >= sim.wind_time.w) {
            particles[index] = respawn(index, particle.misc.x, shared_wind_velocity);
            return;
        }
    }

    particles[index] = particle;
}
