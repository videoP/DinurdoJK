// GPU preparation pass for visible GodotGrass blades.
// Hoists root-level noise/hash/wind work out of the per-vertex shader so each
// visible blade evaluates those inputs once instead of once per mesh vertex.

struct GrassGlobals {
    sun_direction_strength: vec4<f32>,
    sun_color: vec4<f32>,
    base_color: vec4<f32>,
    tip_color: vec4<f32>,
    sss_color: vec4<f32>,
    params: vec4<f32>,
    weather_wind: vec4<f32>,
};

@group(0) @binding(0) var clump_noise: texture_2d<f32>;
@group(0) @binding(1) var wind_noise: texture_2d<f32>;
@group(0) @binding(2) var noise_sampler: sampler;
@group(0) @binding(3) var<uniform> grass: GrassGlobals;

// Reuse the renderer's existing weather-surface resources. Wetness is evaluated
// once at each visible blade root in this compute pass, never per grass fragment.
struct WeatherSurfaceSettings {
    amount_distance: vec4<f32>,
    puddle: vec4<f32>,
    occlusion_uv: vec4<f32>,
    occlusion_size: vec4<u32>,
};
@group(2) @binding(4) var weather_occlusion_height: texture_2d<f32>;
@group(2) @binding(5) var<uniform> weather_surface: WeatherSurfaceSettings;

struct GrassInstanceWords { words: array<u32>, };
@group(1) @binding(0) var<storage, read> grass_instances: GrassInstanceWords;
@group(1) @binding(1) var<storage, read> visible_indices: array<u32>;
struct PreparedWords { words: array<u32>, };
@group(1) @binding(2) var<storage, read_write> prepared_instances: PreparedWords;

struct PrepareJob {
    low_visible_offset: u32,
    low_count: u32,
    low_output_offset: u32,
    mid_visible_offset: u32,
    mid_count: u32,
    mid_output_offset: u32,
    high_visible_offset: u32,
    high_count: u32,
    high_output_offset: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    camera_pos_time: vec4<f32>,
    player_position_enabled: vec4<f32>,
};
@group(1) @binding(3) var<uniform> job: PrepareJob;

var<workgroup> shared_base_wind_dir: vec2<f32>;
var<workgroup> shared_prevailing_world_angle: f32;
var<workgroup> shared_wind_speed: f32;
var<workgroup> shared_wind_advection_offset: vec2<f32>;

const PI: f32 = 3.14159265358979323846;
const TAU: f32 = 6.28318530717958647692;
const PREPARED_WORDS: u32 = 11u;

fn hash12(x: vec2<f32>) -> f32 {
    var p = bitcast<vec2<u32>>(x);
    p = vec2<u32>(1103515245u) * ((p >> vec2<u32>(1u)) ^ p.yx);
    let h32 = 1103515245u * (p.x ^ (p.y >> 3u));
    let n = h32 ^ (h32 >> 16u);
    return f32(n) * (1.0 / 4294967295.0);
}

fn ease_in_quartic(x: f32) -> f32 {
    let a = x * x;
    return a * a;
}

struct LoadedGrassInstance {
    position: vec3<f32>,
    height: f32,
    baked_light: u32,
    ground_tint: u32,
    clump_values: vec3<f32>,
    fog_slot: u32,
};

fn load_grass_instance(index: u32) -> LoadedGrassInstance {
    let base = index * 6u;
    var result: LoadedGrassInstance;
    result.position = bitcast<vec3<f32>>(vec3<u32>(
        grass_instances.words[base + 0u],
        grass_instances.words[base + 1u],
        grass_instances.words[base + 2u],
    ));
    let height_fog_word = grass_instances.words[base + 3u];
    result.height = bitcast<f32>(height_fog_word & 0xffffff00u);
    result.fog_slot = height_fog_word & 0xffu;
    result.baked_light = grass_instances.words[base + 4u];
    result.ground_tint = grass_instances.words[base + 5u];
    let packed_clump = ((result.baked_light >> 24u) & 0xffu)
        | (((result.ground_tint >> 24u) & 0xffu) << 8u);
    result.clump_values = vec3<f32>(
        f32(packed_clump & 31u) / 31.0,
        f32((packed_clump >> 5u) & 31u) / 31.0,
        f32((packed_clump >> 10u) & 31u) / 31.0,
    );
    return result;
}

