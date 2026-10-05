// wgpu port of 2Retr0/GodotGrass/assets/shaders/spatial/grass.gdshader.
// BSP roots replace the source heightmap placement. The blade animation remains
// source-faithful; JKA integrations below add baked root lighting, cascaded sun
// shadow reception, and baked opaque blade pigmentation without alpha cards.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

// Pipeline-specialized: false compiles self-applied Legacy fog out entirely.
// Rebuilt only when global/manual or source-local Legacy fog activation changes.
override ENABLE_LEGACY_FOG: bool = false;

struct GrassGlobals {
    sun_direction_strength: vec4<f32>,
    sun_color: vec4<f32>,
    base_color: vec4<f32>,
    tip_color: vec4<f32>,
    sss_color: vec4<f32>,
    params: vec4<f32>,
    weather_wind: vec4<f32>,
    // Self-applied Legacy fog: linear RGB, depthForOpaque.
    legacy_fog_color_depth: vec4<f32>,
    // x: 0 off, 1 authored global EXP2, 2 manual; y: self-fog scale;
    // z: local brush-fog authored scale.
    legacy_fog_params: vec4<f32>,
    // Slot 0 is none. Slots 1..255 mirror local BSP fogs used by grass roots.
    local_fog_color_depth: array<vec4<f32>, 256>,
};

struct ShadowSettings {
    view_proj: array<mat4x4<f32>, 4>,
    split_depths: vec4<f32>,
    light_direction_enabled: vec4<f32>,
    params: vec4<f32>,
    camera_forward: vec4<f32>,
    cascade_texel_sizes: vec4<f32>,
    bevy_params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var clump_noise: texture_2d<f32>;
@group(1) @binding(1) var wind_noise: texture_2d<f32>;
@group(1) @binding(2) var noise_sampler: sampler;
@group(1) @binding(3) var<uniform> grass: GrassGlobals;
@group(1) @binding(4) var blade_detail: texture_2d<f32>;
@group(2) @binding(0) var shadow_texture: texture_depth_2d_array;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;
@group(2) @binding(2) var<uniform> shadow_settings: ShadowSettings;
@group(2) @binding(7) var sky_admission_texture: texture_depth_2d_array;

struct GrassInstanceWords {
    words: array<u32>,
};
@group(3) @binding(0) var<storage, read> grass_instances: GrassInstanceWords;

const PI: f32 = 3.14159265358979323846;
const TAU: f32 = 6.28318530717958647692;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) blade_index: u32,
};


struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) camera_distance_m: f32,
    @location(4) blade_random: f32,
    @location(5) baked_light: vec3<f32>,
    @location(6) ground_tint: vec4<f32>,
    @location(7) @interpolate(flat) fog_slot: u32,
};

// Source: https://www.shadertoy.com/view/Xt3cDn
// Exact hash used by GodotGrass, translated to WGSL integer operations.
fn hash12(x: vec2<f32>) -> f32 {
    var p = bitcast<vec2<u32>>(x);
    p = vec2<u32>(1103515245u) * ((p >> vec2<u32>(1u)) ^ p.yx);
    let h32 = 1103515245u * (p.x ^ (p.y >> 3u));
    let n = h32 ^ (h32 >> 16u);
    return f32(n) * (1.0 / 4294967295.0);
}

fn rotate_x(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
}

fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn ease_in_quartic(x: f32) -> f32 {
    let a = x * x;
    return a * a;
}

fn sample_shadow_map_bevy_hardware(light_local: vec2<f32>, depth: f32, cascade: i32) -> f32 {
    return textureSampleCompareLevel(
        shadow_texture,
        shadow_sampler,
        light_local,
        cascade,
        depth
    );
}

// Faithful Bevy 0.19.1 Castano '13 directional-shadow filter from
// bevy_pbr/src/render/shadow_sampling.wgsl.
fn sample_shadow_map_bevy_castano(light_local: vec2<f32>, depth: f32, cascade: i32) -> f32 {
    let shadow_map_size = vec2<f32>(textureDimensions(shadow_texture));
    let inv_shadow_map_size = 1.0 / shadow_map_size;
    let uv = light_local * shadow_map_size;
    var base_uv = floor(uv + 0.5);
    let ss = uv.x + 0.5 - base_uv.x;
    let tt = uv.y + 0.5 - base_uv.y;
    base_uv = (base_uv - 0.5) * inv_shadow_map_size;

    let uw0 = 4.0 - 3.0 * ss;
    let uw1 = 7.0;
    let uw2 = 1.0 + 3.0 * ss;
    let u0 = (3.0 - 2.0 * ss) / uw0 - 2.0;
    let u1 = (3.0 + ss) / uw1;
    let u2 = ss / uw2 + 2.0;

    let vw0 = 4.0 - 3.0 * tt;
    let vw1 = 7.0;
    let vw2 = 1.0 + 3.0 * tt;
    let v0 = (3.0 - 2.0 * tt) / vw0 - 2.0;
    let v1 = (3.0 + tt) / vw1;
    let v2 = tt / vw2 + 2.0;

    var sum = 0.0;
    sum += uw0 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw0 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v0) * inv_shadow_map_size, depth, cascade);
    sum += uw0 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw1 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v1) * inv_shadow_map_size, depth, cascade);
    sum += uw0 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u0, v2) * inv_shadow_map_size, depth, cascade);
    sum += uw1 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u1, v2) * inv_shadow_map_size, depth, cascade);
    sum += uw2 * vw2 * sample_shadow_map_bevy_hardware(base_uv + vec2<f32>(u2, v2) * inv_shadow_map_size, depth, cascade);
    return sum * (1.0 / 144.0);
}

fn sample_cascade_shadow_legacy(world_position: vec3<f32>, normal: vec3<f32>, cascade: i32) -> f32 {
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(world_position, 1.0);
    if (clip.w <= 0.0) { return 1.0; }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if (uv.x <= 0.0 || uv.x >= 1.0 || uv.y <= 0.0 || uv.y >= 1.0 || ndc.z <= 0.0 || ndc.z >= 1.0) {
        return 1.0;
    }
    let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
    let receiver_normal = normalize(normal);
    let slope = 1.0 - max(dot(receiver_normal, -light_direction), 0.0);
    let bias = shadow_settings.params.y * (1.0 + slope * 4.0);
    let texel = 1.0 / max(shadow_settings.params.x, 1.0);
    var visibility = 0.0;
    for (var y: i32 = -1; y <= 1; y += 1) {
        for (var x: i32 = -1; x <= 1; x += 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            visibility += textureSampleCompare(shadow_texture, shadow_sampler, uv + offset, cascade, ndc.z - bias);
        }
    }
    return visibility / 9.0;
}

fn sample_cascade_shadow_bevy(world_position: vec3<f32>, normal: vec3<f32>, cascade: i32) -> f32 {
    let light_direction = normalize(shadow_settings.light_direction_enabled.xyz);
    let texel_size = shadow_settings.cascade_texel_sizes[u32(cascade)];
    let normal_offset = shadow_settings.bevy_params.y * texel_size * normal;
    let depth_offset = shadow_settings.bevy_params.x * -light_direction;
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(world_position + normal_offset + depth_offset, 1.0);
    if (clip.w <= 0.0) { return 1.0; }
    let ndc = clip.xyz / clip.w;
    if (any(ndc.xy < vec2<f32>(-1.0)) || ndc.z < 0.0 || any(ndc > vec3<f32>(1.0))) { return 1.0; }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return sample_shadow_map_bevy_castano(uv, ndc.z, cascade);
}