fn store_prepared(
    output_index: u32,
    root_world: vec3<f32>,
    authored_height_scale: f32,
    fog_slot: u32,
    camera_distance_m: f32,
    crushed_factor: f32,
    height_offset: f32,
    width_scale: f32,
    sin_turn: f32,
    cos_turn: f32,
    bend_base_coeff: f32,
    turbulence_base: f32,
    turbulence_tip: f32,
    blade_random: f32,
    baked_light: u32,
    ground_tint: u32,
) {
    let base = output_index * PREPARED_WORDS;
    prepared_instances.words[base + 0u] = bitcast<u32>(root_world.x);
    prepared_instances.words[base + 1u] = bitcast<u32>(root_world.y);
    prepared_instances.words[base + 2u] = bitcast<u32>(root_world.z);
    prepared_instances.words[base + 3u] =
        (bitcast<u32>(authored_height_scale) & 0xffffff00u) | (fog_slot & 0xffu);
    // Half precision is ample for a 200 m grass radius and frees the other half
    // of this existing word for the source-faithful player crush factor.
    prepared_instances.words[base + 4u] = pack2x16float(vec2<f32>(camera_distance_m, crushed_factor));
    prepared_instances.words[base + 5u] = pack2x16float(vec2<f32>(height_offset, width_scale));
    prepared_instances.words[base + 6u] = pack2x16float(vec2<f32>(sin_turn, cos_turn));
    prepared_instances.words[base + 7u] = pack2x16float(vec2<f32>(bend_base_coeff, turbulence_base));
    prepared_instances.words[base + 8u] = pack2x16float(vec2<f32>(turbulence_tip, blade_random));
    prepared_instances.words[base + 9u] = baked_light;
    prepared_instances.words[base + 10u] = ground_tint;
}

fn weather_exposure_from_cover(cover_y: f32, surface_y: f32) -> f32 {
    if (cover_y <= -1.0e19) {
        return 1.0;
    }
    return 1.0 - smoothstep(6.0, 24.0, cover_y - surface_y);
}

fn blade_root_wetness(root_world: vec3<f32>, camera_distance_m: f32, world_scale: f32) -> f32 {
    let wetness_amount = weather_surface.amount_distance.x * weather_surface.amount_distance.w;
    if (wetness_amount <= 0.001) {
        return 0.0;
    }

    // Weather fade distances are stored in JKA world units while the grass
    // animation uses Godot metres. Reject distant blades before touching the
    // weather texture so the read is limited to the small visible near field.
    let camera_distance_world = camera_distance_m / max(world_scale, 0.000001);
    let fade_end = max(weather_surface.amount_distance.z, 1.0);
    let distance_weight = 1.0 - smoothstep(
        weather_surface.amount_distance.y,
        fade_end,
        camera_distance_world,
    );
    if (distance_weight <= 0.001) {
        return 0.0;
    }

    var exposure = 1.0;
    if (weather_surface.occlusion_size.z != 0u) {
        let uv = (root_world.xz - weather_surface.occlusion_uv.xy) * weather_surface.occlusion_uv.zw;
        if (all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0))) {
            let dims = weather_surface.occlusion_size.xy;
            let texel = min(
                vec2<u32>(uv * vec2<f32>(f32(dims.x), f32(dims.y))),
                dims - vec2<u32>(1u),
            );
            // One nearest cached cover sample per *near visible blade*. The world
            // shader uses four gathered taps for smooth puddle topology, but grass
            // only needs exposed-vs-covered wetness and is too fine to justify it.
            let cover_y = textureLoad(weather_occlusion_height, vec2<i32>(texel), 0).r;
            exposure = weather_exposure_from_cover(cover_y, root_world.y);
        }
    }
    return clamp(wetness_amount * distance_weight * exposure, 0.0, 1.0);
}