fn shader_sun_sky_admission(world_position: vec3<f32>, cascade: i32) -> f32 {
    let mode = shadow_settings.camera_forward.w;
    if (mode < 0.5) {
        return 1.0;
    }
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4<f32>(world_position, 1.0);
    if (clip.w <= 0.0) {
        return 0.0;
    }
    let ndc = clip.xyz / clip.w;
    // See bsp.wgsl: only X/Y bound the sky map; encoded sky depth is valid for any Z.
    if (any(abs(ndc.xy) > vec2<f32>(1.0))) {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let dims = vec2<i32>(textureDimensions(sky_admission_texture));
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - vec2<i32>(1));
    let sky_depth = textureLoad(sky_admission_texture, pixel, cascade, 0);
    let bevy_mode = shadow_settings.params.w > 0.5;
    let clear_depth = select(1.0, 0.0, bevy_mode);
    if (abs(sky_depth - clear_depth) < 0.000001) {
        return 0.0;
    }
    let receiver_depth = 0.5 + atan(ndc.z) / 3.14159265358979323846;
    let epsilon = 0.0005;
    return select(
        select(0.0, 1.0, sky_depth <= receiver_depth + epsilon),
        select(0.0, 1.0, sky_depth >= receiver_depth - epsilon),
        bevy_mode
    );
}

fn cascaded_shadow_visibility(world_position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if (camera.render_flags.x != 0u || shadow_settings.light_direction_enabled.w < 0.5) {
        return 1.0;
    }
    let camera_depth = max(dot(world_position - camera.camera_pos_time.xyz, normalize(shadow_settings.camera_forward.xyz)), 0.0);
    let bevy_mode = shadow_settings.params.w > 0.5;
    let cascade_count = i32(shadow_settings.bevy_params.w + 0.5);
    var cascade = 0i;
    if (camera_depth > shadow_settings.split_depths.x) { cascade = 1i; }
    if (camera_depth > shadow_settings.split_depths.y) { cascade = 2i; }
    if (camera_depth > shadow_settings.split_depths.z) { cascade = 3i; }
    if (cascade >= cascade_count || camera_depth > shadow_settings.split_depths[u32(cascade)]) { return 1.0; }

    if (bevy_mode) {
        var visibility = sample_cascade_shadow_bevy(world_position, normal, cascade);
        let next_cascade = cascade + 1i;
        if (next_cascade < cascade_count) {
            let this_far_bound = shadow_settings.split_depths[u32(cascade)];
            let next_near_bound = (1.0 - shadow_settings.bevy_params.z) * this_far_bound;
            if (camera_depth >= next_near_bound) {
                let next_visibility = sample_cascade_shadow_bevy(world_position, normal, next_cascade);
                visibility = mix(visibility, next_visibility, (camera_depth - next_near_bound) / (this_far_bound - next_near_bound));
            }
        }
        return visibility * shader_sun_sky_admission(world_position, cascade);
    }

    var visibility = sample_cascade_shadow_legacy(world_position, normal, cascade);
    if (cascade < 2i && cascade + 1i < cascade_count) {
        let split = shadow_settings.split_depths[u32(cascade)];
        let blend_width = max(split * 0.08, 64.0);
        let blend = smoothstep(split - blend_width, split, camera_depth);
        if (blend > 0.0) {
            let next_visibility = sample_cascade_shadow_legacy(world_position, normal, cascade + 1i);
            visibility = mix(visibility, next_visibility, blend);
        }
    }
    return visibility * shader_sun_sky_admission(world_position, cascade);
}

struct LoadedGrassInstance {
    position: vec3<f32>,
    height: f32,
    baked_light: vec4<f32>,
    ground_tint: vec4<f32>,
    clump_values: vec3<f32>,
    fog_slot: u32,
};