@compute @workgroup_size(64)
fn cs_main(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_index) workgroup_lane: u32,
) {
    let time = job.camera_pos_time.w;
    if (workgroup_lane == 0u) {
        let gust_wave = sin(time * 0.83) * 0.65 + sin(time * 1.71 + 1.9) * 0.35;
        let shift_wave = sin(time * 0.13) * 0.7 + sin(time * 0.047 + 2.4) * 0.3;
        let base_world_angle = grass.params.z;
        shared_base_wind_dir = vec2<f32>(cos(base_world_angle), sin(base_world_angle));
        shared_prevailing_world_angle = base_world_angle + grass.weather_wind.z * shift_wave;
        let gust_scale = max(0.0, 1.0 + grass.weather_wind.y * gust_wave);
        shared_wind_speed = max(grass.weather_wind.w, 0.0) * gust_scale;
        shared_wind_advection_offset = shared_base_wind_dir * max(grass.weather_wind.x, 0.0) * time;
    }
    workgroupBarrier();

    let local_index = global_id.x;
    let total_count = job.low_count + job.mid_count + job.high_count;
    if (local_index >= total_count) {
        return;
    }

    var visible_offset: u32;
    var output_index: u32;
    if (local_index < job.low_count) {
        visible_offset = job.low_visible_offset + local_index;
        output_index = job.low_output_offset + local_index;
    } else if (local_index < job.low_count + job.mid_count) {
        let mid_index = local_index - job.low_count;
        visible_offset = job.mid_visible_offset + mid_index;
        output_index = job.mid_output_offset + mid_index;
    } else {
        let high_index = local_index - job.low_count - job.mid_count;
        visible_offset = job.high_visible_offset + high_index;
        output_index = job.high_output_offset + high_index;
    }

    let source_index = visible_indices[visible_offset];
    let instance = load_grass_instance(source_index);
    let world_scale = grass.params.x;
    let clumping_factor = grass.params.y;
    let base_wind_dir = shared_base_wind_dir;
    let prevailing_world_angle = shared_prevailing_world_angle;
    let wind_speed = shared_wind_speed;
    let wind_advection_offset = shared_wind_advection_offset;
    let reference_sprite_height = max(grass.params.w, 0.0001);

    let root_world = instance.position;
    let root_m3 = root_world * world_scale;
    let camera_m = job.camera_pos_time.xyz * world_scale;

    let hash0 = hash12(root_m3.xz);
    let hash1 = hash12(-root_m3.zx);
    let clump0 = instance.clump_values.x;
    let clump1 = instance.clump_values.y;
    let clump2 = instance.clump_values.z;

    let camera_distance_m = length(root_m3 - camera_m);
    var crushed_factor = 1.0;
    if (job.player_position_enabled.w > 0.5) {
        let player_m = job.player_position_enabled.xyz * world_scale;
        let player_distance = length(player_m - root_m3);
        // Preserve the source's quadratic proximity falloff, but extend the
        // fully-recovered radius by 20%. Because the falloff is d^2*k, k must
        // scale by 1/(1.2^2): 1.2 / 1.44 = 0.833333.
        crushed_factor = min(player_distance * player_distance * 0.8333333, 1.0);
    }
    let wetness = blade_root_wetness(root_world, camera_distance_m, world_scale);
    let height_offset = mix(0.4, 2.0, mix(hash0, clump0, 0.8));
    let authored_height_scale = max(instance.height / reference_sprite_height, 0.05);
    let width_scale = 0.80
        * mix(0.6, 1.0, hash1)
        * (1.0 + min(ease_in_quartic(0.033 * camera_distance_m), 75.0));

    let turn_angle_base = (mix(-0.15, 0.15, hash0) + clump2 * clumping_factor) * TAU;
    let advected_root = root_m3.xz - wind_advection_offset;
    let direction_noise = textureSampleLevel(
        wind_noise,
        noise_sampler,
        advected_root.yx * 0.005,
        0.0,
    ).x;
    let local_world_angle = prevailing_world_angle + (direction_noise * 2.0 - 1.0) * (PI / 6.0);
    let wind_direction = 0.5 * PI - local_world_angle;
    var wind_strength = mix(
        0.25,
        1.0,
        textureSampleLevel(
            wind_noise,
            noise_sampler,
            advected_root * 0.025,
            0.0,
        ).x,
    ) * crushed_factor;
    wind_strength *= wind_strength;
    wind_strength = min(wind_strength * mix(0.6, 0.7, hash1) * wind_speed, 1.0);
    let turn_angle = mix(turn_angle_base, wind_direction, wind_strength);

    let bend_base_coeff = mix(0.12, 0.25, hash1 + height_offset * 0.1) * PI;
    let turbulence_scale = mix(0.16, 0.25 * hash0, clump1) * PI * min(wind_speed, 1.0);

    var turbulence0 = mix(
        0.25,
        1.0,
        textureSampleLevel(
            wind_noise,
            noise_sampler,
            advected_root * 0.025,
            0.0,
        ).x,
    );
    turbulence0 *= turbulence0;
    turbulence0 *= turbulence_scale;

    // The source shifts the turbulence sample smoothly with blade height. Sample
    // the same advected weather field a quarter-second farther along for the tip.
    let tip_root = root_m3.xz
        - base_wind_dir * max(grass.weather_wind.x, 0.0) * (time + 0.25);
    var turbulence1 = mix(
        0.25,
        1.0,
        textureSampleLevel(
            wind_noise,
            noise_sampler,
            tip_root * 0.025,
            0.0,
        ).x,
    );
    turbulence1 *= turbulence1;
    turbulence1 *= turbulence_scale;

    store_prepared(
        output_index,
        root_world,
        authored_height_scale,
        instance.fog_slot,
        camera_distance_m,
        crushed_factor,
        height_offset,
        width_scale,
        sin(turn_angle),
        cos(turn_angle),
        bend_base_coeff,
        turbulence0,
        turbulence1,
        hash1,
        // The compact source instance borrows both alpha bytes for static clump
        // data. The prepared record no longer needs those clump bits, so reuse the
        // baked-light alpha byte for an 8-bit root wetness scalar at zero stride cost.
        (instance.baked_light & 0x00ffffffu)
            | (u32(clamp(wetness, 0.0, 1.0) * 255.0 + 0.5) << 24u),
        (instance.ground_tint & 0x00ffffffu)
            | select(0u, 0xff000000u, ((instance.ground_tint >> 31u) & 1u) != 0u),
    );
}