fn load_grass_instance(index: u32) -> LoadedGrassInstance {
    // Rust packs every blade into exactly six u32 words (24 bytes): xyz, height,
    // baked RGBA8, ground-tint RGBA8. Address it as raw words so WGSL storage
    // alignment cannot silently inflate the per-instance stride to 32 bytes.
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
    let baked_word = grass_instances.words[base + 4u];
    let ground_word = grass_instances.words[base + 5u];
    result.baked_light = unpack4x8unorm(baked_word);
    result.ground_tint = unpack4x8unorm(ground_word);
    let packed_clump = ((baked_word >> 24u) & 0xffu) | (((ground_word >> 24u) & 0xffu) << 8u);
    result.clump_values = vec3<f32>(
        f32(packed_clump & 31u) / 31.0,
        f32((packed_clump >> 5u) & 31u) / 31.0,
        f32((packed_clump >> 10u) & 31u) / 31.0,
    );
    result.ground_tint.a = f32((packed_clump >> 15u) & 1u);
    return result;
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let instance = load_grass_instance(input.blade_index);
    let world_scale = grass.params.x;
    let clumping_factor = grass.params.y;
    let time = camera.camera_pos_time.w;
    let gust_wave = sin(time * 0.83) * 0.65 + sin(time * 1.71 + 1.9) * 0.35;
    let shift_wave = sin(time * 0.13) * 0.7 + sin(time * 0.047 + 2.4) * 0.3;
    let base_world_angle = grass.params.z;
    let base_wind_dir = vec2<f32>(cos(base_world_angle), sin(base_world_angle));
    let prevailing_world_angle = base_world_angle + grass.weather_wind.z * shift_wave;
    let gust_scale = max(0.0, 1.0 + grass.weather_wind.y * gust_wave);
    let wind_speed = max(grass.weather_wind.w, 0.0) * gust_scale;
    // Keep the shared Perlin field on the base transport path. Gust/veer change
    // the instantaneous bend direction and strength below, but never multiply a
    // changing heading by total uptime (which would make the noise field jump).
    let wind_advection_offset = base_wind_dir * max(grass.weather_wind.x, 0.0) * time;
    let reference_sprite_height = max(grass.params.w, 0.0001);

    let root_world = instance.position;
    let root_m3 = root_world * world_scale;
    let camera_m = camera.camera_pos_time.xyz * world_scale;
    let height_factor = 1.0 - input.uv.y;

    let hash0 = hash12(root_m3.xz);
    let hash1 = hash12(-root_m3.zx);
    // Static source clump texture samples were baked into the two unused alpha
    // bytes at map upload time. This removes three repeated texture samples from
    // every blade vertex without increasing the 24-byte instance stride.
    let clump0 = instance.clump_values.x;
    let clump1 = instance.clump_values.y;
    let clump2 = instance.clump_values.z;

    // --- GRASS BLADE HEIGHT/WIDTH ---
    let camera_offset_m = root_m3 - camera_m;
    let camera_distance_m = length(camera_offset_m);
    let height_offset = mix(0.4, 2.0, mix(hash0, clump0, 0.8));

    let authored_height_scale = max(instance.height / reference_sprite_height, 0.05);
    var vertex_m = input.position * authored_height_scale;
    // User tuning on top of the source silhouette: 20% thinner, 20% taller.
    vertex_m.x *= 0.80;
    vertex_m.x *= mix(0.6, 1.0, hash1);
    vertex_m.x *= 1.0 + min(ease_in_quartic(0.033 * camera_distance_m), 75.0);
    vertex_m.y *= height_offset * 1.20;
    let vertex_model = vertex_m;

    // --- WIND ---
    // Weather is the prevailing atmospheric wind. The exact GodotGrass noise
    // texture adds only broad local direction/gust variance, and the whole field
    // follows the continuously integrated shared wind so every weather effect sees
    // the same gust at the same world location/time. Grass responds to 20% of that
    // air speed (prepared on the CPU in grass.weather_wind.w).
    let crushed_factor = 1.0;
    let turn_angle_base = (mix(-0.15, 0.15, hash0) + clump2 * clumping_factor) * TAU;
    let advected_root = root_m3.xz - wind_advection_offset;
    let direction_noise = textureSampleLevel(
        wind_noise,
        noise_sampler,
        advected_root.yx * 0.005,
        0.0,
    ).x;
    let local_world_angle = prevailing_world_angle + (direction_noise * 2.0 - 1.0) * (PI / 6.0);
    // Grass' authored turn angle uses +Z at zero, while weather direction uses
    // +X at zero like the atmospheric shaders. Convert between those conventions.
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

    let bend_angle_base = mix(0.12, 0.25, hash1 + height_offset * 0.1) * PI * height_factor;
    let turbulence_time = time + height_factor * height_factor * 0.25;
    let turbulence_root = root_m3.xz
        - base_wind_dir * max(grass.weather_wind.x, 0.0) * turbulence_time;
    var wind_strength_turbulence = mix(
        0.25,
        1.0,
        textureSampleLevel(
            wind_noise,
            noise_sampler,
            turbulence_root * 0.025,
            0.0,
        ).x,
    );
    wind_strength_turbulence *= wind_strength_turbulence;
    wind_strength_turbulence *= mix(0.16, 0.25 * hash0, clump1) * PI * min(wind_speed, 1.0);
    let bend_angle = mix(0.45 * PI, bend_angle_base + wind_strength_turbulence, crushed_factor);

    // These two angles are shared by every transform below. Spell out the sin/cos
    // once so the GPU does not have to rediscover them for the vertex, normal and
    // view-thickening side test. This is algebraically the same source transform.
    let sin_bend = sin(bend_angle);
    let cos_bend = cos(bend_angle);
    let sin_turn = sin(turn_angle);
    let cos_turn = cos(turn_angle);
    let bent_vertex = vec3<f32>(
        vertex_m.x,
        cos_bend * vertex_m.y - sin_bend * vertex_m.z,
        sin_bend * vertex_m.y + cos_bend * vertex_m.z,
    );
    vertex_m = vec3<f32>(
        cos_turn * bent_vertex.x + sin_turn * bent_vertex.z,
        bent_vertex.y,
        -sin_turn * bent_vertex.x + cos_turn * bent_vertex.z,
    );
    let bent_normal = vec3<f32>(0.0, -sin_bend, cos_bend);
    var blade_normal = vec3<f32>(
        sin_turn * bent_normal.z,
        bent_normal.y,
        cos_turn * bent_normal.z,
    );

    // --- VIEW SPACE THICKENING ---
    let normal_world = normalize(blade_normal);
    let dot_nv = dot(normal_world, normalize(camera_offset_m));
    let sign_vertex = cos_turn * vertex_model.x + sin_turn * vertex_model.z;
    let thicken_direction = sign(round(sign_vertex * 1.0e6));
    let thicken_factor = ease_in_quartic(1.0 - abs(dot_nv)) * abs(vertex_model.x);

    let forward = normalize(camera.camera_forward.xyz);
    var camera_right = cross(forward, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(camera_right, camera_right) < 0.000001) {
        camera_right = vec3<f32>(1.0, 0.0, 0.0);
    } else {
        camera_right = normalize(camera_right);
    }
    vertex_m += camera_right * thicken_factor * thicken_direction;

    let world_position = root_world + vertex_m / world_scale;
    var out: VertexOutput;
    out.position = camera.view_proj * vec4<f32>(world_position, 1.0);
    out.uv = input.uv;
    out.world_position = world_position;
    out.normal = blade_normal;
    // Root distance is already available and differs from per-vertex distance by at
    // most the blade height; using it for fog/material LOD removes a second length().
    out.camera_distance_m = camera_distance_m;
    out.blade_random = hash1;
    out.baked_light = instance.baked_light.rgb;
    out.ground_tint = instance.ground_tint;
    out.fog_slot = instance.fog_slot;
    return out;
}

fn grass_legacy_srgb_channel_to_linear(value: f32) -> f32 {
    let c = clamp(value, 0.0, 1.0);
    return select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
}

fn grass_legacy_authored_fog_color(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        grass_legacy_srgb_channel_to_linear(color.r),
        grass_legacy_srgb_channel_to_linear(color.g),
        grass_legacy_srgb_channel_to_linear(color.b)
    );
}

// Legacy fog: procedural grass is not a BSP material stage, so it fogs itself
// with the same curves as bsp.wgsl legacy_fog_color_amount (see
// FogSystem::legacy_self_fog). Legacy 1 authored global fog is instead composited
// in post over the grass using the ground depth behind it.
fn apply_grass_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>, fog_slot: u32) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) {
        return rgb;
    }
    if (fog_slot != 0u) {
        let local_fog = grass.local_fog_color_depth[min(fog_slot, 255u)];
        if (local_fog.a > 0.0) {
            let radial = distance(world_position, camera.camera_pos_time.xyz)
                / max(local_fog.a, 0.001);
            let amount = clamp(radial * grass.legacy_fog_params.z, 0.0, 1.0);
            return mix(rgb, grass_legacy_authored_fog_color(local_fog.rgb), amount);
        }
    }
    let mode = grass.legacy_fog_params.x;
    if (mode < 0.5) {
        return rgb;
    }
    let authored_depth = max(grass.legacy_fog_color_depth.a, 0.001);
    let scale = grass.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        let forward_distance = max(
            dot(world_position - camera.camera_pos_time.xyz, normalize(camera.camera_forward.xyz)),
            0.0
        );
        let scaled = forward_distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(world_position, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    return mix(rgb, grass.legacy_fog_color_depth.rgb, clamp(amount, 0.0, 1.0));
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let fog_factor = exp(-input.camera_distance_m * 0.017);

    // Horizontally rounded blade normal from the source fragment().
    var normal = normalize(rotate_y(input.normal, mix(-0.12, 0.12, input.uv.x) * PI));
    normal = mix(vec3<f32>(0.0, 1.0, 0.0), normal, fog_factor);

    let height_factor = 1.0 - input.uv.y;
    var albedo = mix(grass.base_color.rgb, grass.tip_color.rgb, ease_in_quartic(height_factor));
    albedo *= mix(0.1, 1.0, height_factor * height_factor);
    albedo = mix(
        mix(grass.base_color.rgb, grass.tip_color.rgb, 0.5) * 0.5,
        albedo,
        fog_factor,
    );

    // One 64x64 RGBA lookup now supplies deliberately visible blade material
    // structure: coarse mottling, lengthwise fibres, chlorophyll variation and a
    // faint dry component. It is tiny enough to remain cache-resident in practice.
    let detail_x = select(input.uv.x, 1.0 - input.uv.x, input.blade_random > 0.5);
    let detail = textureSampleLevel(
        blade_detail,
        noise_sampler,
        vec2<f32>(detail_x, input.uv.y),
        0.0,
    ).rgb;
    let blade_value = mix(0.78, 1.20, input.blade_random);
    let detail_tint = vec3<f32>(
        mix(0.52, 1.43, detail.r),
        mix(0.58, 1.38, detail.g),
        mix(0.48, 1.28, detail.b),
    );
    albedo *= detail_tint * blade_value;

    // A small dry-tip contribution remains material texture, not the SSS effect;
    // keep it weak so yellow only becomes pronounced where sunlight transmits.
    let dry_tip = smoothstep(0.72, 1.0, height_factor)
        * smoothstep(0.66, 0.91, input.blade_random * 0.55 + detail.b * 0.45);
    albedo = mix(albedo, albedo * vec3<f32>(1.06, 1.01, 0.82), dry_tip * 0.07);

    // At distance, converge the blade's *chromaticity* toward the average opaque
    // base texture of the BSP material that emitted it. Baked lighting is applied
    // later to both, so sparse far LOD grass visually dissolves into the ground
    // instead of forming a differently colored layer.
    if (input.ground_tint.a > 0.5) {
        let luma_weights = vec3<f32>(0.2126, 0.7152, 0.0722);
        let albedo_luma = max(dot(albedo, luma_weights), 0.025);
        let ground_luma = max(dot(input.ground_tint.rgb, luma_weights), 0.025);
        let ground_chroma = input.ground_tint.rgb * (albedo_luma / ground_luma);
        let normalized_ground = mix(ground_chroma, input.ground_tint.rgb, 0.30);
        let ground_match = smoothstep(48.0, 155.0, input.camera_distance_m) * 0.88;
        albedo = mix(albedo, normalized_ground, ground_match);
    }

    let edge_distance = min(input.uv.x, 1.0 - input.uv.x);
    let edge_mask = 1.0 - smoothstep(0.008, 0.085, edge_distance);

    // GodotGrass light(): custom diffuse that never reaches zero, plus back-light SSS.
    let toward_sun = normalize(grass.sun_direction_strength.xyz);
    let view = normalize(camera.camera_pos_time.xyz - input.world_position);
    let diffuse_factor = pow(4.0, dot(normal, toward_sun)) / 4.0;
    let sss_factor = max(-dot(view, toward_sun), 0.0) * 0.5;

    // Reuse the world's existing cascaded sun map. This only receives terrain/world
    // shadows; grass is intentionally not added as a shadow caster in this pass.
    var shadow_visibility = 1.0;
    if (grass.sun_direction_strength.w > 0.0) {
        shadow_visibility = cascaded_shadow_visibility(input.world_position, normal);
    }
    let shadow_amount = clamp(shadow_settings.params.z, 0.0, 1.0);
    let sun_attenuation = mix(1.0, shadow_visibility, shadow_amount);
    // Keep the source's broad back-light response as a mostly neutral brightness
    // lift. The *yellow* transmission is handled below only at blade edges.
    let direct = vec3<f32>(diffuse_factor + sss_factor * 0.10)
        * grass.sun_color.rgb
        * grass.sun_direction_strength.w
        * sun_attenuation;

    // Thin-blade transmission cheat. Unlike the old broad yellow wash, this is
    // confined to the blade edges, requires a grazing/backlit configuration, and
    // uses raw shadow visibility so the yellow-green transmission vanishes under
    // occluders. All inputs are already available, so this adds negligible cost.
    let grazing = pow(1.0 - abs(dot(normal, view)), 1.45);
    let sun_through_blade = pow(1.0 - abs(dot(normal, toward_sun)), 1.25);
    let backlit = pow(max(-dot(view, toward_sun), 0.0), 0.70);
    let edge_sss = edge_mask
        * grazing
        * sun_through_blade
        * mix(0.38, 1.0, backlit)
        * shadow_visibility;
    let edge_transmission = vec3<f32>(1.0, 0.88, 0.20)
        * grass.sun_color.rgb
        * grass.sun_direction_strength.w
        * edge_sss
        * 0.135;

    // input.baked_light is a bilinear sample of the actual JKA lightmap at the blade
    // root (or interpolated vertex lighting on vertex-lit surfaces). Treat it as the
    // environment/baked term while retaining the GodotGrass directional sun response.
    let baked = clamp(input.baked_light, vec3<f32>(0.0), vec3<f32>(1.5));
    let baked_luma = dot(baked, vec3<f32>(0.2126, 0.7152, 0.0722));
    let baked_tint = select(vec3<f32>(1.0), baked / max(baked_luma, 0.06), baked_luma > 0.01);
    let ambient_strength = mix(0.08, 0.52, clamp(baked_luma, 0.0, 1.0));
    let ambient = baked_tint * ambient_strength;

    let lit = albedo * (ambient + direct) + edge_transmission;
    return vec4<f32>(apply_grass_legacy_fog(lit, input.world_position, input.fog_slot), 1.0);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
